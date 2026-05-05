use gpui::{
    div, img, prelude::*, px, rgb, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    WindowControlArea,
};

use crate::{LazytermApp, BG, BORDER, ROW_ACTIVE, SURFACE, TEXT_SOFT};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IconKind {
    NewPane,
    SplitLayout,
    CommandPalette,
    Minimize,
    Maximize,
    Close,
}

impl IconKind {
    pub(super) fn asset_path(self) -> &'static str {
        match self {
            Self::NewPane => "icons/plus.svg",
            Self::SplitLayout => "icons/split.svg",
            Self::CommandPalette => "icons/command.svg",
            Self::Minimize => "icons/minus.svg",
            Self::Maximize => "icons/maximize.svg",
            Self::Close => "icons/close.svg",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NewPane => "new pane",
            Self::SplitLayout => "toggle split layout",
            Self::CommandPalette => "command palette",
            Self::Minimize => "minimize window",
            Self::Maximize => "maximize window",
            Self::Close => "close window",
        }
    }
}

fn window_control_area_for_icon(icon: IconKind) -> Option<WindowControlArea> {
    match icon {
        IconKind::Minimize => Some(WindowControlArea::Min),
        IconKind::Maximize => Some(WindowControlArea::Max),
        IconKind::Close => Some(WindowControlArea::Close),
        _ => None,
    }
}

pub(super) struct TooltipView {
    pub(super) label: SharedString,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(3.0))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .font_family("JetBrains Mono")
            .text_size(px(10.0))
            .text_color(rgb(TEXT_SOFT))
            .child(self.label.clone())
    }
}

impl LazytermApp {
    pub(super) fn render_rail_header(&self) -> impl IntoElement {
        div().flex().flex_col().items_center().gap_1().pb_1().child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h(px(34.0))
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, window, _| {
                    window.start_window_move();
                })
                .id("rail-window-drag")
                .tooltip(|_, cx| {
                    cx.new(|_| TooltipView {
                        label: SharedString::from("drag window"),
                    })
                    .into()
                })
                .child(
                    div()
                        .size(px(18.0))
                        .rounded(px(3.0))
                        .overflow_hidden()
                        .child(img("logoblackbackground.svg").size_full()),
                ),
        )
    }

    pub(super) fn render_titlebar_button(
        &self,
        icon: IconKind,
        id: &'static str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(24.0))
            .h(px(22.0))
            .rounded(px(3.0))
            .bg(rgb(BG))
            .when_some(window_control_area_for_icon(icon), |this, area| {
                this.window_control_area(area)
            })
            .hover(|this| this.bg(rgb(ROW_ACTIVE)))
            .child(
                img(icon.asset_path())
                    .w(px(12.0))
                    .h(px(12.0))
                    .id(format!("{}-icon", icon.label())),
            )
            .id(id)
            .tooltip(move |_, cx| {
                cx.new(move |_| TooltipView {
                    label: SharedString::from(icon.label()),
                })
                .into()
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                action(this, window);
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    fn render_window_control_button(
        &self,
        icon: IconKind,
        id: &'static str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Window) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.0))
            .h(px(28.0))
            .rounded(px(4.0))
            .when_some(window_control_area_for_icon(icon), |this, area| {
                this.window_control_area(area)
            })
            .hover(|this| this.bg(rgb(SURFACE)))
            .child(
                img(icon.asset_path())
                    .w(px(14.0))
                    .h(px(14.0))
                    .id(format!("{}-window-icon", icon.label())),
            )
            .id(id)
            .tooltip(move |_, cx| {
                cx.new(move |_| TooltipView {
                    label: SharedString::from(icon.label()),
                })
                .into()
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                action(window);
                this.focus_terminal(window, cx);
                cx.notify();
            }))
    }

    pub(super) fn render_window_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .flex()
            .items_center()
            .gap(px(1.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG))
            .id("window-controls")
            .child(self.render_window_control_button(
                IconKind::Minimize,
                "window-minimize",
                cx,
                |window| {
                    window.minimize_window();
                },
            ))
            .child(self.render_window_control_button(
                IconKind::Maximize,
                "window-maximize",
                cx,
                |window| {
                    window.zoom_window();
                },
            ))
            .child(self.render_window_control_button(
                IconKind::Close,
                "window-close",
                cx,
                |window| {
                    window.remove_window();
                },
            ))
    }

    pub(super) fn render_window_drag_strip(&self) -> impl IntoElement {
        div()
            .absolute()
            .top(px(0.0))
            .left(px(self.sidebar_width() + 8.0))
            .right(px(112.0))
            .h(px(7.0))
            .window_control_area(WindowControlArea::Drag)
            .id("window-top-drag-strip")
            .tooltip(|_, cx| {
                cx.new(|_| TooltipView {
                    label: SharedString::from("drag window"),
                })
                .into()
            })
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                window.start_window_move();
            })
    }
}
