mod bar;

use gpui::{
    layer_shell::*, point, px, App, AppContext, Bounds, Size, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions,
};
use gpui_platform::application;
use zbar::theme::BAR_HEIGHT;

use crate::bar::Bar;

fn main() {
    env_logger::init();

    let backend = zbar::backend::detect::detect_backend();

    application().run(move |cx: &mut App| {
        let backend = backend.clone();
        cx.open_window(
            WindowOptions {
                titlebar: None,
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: Size::new(px(1920.), BAR_HEIGHT),
                })),
                app_id: Some("zbar".to_string()),
                window_background: WindowBackgroundAppearance::Transparent,
                kind: WindowKind::LayerShell(LayerShellOptions {
                    namespace: "zbar".to_string(),
                    layer: Layer::Top,
                    anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
                    exclusive_zone: Some(BAR_HEIGHT),
                    keyboard_interactivity: KeyboardInteractivity::None,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Bar::new(backend, cx)),
        )
        .unwrap();
    });
}
