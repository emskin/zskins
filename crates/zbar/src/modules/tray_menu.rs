//! DBusMenu protocol client and popup renderer for tray context menus.

use crate::theme;
use gpui::{div, prelude::*, px, App, Context, FocusHandle, Focusable, MouseButton, Window};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use zbus::zvariant::{OwnedValue, Structure, Value};

// ---------------------------------------------------------------------------
// DBusMenu proxy
// ---------------------------------------------------------------------------

#[zbus::proxy(interface = "com.canonical.dbusmenu", assume_defaults = true)]
trait DBusMenu {
    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &zbus::zvariant::Value<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;

    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: &[&str],
    ) -> zbus::Result<(u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>))>;
}

// ---------------------------------------------------------------------------
// Menu model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub id: i32,
    pub label: String,
    pub menu_type: MenuItemType,
    pub enabled: bool,
    pub visible: bool,
    pub toggle_state: Option<bool>,
    pub submenu: Vec<MenuItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItemType {
    Standard,
    Separator,
}

fn parse_menu_layout(raw: (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>)) -> Vec<MenuItem> {
    let (_id, _props, children) = raw;
    children
        .iter()
        .filter_map(|child| parse_menu_item(child).ok())
        .collect()
}

fn parse_menu_item(value: &OwnedValue) -> anyhow::Result<MenuItem> {
    let structure = value
        .downcast_ref::<&Structure>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let fields = structure.fields();

    let id = fields
        .first()
        .and_then(|v| v.downcast_ref::<i32>().ok())
        .unwrap_or(0);

    let mut label = String::new();
    let mut menu_type = MenuItemType::Standard;
    let mut enabled = true;
    let mut visible = true;
    let mut toggle_state = None;

    if let Some(Value::Dict(dict)) = fields.get(1) {
        if let Ok(Some(s)) = dict.get::<&str, &str>(&"label") {
            label = s.replace('_', "");
        }
        if let Ok(Some(s)) = dict.get::<&str, &str>(&"type") {
            if s == "separator" {
                menu_type = MenuItemType::Separator;
            }
        }
        if let Ok(Some(e)) = dict.get::<&str, bool>(&"enabled") {
            enabled = e;
        }
        if let Ok(Some(v)) = dict.get::<&str, bool>(&"visible") {
            visible = v;
        }
        if let Ok(Some(ts)) = dict.get::<&str, i32>(&"toggle-state") {
            toggle_state = Some(ts == 1);
        }
    }

    let mut submenu = Vec::new();
    if let Some(Value::Array(array)) = fields.get(2) {
        for child in array.iter() {
            if let Ok(owned) = OwnedValue::try_from(child) {
                if let Ok(item) = parse_menu_item(&owned) {
                    submenu.push(item);
                }
            }
        }
    }

    Ok(MenuItem {
        id,
        label,
        menu_type,
        enabled,
        visible,
        toggle_state,
        submenu,
    })
}

// ---------------------------------------------------------------------------
// Fetch and activate
// ---------------------------------------------------------------------------

/// Fetch the full menu tree from a tray item's DBusMenu path.
pub async fn fetch_menu(
    conn: &zbus::Connection,
    addr: &str,
    menu_path: &str,
) -> anyhow::Result<Vec<MenuItem>> {
    let (destination, _) = super::tray::parse_address(addr);
    let proxy = DBusMenuProxy::builder(conn)
        .destination(destination.to_string())?
        .path(menu_path.to_string())?
        .build()
        .await?;

    let _ = proxy.about_to_show(0).await;
    let (_rev, layout) = proxy.get_layout(0, -1, &[]).await?;
    Ok(parse_menu_layout(layout))
}

/// Send a "clicked" event for a menu item.
pub async fn activate_menu_item(
    conn: &zbus::Connection,
    addr: &str,
    menu_path: &str,
    item_id: i32,
) -> anyhow::Result<()> {
    let (destination, _) = super::tray::parse_address(addr);
    let proxy = DBusMenuProxy::builder(conn)
        .destination(destination.to_string())?
        .path(menu_path.to_string())?
        .build()
        .await?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;

    proxy
        .event(item_id, "clicked", &Value::I32(0), timestamp)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Popup component
// ---------------------------------------------------------------------------

pub struct TrayMenuPopup {
    items: Vec<MenuItem>,
    addr: String,
    menu_path: String,
    click_tx: async_channel::Sender<MenuClickReq>,
    /// Notify tray module to clear popup state when we close.
    close_tx: async_channel::Sender<super::tray::TrayMsg>,
    focus_handle: FocusHandle,
}

pub struct MenuClickReq {
    pub addr: String,
    pub menu_path: String,
    pub item_id: i32,
}

impl TrayMenuPopup {
    pub(crate) fn new(
        items: Vec<MenuItem>,
        addr: String,
        menu_path: String,
        click_tx: async_channel::Sender<MenuClickReq>,
        close_tx: async_channel::Sender<super::tray::TrayMsg>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            items,
            addr,
            menu_path,
            click_tx,
            close_tx,
            focus_handle: cx.focus_handle(),
        }
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, _cx: &mut Context<Self>) {
        let _ = self.close_tx.try_send(super::tray::TrayMsg::CloseMenu);
        window.remove_window();
    }
}

gpui::actions!(zbar_tray_menu, [Dismiss]);

pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    vec![gpui::KeyBinding::new("escape", Dismiss, Some("TrayMenu"))]
}

impl Focusable for TrayMenuPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TrayMenuPopup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div()
            .key_context("TrayMenu")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::dismiss))
            .bg(theme::bg())
            .text_color(theme::fg())
            .text_size(theme::FONT_SIZE)
            .border_1()
            .border_color(theme::border())
            .rounded_md()
            .p_1()
            .flex()
            .flex_col()
            .min_w(px(180.))
            .overflow_hidden();

        for item in &self.items {
            if !item.visible {
                continue;
            }
            col = col.child(self.render_item(item));
        }

        col
    }
}

impl TrayMenuPopup {
    fn render_item(&self, item: &MenuItem) -> gpui::AnyElement {
        if item.menu_type == MenuItemType::Separator {
            return div()
                .h(px(1.))
                .my(px(3.))
                .bg(theme::separator())
                .into_any_element();
        }

        let text_color = if item.enabled {
            theme::fg()
        } else {
            theme::fg_dim()
        };

        let click_tx = self.click_tx.clone();
        let addr = self.addr.clone();
        let menu_path = self.menu_path.clone();
        let item_id = item.id;
        let enabled = item.enabled;

        let mut row = div()
            .id(("menu-item", item.id as usize))
            .px_2()
            .py(px(4.))
            .rounded_sm()
            .text_color(text_color)
            .flex()
            .items_center()
            .gap_2();

        if enabled {
            row = row
                .cursor_pointer()
                .hover(|s| s.bg(theme::surface_hover()))
                .on_mouse_down(MouseButton::Left, move |_, window, _cx| {
                    let _ = click_tx.try_send(MenuClickReq {
                        addr: addr.clone(),
                        menu_path: menu_path.clone(),
                        item_id,
                    });
                    window.remove_window();
                });
        }

        // Toggle indicator.
        if let Some(checked) = item.toggle_state {
            let indicator = if checked { "✓" } else { " " };
            row = row.child(
                div()
                    .w(px(14.))
                    .text_color(theme::accent())
                    .child(indicator.to_string()),
            );
        }

        row = row.child(div().flex_1().child(item.label.clone()));

        // Submenu arrow.
        if !item.submenu.is_empty() {
            row = row.child(div().text_color(theme::fg_dim()).child("▸"));
        }

        row.into_any_element()
    }
}

// ---------------------------------------------------------------------------
// Open popup helper
// ---------------------------------------------------------------------------

use gpui::{
    layer_shell::*, point, Bounds, DisplayId, Size, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn open_menu_popup(
    cx: &mut App,
    items: Vec<MenuItem>,
    addr: String,
    menu_path: String,
    click_tx: async_channel::Sender<MenuClickReq>,
    close_tx: async_channel::Sender<super::tray::TrayMsg>,
    display_id: Option<DisplayId>,
    click_x: f32,
) -> Option<gpui::WindowHandle<TrayMenuPopup>> {
    let visible_count = items.iter().filter(|i| i.visible).count().max(1);
    let height = (visible_count as f32) * 26.0 + 12.0;
    let menu_width: f32 = 220.0;

    // Position menu so its left edge aligns with the click X, clamped to screen.
    // With layer-shell we can only use LEFT anchor + left margin.
    let left_margin = (click_x - menu_width / 2.0).max(0.0);

    let opts = WindowOptions {
        titlebar: None,
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: Size::new(px(menu_width), px(height)),
        })),
        display_id,
        app_id: Some("zbar-tray-menu".to_string()),
        window_background: WindowBackgroundAppearance::Transparent,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "zbar-tray-menu".to_string(),
            layer: Layer::Top,
            anchor: Anchor::TOP | Anchor::LEFT,
            margin: Some((px(0.), px(0.), px(0.), px(left_margin))),
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            exclusive_zone: None,
            ..Default::default()
        }),
        ..Default::default()
    };

    match cx.open_window(opts, |_, cx| {
        cx.new(|cx| TrayMenuPopup::new(items, addr, menu_path, click_tx, close_tx, cx))
    }) {
        Ok(handle) => Some(handle),
        Err(e) => {
            tracing::warn!("tray: failed to open menu popup: {e}");
            None
        }
    }
}
