use gpui::prelude::*;
use gpui::*;
use gpui_component::{tooltip::Tooltip, IconName};
use std::sync::Arc;

use crate::ui::style::{ClaudeLikeColors, TaskTypography};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Dashboard,
    Tasks,
    Records,
    Timeline,
    AI,
    Search,
    Settings,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "看板",
            Self::Tasks => "任务",
            Self::Records => "记录",
            Self::Timeline => "时间线",
            Self::AI => "AI",
            Self::Search => "搜索",
            Self::Settings => "设置",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SidebarLayoutMode {
    Expanded,
    Compact,
}

pub const MAIN_SIDEBAR_EXPANDED_WIDTH: Pixels = px(200.0);
pub const MAIN_SIDEBAR_COMPACT_WIDTH: Pixels = px(64.0);
pub const MAIN_SIDEBAR_BREAKPOINT: Pixels = px(840.0);

pub fn main_sidebar_layout_mode(window_width: Pixels) -> SidebarLayoutMode {
    if window_width >= MAIN_SIDEBAR_BREAKPOINT {
        SidebarLayoutMode::Expanded
    } else {
        SidebarLayoutMode::Compact
    }
}

pub fn main_sidebar_width(layout_mode: SidebarLayoutMode) -> Pixels {
    match layout_mode {
        SidebarLayoutMode::Expanded => MAIN_SIDEBAR_EXPANDED_WIDTH,
        SidebarLayoutMode::Compact => MAIN_SIDEBAR_COMPACT_WIDTH,
    }
}

#[derive(IntoElement)]
pub struct Sidebar {
    current_panel: Panel,
    layout_mode: SidebarLayoutMode,
    on_panel_select: Arc<dyn Fn(Panel, &mut Window, &mut App)>,
}

impl Sidebar {
    pub fn new<F>(on_select: F) -> Self
    where
        F: Fn(Panel, &mut Window, &mut App) + 'static,
    {
        Self {
            current_panel: Panel::Tasks,
            layout_mode: SidebarLayoutMode::Expanded,
            on_panel_select: Arc::new(on_select),
        }
    }

    pub fn with_panel(mut self, panel: Panel) -> Self {
        self.current_panel = panel;
        self
    }

    pub fn with_layout_mode(mut self, layout_mode: SidebarLayoutMode) -> Self {
        self.layout_mode = layout_mode;
        self
    }
}

fn render_sidebar_item(
    panel: Panel,
    label: &'static str,
    icon: IconName,
    current_panel: Panel,
    layout_mode: SidebarLayoutMode,
    id: usize,
    on_click: Arc<dyn Fn(Panel, &mut Window, &mut App)>,
) -> impl IntoElement {
    let is_active = current_panel == panel;
    let is_compact = layout_mode == SidebarLayoutMode::Compact;

    div()
        .id(id)
        .w_full()
        .px(px(if is_compact { 0.0 } else { 10.0 }))
        .py(px(7.0))
        .rounded(px(10.0))
        .cursor_pointer()
        .text_size(px(TaskTypography::SIDEBAR_ITEM_SIZE))
        .font_weight(if is_active {
            TaskTypography::heading_font_weight()
        } else {
            FontWeight::NORMAL
        })
        .text_color(ClaudeLikeColors::text_primary())
        .when(is_active, |this| {
            this.bg(ClaudeLikeColors::selected_surface())
        })
        .hover(|style| style.bg(ClaudeLikeColors::hover_surface()))
        .flex()
        .items_center()
        .when(is_compact, |this| this.justify_center())
        .when(!is_compact, |this| this.gap(px(8.0)))
        .when(is_compact, |this| {
            this.tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
        })
        .child(
            gpui_component::Icon::new(icon)
                .size(px(16.0))
                .text_color(ClaudeLikeColors::text_primary()),
        )
        .when(!is_compact, |this| this.child(label))
        .on_click(move |_event, window, cx| on_click(panel, window, cx))
}

impl RenderOnce for Sidebar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let current_panel = self.current_panel;
        let layout_mode = self.layout_mode;
        let on_panel_select = self.on_panel_select.clone();

        let main_items: Vec<(Panel, &'static str, IconName)> = vec![
            (Panel::Dashboard, "看板", IconName::GalleryVerticalEnd),
            (Panel::Tasks, "任务", IconName::Check),
            (Panel::Records, "记录", IconName::File),
            (Panel::Timeline, "时间线", IconName::Calendar),
            (Panel::AI, "AI", IconName::Bot),
        ];

        let bottom_items: Vec<(Panel, &'static str, IconName)> = vec![
            (Panel::Search, "搜索", IconName::Search),
            (Panel::Settings, "设置", IconName::Settings),
        ];

        let on_panel_select_for_main = on_panel_select.clone();

        div()
            .w(main_sidebar_width(layout_mode))
            .h_full()
            .flex()
            .flex_col()
            .bg(ClaudeLikeColors::sidebar_background())
            .overflow_hidden()
            .text_color(ClaudeLikeColors::text_primary())
            .font_family(TaskTypography::SYSTEM_FONT_FAMILY)
            .child(
                div()
                    .h_full()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .px(px(12.0))
                    .pt(px(20.0))
                    .pb(px(12.0))
                    .child(div().flex().flex_col().gap(px(3.0)).flex_1().children(
                        main_items.into_iter().enumerate().map(
                            move |(idx, (panel, label, icon))| {
                                render_sidebar_item(
                                    panel,
                                    label,
                                    icon,
                                    current_panel,
                                    layout_mode,
                                    idx,
                                    on_panel_select_for_main.clone(),
                                )
                            },
                        ),
                    ))
                    .child(div().flex().flex_col().gap(px(3.0)).pt(px(16.0)).children(
                        bottom_items.into_iter().enumerate().map(
                            move |(idx, (panel, label, icon))| {
                                render_sidebar_item(
                                    panel,
                                    label,
                                    icon,
                                    current_panel,
                                    layout_mode,
                                    idx + 100,
                                    on_panel_select.clone(),
                                )
                            },
                        ),
                    )),
            )
    }
}
