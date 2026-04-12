use crate::backend::sway::run_window_title_session;
use crate::theme;
use gpui::{div, px, Context, IntoElement, ParentElement, Render, Styled, Window};

pub struct WindowTitleModule {
    title: Option<String>,
}

impl WindowTitleModule {
    pub fn new(cx: &mut Context<Self>) -> Self {
        if std::env::var("SWAYSOCK").is_err() {
            return WindowTitleModule { title: None };
        }

        let (tx, rx) = async_channel::bounded::<Option<String>>(16);

        cx.background_executor()
            .spawn(async move {
                let mut delay_ms: u64 = 1000;
                loop {
                    match run_window_title_session(tx.clone()) {
                        Ok(()) => {}
                        Err(e) => tracing::warn!(
                            "window-title session error: {e:#}; reconnecting in {delay_ms}ms"
                        ),
                    }
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    delay_ms = (delay_ms * 2).min(30_000);
                }
            })
            .detach();

        cx.spawn(async move |this, cx| {
            while let Ok(title) = rx.recv().await {
                if this
                    .update(cx, |m, cx| {
                        m.title = title;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        WindowTitleModule { title: None }
    }
}

impl Render for WindowTitleModule {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.title.clone().unwrap_or_default();
        div()
            .max_w(px(500.))
            .overflow_x_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .text_color(theme::fg_dim())
            .child(text)
    }
}
