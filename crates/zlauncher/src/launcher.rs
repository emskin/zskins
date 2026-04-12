use std::process::{Command, Stdio};

use gpui::{
    actions, div, img, prelude::*, px, uniform_list, App, Context, Entity, FocusHandle, Focusable,
    FontWeight, KeyBinding, MouseButton, ObjectFit, ScrollStrategy, UniformListScrollHandle,
    Window,
};

use crate::desktop::{self, DesktopEntry};
use crate::input::TextInput;
use crate::theme;

actions!(zlauncher, [MoveUp, MoveDown, Confirm, Dismiss]);

pub fn key_bindings() -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new("up", MoveUp, Some("Launcher")),
        KeyBinding::new("down", MoveDown, Some("Launcher")),
        KeyBinding::new("ctrl-p", MoveUp, Some("Launcher")),
        KeyBinding::new("ctrl-n", MoveDown, Some("Launcher")),
        KeyBinding::new("enter", Confirm, Some("Launcher")),
        KeyBinding::new("escape", Dismiss, None),
    ];
    bindings.extend(crate::input::input_key_bindings());
    bindings
}

pub struct Launcher {
    entries: Vec<DesktopEntry>,
    filtered: Vec<usize>,
    selected: usize,
    text_input: Entity<TextInput>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
}

impl Launcher {
    pub fn new(entries: Vec<DesktopEntry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filtered: Vec<usize> = (0..entries.len()).collect();
        let text_input = cx.new(|cx| TextInput::new("Search applications...", cx));

        let launcher_entity = cx.entity().downgrade();
        text_input.update(cx, |input, _cx| {
            input.set_on_change(Box::new(move |query, cx| {
                if let Some(launcher) = launcher_entity.upgrade() {
                    launcher.update(cx, |this, cx| {
                        this.update_filter(query);
                        cx.notify();
                    });
                }
            }));
        });

        window.focus(&text_input.focus_handle(cx), cx);

        Self {
            entries,
            filtered,
            selected: 0,
            text_input,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
        }
    }

    fn update_filter(&mut self, query: &str) {
        self.filtered.clear();
        let q = query.to_lowercase();
        if q.is_empty() {
            self.filtered.extend(0..self.entries.len());
        } else {
            self.filtered.extend(
                self.entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.search_key.contains(&q))
                    .map(|(i, _)| i),
            );
        }
        self.selected = 0;
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        self.scroll_handle
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
        self.scroll_handle
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(&idx) = self.filtered.get(self.selected) {
            launch(&self.entries[idx]);
        }
        cx.quit();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn render_item(&self, list_ix: usize) -> gpui::Stateful<gpui::Div> {
        let entry_ix = self.filtered[list_ix];
        let entry = &self.entries[entry_ix];
        let sel = list_ix == self.selected;

        let icon = render_icon(entry);

        let content = div()
            .h_full()
            .px(theme::PAD_X)
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(icon)
            .child(
                div()
                    .flex_1()
                    .overflow_x_hidden()
                    .text_size(theme::FONT_SIZE)
                    .font_weight(if sel {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if sel { theme::fg_accent() } else { theme::fg() })
                    .child(entry.name.clone()),
            );

        let mut row = div().h(theme::ITEM_HEIGHT);
        if sel {
            row = row.child(
                div()
                    .size_full()
                    .mx(px(4.0))
                    .rounded(theme::ITEM_RADIUS)
                    .bg(theme::selected_bg())
                    .child(content),
            );
        } else {
            row = row.hover(|s| s.bg(theme::hover_bg())).child(content);
        }

        row.id(list_ix)
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.dispatch_action(&Confirm);
            })
    }
}

impl Render for Launcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.filtered.len();
        let pos = if count > 0 {
            format!("{}/{}", self.selected + 1, count)
        } else {
            "0/0".to_string()
        };

        div()
            .key_context("Launcher")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.dispatch_action(&Dismiss);
            })
            // ── Panel ───────────────────────────────────
            .child(
                div()
                    .w(theme::PANEL_W)
                    .h(theme::PANEL_H)
                    .flex()
                    .flex_col()
                    .bg(theme::panel_bg())
                    .rounded(theme::PANEL_RADIUS)
                    .border_1()
                    .border_color(theme::panel_border())
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, _| {})
                    // ── Search + counter ─────────────────
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .px(theme::PAD_X)
                            .h(px(44.0))
                            .child(div().flex_1().child(self.text_input.clone()))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(theme::FONT_SIZE_SM)
                                    .text_color(theme::fg_dim())
                                    .child(pos),
                            ),
                    )
                    // ── Separator ────────────────────────
                    .child(div().h(px(1.0)).bg(theme::bar_border()))
                    // ── List ─────────────────────────────
                    .child(
                        div()
                            .flex_1()
                            .pt(px(4.0))
                            .overflow_hidden()
                            .child(if count > 0 {
                                div().size_full().child(
                                    uniform_list(
                                        "app-list",
                                        count,
                                        cx.processor(|this, range, _window, _cx| {
                                            let mut items = Vec::new();
                                            for ix in range {
                                                items.push(this.render_item(ix));
                                            }
                                            items
                                        }),
                                    )
                                    .track_scroll(&self.scroll_handle)
                                    .size_full(),
                                )
                            } else {
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(theme::fg_dim())
                                    .text_size(theme::FONT_SIZE)
                                    .child("No matching applications")
                            }),
                    )
                    // ── Bottom bar ───────────────────────
                    .child(
                        div()
                            .h(px(30.0))
                            .px(theme::PAD_X)
                            .flex()
                            .items_center()
                            .justify_end()
                            .border_t_1()
                            .border_color(theme::bar_border())
                            .text_size(theme::FONT_SIZE_SM)
                            .gap(px(16.0))
                            .child(key_hint("Close", "esc"))
                            .child(key_hint("Open", "enter")),
                    ),
            )
    }
}

impl Focusable for Launcher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn key_hint(label: &str, key: &str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(
            div()
                .text_color(theme::fg_accent())
                .font_weight(FontWeight::MEDIUM)
                .child(label.to_string()),
        )
        .child(div().text_color(theme::fg_dim()).child(key.to_string()))
}

fn render_icon(entry: &DesktopEntry) -> gpui::Div {
    if let Some(ref data) = entry.icon_data {
        div().size(theme::ICON_SIZE).flex_shrink_0().child(
            img(data.clone())
                .size(theme::ICON_SIZE)
                .object_fit(ObjectFit::Contain),
        )
    } else if let Some(ref path) = entry.icon_path {
        div().size(theme::ICON_SIZE).flex_shrink_0().child(
            img(path.clone())
                .size(theme::ICON_SIZE)
                .object_fit(ObjectFit::Contain),
        )
    } else {
        div()
            .size(theme::ICON_SIZE)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::fg_dim())
            .text_size(theme::FONT_SIZE_SM)
            .child("\u{25cb}")
    }
}

fn launch(entry: &DesktopEntry) {
    let exec = desktop::strip_field_codes(&entry.exec);
    let parts: Vec<&str> = exec.split_whitespace().collect();
    if let Some((cmd, args)) = parts.split_first() {
        match Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => tracing::info!("launched: {}", entry.name),
            Err(e) => tracing::error!("failed to launch {}: {e}", entry.name),
        }
    }
}
