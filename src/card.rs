//! The card's SVG, shared by the Worker (lib.rs) and the native server (server/).

use std::collections::HashMap;

pub fn generate_svg(
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
          <text x="460" y="270" fill="#4B5563" font-size="11" font-family="Roboto" text-anchor="end">&#169; CNCVerse</text>
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

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
     .replace('"', "&quot;").replace('\'', "&apos;")
}