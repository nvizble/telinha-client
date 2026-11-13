use gpui::{prelude::*, *};
use std::time::Duration;

// Geometry and colors match app/assets/icon.svg. Only the display moves;
// the case and stem stay still, including when the display changes color.
pub fn home_logo(size: f32) -> impl IntoElement {
    let scale = size / 96.;
    div()
        .relative()
        .flex_shrink_0()
        .size(px(size))
        .rounded(px(22. * scale))
        .bg(rgb(0x1B1C1F))
        .child(
            div()
                .absolute()
                .left(px(41. * scale))
                .top(px(56. * scale))
                .w(px(14. * scale))
                .h(px(20. * scale))
                .bg(rgb(0xF2F2F0)),
        )
        .with_animation(
            "home-logo-power",
            Animation::new(Duration::from_millis(5600)).repeat(),
            move |logo, progress| {
                // A brief TV-like closing/opening motion separates two long,
                // quiet holds. The loop begins and ends in the same white state.
                let (height, live) = match progress {
                    p if p < 0.18 => (1., false),
                    p if p < 0.24 => (1. - ease_in_out((p - 0.18) / 0.06), false),
                    p if p < 0.32 => (ease_in_out((p - 0.24) / 0.08), true),
                    p if p < 0.66 => (1., true),
                    p if p < 0.72 => (1. - ease_in_out((p - 0.66) / 0.06), true),
                    p if p < 0.80 => (ease_in_out((p - 0.72) / 0.08), false),
                    _ => (1., false),
                };
                let height = 27. * scale * height;
                logo.child(
                    div()
                        .absolute()
                        .left(px(26. * scale))
                        .top(px(34.5 * scale - height / 2.))
                        .w(px(44. * scale))
                        .h(px(height))
                        .rounded(px((5. * scale).min(height / 2.)))
                        .bg(rgb(if live { 0xF0443A } else { 0xF2F2F0 })),
                )
            },
        )
}
