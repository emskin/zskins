use crate::backend::sway::run_window_title_session;
use crate::theme;
use gpui::{div, px, Context, IntoElement, ParentElement, Render, Styled, Window};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

pub struct WindowTitleModule {
    title: Option<String>,
}

enum TitleSource {
    Sway,
    Niri,
    None,
}

fn detect_title_source() -> TitleSource {
    if let Ok(path) = std::env::var("SWAYSOCK") {
        // SWAYSOCK can linger from a previous sway session — confirm the
        // socket actually accepts a connection before committing to it.
        if UnixStream::connect(&path).is_ok() {
            return TitleSource::Sway;
        }
    }
    if Command::new("niri")
        .arg("msg")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return TitleSource::Niri;
    }
    TitleSource::None
}

impl WindowTitleModule {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<Option<String>>(16);

        match detect_title_source() {
            TitleSource::None => return WindowTitleModule { title: None },
            TitleSource::Sway => {
                let tx = tx.clone();
                cx.background_executor()
                    .spawn(async move {
                        let mut delay_ms: u64 = 1000;
                        loop {
                            match run_window_title_session(tx.clone()) {
                                Ok(()) => {}
                                Err(e) => tracing::warn!(
                                    "window-title (sway) error: {e:#}; reconnecting in {delay_ms}ms"
                                ),
                            }
                            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                            delay_ms = (delay_ms * 2).min(30_000);
                        }
                    })
                    .detach();
            }
            TitleSource::Niri => {
                let tx = tx.clone();
                cx.background_executor()
                    .spawn(async move {
                        let mut delay_ms: u64 = 1000;
                        loop {
                            match run_niri_title_session(&tx) {
                                Ok(()) => {}
                                Err(e) => tracing::warn!(
                                    "window-title (niri) error: {e:#}; reconnecting in {delay_ms}ms"
                                ),
                            }
                            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                            delay_ms = (delay_ms * 2).min(30_000);
                        }
                    })
                    .detach();
            }
        }

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

#[derive(Debug, thiserror::Error)]
enum NiriTitleError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("niri msg exited: {0}")]
    Exit(std::process::ExitStatus),
}

/// Subscribe to `niri msg --json event-stream` and emit the focused
/// window's title whenever it changes.
fn run_niri_title_session(
    tx: &async_channel::Sender<Option<String>>,
) -> Result<(), NiriTitleError> {
    let mut child = Command::new("niri")
        .arg("msg")
        .arg("--json")
        .arg("event-stream")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        NiriTitleError::Io(std::io::Error::other("niri msg: stdout not piped"))
    })?;
    let reader = BufReader::new(stdout);

    let mut titles: HashMap<u64, String> = HashMap::new();
    let mut focused: Option<u64> = None;
    let mut last_emitted: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(obj) = event.as_object() else {
            continue;
        };
        for (kind, payload) in obj {
            match kind.as_str() {
                "WindowsChanged" => {
                    titles.clear();
                    if let Some(arr) = payload.get("windows").and_then(|v| v.as_array()) {
                        for w in arr {
                            let id = w.get("id").and_then(|v| v.as_u64());
                            let title = w.get("title").and_then(|v| v.as_str());
                            if let (Some(id), Some(title)) = (id, title) {
                                titles.insert(id, title.to_string());
                            }
                            if w.get("is_focused").and_then(|v| v.as_bool()) == Some(true) {
                                focused = id;
                            }
                        }
                    }
                }
                "WindowOpenedOrChanged" => {
                    if let Some(w) = payload.get("window") {
                        let id = w.get("id").and_then(|v| v.as_u64());
                        let title = w.get("title").and_then(|v| v.as_str());
                        if let (Some(id), Some(title)) = (id, title) {
                            titles.insert(id, title.to_string());
                            if w.get("is_focused").and_then(|v| v.as_bool()) == Some(true) {
                                focused = Some(id);
                            }
                        }
                    }
                }
                "WindowClosed" => {
                    if let Some(id) = payload.get("id").and_then(|v| v.as_u64()) {
                        titles.remove(&id);
                        if focused == Some(id) {
                            focused = None;
                        }
                    }
                }
                "WindowFocusChanged" => {
                    focused = payload.get("id").and_then(|v| v.as_u64());
                }
                _ => {}
            }
        }

        let current = focused.and_then(|id| titles.get(&id).cloned());
        if current != last_emitted {
            last_emitted = current.clone();
            if tx.send_blocking(current).is_err() {
                break;
            }
        }
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(NiriTitleError::Exit(status))
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
