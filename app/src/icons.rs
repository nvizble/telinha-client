use gpui::{Styled, Svg, px, rgb, svg};

pub const PATHS: &[&str] = &[
    "icons/star.svg",
    "icons/star-filled.svg",
    "icons/monitor.svg",
    "icons/window.svg",
    "icons/volume.svg",
    "icons/mute.svg",
    "icons/fullscreen.svg",
    "icons/restore.svg",
    "icons/link.svg",
    "icons/copy.svg",
    "icons/refresh.svg",
    "icons/check.svg",
    "icons/people.svg",
    "icons/chevron.svg",
    "icons/arrow.svg",
    "icons/close.svg",
    "icons/stop.svg",
    "icons/share.svg",
    "icons/camera.svg",
    "icons/camera-off.svg",
];

pub fn icon(name: &'static str, size: f32, color: u32) -> Svg {
    // GPUI 0.2.2 skips SVG painting without a color on the SVG itself;
    // unlike text, it does not inherit the color from its parent.
    svg()
        .path(name)
        .size(px(size))
        .flex_shrink_0()
        .text_color(rgb(color))
}

pub fn load(path: &str) -> Option<Vec<u8>> {
    let drawing = match path {
        "icons/star.svg" => {
            r#"<path d="m12 2 3.1 6.3 6.9 1-5 4.9 1.2 6.8-6.2-3.2L5.8 21 7 14.2 2 9.3l6.9-1z"/>"#
        }
        "icons/star-filled.svg" => {
            r#"<path fill="currentColor" d="m12 2 3.1 6.3 6.9 1-5 4.9 1.2 6.8-6.2-3.2L5.8 21 7 14.2 2 9.3l6.9-1z"/>"#
        }
        "icons/monitor.svg" => {
            r#"<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8M12 17v4"/>"#
        }
        "icons/window.svg" => {
            r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 9h18M7 6.5h.01M10 6.5h.01"/>"#
        }
        "icons/volume.svg" => {
            r#"<path d="m11 4-6 5H2v6h3l6 5zM15 8a6 6 0 0 1 0 8M18 5a10 10 0 0 1 0 14"/>"#
        }
        "icons/mute.svg" => r#"<path d="m11 4-6 5H2v6h3l6 5zM17 9l5 6M22 9l-5 6"/>"#,
        "icons/fullscreen.svg" => r#"<path d="M8 3H3v5M16 3h5v5M3 16v5h5M21 16v5h-5"/>"#,
        "icons/restore.svg" => r#"<path d="M3 8h5V3M21 8h-5V3M8 21v-5H3M16 21v-5h5"/>"#,
        "icons/link.svg" => {
            r#"<path d="M10 13a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-2 2M14 11a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l2-2"/>"#
        }
        "icons/copy.svg" => {
            r#"<rect x="8" y="8" width="12" height="13" rx="2"/><path d="M16 8V4a1 1 0 0 0-1-1H4a1 1 0 0 0-1 1v11a1 1 0 0 0 1 1h4"/>"#
        }
        "icons/refresh.svg" => {
            r#"<path d="M20 7a9 9 0 0 0-15-2L2 8m0-5v5h5M4 17a9 9 0 0 0 15 2l3-3m0 5v-5h-5"/>"#
        }
        "icons/check.svg" => r#"<path d="m5 12 4 4L19 6"/>"#,
        "icons/people.svg" => {
            r#"<circle cx="9" cy="7" r="3"/><path d="M3 21v-2a6 6 0 0 1 12 0v2M16 4a3 3 0 0 1 0 6M18 14a5 5 0 0 1 3 5v2"/>"#
        }
        "icons/chevron.svg" => r#"<path d="m6 9 6 6 6-6"/>"#,
        "icons/arrow.svg" => r#"<path d="M4 12h16m-6-6 6 6-6 6"/>"#,
        "icons/close.svg" => r#"<path d="m6 6 12 12M6 18 18 6"/>"#,
        "icons/stop.svg" => r#"<rect x="6" y="6" width="12" height="12" rx="2"/>"#,
        "icons/share.svg" => {
            r#"<path d="M8 17H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h16a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1h-4M12 21V10m-4 4 4-4 4 4"/>"#
        }
        "icons/camera.svg" => {
            r#"<rect x="2" y="5" width="14" height="14" rx="3"/><path d="m16 10 6-4v12l-6-4z"/>"#
        }
        "icons/camera-off.svg" => {
            r#"<path d="m3 3 18 18M10 5h3a3 3 0 0 1 3 3v2l6-4v12l-6-4M16 16a3 3 0 0 1-3 3H5a3 3 0 0 1-3-3V8a3 3 0 0 1 1-2"/>"#
        }
        _ => return None,
    };
    Some(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">{drawing}</svg>"#).into_bytes())
}
