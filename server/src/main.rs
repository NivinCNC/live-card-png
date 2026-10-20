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

const FONT_DATA: &[u8] = include_bytes!("../../src/fonts/Roboto-Regular.ttf");
/// How long a rendered card is reused. Short: "upcoming" turns into "ended" by the clock.
const CARD_TTL: Duration = Duration::from_secs(10 * 60);
/// How long a fetched logo is reused.
const IMAGE_TTL: Duration = Duration::from_secs(6 * 3600);
const MAX_IMAGE_BYTES: usize = 3 << 20;
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

type Shared<T> = Arc<OnceCell<T>>;

struct App {
    http: reqwest::Client,
    options: Options<'static>,
    render_slots: Semaphore,
    cards: Mutex<LruCache<String, (Instant, Shared<Result<Arc<Vec<u8>>, String>>)>>,
    images: Mutex<LruCache<String, (Instant, Shared<String>)>>,
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
    db.load_font_data(FONT_DATA.to_vec());
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
    let key = hex(&Sha256::digest(format!("{sorted:?}").as_bytes()));

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
        Ok(png) => {
            let mut resp = (*png).clone().into_response();
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
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
    let path = app.disk.as_ref().map(|d| d.join(format!("{key}.png")));
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
    let svg = card::generate_svg(&query, &a, &b, &logo, is_ended(&query));

    let _slot = app.render_slots.acquire().await.map_err(|e| e.to_string())?;
    let app2 = app.clone();
    let png = tokio::task::spawn_blocking(move || render(&app2.options, &svg))
        .await
        .map_err(|e| e.to_string())??;
    if let Some(p) = path {
        let tmp = p.with_extension("tmp");
        if tokio::fs::write(&tmp, &png).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, &p).await;
        }
    }
    Ok(Arc::new(png))
}

/// The image at [url] as a data: URI (empty when it can't be fetched), shared and cached.
async fn image_data_uri(app: &Arc<App>, url: String) -> String {
    if url.is_empty() {
        return String::new();
    }
    let cell = {
        let mut images = app.images.lock().unwrap();
        match images.get(&url) {
            Some((at, cell)) if at.elapsed() < IMAGE_TTL => cell.clone(),
            _ => {
                let cell: Shared<String> = Arc::new(OnceCell::new());
                images.put(url.clone(), (Instant::now(), cell.clone()));
                cell
            }
        }
    };
    let uri = cell.get_or_init(|| fetch_image(app.http.clone(), url.clone())).await.clone();
    if uri.is_empty() {
        // A failed fetch is retried by the next card that needs it
        app.images.lock().unwrap().pop(&url);
    }
    uri
}

async fn fetch_image(http: reqwest::Client, url: String) -> String {
    let Ok(resp) = http.get(&url).send().await else { return String::new() };
    if resp.status() != reqwest::StatusCode::OK {
        return String::new();
    }
    let mime = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/png")
        .to_string();
    match resp.bytes().await {
        Ok(b) if b.len() <= MAX_IMAGE_BYTES => format!("data:{mime};base64,{}", general_purpose::STANDARD.encode(&b)),
        _ => String::new(),
    }
}

fn render(options: &Options, svg: &str) -> Result<Vec<u8>, String> {
    let tree = Tree::from_str(svg, options).map_err(|e| format!("SVG Parse Error: {e}"))?;
    let size = tree.size().to_int_size();
    let mut pixmap = Pixmap::new(size.width(), size.height()).ok_or("Failed to create pixmap")?;
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| format!("PNG Encode Error: {e}"))
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
