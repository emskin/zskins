use gpui::{
    Context, IntoElement, ParentElement, Render, Styled, Window, div,
};
use crate::backend::sway::run_window_title_session;
use crate::theme;

pub struct WindowTitleModule {
    title: Option<String>,
}

impl WindowTitleModule {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Only spawn if SWAYSOCK is set; otherwise this module is permanently empty.
        if std::env::var("SWAYSOCK").is_err() {
            return WindowTitleModule { title: None };
        }

        let (tx, rx) = async_channel::unbounded::<Option<String>>();

        cx.background_executor().spawn(async move {
            if let Err(e) = run_window_title_session(tx) {
                log::warn!("window-title session error: {e:#}");
            }
        }).detach();

        cx.spawn(async move |this, cx| {
            while let Ok(title) = rx.recv().await {
                if this.update(cx, |m, cx| { m.title = title; cx.notify(); }).is_err() {
                    return;
                }
            }
        }).detach();

        WindowTitleModule { title: None }
    }
}

impl Render for WindowTitleModule {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.title.clone().unwrap_or_default();
        div()
            .max_w(gpui::px(600.))
            .text_color(theme::fg())
            .child(text)
    }
}
