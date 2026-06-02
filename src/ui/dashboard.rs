use crate::models::{Priority, Record, RecordType, TaskStatus};
use crate::store::{DashboardData, StatsData, Store, StoreEvent};
use crate::ui::record_detail_sidebar::{RecordDetailSidebar, SavePayload as RecordSavePayload};
use crate::ui::task_detail_sidebar::{SavePayload as TaskSavePayload, TaskDetailSidebar};
use crate::ui::task_panel::TaskFocusPreset;
use chrono::{Duration, Local, Utc};
use gpui::{prelude::*, *};
use gpui_component::{h_flex, scroll::ScrollableElement, v_flex};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum DashboardAction {
    OpenTaskPreset(TaskFocusPreset),
    OpenTimeline,
    FilterByTag(String),
    FilterByPerson(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DashboardPage {
    Overview,
    Stats,
}

pub struct Dashboard {
    store: Store,
    dashboard_data: Option<DashboardData>,
    stats_data: Option<StatsData>,
    page: DashboardPage,
    focus_handle: FocusHandle,
    task_detail_sidebar: Entity<TaskDetailSidebar>,
    record_detail_sidebar: Entity<RecordDetailSidebar>,
    on_action: Option<Box<dyn Fn(DashboardAction, &mut Window, &mut Context<Self>) + Send + Sync>>,
    _window_activation_subscription: Subscription,
}

impl Dashboard {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let sidebar_task_store = store.clone();
        let sidebar_record_store = store.clone();
        let mut dashboard = Self {
            store,
            dashboard_data: None,
            stats_data: None,
            page: DashboardPage::Overview,
            focus_handle,
            task_detail_sidebar: cx
                .new(|cx| TaskDetailSidebar::new(sidebar_task_store.clone(), window, cx)),
            record_detail_sidebar: cx
                .new(|cx| RecordDetailSidebar::new(sidebar_record_store.clone(), window, cx)),
            on_action: None,
            _window_activation_subscription: cx.observe_window_activation(
                window,
                |this, window, cx| {
                    if window.is_window_active() {
                        this.load_data(cx);
                    }
                },
            ),
        };

        let handle = cx.entity().clone();
        dashboard.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_save(move |payload, cx| {
                handle.update(cx, |this, cx| {
                    this.handle_task_sidebar_save(&payload, cx);
                });
            });
        });
        let handle = cx.entity().clone();
        dashboard.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_delete(move |task_id, cx| {
                if let Ok(task_id) = Uuid::parse_str(&task_id) {
                    handle.update(cx, |this, cx| {
                        this.delete_record(task_id, cx);
                    });
                }
            });
        });
        let handle = cx.entity().clone();
        dashboard.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_close(move |cx| {
                handle.update(cx, |this, cx| {
                    this.dismiss_sidebars(cx);
                });
            });
        });

        let handle = cx.entity().clone();
        dashboard.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_save(move |payload, cx| {
                handle.update(cx, |this, cx| {
                    this.handle_record_sidebar_save(&payload, cx);
                });
            });
        });
        let handle = cx.entity().clone();
        dashboard.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_delete(move |record_id, cx| {
                if let Ok(record_id) = Uuid::parse_str(&record_id) {
                    handle.update(cx, |this, cx| {
                        this.delete_record(record_id, cx);
                    });
                }
            });
        });
        let handle = cx.entity().clone();
        dashboard.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_close(move |cx| {
                handle.update(cx, |this, cx| {
                    this.dismiss_sidebars(cx);
                });
            });
        });

        dashboard.install_data_change_listener(cx);
        dashboard.refresh_data(cx);
        dashboard
    }

    pub fn on_action<F>(&mut self, callback: F)
    where
        F: Fn(DashboardAction, &mut Window, &mut Context<Self>) + Send + Sync + 'static,
    {
        self.on_action = Some(Box::new(callback));
    }

    pub fn refresh_data(&mut self, cx: &mut Context<Self>) {
        self.load_data(cx);
    }

    fn install_data_change_listener(&mut self, cx: &mut Context<Self>) {
        let mut changes = self.store.subscribe_data_changes();
        cx.spawn(async move |view, cx| loop {
            match changes.recv().await {
                Ok(StoreEvent::DataChanged)
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if view
                        .update(cx, |this, cx| {
                            this.refresh_data(cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        })
        .detach();
    }

    fn load_data(&mut self, cx: &mut Context<Self>) {
        self.load_dashboard(cx);
        self.load_stats(cx);
    }

    fn load_dashboard(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |view, cx| match store.get_dashboard().await {
            Ok(data) => {
                let _ = view.update(cx, |this, cx| {
                    this.dashboard_data = Some(data);
                    cx.notify();
                });
            }
            Err(err) => {
                eprintln!("[Dashboard] Failed to load dashboard data: {}", err);
            }
        })
        .detach();
    }

    fn load_stats(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |view, cx| match store.get_stats().await {
            Ok(data) => {
                let _ = view.update(cx, |this, cx| {
                    this.stats_data = Some(data);
                    cx.notify();
                });
            }
            Err(err) => {
                eprintln!("[Dashboard] Failed to load stats data: {}", err);
            }
        })
        .detach();
    }

    fn set_page(&mut self, page: DashboardPage, cx: &mut Context<Self>) {
        if self.page != page {
            self.page = page;
            self.dismiss_sidebars(cx);
            if page == DashboardPage::Stats && self.stats_data.is_none() {
                self.load_stats(cx);
            }
            cx.notify();
        }
    }

    fn emit_action(
        &mut self,
        action: DashboardAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(callback) = &self.on_action {
            callback(action, window, cx);
        }
    }

    fn dismiss_sidebars(&mut self, cx: &mut Context<Self>) {
        self.task_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.dismiss(cx);
        });
        self.record_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.dismiss(cx);
        });
    }

    fn show_record_detail(&mut self, record: &Record, window: &mut Window, cx: &mut Context<Self>) {
        self.task_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.dismiss(cx);
        });
        self.record_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.show_record(record, window, cx);
        });
        cx.notify();
    }

    fn show_task_detail(&mut self, task: &Record, window: &mut Window, cx: &mut Context<Self>) {
        self.record_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.dismiss(cx);
        });
        self.task_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.show_task(task, window, cx);
        });
        cx.notify();
    }

    fn open_record_from_dashboard(
        &mut self,
        record: &Record,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match record.record_type {
            RecordType::Task => self.show_task_detail(record, window, cx),
            RecordType::Note | RecordType::Idea | RecordType::Event => {
                self.show_record_detail(record, window, cx)
            }
        }
    }

    fn handle_task_sidebar_save(&mut self, payload: &TaskSavePayload, cx: &mut Context<Self>) {
        let Ok(task_id) = Uuid::parse_str(&payload.task_id) else {
            return;
        };

        let store = self.store.clone();
        let payload = payload.clone();
        cx.spawn(async move |view, cx| {
            let line_ref = payload.line.clone();
            let Some(mut task) = store.get_record_by_id(task_id).await.ok().flatten() else {
                return;
            };

            let previous_status = task.status.clone();
            task.title = payload.title.clone();
            task.content = payload.content.clone();
            task.priority = Some(payload.priority.clone());
            task.status = Some(payload.status.clone());
            task.due_date = payload.due_date;
            task.scheduled_for = payload.scheduled_for;
            task.cancelled_reason = payload.cancel_reason.clone();
            task.tags = payload.tags.clone();
            task.persons = payload.persons.clone();
            let now = Utc::now();
            task.updated_at = now;
            task.sync_task_lifecycle_fields(previous_status, now);

            if let Err(err) = store.update_record_with_line(task, line_ref).await {
                eprintln!("[Dashboard] Failed to update task: {}", err);
                return;
            }

            let _ = view.update(cx, |this, cx| {
                this.load_data(cx);
            });
        })
        .detach();
    }

    fn handle_record_sidebar_save(&mut self, payload: &RecordSavePayload, cx: &mut Context<Self>) {
        let Ok(record_id) = Uuid::parse_str(&payload.record_id) else {
            return;
        };

        let store = self.store.clone();
        let payload = payload.clone();
        cx.spawn(async move |view, cx| {
            let line_ref = payload.line.clone();
            let Some(mut record) = store.get_record_by_id(record_id).await.ok().flatten() else {
                return;
            };

            record.title = payload.title.clone();
            record.content = payload.content.clone();
            record.tags = payload.tags.clone();
            record.persons = payload.persons.clone();
            record.updated_at = Utc::now();

            if let Err(err) = store.update_record_with_line(record, line_ref).await {
                eprintln!("[Dashboard] Failed to update record: {}", err);
                return;
            }

            let _ = view.update(cx, |this, cx| {
                this.load_data(cx);
            });
        })
        .detach();
    }

    fn delete_record(&mut self, record_id: Uuid, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            if let Err(err) = store.delete_record(record_id).await {
                eprintln!("[Dashboard] Failed to delete record: {}", err);
                return;
            }

            let _ = view.update(cx, |this, cx| {
                this.dismiss_sidebars(cx);
                this.load_data(cx);
            });
        })
        .detach();
    }

    fn display_text(record: &Record, limit: usize) -> String {
        let base = record
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                record
                    .content
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_string())
            })
            .unwrap_or_else(|| "无标题".to_string());
        if base.chars().count() <= limit {
            base
        } else {
            format!("{}...", base.chars().take(limit).collect::<String>())
        }
    }

    fn format_relative_time(dt: chrono::DateTime<chrono::Utc>) -> String {
        let local = dt.with_timezone(&Local);
        let today = Local::now().date_naive();
        let yesterday = today - Duration::days(1);
        let dt_date = local.date_naive();

        if dt_date == today {
            format!("今天 {}", local.format("%H:%M"))
        } else if dt_date == yesterday {
            format!("昨天 {}", local.format("%H:%M"))
        } else {
            local.format("%m-%d %H:%M").to_string()
        }
    }

    fn format_duration(start: chrono::DateTime<chrono::Utc>) -> String {
        let now = Utc::now();
        let duration = now.signed_duration_since(start);
        let hours = duration.num_hours();
        let minutes = duration.num_minutes() % 60;

        if hours > 0 {
            format!("{}小时{}分", hours, minutes)
        } else {
            format!("{}分钟", minutes)
        }
    }

    fn review_icon(record: &Record) -> &'static str {
        match record.record_type {
            RecordType::Idea => "💡",
            RecordType::Task if record.status == Some(TaskStatus::Cancelled) => "✕",
            RecordType::Task if record.completed_at.is_some() => "☑",
            RecordType::Task => "▶",
            _ => "📝",
        }
    }

    fn priority_label(task: &Record) -> &'static str {
        match task.priority {
            Some(Priority::High) => "高",
            Some(Priority::Medium) => "中",
            _ => "低",
        }
    }

    fn priority_color(task: &Record) -> Rgba {
        match task.priority {
            Some(Priority::High) => rgb(0xff4d4f),
            Some(Priority::Medium) => rgb(0xfa8c16),
            _ => rgb(0x8c8c8c),
        }
    }

    fn panel_title(text: &str) -> impl IntoElement {
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(0x262626))
            .child(text.to_string())
    }

    fn meta_text(text: impl Into<SharedString>) -> impl IntoElement {
        div().text_xs().text_color(rgb(0x8c8c8c)).child(text.into())
    }

    fn compact_stat(label: &str, value: usize, accent: Rgba) -> impl IntoElement {
        v_flex()
            .gap(px(2.0))
            .child(Self::meta_text(label.to_string()))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(accent)
                    .child(value.to_string()),
            )
    }

    fn render_today_row(&self, idx: usize, task: &Record, cx: &mut Context<Self>) -> Stateful<Div> {
        let due_text = task
            .due_date
            .map(|due| due.with_timezone(&Local).format("%m-%d").to_string())
            .unwrap_or_else(|| "未设置截止".to_string());

        div()
            .id(("dashboard-today-row", idx))
            .cursor_pointer()
            .py(px(8.0))
            .border_b_1()
            .border_color(rgb(0xf0f0f0))
            .hover(|s| s.bg(rgb(0xfcfcfc)))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .flex_1()
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(0xf5f5f5))
                                    .text_xs()
                                    .text_color(Self::priority_color(task))
                                    .child(Self::priority_label(task)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(rgb(0x262626))
                                    .child(Self::display_text(task, 24)),
                            ),
                    )
                    .child(Self::meta_text(due_text)),
            )
            .on_click(cx.listener({
                let task = task.clone();
                move |this, _event: &ClickEvent, window, cx| {
                    this.show_task_detail(&task, window, cx);
                }
            }))
    }

    fn render_overview(
        &mut self,
        data: DashboardData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let primary_task = data.in_progress.first().cloned();
        let viewport_width = window.viewport_size().width;
        let wide_layout = viewport_width >= px(760.0);
        let bottom_wide = viewport_width >= px(720.0);

        v_flex()
            .size_full()
            .gap(px(12.0))
            .p(px(18.0))
            .pr(px(12.0))
            .overflow_y_scrollbar()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x262626))
                    .child("看板"),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .items_start()
                    .when(wide_layout, |el| el.flex_row())
                    .when(!wide_layout, |el| el.flex_col())
                    .child(match primary_task {
                        Some(task) => {
                            let start_time = task.started_at.unwrap_or(task.updated_at);
                            div()
                                .flex_1()
                                .when(!wide_layout, |el| el.w_full())
                                .rounded(px(14.0))
                                .bg(rgb(0xf7fbff))
                                .border_1()
                                .border_color(rgb(0xe6f4ff))
                                .px(px(14.0))
                                .py(px(12.0))
                                .child(Self::panel_title("进行中"))
                                .child(
                                    h_flex()
                                        .mt(px(10.0))
                                        .justify_between()
                                        .items_end()
                                        .gap(px(12.0))
                                        .child(
                                            v_flex()
                                                .gap(px(6.0))
                                                .flex_1()
                                                .child(
                                                    div()
                                                        .text_xl()
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(rgb(0x111111))
                                                        .child(Self::display_text(&task, 30)),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap(px(10.0))
                                                        .items_center()
                                                        .child(Self::meta_text(format!(
                                                            "开始于 {}",
                                                            start_time.with_timezone(&Local).format("%H:%M")
                                                        )))
                                                        .child(
                                                            div()
                                                                .id("dashboard-open-task-detail")
                                                                .cursor_pointer()
                                                                .text_xs()
                                                                .text_color(rgb(0x1677ff))
                                                                .hover(|s| s.text_color(rgb(0x4096ff)))
                                                                .on_click(cx.listener({
                                                                    let task = task.clone();
                                                                    move |this, _event: &ClickEvent, window, cx| {
                                                                        this.show_task_detail(&task, window, cx);
                                                                    }
                                                                }))
                                                                .child("查看详情"),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_xl()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(0x1677ff))
                                                .child(Self::format_duration(start_time)),
                                        ),
                                )
                                .into_any_element()
                        }
                        None => div()
                            .flex_1()
                            .when(!wide_layout, |el| el.w_full())
                            .rounded(px(14.0))
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(0xe8e8e8))
                            .px(px(14.0))
                            .py(px(12.0))
                            .child(Self::panel_title("进行中"))
                            .child(
                                h_flex()
                                    .mt(px(10.0))
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(0x595959))
                                            .child("当前没有进行中的任务"),
                                    )
                                    .child(
                                        div()
                                            .id("dashboard-open-task-panel")
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(rgb(0x1677ff))
                                            .hover(|s| s.text_color(rgb(0x4096ff)))
                                            .on_click(cx.listener(
                                                |this, _event: &ClickEvent, window, cx| {
                                                    this.emit_action(
                                                        DashboardAction::OpenTaskPreset(
                                                            TaskFocusPreset::None,
                                                        ),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ))
                                            .child("去任务面板"),
                                    ),
                            )
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .when(!wide_layout, |el| el.w_full())
                            .rounded(px(14.0))
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(0xe8e8e8))
                            .px(px(14.0))
                            .py(px(12.0))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(Self::panel_title("摘要"))
                                    .child(
                                        div()
                                            .id("dashboard-open-stats")
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(rgb(0x1677ff))
                                            .hover(|s| s.text_color(rgb(0x4096ff)))
                                            .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                                                this.set_page(DashboardPage::Stats, cx);
                                            }))
                                            .child("查看更多统计"),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .mt(px(10.0))
                                    .gap(px(20.0))
                                    .items_start()
                                    .children([
                                        Self::compact_stat("已逾期", data.overdue_count, rgb(0xff4d4f))
                                            .into_any_element(),
                                        Self::compact_stat("今天到期", data.due_today_count, rgb(0xfa8c16))
                                            .into_any_element(),
                                        Self::compact_stat(
                                            "高优未完",
                                            data.high_priority_open_count,
                                            rgb(0x722ed1),
                                        )
                                        .into_any_element(),
                                    ]),
                            )
                            .child(
                                h_flex()
                                    .mt(px(12.0))
                                    .gap(px(12.0))
                                    .items_center()
                                    .child(Self::meta_text(format!("未完成 {}", data.total_open_count)))
                                    .child(Self::meta_text(format!("进行中 {}", data.total_in_progress)))
                                    .child(Self::meta_text(format!(
                                        "今日完成 {}",
                                        data.completed_today_count
                                    ))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .items_start()
                    .when(bottom_wide, |el| el.flex_row())
                    .when(!bottom_wide, |el| el.flex_col())
                    .child(
                        div()
                            .flex_1()
                            .when(!bottom_wide, |el| el.w_full())
                            .rounded(px(14.0))
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(0xe8e8e8))
                            .px(px(14.0))
                            .py(px(12.0))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(Self::panel_title("回顾"))
                                    .child(
                                        div()
                                            .id("dashboard-open-full-timeline")
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(rgb(0x1677ff))
                                            .hover(|s| s.text_color(rgb(0x4096ff)))
                                            .on_click(cx.listener(
                                                |this, _event: &ClickEvent, window, cx| {
                                                    this.emit_action(
                                                        DashboardAction::OpenTimeline,
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ))
                                            .child("查看时间线"),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .mt(px(8.0))
                                    .children(if data.recent_review_items.is_empty() {
                                        vec![Self::meta_text("最近还没有可展示的回顾内容。").into_any_element()]
                                    } else {
                                        data.recent_review_items
                                            .iter()
                                            .enumerate()
                                            .map(|(idx, record)| {
                                                let review_time =
                                                    record.completed_at.unwrap_or(record.created_at);
                                                div()
                                                    .id(("dashboard-review", idx))
                                                    .cursor_pointer()
                                                    .py(px(8.0))
                                                    .border_b_1()
                                                    .border_color(rgb(0xf0f0f0))
                                                    .hover(|s| s.bg(rgb(0xfcfcfc)))
                                                    .on_click(cx.listener({
                                                        let record = record.clone();
                                                        move |this, _event: &ClickEvent, window, cx| {
                                                            this.open_record_from_dashboard(&record, window, cx);
                                                        }
                                                    }))
                                                    .child(
                                                        h_flex()
                                                            .gap(px(10.0))
                                                            .items_center()
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .child(Self::review_icon(record)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .text_xs()
                                                                    .text_color(rgb(0x262626))
                                                                    .child(Self::display_text(record, 28)),
                                                            )
                                                            .child(Self::meta_text(Self::format_relative_time(
                                                                review_time,
                                                            ))),
                                                    )
                                                    .into_any_element()
                                            })
                                            .collect()
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .when(!bottom_wide, |el| el.w_full())
                            .rounded(px(14.0))
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(0xe8e8e8))
                            .px(px(14.0))
                            .py(px(12.0))
                            .child(Self::panel_title("今天"))
                            .child(
                                v_flex()
                                    .mt(px(8.0))
                                    .children(if data.today_tasks.is_empty() {
                                        vec![Self::meta_text("今天没有待办任务。").into_any_element()]
                                    } else {
                                        data.today_tasks
                                            .iter()
                                            .enumerate()
                                            .map(|(idx, task)| {
                                                self.render_today_row(idx, task, cx).into_any_element()
                                            })
                                            .collect()
                                    }),
                            ),
                    ),
            )
            .when(
                !data.common_tags.is_empty() || !data.common_persons.is_empty(),
                |el| {
                    el.child(
                        div()
                            .rounded(px(14.0))
                            .bg(rgb(0xffffff))
                            .border_1()
                            .border_color(rgb(0xe8e8e8))
                            .px(px(14.0))
                            .py(px(12.0))
                            .child(Self::panel_title("常用入口"))
                            .child(
                                h_flex()
                                    .mt(px(10.0))
                                    .gap(px(8.0))
                                    .flex_wrap()
                                    .children(data.common_tags.iter().enumerate().map(|(idx, tag)| {
                                        let tag_clone = tag.clone();
                                        div()
                                            .id(("dashboard-tag", idx))
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(5.0))
                                            .rounded(px(999.0))
                                            .bg(rgb(0xf0f7ff))
                                            .text_xs()
                                            .text_color(rgb(0x1677ff))
                                            .hover(|s| s.bg(rgb(0xe6f4ff)))
                                            .child(format!("#{}", tag))
                                            .on_click(cx.listener(
                                                move |this, _event: &ClickEvent, window, cx| {
                                                    this.emit_action(
                                                        DashboardAction::FilterByTag(tag_clone.clone()),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ))
                                    }))
                                    .children(
                                        data.common_persons.iter().enumerate().map(|(idx, person)| {
                                            let person_clone = person.clone();
                                            div()
                                                .id(("dashboard-person", idx))
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(5.0))
                                                .rounded(px(999.0))
                                                .bg(rgb(0xf6ffed))
                                                .text_xs()
                                                .text_color(rgb(0x389e0d))
                                                .hover(|s| s.bg(rgb(0xf0f5ff)))
                                                .child(format!("@{}", person))
                                                .on_click(cx.listener(
                                                    move |this, _event: &ClickEvent, window, cx| {
                                                        this.emit_action(
                                                            DashboardAction::FilterByPerson(
                                                                person_clone.clone(),
                                                            ),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                        }),
                                    ),
                            ),
                    )
                },
            )
    }

    fn render_stats_page(&mut self, stats: StatsData, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap(px(12.0))
            .p(px(18.0))
            .pr(px(12.0))
            .overflow_y_scrollbar()
            .child(
                h_flex().justify_between().items_center().child(
                    h_flex()
                        .gap(px(8.0))
                        .items_center()
                        .child(
                            div()
                                .id("dashboard-back-overview")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(999.0))
                                .bg(rgb(0xf5f5f5))
                                .text_xs()
                                .text_color(rgb(0x595959))
                                .hover(|s| s.bg(rgb(0xefefef)))
                                .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                                    this.set_page(DashboardPage::Overview, cx);
                                }))
                                .child("返回"),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0x262626))
                                .child("统计"),
                        ),
                ),
            )
            .child(
                div()
                    .rounded(px(14.0))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe8e8e8))
                    .px(px(14.0))
                    .py(px(12.0))
                    .child(Self::panel_title("关键统计"))
                    .child(
                        h_flex().mt(px(10.0)).gap(px(12.0)).children([
                            Self::compact_stat("未完成", stats.total_open_count, rgb(0x262626))
                                .into_any_element(),
                            Self::compact_stat("进行中", stats.total_in_progress, rgb(0x1677ff))
                                .into_any_element(),
                            Self::compact_stat(
                                "今日完成",
                                stats.completed_today_count,
                                rgb(0x52c41a),
                            )
                            .into_any_element(),
                            Self::compact_stat("已逾期", stats.overdue_count, rgb(0xff4d4f))
                                .into_any_element(),
                        ]),
                    )
                    .child(
                        h_flex().mt(px(8.0)).gap(px(12.0)).children([
                            Self::compact_stat("今天到期", stats.due_today_count, rgb(0xfa8c16))
                                .into_any_element(),
                            Self::compact_stat("明天到期", stats.due_tomorrow_count, rgb(0xfaad14))
                                .into_any_element(),
                            Self::compact_stat(
                                "高优未完",
                                stats.high_priority_open_count,
                                rgb(0x722ed1),
                            )
                            .into_any_element(),
                        ]),
                    ),
            )
            .child(
                div()
                    .rounded(px(14.0))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xe8e8e8))
                    .px(px(14.0))
                    .py(px(12.0))
                    .child(Self::panel_title("近 7 天完成数"))
                    .child(
                        v_flex().mt(px(12.0)).gap(px(10.0)).children(
                            stats
                                .last_7_days_completed
                                .iter()
                                .enumerate()
                                .map(|(idx, item)| {
                                    let width = px((item.count as f32 * 28.0).max(12.0));
                                    h_flex()
                                        .id(("stats-bar-row", idx))
                                        .gap(px(12.0))
                                        .items_center()
                                        .child(
                                            div()
                                                .w(px(56.0))
                                                .text_xs()
                                                .text_color(rgb(0x666666))
                                                .child(item.label.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .h(px(10.0))
                                                .rounded(px(999.0))
                                                .bg(rgb(0xf0f0f0))
                                                .child(
                                                    div()
                                                        .h_full()
                                                        .w(width)
                                                        .rounded(px(999.0))
                                                        .bg(rgb(0x52c41a)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .w(px(24.0))
                                                .text_xs()
                                                .text_color(rgb(0x595959))
                                                .child(item.count.to_string()),
                                        )
                                }),
                        ),
                    ),
            )
    }
}

impl Focusable for Dashboard {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dashboard_data = self.dashboard_data.clone().unwrap_or(DashboardData {
            in_progress: Vec::new(),
            today_tasks: Vec::new(),
            due_today_count: 0,
            due_tomorrow_count: 0,
            overdue_count: 0,
            high_priority_open_count: 0,
            total_open_count: 0,
            total_in_progress: 0,
            completed_today_count: 0,
            recent_review_items: Vec::new(),
            common_tags: Vec::new(),
            common_persons: Vec::new(),
        });
        let stats_data = self.stats_data.clone().unwrap_or_default();

        div()
            .size_full()
            .flex()
            .flex_row()
            .relative()
            .track_focus(&self.focus_handle(cx))
            .child(
                div()
                    .id("dashboard-main")
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(match self.page {
                        DashboardPage::Overview => self
                            .render_overview(dashboard_data, window, cx)
                            .into_any_element(),
                        DashboardPage::Stats => {
                            self.render_stats_page(stats_data, cx).into_any_element()
                        }
                    }),
            )
            .child(self.task_detail_sidebar.clone())
            .child(self.record_detail_sidebar.clone())
    }
}
