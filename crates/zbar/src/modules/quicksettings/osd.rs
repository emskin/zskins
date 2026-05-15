//! Brightness OSD — screen-centered overlay shown on the display being
//! adjusted via the brightness slider. Auto-dismisses ~700ms after the
//! last update (timer driven from the panel's drag-end handler).

use gpui::{div, prelude::*, px, Context, Window};
use ztheme::Theme;

pub(super) const OSD_SIZE_PX: f32 = 160.0;

pub struct BrightnessOsd {
    pub(super) pct: u32,
    pub(super) label: String,
}

impl BrightnessOsd {
    pub(super) fn new(pct: u32, label: String, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        Self { pct, label }
    }
}

impl Render for BrightnessOsd {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.global::<Theme>();
        let mut bg = t.bg;
        bg.a = 1.0;
        let pct = self.pct.min(100);
        let fill_frac = pct as f32 / 100.0;
        // Full-screen transparent root; a centered 160x160 card carries
        // the actual visuals.
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(OSD_SIZE_PX))
                    .h(px(OSD_SIZE_PX))
                    .rounded_lg()
                    .bg(bg)
                    .border_1()
                    .border_color(t.border)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(48.))
                            .text_color(t.accent)
                            .child("\u{f00de}".to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(26.))
                            .text_color(t.fg)
                            .child(format!("{pct}%")),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(OSD_SIZE_PX - 40.0))
                            .h(px(4.))
                            .rounded_full()
                            .bg(t.border)
                            .child(
                                div()
                                    .absolute()
                                    .left(px(0.))
                                    .top(px(0.))
                                    .bottom(px(0.))
                                    .w(gpui::relative(fill_frac.max(0.001)))
                                    .rounded_full()
                                    .bg(t.accent),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.fg_dim)
                            .child(self.label.clone()),
                    ),
            )
    }
}
