use crate::theme;
use chrono::Local;
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window};
use std::time::Duration;

pub struct ClockModule;

impl ClockModule {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        })
        .detach();
        ClockModule
    }
}

impl Render for ClockModule {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let now = Local::now();
        theme::pill()
            .text_color(theme::fg())
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(now.format("%H:%M:%S").to_string())
    }
}
