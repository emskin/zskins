//! Stateless render helpers used by the QuickSettingsPanel views.

use super::panel::{QuickSettingsPanel, SliderKind, View};
use super::{SLIDER_TRACK_LEFT_PX, SLIDER_TRACK_W_PX};
use crate::modules::battery::BatteryStatus;
use gpui::{div, prelude::*, px, App, Context, MouseButton, Window};
use std::time::Duration;
use ztheme::Theme;

pub(super) fn pill<F>(
    cx: &mut Context<QuickSettingsPanel>,
    icon: &str,
    title: &str,
    sub: &str,
    on: bool,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut App) + 'static,
{
    let t = *cx.global::<Theme>();
    let (bg, fg_title, fg_sub, icon_bg, icon_fg) = if on {
        (
            t.accent,
            gpui::white(),
            gpui::rgba(0xffffffc7).into(),
            gpui::rgba(0xffffff29).into(),
            gpui::white(),
        )
    } else {
        (t.surface_hover, t.fg, t.fg_dim, t.surface, t.fg_dim)
    };
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(10.))
        .rounded_md()
        .bg(bg)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_click(cx))
        .child(
            div()
                .w(px(32.))
                .h(px(32.))
                .flex_shrink_0()
                .rounded_full()
                .bg(icon_bg)
                .text_color(icon_fg)
                .text_size(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .child(icon.to_string()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .overflow_hidden()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(fg_title)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(fg_sub)
                        .child(sub.to_string()),
                ),
        )
        .into_any_element()
}

pub(super) fn slider_row<F>(
    cx: &mut Context<QuickSettingsPanel>,
    kind: SliderKind,
    icon: &str,
    value: u32,
    on_change: F,
) -> gpui::AnyElement
where
    F: Fn(u32, &mut App) + 'static + Clone,
{
    let t = *cx.global::<Theme>();
    let pct = value.min(100);
    let fill_frac = pct as f32 / 100.0;
    let weak = cx.entity().downgrade();

    // Mouse-down on the track immediately commits the click position AND
    // arms drag state. Subsequent mouse_move events on the panel root
    // keep updating while held, mouse_up there clears the drag.
    let on_change_md = on_change.clone();
    let click_handler = move |ev: &gpui::MouseDownEvent, _: &mut Window, cx: &mut App| {
        let frac = ((f32::from(ev.position.x) - SLIDER_TRACK_LEFT_PX) / SLIDER_TRACK_W_PX)
            .clamp(0.0, 1.0);
        let new_pct = (frac * 100.0).round() as u32;
        on_change_md(new_pct, cx);
        let _ = weak.update(cx, |this: &mut QuickSettingsPanel, _| {
            this.drag = Some(kind);
        });
    };

    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(8.))
        .py(px(6.))
        .rounded_md()
        .hover(move |s| s.bg(t.surface_hover))
        .child(
            div()
                .w(px(22.))
                .flex_shrink_0()
                .text_size(px(16.))
                .text_color(t.fg_dim)
                .child(icon.to_string()),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .h(px(24.))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, click_handler)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .top(px(10.))
                        .bottom(px(10.))
                        .rounded_full()
                        .bg(t.border),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(10.))
                        .bottom(px(10.))
                        .left(px(0.))
                        .w(gpui::relative(fill_frac.max(0.001)))
                        .rounded_full()
                        .bg(t.accent),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .left(px(0.))
                        .h(px(24.))
                        .w(gpui::relative(fill_frac))
                        .flex()
                        .items_center()
                        .justify_end()
                        .child(
                            div()
                                .w(px(18.))
                                .h(px(18.))
                                .mr(px(-9.))
                                .rounded_full()
                                .bg(t.surface)
                                .border_1()
                                .border_color(t.border),
                        ),
                ),
        )
        .child(
            div()
                .min_w(px(28.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_color(t.fg_dim)
                .text_size(px(11.))
                .child(format!("{pct}")),
        )
        .into_any_element()
}

pub(super) fn nav_row<F>(
    cx: &mut Context<QuickSettingsPanel>,
    icon: &str,
    label: &str,
    value: &str,
    on_click: F,
) -> gpui::AnyElement
where
    F: Fn(&mut App) + 'static,
{
    let t = *cx.global::<Theme>();
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .pl(px(8.))
        .pr(px(10.))
        .py(px(10.))
        .rounded_md()
        .cursor_pointer()
        .hover(move |s| s.bg(t.surface_hover))
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_click(cx))
        .child(
            div()
                .w(px(28.))
                .flex_shrink_0()
                .text_size(px(18.))
                .text_color(t.fg)
                .child(icon.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(t.fg)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.fg_dim)
                        .child(value.to_string()),
                ),
        )
        .child(div().text_size(px(14.)).text_color(t.fg_dim).child("›"))
        .into_any_element()
}

/// Render a human "约 4 小时 12 分钟" / "约 35 分钟" string for a remaining
/// duration. Returns `None` for durations under a minute.
fn fmt_duration_zh(d: Duration) -> Option<String> {
    let total_min = d.as_secs() / 60;
    if total_min == 0 {
        return None;
    }
    let (h, m) = (total_min / 60, total_min % 60);
    Some(match (h, m) {
        (0, m) => format!("约 {m} 分钟"),
        (h, 0) => format!("约 {h} 小时"),
        (h, m) => format!("约 {h} 小时 {m} 分钟"),
    })
}

pub(super) fn render_battery_strip(
    cx: &mut Context<QuickSettingsPanel>,
    cap: u8,
    status: BatteryStatus,
    time_remaining: Option<Duration>,
) -> gpui::AnyElement {
    let t = *cx.global::<Theme>();
    let cap_str = format!("{cap}%");
    let (chip_text, icon_color) = match status {
        BatteryStatus::Charging => (Some("充电中"), t.success),
        BatteryStatus::Full => (Some("已充满"), t.success),
        BatteryStatus::Discharging => (None, t.fg),
        BatteryStatus::Unknown => (None, t.fg_dim),
    };
    // Real estimate from the energy/power counters; the prefix follows the
    // charge direction. Absent on Full/Unknown or when sysfs lacks the
    // counters, in which case the secondary line is simply omitted.
    let time_text = time_remaining.and_then(fmt_duration_zh).map(|s| match status {
        BatteryStatus::Charging => format!("距充满 {s}"),
        _ => format!("剩余 {s}"),
    });
    let mut info = div()
        .flex_1()
        .flex()
        .items_baseline()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(14.))
                .text_color(t.fg)
                .child(cap_str),
        );
    if let Some(time_text) = time_text {
        info = info.child(
            div()
                .text_size(px(12.))
                .text_color(t.fg_dim)
                .child(time_text),
        );
    }
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(10.))
        .rounded_md()
        .bg(t.surface_hover)
        .child(
            div()
                .w(px(28.))
                .flex_shrink_0()
                .text_size(px(20.))
                .text_color(icon_color)
                .child("󰂄".to_string()),
        )
        .child(info);
    if let Some(label) = chip_text {
        row = row.child(
            div()
                .text_size(px(10.))
                .px(px(7.))
                .py(px(3.))
                .rounded_full()
                .bg(t.success)
                .text_color(gpui::white())
                .child(label.to_string()),
        );
    }
    row.into_any_element()
}

pub(super) fn secondary_header<F>(
    cx: &mut Context<QuickSettingsPanel>,
    title: &str,
    enabled: bool,
    on_toggle: F,
) -> gpui::AnyElement
where
    F: Fn(&mut App) + 'static,
{
    let t = *cx.global::<Theme>();
    let entity = cx.entity().clone();
    div()
        .flex()
        .items_center()
        .gap_2()
        .pb_2()
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .w(px(28.))
                .h(px(28.))
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(t.fg_dim)
                .hover(move |s| s.bg(t.surface_hover).text_color(t.fg))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    entity.update(cx, |p, cx| {
                        p.view = View::Main;
                        cx.notify();
                    });
                })
                .child("‹"),
        )
        .child(div().flex_1().text_color(t.fg).child(title.to_string()))
        .child(simple_switch(cx, enabled, on_toggle))
        .into_any_element()
}

fn simple_switch<F>(
    cx: &mut Context<QuickSettingsPanel>,
    on: bool,
    on_toggle: F,
) -> gpui::AnyElement
where
    F: Fn(&mut App) + 'static,
{
    let t = *cx.global::<Theme>();
    let track_bg = if on { t.accent } else { t.border };
    let knob_left = if on { px(16.) } else { px(2.) };
    div()
        .relative()
        .w(px(34.))
        .h(px(20.))
        .rounded_full()
        .bg(track_bg)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_toggle(cx))
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(knob_left)
                .w(px(16.))
                .h(px(16.))
                .rounded_full()
                .bg(gpui::white()),
        )
        .into_any_element()
}

pub(super) fn section_label(cx: &mut Context<QuickSettingsPanel>, text: &str) -> gpui::AnyElement {
    let t = *cx.global::<Theme>();
    div()
        .px_2()
        .pt_3()
        .pb_1()
        .text_size(px(10.))
        .text_color(t.fg_dim)
        .child(text.to_string().to_uppercase())
        .into_any_element()
}

pub(super) fn placeholder_row(
    cx: &mut Context<QuickSettingsPanel>,
    text: &str,
) -> gpui::AnyElement {
    let t = *cx.global::<Theme>();
    div()
        .px(px(8.))
        .py(px(8.))
        .text_size(px(12.))
        .text_color(t.fg_dim)
        .child(text.to_string())
        .into_any_element()
}
