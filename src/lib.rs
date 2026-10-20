use worker::*;
mod card;
use card::generate_svg;
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
    let headers = Headers::new();
    headers.set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36").ok();

    let mut req_init = RequestInit::new();
    req_init.with_method(Method::Get);
    req_init.with_headers(headers);

    // Construct request
    let req = match Request::new_with_init(&original_url, &req_init) {
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
                console_log!("Proxy failed for {}: Status {}", original_url, resp.status_code());
            }
        }
        Err(e) => console_log!("Network error: {}", e),
    }
    String::new()
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
