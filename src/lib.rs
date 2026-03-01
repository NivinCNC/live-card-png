use worker::*;
use std::collections::HashMap;
use std::sync::Arc;
use resvg::usvg::{fontdb, Tree, Options};
use resvg::tiny_skia::{Pixmap, Transform};
use base64::{Engine as _, engine::general_purpose};

// Embed the font directly into the WASM binary
const FONT_DATA: &[u8] = include_bytes!("fonts/Roboto-Regular.ttf");

#[event(fetch)]
pub async fn main(req: Request, _env: Env, _ctx: worker::Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    let url = req.url()?;
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();

    // 1. Fetch external images immediately (Parallelized)
    let team_a_url = query.get("teamAImg").cloned().unwrap_or_default();
    let team_b_url = query.get("teamBImg").cloned().unwrap_or_default();
    let event_logo_url = query.get("eventLogo").cloned().unwrap_or_default();

    let (team_a_b64, team_b_b64, event_logo_b64) = futures::join!(
        fetch_image_as_base64(team_a_url),
        fetch_image_as_base64(team_b_url),
        fetch_image_as_base64(event_logo_url)
    );

    // 2. Determine if event has ended. Prefer explicit `isEnded` query param.
    // If missing, fall back to the previous `time` parsing (unix ts or date string).
    let is_ended = query.get("isEnded")
        .map(|s| s == "true")
        .or_else(|| {
            query.get("time").map(|ts_str| {
                if let Ok(ts) = ts_str.parse::<u64>() {
                    let now_ms = js_sys::Date::now() as u64;
                    let now_secs = now_ms / 1000;
                    ts < now_secs
                } else {
                    let parsed_date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(ts_str));
                    if !parsed_date.get_time().is_nan() {
                        let event_time_ms = parsed_date.get_time() as u64;
                        let now_ms = js_sys::Date::now() as u64;
                        event_time_ms < now_ms
                    } else { false }
                }
            })
        })
        .unwrap_or(false);

    // 3. Generate SVG String
    let svg_string = generate_svg(&query, &team_a_b64, &team_b_b64, &event_logo_b64, is_ended);

    // 3. Render to PNG
    match render_svg_to_png(&svg_string) {
        Ok(png_data) => {
            let headers = Headers::new();
            headers.set("content-type", "image/png")?;
            headers.set("cache-control", "public, max-age=3600")?;
            
            Ok(Response::from_bytes(png_data)?.with_headers(headers))
        },
        Err(e) => {
            console_log!("Render Error: {}", e);
            Response::error(format!("Failed to render: {}", e), 500)
        }
    }
}

// UPDATED: Now detects MIME type (jpg/png) correctly
async fn fetch_image_as_base64(original_url: String) -> String {
    if original_url.is_empty() { return String::new(); }

    // THE FIX: Route through a public caching proxy (wsrv.nl)
    // This bypasses the IP block from Sofascore.
    let target_url = format!(
        "https://external-content.duckduckgo.com/iu/?u={}&f=1", 
        original_url
    );

    let headers = Headers::new();
    headers.set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36").ok();
    
    let req_init = RequestInit {
        method: Method::Get,
        headers,
        ..Default::default()
    };

    // Construct request
    let req = match Request::new_with_init(&target_url, &req_init) {
        Ok(r) => r,
        Err(e) => {
            console_log!("Invalid URL: {}", e);
            return String::new();
        }
    };

    // Fetch
    match Fetch::Request(req).send().await {
        Ok(mut resp) => {
            if resp.status_code() == 200 {
                // Since we forced output=png in the proxy url, we can assume png,
                // but let's be safe and default to png if header is missing.
                let mime_type = resp.headers().get("content-type").ok().flatten().unwrap_or_else(|| "image/png".to_string());
                
                if let Ok(bytes) = resp.bytes().await {
                    let b64 = general_purpose::STANDARD.encode(&bytes);
                    return format!("data:{};base64,{}", mime_type, b64); 
                }
            } else {
                console_log!("Proxy failed for {}: Status {}", target_url, resp.status_code());
            }
        }
        Err(e) => console_log!("Network error: {}", e),
    }
    String::new()
}

fn generate_svg(
    params: &HashMap<String, String>, 
    team_a_b64: &str, 
    team_b_b64: &str, 
    event_logo_b64: &str,
    is_ended: bool
) -> String {
    let title = params.get("title").map(|s| s.as_str()).unwrap_or("Match");
    let team_a = params.get("teamA").map(|s| s.as_str()).unwrap_or("Team A");
    let team_b = params.get("teamB").map(|s| s.as_str()).unwrap_or("Team B");
    let time = params.get("time").map(|s| s.as_str()).unwrap_or("00:00");
    let is_live = params.get("isLive").map(|s| s == "true").unwrap_or(false);

    let (status_text, status_color) = if is_live {
        ("LIVE", "#EF4444")
    } else if is_ended {
        ("ENDED", "#6B7280")
    } else {
        ("UPCOMING", "#3B82F6")
    };

    // Determine layout mode: solo teamA (centered) vs normal (VS)
    let team_a_only = !team_a_b64.is_empty() && team_b_b64.is_empty();

    // Use r###"..."### to prevent "unknown prefix" errors
    let event_logo_svg = if !event_logo_b64.is_empty() {
        format!(
            r###"<circle cx="210" cy="65" r="25" fill="#0f172a"/>
               <image href="{}" x="185" y="40" width="50" height="50" clip-path="url(#clipCircle25)"/>"###,
            event_logo_b64
        )
    } else { String::new() };

    let team_a_svg = if !team_a_b64.is_empty() {
        format!(r###"<image href="{}" x="-35" y="-35" width="70" height="70" clip-path="url(#clipCircle35)"/>"###, team_a_b64)
    } else {
        r###"<circle cx="0" cy="0" r="35" fill="#333"/>"###.to_string()
    };

    let team_b_svg = if !team_b_b64.is_empty() {
        format!(r###"<image href="{}" x="-35" y="-35" width="70" height="70" clip-path="url(#clipCircle35)"/>"###, team_b_b64)
    } else {
        r###"<circle cx="0" cy="0" r="35" fill="#333"/>"###.to_string()
    };

    // Build the teams section based on layout mode
    let teams_section = if team_a_only {
        // Solo mode: full-width centered teamA image
        format!(
            r###"<g transform="translate(210, 140)">
              <image href="{team_a_img}" x="-230" y="-55" width="460" height="110" preserveAspectRatio="xMidYMid meet"/>
              <text y="80" fill="#FFF" font-size="18" font-family="Roboto" text-anchor="middle">{team_a}</text>
            </g>"###,
            team_a_img = team_a_b64,
            team_a = escape_xml(team_a)
        )
    } else {
        // Normal mode: teamA on left, VS in center, teamB on right
        format!(
            r###"<g transform="translate(80, 155)">
              {team_a_svg}
              <text y="60" fill="#FFF" font-size="16" font-family="Roboto" text-anchor="middle">{team_a}</text>
            </g>
            <text x="210" y="195" fill="#FACC15" font-size="32" font-weight="bold" font-family="Roboto" text-anchor="middle">VS</text>
            <g transform="translate(340, 155)">
              {team_b_svg}
              <text y="60" fill="#FFF" font-size="16" font-family="Roboto" text-anchor="middle">{team_b}</text>
            </g>"###,
            team_a_svg = team_a_svg,
            team_a = escape_xml(team_a),
            team_b_svg = team_b_svg,
            team_b = escape_xml(team_b)
        )
    };

    format!(
        r###"
        <svg width="480" height="280" viewBox="0 0 480 280" xmlns="http://www.w3.org/2000/svg">
          <defs>
            <clipPath id="clipCircle35"><circle cx="0" cy="0" r="35"/></clipPath>
            <clipPath id="clipCircle55"><circle cx="0" cy="0" r="55"/></clipPath>
            <clipPath id="clipCircle25"><circle cx="210" cy="65" r="25"/></clipPath>
          </defs>

          <rect width="480" height="280" rx="20" fill="#111827"/>
          
          <g transform="translate(30, 0)">
            <text x="0" y="45" fill="#E5E7EB" font-size="18" font-family="Roboto">{title}</text>
            <text x="420" y="45" fill="#9CA3AF" font-size="14" font-family="Roboto" text-anchor="end">{time}</text>
            
            <rect x="0" y="55" width="80" height="21" rx="10" fill="{status_color}"/>
            <text x="40" y="70" fill="#FFF" font-size="13" font-family="Roboto" text-anchor="middle">{status_text}</text>
            
            {event_logo_svg}
            
            {teams_section}
          </g>
        </svg>
        "###,
        title = escape_xml(title),
        time = escape_xml(time),
        status_color = status_color,
        status_text = status_text,
        event_logo_svg = event_logo_svg,
        teams_section = teams_section
    )
}

fn render_svg_to_png(svg_data: &str) -> Result<Vec<u8>, String> {
    let mut fontdb = fontdb::Database::new();
    fontdb.load_font_data(FONT_DATA.to_vec()); 

    let opt = Options {
        fontdb: Arc::new(fontdb), 
        ..Options::default()
    };
    
    let tree = Tree::from_str(svg_data, &opt)
        .map_err(|e| format!("SVG Parse Error: {}", e))?;

    let pixmap_size = tree.size().to_int_size();
    let mut pixmap = Pixmap::new(pixmap_size.width(), pixmap_size.height())
        .ok_or("Failed to create pixmap")?;

    resvg::render(
        &tree,
        Transform::identity(), 
        &mut pixmap.as_mut(),
    );

    pixmap.encode_png().map_err(|e| format!("PNG Encode Error: {}", e))
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
     .replace('"', "&quot;").replace('\'', "&apos;")
}