//! live-card-png as a native server: the Worker's card (../src/card.rs) without Cloudflare's
//! request quota. Rendered cards and fetched logos are cached (memory + disk for cards), and a
//! card asked for by many viewers at once is rendered once.

#[path = "../../src/card.rs"]
mod card;

use axum::{
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use base64::{engine::general_purpose, Engine as _};
use lru::LruCache;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{fontdb, Options, Tree};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{OnceCell, Semaphore};

const FONTS: [&[u8]; 3] = [
    include_bytes!("../../src/fonts/Roboto-Regular.ttf"),
    include_bytes!("../../src/fonts/Poppins-SemiBold.ttf"),
    include_bytes!("../../src/fonts/Poppins-Bold.ttf"),
];
/// Bump when the design changes, so no earlier card is served from the cache.
const DESIGN: &str = "poster-5";
/// How long a rendered card is reused. Short: "upcoming" turns into "ended" by the clock.
const CARD_TTL: Duration = Duration::from_secs(10 * 60);
/// How long a fetched logo is reused.
const IMAGE_TTL: Duration = Duration::from_secs(6 * 3600);
const MAX_IMAGE_BYTES: usize = 3 << 20;
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

type Shared<T> = Arc<OnceCell<T>>;
/// A fetched image as a data: URI (empty when it couldn't be fetched) and its main colour.
type Logo = (String, Option<(u8, u8, u8)>);

struct App {
    http: reqwest::Client,
    options: Options<'static>,
    render_slots: Semaphore,
    cards: Mutex<LruCache<String, (Instant, Shared<Result<Arc<Vec<u8>>, String>>)>>,
    images: Mutex<LruCache<String, (Instant, Shared<Logo>)>>,
    disk: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    let addr = std::env::var("LISTEN").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let disk = std::env::var("CACHE_DIR").ok().map(PathBuf::from);
    if let Some(d) = &disk {
        let _ = std::fs::create_dir_all(d);
    }
    let mut db = fontdb::Database::new();
    for f in FONTS {
        db.load_font_data(f.to_vec());
    }
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let app = Arc::new(App {
        http: reqwest::Client::builder()
            .user_agent(UA)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .pool_max_idle_per_host(4)
            .build()
            .expect("http client"),
        options: Options { fontdb: Arc::new(db), ..Options::default() },
        render_slots: Semaphore::new(cpus),
        cards: Mutex::new(LruCache::new(NonZeroUsize::new(400).unwrap())),
        images: Mutex::new(LruCache::new(NonZeroUsize::new(600).unwrap())),
        disk,
    });
    if let Some(d) = app.disk.clone() {
        tokio::spawn(sweep_disk(d));
    }
    let router = Router::new()
        .route("/", get(card_handler))
        .route("/health", get(|| async { "ok" }))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("live-card-server on {addr}");
    axum::serve(listener, router).await.expect("serve");
}

async fn card_handler(State(app): State<Arc<App>>, Query(query): Query<HashMap<String, String>>) -> Response {
    // Same query in any order → same card
    let sorted: BTreeMap<_, _> = query.iter().collect();
    let key = hex(&Sha256::digest(format!("{DESIGN}{sorted:?}").as_bytes()));

    let cell = {
        let mut cards = app.cards.lock().unwrap();
        match cards.get(&key) {
            Some((at, cell)) if at.elapsed() < CARD_TTL => cell.clone(),
            _ => {
                let cell: Shared<_> = Arc::new(OnceCell::new());
                cards.put(key.clone(), (Instant::now(), cell.clone()));
                cell
            }
        }
    };
    let result = cell.get_or_init(|| make_card(app.clone(), key.clone(), query)).await.clone();
    match result {
        Ok(jpg) => {
            let mut resp = (*jpg).clone().into_response();
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
            resp
        }
        Err(e) => {
            // Don't keep a failure around
            app.cards.lock().unwrap().pop(&key);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to render: {e}")).into_response()
        }
    }
}

async fn make_card(app: Arc<App>, key: String, query: HashMap<String, String>) -> Result<Arc<Vec<u8>>, String> {
    // A card rendered by an earlier run (or evicted from memory) is read back from disk
    let path = app.disk.as_ref().map(|d| d.join(format!("{key}.jpg")));
    if let Some(p) = &path {
        if let Ok(meta) = tokio::fs::metadata(p).await {
            let fresh = meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age < CARD_TTL);
            if fresh {
                if let Ok(bytes) = tokio::fs::read(p).await {
                    return Ok(Arc::new(bytes));
                }
            }
        }
    }

    let get = |k: &str| query.get(k).cloned().unwrap_or_default();
    let (a, b, logo) = tokio::join!(
        image_data_uri(&app, get("teamAImg")),
        image_data_uri(&app, get("teamBImg")),
        image_data_uri(&app, get("eventLogo")),
    );
    let svg = card::generate_svg(&query, &a.0, &b.0, &logo.0, is_ended(&query), [a.1, b.1]);

    let _slot = app.render_slots.acquire().await.map_err(|e| e.to_string())?;
    let app2 = app.clone();
    let jpg = tokio::task::spawn_blocking(move || render(&app2.options, &svg))
        .await
        .map_err(|e| e.to_string())??;
    if let Some(p) = path {
        let tmp = p.with_extension("tmp");
        if tokio::fs::write(&tmp, &jpg).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, &p).await;
        }
    }
    Ok(Arc::new(jpg))
}

/// The image at [url] as a data: URI with its main colour, shared and cached.
async fn image_data_uri(app: &Arc<App>, url: String) -> Logo {
    if url.is_empty() {
        return (String::new(), None);
    }
    let cell = {
        let mut images = app.images.lock().unwrap();
        match images.get(&url) {
            Some((at, cell)) if at.elapsed() < IMAGE_TTL => cell.clone(),
            _ => {
                let cell: Shared<Logo> = Arc::new(OnceCell::new());
                images.put(url.clone(), (Instant::now(), cell.clone()));
                cell
            }
        }
    };
    let logo = cell.get_or_init(|| fetch_image(app.http.clone(), url.clone())).await.clone();
    if logo.0.is_empty() {
        // A failed fetch is retried by the next card that needs it
        app.images.lock().unwrap().pop(&url);
    }
    logo
}

async fn fetch_image(http: reqwest::Client, url: String) -> Logo {
    let Ok(resp) = http.get(&url).send().await else { return (String::new(), None) };
    if resp.status() != reqwest::StatusCode::OK {
        return (String::new(), None);
    }
    let mime = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/png")
        .to_string();
    match resp.bytes().await {
        Ok(b) if b.len() <= MAX_IMAGE_BYTES => {
            let bytes = b.to_vec();
            let color = tokio::task::spawn_blocking(move || main_color(&bytes)).await.ok().flatten();
            (format!("data:{mime};base64,{}", general_purpose::STANDARD.encode(&b)), color)
        }
        _ => (String::new(), None),
    }
}

/// A logo's main colour: the most common clearly coloured hue (white, black, grey and
/// see-through pixels don't count), brightened to a usable tint. None for colourless logos.
fn main_color(bytes: &[u8]) -> Option<(u8, u8, u8)> {
    let img = image::load_from_memory(bytes).ok()?.thumbnail(48, 48).to_rgba8();
    const BUCKETS: usize = 18;
    let mut weight = [0f32; BUCKETS];
    let mut sum = [[0f32; 3]; BUCKETS];
    let mut counted = 0f32;
    for p in img.pixels() {
        let [r, g, b, a] = p.0;
        if a < 160 {
            continue;
        }
        counted += 1.0;
        let (rf, gf, bf) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let max = rf.max(gf).max(bf);
        let min = rf.min(gf).min(bf);
        let (s, v) = (if max > 0.0 { (max - min) / max } else { 0.0 }, max);
        if s < 0.28 || v < 0.2 {
            continue;
        }
        let d = max - min;
        let sector = if max == rf { ((gf - bf) / d).rem_euclid(6.0) } else if max == gf { (bf - rf) / d + 2.0 } else { (rf - gf) / d + 4.0 };
        let h = sector / 6.0;
        let i = ((h * BUCKETS as f32) as usize).min(BUCKETS - 1);
        // Gold/yellow is mostly trim on crests (Man United, Barcelona): it counts for less
        let gold = if (0.10..0.19).contains(&h) { 0.7 } else { 1.0 };
        let w = s * v * gold;
        weight[i] += w;
        sum[i][0] += rf * w;
        sum[i][1] += gf * w;
        sum[i][2] += bf * w;
    }
    // Each hue with its neighbours, around the wheel: red sits at both ends (~355° and ~5°)
    // and would otherwise be split in two and lose to a smaller colour
    let near = |i: usize| [(i + BUCKETS - 1) % BUCKETS, i, (i + 1) % BUCKETS];
    let score = |i: usize| near(i).iter().map(|&j| weight[j]).sum::<f32>();
    let i = (0..BUCKETS).max_by(|&a, &b| score(a).total_cmp(&score(b)))?;
    let w = score(i);
    // Too little colour (a mostly black/white crest): use the theme instead
    if counted == 0.0 || w < counted * 0.04 {
        return None;
    }
    let total = |c: usize| near(i).iter().map(|&j| sum[j][c]).sum::<f32>();
    let (r, g, b) = (total(0) / w, total(1) / w, total(2) / w);
    let k = 0.9 / r.max(g).max(b).max(0.01);
    let c = |v: f32| ((v * k).clamp(0.0, 1.0) * 255.0).round() as u8;
    Some((c(r), c(g), c(b)))
}

fn render(options: &Options, svg: &str) -> Result<Vec<u8>, String> {
    let tree = Tree::from_str(svg, options).map_err(|e| format!("SVG Parse Error: {e}"))?;
    let size = tree.size().to_int_size();
    let mut pixmap = Pixmap::new(size.width(), size.height()).ok_or("Failed to create pixmap")?;
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    // JPEG (the poster is opaque): about a quarter of the PNG's size
    let rgb: Vec<u8> = pixmap.data().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut out = Vec::with_capacity(96 * 1024);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 86)
        .encode(&rgb, size.width(), size.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("JPEG Encode Error: {e}"))?;
    Ok(out)
}

/// As the Worker: `isEnded` when given, else whether `time` (unix seconds or a date) has passed.
fn is_ended(query: &HashMap<String, String>) -> bool {
    if let Some(v) = query.get("isEnded") {
        return v == "true";
    }
    let Some(t) = query.get("time") else { return false };
    let now = chrono::Utc::now();
    if let Ok(secs) = t.trim().parse::<i64>() {
        return secs < now.timestamp();
    }
    parse_date(t.trim()).is_some_and(|d| d < now)
}

/// The date formats JavaScript's Date() accepted for the Worker that show up in these links.
fn parse_date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = DateTime::parse_from_rfc2822(s) {
        return Some(d.with_timezone(&Utc));
    }
    for f in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M", "%Y/%m/%d %H:%M:%S", "%Y/%m/%d %H:%M"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            // Like JS: a date-time without a zone is local time (UTC on the server)
            return Utc.from_local_datetime(&d).single();
        }
    }
    // A bare date is UTC midnight in JS
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)).map(|d| Utc.from_utc_datetime(&d))
}

/// Drops disk-cached cards older than CARD_TTL, every few minutes.
async fn sweep_disk(dir: PathBuf) {
    loop {
        tokio::time::sleep(Duration::from_secs(300)).await;
        let Ok(mut rd) = tokio::fs::read_dir(&dir).await else { continue };
        while let Ok(Some(e)) = rd.next_entry().await {
            let old = e
                .metadata()
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| SystemTime::now().duration_since(m).ok())
                .is_some_and(|age| age > CARD_TTL * 2);
            if old {
                let _ = tokio::fs::remove_file(e.path()).await;
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
