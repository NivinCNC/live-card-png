//! The card's SVG, shared by the Worker (lib.rs) and the native server (server/).
//!
//! 16:9 poster: the two teams on halves split diagonally through the VS ("/"), each half tinted
//! with its team's colour (read from the logo by the server; violet/indigo by default) and
//! fading into the CNCVerse Bridge purple.

use std::collections::HashMap;

pub const WIDTH: u32 = 960;
pub const HEIGHT: u32 = 540;

/// Team colours as RGB; None = the theme's defaults.
pub type TeamColors = [Option<(u8, u8, u8)>; 2];

const DEFAULT_COLORS: [(u8, u8, u8); 2] = [(0x7C, 0x3A, 0xED), (0xC0, 0x26, 0xD3)];
const FONTS: &str = "Poppins, Roboto";

pub fn generate_svg(
    params: &HashMap<String, String>,
    team_a_b64: &str,
    team_b_b64: &str,
    event_logo_b64: &str,
    is_ended: bool,
    colors: TeamColors,
) -> String {
    let title = params.get("title").map(|s| s.as_str()).unwrap_or("Match");
    let team_a = params.get("teamA").map(|s| s.as_str()).unwrap_or("Team A");
    let team_b = params.get("teamB").map(|s| s.as_str()).unwrap_or("Team B");
    let time = params.get("time").map(|s| s.as_str()).unwrap_or("");
    let is_live = params.get("isLive").map(|s| s == "true").unwrap_or(false);

    let (status_text, dot) = if is_live {
        ("LIVE", "#EF4444")
    } else if is_ended {
        ("ENDED", "#9CA3AF")
    } else {
        ("UPCOMING", "#A78BFA")
    };
    // LIVE gets a halo around its dot
    let halo = if is_live { r###"<circle cx="92" cy="70" r="15" fill="#EF4444" fill-opacity="0.28"/>"### } else { "" };

    // Solo layout: one wide image (e.g. a race or a tournament banner), one colour
    let solo = !team_a_b64.is_empty() && team_b_b64.is_empty();
    let ca = colors[0].unwrap_or(DEFAULT_COLORS[0]);
    let cb = if solo { ca } else { colors[1].unwrap_or(DEFAULT_COLORS[1]) };

    let teams = if solo {
        format!(
            r###"<image href="{img}" x="140" y="150" width="680" height="250" preserveAspectRatio="xMidYMid meet" filter="url(#shadow)"/>
            {name}"###,
            img = team_a_b64,
            name = team_name(team_a, 480.0, 466.0, 700.0)
        )
    } else {
        format!(
            r###"{logo_a}
            {logo_b}
            {name_a}
            {name_b}
            <text x="480" y="352" fill="#F3F4F6" fill-opacity="0.82" font-size="88" font-weight="700" font-family="{FONTS}" text-anchor="middle" filter="url(#shadow)">VS</text>"###,
            logo_a = team_logo(team_a_b64, team_a, 225.0, 270.0, ca),
            logo_b = team_logo(team_b_b64, team_b, 735.0, 270.0, cb),
            name_a = team_name(team_a, 235.0, 466.0, 340.0),
            name_b = team_name(team_b, 725.0, 466.0, 340.0),
        )
    };

    // The two halves, split along "/" through the VS; one background for a single image
    let halves = if solo {
        format!(r###"<rect width="{WIDTH}" height="{HEIGHT}" fill="url(#halfA)"/>"###)
    } else {
        r###"<polygon points="0,0 560,0 400,540 0,540" fill="url(#halfA)"/>
  <polygon points="560,0 960,0 960,540 400,540" fill="url(#halfB)"/>"###.to_string()
    };
    // Darker middle and a glowing seam along the split; nothing to split for a single image
    let split = if solo {
        String::new()
    } else {
        format!(r###"<rect x="300" width="360" height="{HEIGHT}" fill="url(#center)"/>
  <line x1="560" y1="0" x2="400" y2="540" stroke="url(#seam)" stroke-width="2.5"/>"###)
    };

    // Small event logo between the teams, above the VS
    let event_logo = if !event_logo_b64.is_empty() && !solo {
        format!(r###"<image href="{event_logo_b64}" x="447" y="178" width="66" height="66" preserveAspectRatio="xMidYMid meet" filter="url(#shadow)"/>"###)
    } else {
        String::new()
    };

    format!(
        r###"<svg width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <linearGradient id="halfA" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{a_top}"/><stop offset="0.55" stop-color="{a_mid}"/><stop offset="1" stop-color="#0F071B"/>
    </linearGradient>
    <linearGradient id="halfB" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{b_top}"/><stop offset="0.55" stop-color="{b_mid}"/><stop offset="1" stop-color="#0F071B"/>
    </linearGradient>
    <radialGradient id="glowA" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="{a_glow}" stop-opacity="0.55"/><stop offset="1" stop-color="{a_glow}" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="glowB" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="{b_glow}" stop-opacity="0.55"/><stop offset="1" stop-color="{b_glow}" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="vignette" cx="0.5" cy="0.45" r="0.75">
      <stop offset="0.55" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity="0.55"/>
    </radialGradient>
    <linearGradient id="center" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#0F071B" stop-opacity="0"/><stop offset="0.5" stop-color="#0F071B" stop-opacity="0.45"/><stop offset="1" stop-color="#0F071B" stop-opacity="0"/>
    </linearGradient>
    <linearGradient id="seam" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0.28" stop-color="#DDD6FE" stop-opacity="0"/><stop offset="0.5" stop-color="#DDD6FE" stop-opacity="0.7"/><stop offset="0.78" stop-color="#A78BFA" stop-opacity="0.3"/><stop offset="0.95" stop-color="#7C3AED" stop-opacity="0"/>
    </linearGradient>
    <filter id="shadow" x="-30%" y="-30%" width="160%" height="160%">
      <feDropShadow dx="0" dy="8" stdDeviation="10" flood-color="#000" flood-opacity="0.55"/>
    </filter>
  </defs>

  <rect width="{WIDTH}" height="{HEIGHT}" fill="#0F071B"/>
  {halves}
  <ellipse cx="225" cy="270" rx="260" ry="210" fill="url(#glowA)"/>
  <ellipse cx="735" cy="270" rx="260" ry="210" fill="url(#glowB)"/>
  {split}
  <rect width="{WIDTH}" height="{HEIGHT}" fill="url(#vignette)"/>

  {halo}<circle cx="92" cy="70" r="8" fill="{dot}"/>
  <text x="116" y="81" fill="#FFF" font-size="30" font-weight="600" font-family="{FONTS}" letter-spacing="1">{status_text}</text>
  <text x="868" y="81" fill="#F5F3FF" fill-opacity="0.92" font-size="28" font-weight="600" font-family="{FONTS}" text-anchor="end" letter-spacing="1">{time}</text>
  <text x="480" y="146" fill="#DDD6FE" fill-opacity="0.8" font-size="20" font-weight="600" font-family="{FONTS}" text-anchor="middle" letter-spacing="4">{title}</text>

  {event_logo}
  {teams}

  <text x="480" y="522" fill="#C4B5FD" fill-opacity="0.7" font-size="17" font-weight="600" font-family="{FONTS}" text-anchor="middle" letter-spacing="6">CNCVERSE</text>
</svg>"###,
        a_top = hex(shade(ca, 0.62)),
        a_mid = hex(shade(ca, 0.30)),
        a_glow = hex(shade(ca, 1.0)),
        b_top = hex(shade(cb, 0.62)),
        b_mid = hex(shade(cb, 0.30)),
        b_glow = hex(shade(cb, 1.0)),
        time = escape_xml(&time.to_uppercase()),
        title = escape_xml(&fit_caps(title, 34)),
    )
}

/// A team's logo (drop-shadowed), or its initials on a disc in its colour when it has none.
fn team_logo(img: &str, name: &str, cx: f32, cy: f32, color: (u8, u8, u8)) -> String {
    if !img.is_empty() {
        return format!(
            r###"<image href="{img}" x="{x}" y="{y}" width="190" height="190" preserveAspectRatio="xMidYMid meet" filter="url(#shadow)"/>"###,
            x = cx - 95.0,
            y = cy - 95.0
        );
    }
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
    format!(
        r###"<circle cx="{cx}" cy="{cy}" r="80" fill="{fill}" stroke="#F5F3FF" stroke-opacity="0.35" stroke-width="3" filter="url(#shadow)"/>
    <text x="{cx}" y="{ty}" fill="#FFF" font-size="64" font-weight="700" font-family="{FONTS}" text-anchor="middle">{initials}</text>"###,
        fill = hex(shade(color, 0.75)),
        ty = cy + 23.0,
        initials = escape_xml(&initials)
    )
}

/// A team name in capitals, shrunk to fit [max_width].
fn team_name(name: &str, cx: f32, y: f32, max_width: f32) -> String {
    let text = name.to_uppercase();
    let chars = text.chars().count().max(1) as f32;
    // Poppins SemiBold capitals average ~0.68 em, plus the letter spacing
    let size = (max_width / (chars * 0.68 + chars * 0.04)).clamp(22.0, 50.0);
    format!(
        r###"<text x="{cx}" y="{y}" fill="#FFF" font-size="{size:.0}" font-weight="600" font-family="{FONTS}" text-anchor="middle" letter-spacing="1.5" filter="url(#shadow)">{t}</text>"###,
        t = escape_xml(&text)
    )
}

/// Upper-cased, cut to [max] characters with an ellipsis.
fn fit_caps(s: &str, max: usize) -> String {
    let up = s.to_uppercase();
    if up.chars().count() <= max {
        return up;
    }
    let cut: String = up.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

/// [c] scaled towards black (factor < 1) — keeps the hue, sets how deep the tint is.
fn shade(c: (u8, u8, u8), f: f32) -> (u8, u8, u8) {
    let s = |v: u8| ((v as f32) * f).round().clamp(0.0, 255.0) as u8;
    (s(c.0), s(c.1), s(c.2))
}

fn hex(c: (u8, u8, u8)) -> String {
    format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2)
}

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
     .replace('"', "&quot;").replace('\'', "&apos;")
}
