use crate::models::{LineGraphData, LineOverview, Record, RecordType, TaskStatus};
use crate::store::{Store, StoreEvent};
use crate::ui::record_detail_sidebar::{RecordDetailSidebar, SavePayload as RecordSavePayload};
use crate::ui::task_detail_sidebar::{SavePayload as TaskSavePayload, TaskDetailSidebar};
use crate::ui::tokenized_text::{
    render_metadata_chip, render_tokenized_text, MetadataChipKind, TokenTextStyle,
};
use chrono::{DateTime, Datelike, Duration, Local, Utc, Weekday};
use gpui::prelude::FluentBuilder as _;
use gpui::StatefulInteractiveElement as _;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::scroll::ScrollableElement;
use gpui_component::{h_flex, v_flex};
use uuid::Uuid;

const LINE_NODE_LIMIT: usize = 5;
const DETAIL_ACTIVITY_LIMIT: usize = 12;
const CONTENT_LIMIT: usize = 44;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TimelineDetailTarget {
    Task,
    Record,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum FocusTarget {
    Line(Uuid),
    Unassigned,
}

pub struct Timeline {
    store: Store,
    graph: LineGraphData,
    selected_project: Option<String>,
    focus_target: Option<FocusTarget>,
    is_loading: bool,
    request_generation: usize,
    focus_handle: FocusHandle,
    task_detail_sidebar: Entity<TaskDetailSidebar>,
    record_detail_sidebar: Entity<RecordDetailSidebar>,
    _window_activation_subscription: Subscription,
}

impl Timeline {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let task_sidebar_store = store.clone();
        let record_sidebar_store = store.clone();

        let mut panel = Self {
            store,
            graph: LineGraphData::default(),
            selected_project: None,
            focus_target: None,
            is_loading: false,
            request_generation: 0,
            focus_handle,
            task_detail_sidebar: cx
                .new(|cx| TaskDetailSidebar::new(task_sidebar_store.clone(), window, cx)),
            record_detail_sidebar: cx
                .new(|cx| RecordDetailSidebar::new(record_sidebar_store.clone(), window, cx)),
            _window_activation_subscription: cx.observe_window_activation(
                window,
                |this, _window, cx| {
                    if _window.is_window_active() {
                        this.refresh_data(cx);
                    }
                },
            ),
        };

        panel.install_sidebar_handlers(cx);
        panel.install_data_change_listener(cx);
        panel.load_graph(cx);
        panel
    }

    fn install_data_change_listener(&mut self, cx: &mut Context<Self>) {
        let mut changes = self.store.subscribe_data_changes();
        cx.spawn(async move |view, cx| loop {
            match changes.recv().await {
                Ok(StoreEvent::DataChanged)
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if view
                        .update(cx, |panel, cx| {
                            panel.refresh_data(cx);
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

    fn install_sidebar_handlers(&mut self, cx: &mut Context<Self>) {
        let handle = cx.entity().clone();
        self.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_save(move |payload, cx| {
                handle.update(cx, |panel, cx| {
                    panel.handle_task_sidebar_save(&payload, cx);
                });
            });
        });
        let handle = cx.entity().clone();
        self.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_delete(move |task_id, cx| {
                if let Ok(task_id) = Uuid::parse_str(&task_id) {
                    handle.update(cx, |panel, cx| {
                        panel.delete_record(task_id, cx);
                    });
                }
            });
        });
        let handle = cx.entity().clone();
        self.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_close(move |cx| {
                handle.update(cx, |panel, cx| {
                    panel.close_detail_sidebars(cx);
                });
            });
        });

        let handle = cx.entity().clone();
        self.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_save(move |payload, cx| {
                handle.update(cx, |panel, cx| {
                    panel.handle_record_sidebar_save(&payload, cx);
                });
            });
        });
        let handle = cx.entity().clone();
        self.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_delete(move |record_id, cx| {
                if let Ok(record_id) = Uuid::parse_str(&record_id) {
                    handle.update(cx, |panel, cx| {
                        panel.delete_record(record_id, cx);
                    });
                }
            });
        });
        let handle = cx.entity().clone();
        self.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_close(move |cx| {
                handle.update(cx, |panel, cx| {
                    panel.close_detail_sidebars(cx);
                });
            });
        });
    }

    pub fn refresh_data(&mut self, cx: &mut Context<Self>) {
        self.request_generation += 1;
        self.load_graph(cx);
    }

    fn load_graph(&mut self, cx: &mut Context<Self>) {
        self.is_loading = true;
        let generation = self.request_generation;
        let project = self.selected_project.clone();
        let store = self.store.clone();
        cx.spawn(
            async move |view, cx| match store.get_line_graph(project).await {
                Ok(graph) => {
                    let _ = view.update(cx, |panel, cx| {
                        if panel.request_generation != generation {
                            return;
                        }
                        panel.graph = graph;
                        panel.is_loading = false;
                        panel.prune_focus_after_reload(cx);
                        cx.notify();
                    });
                }
                Err(err) => {
                    eprintln!("[LineGraph] Failed to load graph: {}", err);
                    let _ = view.update(cx, |panel, cx| {
                        if panel.request_generation != generation {
                            return;
                        }
                        panel.is_loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn prune_focus_after_reload(&mut self, cx: &mut Context<Self>) {
        if let Some(FocusTarget::Line(line_id)) = self.focus_target {
            if !self
                .graph
                .lines
                .iter()
                .any(|overview| overview.line.id == line_id)
            {
                self.focus_target = None;
                self.close_detail_sidebars(cx);
            }
        }
    }

    pub fn apply_filters(
        &mut self,
        _tags: Vec<String>,
        _persons: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.focus_target = None;
        self.refresh_data(cx);
    }

    pub fn open_record(&mut self, record_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(record) = self.find_record(record_id).cloned() {
            self.focus_target = record
                .line_id
                .map(FocusTarget::Line)
                .or(Some(FocusTarget::Unassigned));
            self.select_record(&record, window, cx);
            return;
        }

        self.refresh_data(cx);
    }

    fn find_record(&self, record_id: Uuid) -> Option<&Record> {
        self.graph
            .lines
            .iter()
            .flat_map(|overview| overview.records.iter())
            .chain(self.graph.unassigned_records.iter())
            .find(|record| record.id == record_id)
    }

    fn selected_line(&self) -> Option<&LineOverview> {
        match self.focus_target.as_ref()? {
            FocusTarget::Line(line_id) => self
                .graph
                .lines
                .iter()
                .find(|overview| overview.line.id == *line_id),
            FocusTarget::Unassigned => None,
        }
    }

    fn focus_line(&mut self, line_id: Uuid, cx: &mut Context<Self>) {
        self.focus_target = Some(FocusTarget::Line(line_id));
        self.close_detail_sidebars(cx);
        cx.notify();
    }

    fn focus_unassigned(&mut self, cx: &mut Context<Self>) {
        self.focus_target = Some(FocusTarget::Unassigned);
        self.close_detail_sidebars(cx);
        cx.notify();
    }

    fn clear_focus(&mut self, cx: &mut Context<Self>) {
        self.focus_target = None;
        self.close_detail_sidebars(cx);
        cx.notify();
    }

    fn set_project_filter(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        if self.selected_project == project {
            return;
        }
        self.selected_project = project;
        self.focus_target = None;
        self.refresh_data(cx);
    }

    fn complete_selected_line(&mut self, cx: &mut Context<Self>) {
        let Some(FocusTarget::Line(line_id)) = self.focus_target.clone() else {
            return;
        };
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            if let Err(err) = store.complete_line(line_id).await {
                eprintln!("[LineGraph] Failed to complete line: {}", err);
                return;
            }
            let _ = view.update(cx, |panel, cx| {
                panel.focus_target = None;
                panel.refresh_data(cx);
            });
        })
        .detach();
    }

    fn delete_selected_line(&mut self, cx: &mut Context<Self>) {
        let Some(FocusTarget::Line(line_id)) = self.focus_target.clone() else {
            return;
        };
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            if let Err(err) = store.delete_line(line_id).await {
                eprintln!("[LineGraph] Failed to delete line: {}", err);
                return;
            }
            let _ = view.update(cx, |panel, cx| {
                panel.focus_target = None;
                panel.refresh_data(cx);
            });
        })
        .detach();
    }

    fn detail_target_for(record: &Record) -> TimelineDetailTarget {
        match record.record_type {
            RecordType::Task => TimelineDetailTarget::Task,
            RecordType::Note | RecordType::Idea | RecordType::Event => TimelineDetailTarget::Record,
        }
    }

    fn select_record(&mut self, record: &Record, window: &mut Window, cx: &mut Context<Self>) {
        match Self::detail_target_for(record) {
            TimelineDetailTarget::Task => {
                self.record_detail_sidebar.update(cx, |sidebar, cx| {
                    sidebar.dismiss(cx);
                });
                self.task_detail_sidebar.update(cx, |sidebar, cx| {
                    sidebar.show_task(record, window, cx);
                });
            }
            TimelineDetailTarget::Record => {
                self.task_detail_sidebar.update(cx, |sidebar, cx| {
                    sidebar.dismiss(cx);
                });
                self.record_detail_sidebar.update(cx, |sidebar, cx| {
                    sidebar.show_record(record, window, cx);
                });
            }
        }
        cx.notify();
    }

    fn close_detail_sidebars(&mut self, cx: &mut Context<Self>) {
        self.task_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.dismiss(cx);
        });
        self.record_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.dismiss(cx);
        });
    }

    fn handle_task_sidebar_save(&mut self, payload: &TaskSavePayload, cx: &mut Context<Self>) {
        let Some(mut task) = self.find_record_by_string_id(&payload.task_id).cloned() else {
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

        let store = self.store.clone();
        let line_ref = payload.line.clone();
        cx.spawn(async move |view, cx| {
            if let Err(err) = store.update_record_with_line(task, line_ref).await {
                eprintln!("[LineGraph] Failed to update task: {}", err);
                return;
            }
            let _ = view.update(cx, |panel, cx| {
                panel.refresh_data(cx);
            });
        })
        .detach();
    }

    fn handle_record_sidebar_save(&mut self, payload: &RecordSavePayload, cx: &mut Context<Self>) {
        let Some(mut record) = self.find_record_by_string_id(&payload.record_id).cloned() else {
            return;
        };
        record.title = payload.title.clone();
        record.content = payload.content.clone();
        record.tags = payload.tags.clone();
        record.persons = payload.persons.clone();
        record.updated_at = Utc::now();

        let store = self.store.clone();
        let line_ref = payload.line.clone();
        cx.spawn(async move |view, cx| {
            if let Err(err) = store.update_record_with_line(record, line_ref).await {
                eprintln!("[LineGraph] Failed to update record: {}", err);
                return;
            }
            let _ = view.update(cx, |panel, cx| {
                panel.refresh_data(cx);
            });
        })
        .detach();
    }

    fn find_record_by_string_id(&self, id: &str) -> Option<&Record> {
        Uuid::parse_str(id)
            .ok()
            .and_then(|record_id| self.find_record(record_id))
    }

    fn delete_record(&mut self, record_id: Uuid, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            if let Err(err) = store.delete_record(record_id).await {
                eprintln!("[LineGraph] Failed to delete record: {}", err);
                return;
            }
            let _ = view.update(cx, |panel, cx| {
                panel.close_detail_sidebars(cx);
                panel.refresh_data(cx);
            });
        })
        .detach();
    }

    fn selected_task_id(&self, cx: &App) -> Option<String> {
        self.task_detail_sidebar
            .read(cx)
            .current_task_id()
            .map(|id| id.to_string())
    }

    fn selected_record_id(&self, cx: &App) -> Option<String> {
        self.record_detail_sidebar
            .read(cx)
            .current_record_id()
            .map(|id| id.to_string())
    }

    fn is_record_selected(
        record: &Record,
        selected_task_id: Option<&str>,
        selected_record_id: Option<&str>,
    ) -> bool {
        match Self::detail_target_for(record) {
            TimelineDetailTarget::Task => selected_task_id == Some(record.id.to_string().as_str()),
            TimelineDetailTarget::Record => {
                selected_record_id == Some(record.id.to_string().as_str())
            }
        }
    }

    fn status_label(record: &Record) -> &'static str {
        match record.record_type {
            RecordType::Task => match record.status {
                Some(TaskStatus::InProgress) => "进行中",
                Some(TaskStatus::Done) => "已完成",
                Some(TaskStatus::Cancelled) => "已取消",
                _ => "待办",
            },
            RecordType::Event => "事件",
            RecordType::Idea => "想法",
            RecordType::Note => "记录",
        }
    }

    fn node_color(record: &Record) -> Rgba {
        match record.record_type {
            RecordType::Task => match record.status {
                Some(TaskStatus::InProgress) => rgb(0x2f80ed),
                Some(TaskStatus::Done) => rgb(0x5f8f63),
                Some(TaskStatus::Cancelled) => rgb(0x9ca3af),
                _ => rgb(0x2f80ed),
            },
            RecordType::Idea => rgb(0xb7791f),
            RecordType::Event => rgb(0x64748b),
            RecordType::Note => rgb(0x6b7280),
        }
    }

    fn record_summary(record: &Record, limit: usize) -> String {
        let raw = record
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| record.content.clone());
        let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.chars().count() <= limit {
            normalized
        } else {
            format!("{}...", normalized.chars().take(limit).collect::<String>())
        }
    }

    fn format_time(dt: DateTime<Utc>) -> String {
        let local = dt.with_timezone(&Local);
        let now = Local::now();
        let today = now.date_naive();
        let yesterday = today - Duration::days(1);
        let date = local.date_naive();

        if date == today {
            local.format("%H:%M").to_string()
        } else if date == yesterday {
            "昨天".to_string()
        } else if date.iso_week() == today.iso_week() && date.year() == today.year() {
            match local.weekday() {
                Weekday::Mon => "周一",
                Weekday::Tue => "周二",
                Weekday::Wed => "周三",
                Weekday::Thu => "周四",
                Weekday::Fri => "周五",
                Weekday::Sat => "周六",
                Weekday::Sun => "周日",
            }
            .to_string()
        } else if date.year() == today.year() {
            local.format("%-m月%-d日").to_string()
        } else {
            local.format("%Y年%-m月%-d日").to_string()
        }
    }

    fn render_project_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let all_selected = self.selected_project.is_none();
        h_flex()
            .gap(px(8.0))
            .flex_wrap()
            .child(
                filter_chip("line-project-all", "全部项目".to_string(), all_selected).on_click(
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.set_project_filter(None, cx);
                    }),
                ),
            )
            .children(
                self.graph
                    .projects
                    .iter()
                    .enumerate()
                    .map(|(idx, project)| {
                        let project_name = project.clone();
                        let is_selected =
                            self.selected_project.as_deref() == Some(project.as_str());
                        filter_chip(("line-project-filter", idx), project.clone(), is_selected)
                            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                                this.set_project_filter(Some(project_name.clone()), cx);
                            }))
                    }),
            )
    }

    fn render_line_lane(
        &self,
        overview: &LineOverview,
        selected_task_id: Option<&str>,
        selected_record_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let line_id = overview.line.id;
        let is_focused = self.focus_target == Some(FocusTarget::Line(line_id));
        let is_dimmed = matches!(self.focus_target, Some(FocusTarget::Line(id)) if id != line_id)
            || self.focus_target == Some(FocusTarget::Unassigned);
        let records = overview
            .records
            .iter()
            .take(LINE_NODE_LIMIT)
            .cloned()
            .collect::<Vec<_>>();

        div()
            .id(format!("line-lane-{line_id}"))
            .w(px(280.0))
            .min_w(px(280.0))
            .h_full()
            .p(px(10.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(if is_focused {
                rgb(0x2f80ed)
            } else {
                rgb(0xe5e7eb)
            })
            .bg(if is_dimmed {
                rgb(0xf8fafc)
            } else {
                rgb(0xffffff)
            })
            .cursor_pointer()
            .hover(|style| style.bg(rgb(0xf8fbff)))
            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                this.focus_line(line_id, cx);
                cx.stop_propagation();
            }))
            .child(
                v_flex()
                    .gap(px(10.0))
                    .child(
                        v_flex()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if is_dimmed {
                                        rgb(0x9ca3af)
                                    } else {
                                        rgb(0x111827)
                                    })
                                    .child(overview.line.name.clone()),
                            )
                            .child(
                                div().text_xs().text_color(rgb(0x6b7280)).child(
                                    overview
                                        .line
                                        .project
                                        .clone()
                                        .unwrap_or_else(|| "全局时间线".to_string()),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .min_h(px(250.0))
                            .pl(px(14.0))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(4.0))
                                    .top(px(0.0))
                                    .bottom(px(0.0))
                                    .w(px(2.0))
                                    .bg(if is_dimmed {
                                        rgb(0xe5e7eb)
                                    } else {
                                        rgb(0xcbd5e1)
                                    }),
                            )
                            .children(records.iter().enumerate().map(|(idx, record)| {
                                self.render_lane_node(
                                    idx,
                                    record,
                                    selected_task_id,
                                    selected_record_id,
                                    cx,
                                )
                            }))
                            .when(overview.records.is_empty(), |el| {
                                el.child(
                                    div()
                                        .pt(px(12.0))
                                        .text_xs()
                                        .text_color(rgb(0x9ca3af))
                                        .child("暂无活动"),
                                )
                            }),
                    ),
            )
    }

    fn render_lane_node(
        &self,
        idx: usize,
        record: &Record,
        selected_task_id: Option<&str>,
        selected_record_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_selected = Self::is_record_selected(record, selected_task_id, selected_record_id);
        let record_for_click = record.clone();
        v_flex()
            .id(format!("line-node-{}", record.id))
            .gap(px(4.0))
            .mb(px(12.0))
            .relative()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.select_record(&record_for_click, window, cx);
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(
                        div()
                            .ml(px(-14.0))
                            .size(px(12.0))
                            .rounded(px(6.0))
                            .bg(Self::node_color(record)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x9ca3af))
                            .child(if idx == 0 {
                                "当前".to_string()
                            } else {
                                Self::format_time(record.created_at)
                            }),
                    ),
            )
            .child(
                div()
                    .ml(px(4.0))
                    .p(px(8.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(if is_selected {
                        rgb(0x2f80ed)
                    } else {
                        rgb(0xe5e7eb)
                    })
                    .bg(if is_selected {
                        rgb(0xebf5ff)
                    } else {
                        rgb(0xffffff)
                    })
                    .w_full()
                    .min_w(px(0.0))
                    .child(
                        v_flex()
                            .w_full()
                            .min_w(px(0.0))
                            .gap(px(5.0))
                            .child(
                                h_flex().gap(px(6.0)).items_center().child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x6b7280))
                                        .child(Self::status_label(record)),
                                ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w(px(0.0))
                                    .text_sm()
                                    .line_height(relative(1.35))
                                    .text_color(rgb(0x1f2937))
                                    .child(render_tokenized_text(
                                        &Self::record_summary(record, CONTENT_LIMIT),
                                        TokenTextStyle::new(rgb(0x1f2937), FontWeight::NORMAL),
                                    )),
                            ),
                    ),
            )
    }

    fn render_graph(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_task_id = self.selected_task_id(cx);
        let selected_record_id = self.selected_record_id(cx);
        h_flex()
            .id("line-graph-scroll")
            .size_full()
            .gap(px(14.0))
            .min_w(px(0.0))
            .overflow_x_scrollbar()
            .children(self.graph.lines.iter().map(|overview| {
                self.render_line_lane(
                    overview,
                    selected_task_id.as_deref(),
                    selected_record_id.as_deref(),
                    cx,
                )
            }))
            .when(self.graph.lines.is_empty() && !self.is_loading, |el| {
                el.child(
                    div()
                        .w_full()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0x9ca3af))
                        .child("暂无时间线，使用 ~时间线 或 ~项目/时间线 创建第一条时间线"),
                )
            })
    }

    fn render_stats_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(overview) = self.selected_line() {
            return self.render_line_detail(overview, cx).into_any_element();
        }
        if self.focus_target == Some(FocusTarget::Unassigned) {
            return self.render_unassigned_detail(cx).into_any_element();
        }

        v_flex()
            .w(px(280.0))
            .min_w(px(280.0))
            .h_full()
            .gap(px(12.0))
            .p(px(14.0))
            .border_l_1()
            .border_color(rgb(0xe5e7eb))
            .child(panel_heading("态势"))
            .child(stat_row("项目数", self.graph.stats.project_count))
            .child(stat_row("开放时间线", self.graph.stats.open_line_count))
            .child(stat_row("记录数", self.graph.stats.record_count))
            .child(stat_row("待办数", self.graph.stats.open_task_count))
            .child(stat_row(
                "有下一步",
                self.graph.stats.lines_with_next_action_count,
            ))
            .child(
                div()
                    .id("line-unassigned-entry")
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(0xe5e7eb))
                    .bg(rgb(0xffffff))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0xf8fafc)))
                    .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.focus_unassigned(cx);
                    }))
                    .child(stat_row(
                        "未归时间线记录",
                        self.graph.stats.unassigned_record_count,
                    )),
            )
            .into_any_element()
    }

    fn render_line_detail(
        &self,
        overview: &LineOverview,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let next_action = overview.next_action.clone();
        v_flex()
            .w(px(320.0))
            .min_w(px(320.0))
            .h_full()
            .gap(px(12.0))
            .p(px(14.0))
            .border_l_1()
            .border_color(rgb(0xe5e7eb))
            .child(
                h_flex()
                    .justify_between()
                    .items_start()
                    .gap(px(10.0))
                    .child(
                        v_flex()
                            .gap(px(3.0))
                            .child(panel_heading(overview.line.name.clone()))
                            .child(
                                div().text_xs().text_color(rgb(0x6b7280)).child(
                                    overview
                                        .line
                                        .project
                                        .clone()
                                        .unwrap_or_else(|| "全局时间线".to_string()),
                                ),
                            ),
                    )
                    .child(
                        Button::new("line-detail-close")
                            .child("关闭")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.clear_focus(cx);
                            })),
                    ),
            )
            .child(stat_row("活动", overview.record_count))
            .child(stat_row("开放任务", overview.open_task_count))
            .child(
                v_flex()
                    .gap(px(6.0))
                    .child(section_label("下一步"))
                    .child(match next_action {
                        Some(record) => {
                            detail_record_preview(record, cx, |this, record, window, cx| {
                                this.select_record(&record, window, cx);
                            })
                            .into_any_element()
                        }
                        None => div()
                            .text_sm()
                            .text_color(rgb(0x9ca3af))
                            .child("暂无开放任务")
                            .into_any_element(),
                    }),
            )
            .child(
                v_flex()
                    .gap(px(8.0))
                    .child(section_label("活动记录"))
                    .children(
                        overview
                            .records
                            .iter()
                            .take(DETAIL_ACTIVITY_LIMIT)
                            .cloned()
                            .map(|record| {
                                detail_record_preview(record, cx, |this, record, window, cx| {
                                    this.select_record(&record, window, cx);
                                })
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.0))
                    .child(
                        Button::new("line-complete")
                            .child("标记完成")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.complete_selected_line(cx);
                            })),
                    )
                    .child(
                        Button::new("line-delete")
                            .child("删除时间线")
                            .text_color(rgb(0xb91c1c))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.delete_selected_line(cx);
                            })),
                    ),
            )
    }

    fn render_unassigned_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(px(320.0))
            .min_w(px(320.0))
            .h_full()
            .gap(px(12.0))
            .p(px(14.0))
            .border_l_1()
            .border_color(rgb(0xe5e7eb))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(panel_heading("未归时间线记录"))
                    .child(
                        Button::new("unassigned-close")
                            .child("关闭")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.clear_focus(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x6b7280))
                    .child("这些记录仍保留原样，可之后通过 ~时间线 归入时间线。"),
            )
            .children(self.graph.unassigned_records.iter().cloned().map(|record| {
                detail_record_preview(record, cx, |this, record, window, cx| {
                    this.select_record(&record, window, cx);
                })
            }))
    }
}

impl Render for Timeline {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .overflow_hidden()
            .relative()
            .p(px(22.0))
            .gap(px(14.0))
            .track_focus(&self.focus_handle(cx))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() == "escape" && this.focus_target.is_some() {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.clear_focus(cx);
                }
            }))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        v_flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x111827))
                                    .child(format!(
                                        "时间线 ({} 条开放时间线)",
                                        self.graph.lines.len()
                                    )),
                            )
                            .child(div().text_sm().text_color(rgb(0x6b7280)).child(
                                "用 ~时间线 或 ~项目/时间线 把任务和记录归入正在跟踪的时间线",
                            )),
                    )
                    .when(self.is_loading, |el| {
                        el.child(div().text_sm().text_color(rgb(0x9ca3af)).child("加载中..."))
                    }),
            )
            .child(self.render_project_filters(cx))
            .child(
                h_flex()
                    .id("line-graph-main")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        if this.focus_target.is_some() {
                            this.clear_focus(cx);
                        }
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .child(self.render_graph(cx)),
                    )
                    .child(self.render_stats_panel(cx)),
            )
            .child(self.task_detail_sidebar.clone())
            .child(self.record_detail_sidebar.clone())
    }
}

impl Focusable for Timeline {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn filter_chip(id: impl Into<ElementId>, label: String, is_selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(11.0))
        .py(px(6.0))
        .rounded(px(16.0))
        .border_1()
        .border_color(if is_selected {
            rgb(0x2f80ed)
        } else {
            rgb(0xe5e7eb)
        })
        .bg(if is_selected {
            rgb(0xebf5ff)
        } else {
            rgb(0xffffff)
        })
        .text_sm()
        .text_color(if is_selected {
            rgb(0x1d4ed8)
        } else {
            rgb(0x374151)
        })
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xf8fafc)))
        .child(label)
}

fn panel_heading(text: impl Into<String>) -> Div {
    div()
        .text_base()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(0x111827))
        .child(text.into())
}

fn section_label(text: impl Into<String>) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(0x6b7280))
        .child(text.into())
}

fn stat_row(label: impl Into<String>, value: usize) -> Div {
    div().child(
        h_flex()
            .justify_between()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x6b7280))
                    .child(label.into()),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x111827))
                    .child(value.to_string()),
            ),
    )
}

fn detail_record_preview(
    record: Record,
    cx: &mut Context<Timeline>,
    on_click: impl Fn(&mut Timeline, Record, &mut Window, &mut Context<Timeline>) + 'static,
) -> Stateful<Div> {
    let tags = record.tags.clone();
    let persons = record.persons.clone();
    let click_record = record.clone();
    div()
        .id(format!("line-detail-record-{}", record.id))
        .p(px(9.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(rgb(0xe5e7eb))
        .bg(rgb(0xffffff))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xf8fafc)))
        .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
            on_click(this, click_record.clone(), window, cx);
            cx.stop_propagation();
        }))
        .child(
            v_flex()
                .gap(px(6.0))
                .child(
                    h_flex()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x9ca3af))
                                .child(Timeline::status_label(&record)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x9ca3af))
                                .child(Timeline::format_time(record.created_at)),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.0))
                        .text_sm()
                        .line_height(relative(1.35))
                        .text_color(rgb(0x1f2937))
                        .child(render_tokenized_text(
                            &Timeline::record_summary(&record, 70),
                            TokenTextStyle::new(rgb(0x1f2937), FontWeight::NORMAL),
                        )),
                )
                .when(!tags.is_empty(), |el| {
                    el.child(h_flex().gap(px(6.0)).flex_wrap().children(
                        tags.into_iter().enumerate().map(|(idx, tag)| {
                            div()
                                .id(("line-detail-tag", idx))
                                .child(render_metadata_chip(MetadataChipKind::Tag, &tag))
                        }),
                    ))
                })
                .when(!persons.is_empty(), |el| {
                    el.child(h_flex().gap(px(6.0)).flex_wrap().children(
                        persons.into_iter().enumerate().map(|(idx, person)| {
                            div()
                                .id(("line-detail-person", idx))
                                .child(render_metadata_chip(MetadataChipKind::Person, &person))
                        }),
                    ))
                }),
        )
}
