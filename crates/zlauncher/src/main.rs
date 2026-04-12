mod desktop;
mod icon;
mod input;
mod launcher;
mod theme;

use gpui::{
    layer_shell::*, App, AppContext, Bounds, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions,
};
use gpui_platform::application;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let entries = desktop::load_entries();
    tracing::info!("loaded {} desktop entries", entries.len());

    let entries = icon::resolve_icons(entries);

    application().run(move |cx: &mut App| {
        cx.bind_keys(launcher::key_bindings());

        cx.open_window(
            WindowOptions {
                titlebar: None,
                window_bounds: Some(WindowBounds::Windowed(Bounds::maximized(None, cx))),
                app_id: Some("zlauncher".to_string()),
                window_background: WindowBackgroundAppearance::Transparent,
                kind: WindowKind::LayerShell(LayerShellOptions {
                    namespace: "zlauncher".to_string(),
                    layer: Layer::Overlay,
                    anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                    exclusive_zone: None,
                    keyboard_interactivity: KeyboardInteractivity::Exclusive,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| launcher::Launcher::new(entries, window, cx)),
        )
        .expect("failed to open launcher window: check compositor supports layer-shell");
    });
}
