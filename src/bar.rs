use std::sync::Arc;
use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    div, prelude::*,
};
use zbar::backend::WorkspaceBackend;
use zbar::modules::clock::ClockModule;
use zbar::modules::window_title::WindowTitleModule;
use zbar::modules::workspaces::WorkspacesModule;
use zbar::theme;

pub struct Bar {
    workspaces: Entity<WorkspacesModule>,
    window_title: Entity<WindowTitleModule>,
    clock: Entity<ClockModule>,
}

impl Bar {
    pub fn new(
        backend: Option<Arc<dyn WorkspaceBackend>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspaces = cx.new(|cx| WorkspacesModule::new(backend, cx));
        let window_title = cx.new(WindowTitleModule::new);
        let clock = cx.new(ClockModule::new);
        Bar { workspaces, window_title, clock }
    }
}

impl Render for Bar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .px(theme::PADDING_X)
            .bg(theme::bg())
            .text_color(theme::fg())
            .text_size(theme::FONT_SIZE)
            .child(
                div().flex_1().flex().items_center().gap(theme::MODULE_GAP)
                    .child(self.workspaces.clone())
            )
            .child(
                div().flex_1().flex().items_center().justify_center()
                    .child(self.window_title.clone())
            )
            .child(
                div().flex_1().flex().items_center().justify_end()
                    .child(self.clock.clone())
            )
    }
}
