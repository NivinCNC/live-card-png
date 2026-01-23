use worker::*;
use std::collections::HashMap;
use std::sync::Arc;
use resvg::usvg::{self, fontdb, Tree, Options};
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

    // 2. Generate SVG String
    let svg_string = generate_svg(&query, &team_a_b64, &team_b_b64, &event_logo_b64);

    // 3. Render to PNG
    match render_svg_to_png(&svg_string) {
        Ok(png_data) => {
            let mut headers = Headers::new();
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

    let mut headers = Headers::new();
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
    event_logo_b64: &str
) -> String {
    let title = params.get("title").map(|s| s.as_str()).unwrap_or("Match");
    let team_a = params.get("teamA").map(|s| s.as_str()).unwrap_or("Team A");
    let team_b = params.get("teamB").map(|s| s.as_str()).unwrap_or("Team B");
    let time = params.get("time").map(|s| s.as_str()).unwrap_or("00:00");
    let is_live = params.get("isLive").map(|s| s == "true").unwrap_or(false);

    let status_text = if is_live { "LIVE" } else { "UPCOMING" };
    let status_color = if is_live { "#EF4444" } else { "#6B7280" };

    // Use r###"..."### to prevent "unknown prefix" errors
    let event_logo_svg = if !event_logo_b64.is_empty() {
        format!(
            r###"<circle cx="230" cy="55" r="25" fill="#0f172a"/>
               <image href="{}" x="205" y="30" width="50" height="50" clip-path="url(#clipCircle25)"/>"###,
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

    format!(
        r###"
        <svg width="480" height="280" viewBox="0 0 480 280" xmlns="http://www.w3.org/2000/svg">
          <defs>
            <clipPath id="clipCircle35"><circle cx="0" cy="0" r="35"/></clipPath>
            <clipPath id="clipCircle25"><circle cx="230" cy="55" r="25"/></clipPath>
          </defs>

          <rect width="480" height="280" rx="20" fill="#111827"/>
          
          <text x="20" y="35" fill="#E5E7EB" font-size="18" font-family="Roboto">{title}</text>
          <text x="460" y="35" fill="#9CA3AF" font-size="14" font-family="Roboto" text-anchor="end">{time}</text>
          
          <rect x="20" y="45" width="80" height="21" rx="10" fill="{status_color}"/>
          <text x="60" y="60" fill="#FFF" font-size="13" font-family="Roboto" text-anchor="middle">{status_text}</text>
          
          {event_logo_svg}
          
          <g transform="translate(60, 120)">
            {team_a_svg}
            <text y="60" fill="#FFF" font-size="16" font-family="Roboto" text-anchor="middle">{team_a}</text>
          </g>
          
          <text x="240" y="160" fill="#FACC15" font-size="32" font-weight="bold" font-family="Roboto" text-anchor="middle">VS</text>
          
          <g transform="translate(420, 120)">
            {team_b_svg}
            <text y="60" fill="#FFF" font-size="16" font-family="Roboto" text-anchor="middle">{team_b}</text>
          </g>
        </svg>
        "###,
        title = escape_xml(title),
        time = escape_xml(time),
        status_color = status_color,
        status_text = status_text,
        event_logo_svg = event_logo_svg,
        team_a_svg = team_a_svg,
        team_a = escape_xml(team_a),
        team_b_svg = team_b_svg,
        team_b = escape_xml(team_b)
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