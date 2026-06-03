use crate::models::{LineGraphData, LineOverview, Record, RecordType, TaskStatus};
use crate::store::{Store, StoreEvent};
use crate::ui::record_detail_sidebar::{RecordDetailSidebar, SavePayload as RecordSavePayload};
use crate::ui::task_detail_sidebar::{SavePayload as TaskSavePayload, TaskDetailSidebar};
use crate::ui::tokenized_text::{
    render_metadata_chip, render_tokenized_text, MetadataChipKind, TokenTextStyle,
};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Utc, Weekday};
use gpui::prelude::FluentBuilder as _;
use gpui::StatefulInteractiveElement as _;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::scroll::{ScrollableElement, Scrollbar};
use gpui_component::{h_flex, v_flex, v_virtual_list, VirtualListScrollHandle};
use std::rc::Rc;
use uuid::Uuid;

const LINE_NODE_LIMIT: usize = 5;
const DETAIL_ACTIVITY_LIMIT: usize = 12;
const CONTENT_LIMIT: usize = 44;
const TIMELINE_DATE_HEADER_HEIGHT: f32 = 36.0;
const TIMELINE_RECORD_COMPACT_HEIGHT: f32 = 82.0;
const TIMELINE_RECORD_EXPANDED_HEIGHT: f32 = 112.0;
const TIMELINE_ROW_WIDTH_ESTIMATE: f32 = 720.0;

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TimelineViewMode {
    Merged,
    SplitByTransaction,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum TimelineSourceKey {
    Line(Uuid),
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TimelineSource {
    key: TimelineSourceKey,
    name: String,
    project: Option<String>,
}

#[derive(Clone, Debug)]
struct TimelineRecordItem {
    record: Record,
    source: TimelineSource,
}

#[derive(Clone, Debug)]
struct TimelineDateGroup {
    date: NaiveDate,
    label: String,
    items: Vec<TimelineRecordItem>,
}

#[derive(Clone, Debug)]
struct TimelineSplitColumn {
    source: TimelineSource,
    items: Vec<TimelineRecordItem>,
}

#[derive(Clone, Debug)]
struct TimelineSplitDateGroup {
    date: NaiveDate,
    label: String,
    columns: Vec<TimelineSplitColumn>,
}

#[derive(Clone, Debug)]
enum TimelineRow {
    DateHeader { date: NaiveDate, label: String },
    Record(TimelineRecordItem),
}

pub struct Timeline {
    store: Store,
    graph: LineGraphData,
    merged_rows: Vec<TimelineRow>,
    merged_row_sizes: Rc<Vec<Size<Pixels>>>,
    merged_scroll_handle: VirtualListScrollHandle,
    selected_project: Option<String>,
    focus_target: Option<FocusTarget>,
    view_mode: TimelineViewMode,
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
            merged_rows: Vec::new(),
            merged_row_sizes: Rc::new(Vec::new()),
            merged_scroll_handle: VirtualListScrollHandle::new(),
            selected_project: None,
            focus_target: None,
            view_mode: TimelineViewMode::Merged,
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
                        panel.rebuild_merged_timeline_cache();
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

    fn rebuild_merged_timeline_cache(&mut self) {
        self.merged_rows = merged_timeline_rows(&self.graph);
        self.merged_row_sizes = timeline_row_sizes(&self.merged_rows);
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
        self.view_mode = TimelineViewMode::Merged;
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

    fn should_show_global_lane(graph: &LineGraphData) -> bool {
        graph.stats.unassigned_record_count > 0 || !graph.unassigned_records.is_empty()
    }

    fn show_merged_view(&mut self, cx: &mut Context<Self>) {
        self.focus_target = None;
        self.view_mode = TimelineViewMode::Merged;
        self.close_detail_sidebars(cx);
        cx.notify();
    }

    fn show_split_by_transaction(&mut self, cx: &mut Context<Self>) {
        self.focus_target = None;
        self.view_mode = TimelineViewMode::SplitByTransaction;
        self.close_detail_sidebars(cx);
        cx.notify();
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
        self.view_mode = TimelineViewMode::Merged;
        self.close_detail_sidebars(cx);
        cx.notify();
    }

    fn set_project_filter(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        if self.selected_project == project {
            return;
        }
        self.selected_project = project;
        self.focus_target = None;
        self.view_mode = TimelineViewMode::Merged;
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
                filter_chip("line-project-all", "全部时间线".to_string(), all_selected).on_click(
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

    fn render_timeline_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .justify_between()
            .items_center()
            .gap(px(12.0))
            .child(self.render_project_filters(cx))
            .child(match self.view_mode {
                TimelineViewMode::Merged => Button::new("timeline-split-by-transaction")
                    .child("按事务拆分")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.show_split_by_transaction(cx);
                    }))
                    .into_any_element(),
                TimelineViewMode::SplitByTransaction => Button::new("timeline-return-merged")
                    .child("返回合并视图")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.show_merged_view(cx);
                    }))
                    .into_any_element(),
            })
    }

    fn render_merged_timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_empty = self.merged_rows.is_empty();
        let item_sizes = self.merged_row_sizes.clone();
        let scroll_handle = self.merged_scroll_handle.clone();

        v_flex()
            .id("timeline-merged")
            .size_full()
            .gap(px(12.0))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0x111827))
                            .child("全部时间流水"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x6b7280))
                            .child("按时间倒序，点击节点查看详情"),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .when(is_empty && !self.is_loading, |el| {
                        el.child(empty_timeline_state())
                    })
                    .when(!is_empty, |el| {
                        el.child(
                            v_virtual_list(
                                cx.entity().clone(),
                                "timeline-merged-virtual-list",
                                item_sizes,
                                |this, visible_range, _window, cx| {
                                    visible_range
                                        .filter_map(|row_index| {
                                            this.render_merged_timeline_row(row_index, cx)
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .track_scroll(&scroll_handle)
                            .size_full(),
                        )
                        .child(
                            div()
                                .absolute()
                                .top(px(0.0))
                                .right(px(0.0))
                                .bottom(px(0.0))
                                .child(Scrollbar::vertical(&scroll_handle)),
                        )
                    }),
            )
    }

    fn render_merged_timeline_row(
        &self,
        row_index: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match self.merged_rows.get(row_index)? {
            TimelineRow::DateHeader { date, label } => Some(
                self.render_merged_date_header(*date, label.clone())
                    .into_any_element(),
            ),
            TimelineRow::Record(item) => Some(
                self.render_merged_record_node(item.clone(), cx)
                    .into_any_element(),
            ),
        }
    }

    fn render_merged_date_header(&self, date: NaiveDate, label: String) -> impl IntoElement {
        h_flex()
            .id(format!("timeline-merged-date-{date}"))
            .h(px(TIMELINE_DATE_HEADER_HEIGHT))
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(72.0))
                    .min_w(px(72.0))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x111827))
                    .child(label),
            )
            .child(
                div().relative().w(px(24.0)).min_w(px(24.0)).h_full().child(
                    div()
                        .absolute()
                        .left(px(11.0))
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .w(px(2.0))
                        .bg(rgb(0xdbe2ea)),
                ),
            )
            .child(div().flex_1())
    }

    fn render_merged_record_node(
        &self,
        item: TimelineRecordItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let record = item.record;
        let record_for_click = record.clone();
        let row_height = timeline_record_row_height(&TimelineRecordItem {
            record: record.clone(),
            source: item.source.clone(),
        });
        h_flex()
            .id(format!("timeline-merged-record-{}", record.id))
            .w_full()
            .h(px(row_height))
            .min_w(px(0.0))
            .items_stretch()
            .gap(px(10.0))
            .overflow_hidden()
            .cursor_pointer()
            .hover(|style| style.bg(rgb(0xf8fafc)))
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.select_record(&record_for_click, window, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .w(px(72.0))
                    .min_w(px(72.0))
                    .pt(px(12.0))
                    .text_xs()
                    .text_color(rgb(0x9ca3af))
                    .child(
                        record
                            .created_at
                            .with_timezone(&Local)
                            .format("%H:%M")
                            .to_string(),
                    ),
            )
            .child(
                div()
                    .relative()
                    .w(px(24.0))
                    .min_w(px(24.0))
                    .child(
                        div()
                            .absolute()
                            .left(px(11.0))
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .w(px(2.0))
                            .bg(rgb(0xdbe2ea)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(5.0))
                            .top(px(14.0))
                            .size(px(14.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(rgb(0xffffff))
                            .bg(Self::node_color(&record)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(6.0))
                    .py(px(9.0))
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .flex_wrap()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x6b7280))
                                    .child(Self::status_label(&record)),
                            )
                            .child(render_source_chip(&item.source))
                            .children(record.tags.iter().enumerate().map(|(idx, tag)| {
                                div()
                                    .id(("timeline-merged-tag", idx))
                                    .child(render_metadata_chip(MetadataChipKind::Tag, tag))
                            }))
                            .children(record.persons.iter().enumerate().map(|(idx, person)| {
                                div()
                                    .id(("timeline-merged-person", idx))
                                    .child(render_metadata_chip(MetadataChipKind::Person, person))
                            })),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .text_sm()
                            .line_height(relative(1.35))
                            .text_color(rgb(0x1f2937))
                            .child(render_tokenized_text(
                                &Self::record_summary(&record, 96),
                                TokenTextStyle::new(rgb(0x1f2937), FontWeight::NORMAL),
                            )),
                    ),
            )
    }

    fn render_split_by_transaction(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let groups = split_timeline_by_transaction(&self.graph);
        let sources = all_timeline_sources(&self.graph);
        let width = 92.0 + (sources.len().max(1) as f32) * 260.0;

        div()
            .id("timeline-split-by-transaction")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                div().w_full().overflow_x_scrollbar().child(
                    v_flex()
                        .w(px(width))
                        .min_w(px(width))
                        .gap(px(10.0))
                        .child(
                            h_flex()
                                .gap(px(10.0))
                                .items_center()
                                .child(
                                    div()
                                        .w(px(82.0))
                                        .min_w(px(82.0))
                                        .text_xs()
                                        .text_color(rgb(0x6b7280))
                                        .child("日期"),
                                )
                                .children(
                                    sources
                                        .iter()
                                        .cloned()
                                        .map(|source| self.render_split_source_header(source, cx)),
                                ),
                        )
                        .children(
                            groups
                                .into_iter()
                                .map(|group| self.render_split_date_group(group, cx)),
                        )
                        .when(sources.is_empty() && !self.is_loading, |el| {
                            el.child(empty_timeline_state())
                        }),
                ),
            )
    }

    fn render_split_source_header(
        &self,
        source: TimelineSource,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = source.key;
        div()
            .id(format!(
                "timeline-split-source-{}",
                source_chip_label(&source)
            ))
            .w(px(250.0))
            .min_w(px(250.0))
            .p(px(9.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(0xe5e7eb))
            .bg(rgb(0xf8fafc))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(0xf8fbff)).border_color(rgb(0x93c5fd)))
            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                match key {
                    TimelineSourceKey::Line(line_id) => this.focus_line(line_id, cx),
                    TimelineSourceKey::Global => this.focus_unassigned(cx),
                }
                cx.stop_propagation();
            }))
            .child(
                v_flex()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if source.key == TimelineSourceKey::Global {
                                rgb(0x065f46)
                            } else {
                                rgb(0x111827)
                            })
                            .child(source.name),
                    )
                    .child(div().text_xs().text_color(rgb(0x6b7280)).child(
                        source.project.unwrap_or_else(|| {
                            if source.key == TimelineSourceKey::Global {
                                "未指定具体时间线".to_string()
                            } else {
                                "全局项目".to_string()
                            }
                        }),
                    )),
            )
    }

    fn render_split_date_group(
        &self,
        group: TimelineSplitDateGroup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(format!("timeline-split-date-{}", group.date))
            .gap(px(10.0))
            .items_start()
            .child(
                div()
                    .w(px(82.0))
                    .min_w(px(82.0))
                    .pt(px(10.0))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x111827))
                    .child(group.label),
            )
            .children(
                group
                    .columns
                    .into_iter()
                    .map(|column| self.render_split_column_day(column, cx)),
            )
    }

    fn render_split_column_day(
        &self,
        column: TimelineSplitColumn,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_empty = column.items.is_empty();
        v_flex()
            .id(format!(
                "timeline-split-column-day-{}",
                source_chip_label(&column.source)
            ))
            .w(px(250.0))
            .min_w(px(250.0))
            .min_h(px(72.0))
            .gap(px(8.0))
            .p(px(8.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(0xe5e7eb))
            .bg(rgb(0xffffff))
            .children(
                column
                    .items
                    .into_iter()
                    .map(|item| self.render_split_record_node(item, cx)),
            )
            .when(is_empty, |el| {
                el.child(div().text_xs().text_color(rgb(0xd1d5db)).child("—"))
            })
    }

    fn render_split_record_node(
        &self,
        item: TimelineRecordItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let record = item.record;
        let record_for_click = record.clone();
        div()
            .id(format!("timeline-split-record-{}", record.id))
            .p(px(8.0))
            .rounded(px(7.0))
            .bg(rgb(0xf8fafc))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(0xebf5ff)))
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.select_record(&record_for_click, window, cx);
                cx.stop_propagation();
            }))
            .child(
                v_flex()
                    .gap(px(4.0))
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div().text_xs().text_color(rgb(0x9ca3af)).child(
                                    record
                                        .created_at
                                        .with_timezone(&Local)
                                        .format("%H:%M")
                                        .to_string(),
                                ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x6b7280))
                                    .child(Self::status_label(&record)),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .line_height(relative(1.3))
                            .text_color(rgb(0x1f2937))
                            .child(render_tokenized_text(
                                &Self::record_summary(&record, 38),
                                TokenTextStyle::new(rgb(0x1f2937), FontWeight::NORMAL),
                            )),
                    ),
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
        let lane_width = if is_focused { 520.0 } else { 280.0 };
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
            .w(px(lane_width))
            .min_w(px(lane_width))
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
        if let Some(overview) = self.selected_line() {
            return div()
                .id("line-graph-focused")
                .size_full()
                .overflow_y_scrollbar()
                .child(self.render_line_lane(
                    overview,
                    selected_task_id.as_deref(),
                    selected_record_id.as_deref(),
                    cx,
                ))
                .into_any_element();
        }

        if self.focus_target == Some(FocusTarget::Unassigned) {
            return self
                .render_global_lane(
                    selected_task_id.as_deref(),
                    selected_record_id.as_deref(),
                    cx,
                )
                .into_any_element();
        }

        match self.view_mode {
            TimelineViewMode::Merged => self.render_merged_timeline(cx).into_any_element(),
            TimelineViewMode::SplitByTransaction => {
                self.render_split_by_transaction(cx).into_any_element()
            }
        }
    }

    fn render_global_lane(
        &self,
        selected_task_id: Option<&str>,
        selected_record_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("line-global-lane")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                div()
                    .w(px(520.0))
                    .min_w(px(520.0))
                    .h_full()
                    .p(px(12.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(0xd1fae5))
                    .bg(rgb(0xf7fefb))
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
                                            .text_color(rgb(0x065f46))
                                            .child("全局时间线"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x6b7280))
                                            .child("未指定具体时间线的记录"),
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
                                            .bg(rgb(0xbbf7d0)),
                                    )
                                    .children(self.graph.unassigned_records.iter().enumerate().map(
                                        |(idx, record)| {
                                            self.render_lane_node(
                                                idx,
                                                record,
                                                selected_task_id,
                                                selected_record_id,
                                                cx,
                                            )
                                        },
                                    ))
                                    .when(self.graph.unassigned_records.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .pt(px(12.0))
                                                .text_xs()
                                                .text_color(rgb(0x9ca3af))
                                                .child("暂无未归属记录"),
                                        )
                                    }),
                            ),
                    ),
            )
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
        let global_suffix = if Self::should_show_global_lane(&self.graph) {
            "，含全局"
        } else {
            ""
        };
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
                } else if event.keystroke.key.as_str() == "escape"
                    && this.view_mode != TimelineViewMode::Merged
                {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.show_merged_view(cx);
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
                                        "时间线 ({} 条跟踪中{})",
                                        self.graph.lines.len(),
                                        global_suffix
                                    )),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x6b7280))
                                    .child("用 ~时间线 或 ~项目/时间线 归入正在跟踪的事情"),
                            ),
                    )
                    .when(self.is_loading, |el| {
                        el.child(div().text_sm().text_color(rgb(0x9ca3af)).child("加载中..."))
                    }),
            )
            .child(self.render_timeline_controls(cx))
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
                    .when(self.focus_target.is_some(), |el| {
                        el.child(self.render_stats_panel(cx))
                    }),
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

fn line_source(overview: &LineOverview) -> TimelineSource {
    TimelineSource {
        key: TimelineSourceKey::Line(overview.line.id),
        name: overview.line.name.clone(),
        project: overview.line.project.clone(),
    }
}

fn global_source() -> TimelineSource {
    TimelineSource {
        key: TimelineSourceKey::Global,
        name: "全局".to_string(),
        project: None,
    }
}

fn source_chip_label(source: &TimelineSource) -> String {
    match source.key {
        TimelineSourceKey::Line(_) => match source.project.as_deref() {
            Some(project) if !project.trim().is_empty() => {
                format!("~{project}/{}", source.name)
            }
            _ => format!("~{}", source.name),
        },
        TimelineSourceKey::Global => "全局".to_string(),
    }
}

fn render_source_chip(source: &TimelineSource) -> AnyElement {
    match source.key {
        TimelineSourceKey::Line(_) => {
            render_metadata_chip(MetadataChipKind::Line, &source_chip_label(source))
        }
        TimelineSourceKey::Global => compact_badge("全局").into_any_element(),
    }
}

fn empty_timeline_state() -> AnyElement {
    div()
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(0x9ca3af))
        .child("暂无时间线，使用 ~时间线 或 ~项目/时间线 创建第一条时间线")
        .into_any_element()
}

fn all_timeline_sources(graph: &LineGraphData) -> Vec<TimelineSource> {
    let mut sources = graph.lines.iter().map(line_source).collect::<Vec<_>>();
    if Timeline::should_show_global_lane(graph) {
        sources.push(global_source());
    }
    sources
}

fn merged_timeline_items(graph: &LineGraphData) -> Vec<TimelineRecordItem> {
    let mut items = Vec::new();
    for overview in &graph.lines {
        let source = line_source(overview);
        items.extend(
            overview
                .records
                .iter()
                .cloned()
                .map(|record| TimelineRecordItem {
                    record,
                    source: source.clone(),
                }),
        );
    }

    let source = global_source();
    items.extend(
        graph
            .unassigned_records
            .iter()
            .cloned()
            .map(|record| TimelineRecordItem {
                record,
                source: source.clone(),
            }),
    );

    items.sort_by(|left, right| right.record.created_at.cmp(&left.record.created_at));
    items
}

fn record_date(record: &Record) -> NaiveDate {
    record.created_at.with_timezone(&Local).date_naive()
}

fn date_group_label(date: NaiveDate) -> String {
    let today = Local::now().date_naive();
    let yesterday = today - Duration::days(1);
    if date == today {
        "今天".to_string()
    } else if date == yesterday {
        "昨天".to_string()
    } else if date.year() == today.year() {
        date.format("%-m月%-d日").to_string()
    } else {
        date.format("%Y年%-m月%-d日").to_string()
    }
}

fn group_items_by_date(items: Vec<TimelineRecordItem>) -> Vec<TimelineDateGroup> {
    let mut groups: Vec<TimelineDateGroup> = Vec::new();
    for item in items {
        let date = record_date(&item.record);
        if let Some(group) = groups.iter_mut().find(|group| group.date == date) {
            group.items.push(item);
        } else {
            groups.push(TimelineDateGroup {
                date,
                label: date_group_label(date),
                items: vec![item],
            });
        }
    }
    groups.sort_by(|left, right| right.date.cmp(&left.date));
    groups
}

fn merged_timeline_rows(graph: &LineGraphData) -> Vec<TimelineRow> {
    group_items_by_date(merged_timeline_items(graph))
        .into_iter()
        .flat_map(|group| {
            let mut rows = Vec::with_capacity(group.items.len() + 1);
            rows.push(TimelineRow::DateHeader {
                date: group.date,
                label: group.label,
            });
            rows.extend(group.items.into_iter().map(TimelineRow::Record));
            rows
        })
        .collect()
}

fn timeline_row_sizes(rows: &[TimelineRow]) -> Rc<Vec<Size<Pixels>>> {
    Rc::new(rows.iter().map(timeline_row_size).collect())
}

fn timeline_row_size(row: &TimelineRow) -> Size<Pixels> {
    let height = match row {
        TimelineRow::DateHeader { .. } => TIMELINE_DATE_HEADER_HEIGHT,
        TimelineRow::Record(item) => timeline_record_row_height(item),
    };
    size(px(TIMELINE_ROW_WIDTH_ESTIMATE), px(height))
}

fn timeline_record_row_height(item: &TimelineRecordItem) -> f32 {
    let record = &item.record;
    let summary_len = Timeline::record_summary(record, 96).chars().count();
    let metadata_count = 1 + record.tags.len() + record.persons.len();
    if summary_len > 48 || metadata_count > 4 {
        TIMELINE_RECORD_EXPANDED_HEIGHT
    } else {
        TIMELINE_RECORD_COMPACT_HEIGHT
    }
}

fn split_timeline_by_transaction(graph: &LineGraphData) -> Vec<TimelineSplitDateGroup> {
    let sources = all_timeline_sources(graph);
    group_items_by_date(merged_timeline_items(graph))
        .into_iter()
        .map(|group| {
            let columns = sources
                .iter()
                .cloned()
                .map(|source| {
                    let items = group
                        .items
                        .iter()
                        .filter(|item| item.source.key == source.key)
                        .cloned()
                        .collect::<Vec<_>>();
                    TimelineSplitColumn { source, items }
                })
                .collect::<Vec<_>>();
            TimelineSplitDateGroup {
                date: group.date,
                label: group.label,
                columns,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        group_items_by_date, merged_timeline_items, merged_timeline_rows,
        split_timeline_by_transaction, timeline_row_sizes, TimelineRow, TimelineSourceKey,
    };
    use crate::models::{Line, LineGraphData, LineGraphStats, LineOverview, LineStatus, Record};
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn test_line(id: u128, name: &str, records: Vec<Record>) -> LineOverview {
        let now = Utc.with_ymd_and_hms(2026, 6, 3, 12, 0, 0).unwrap();
        LineOverview {
            line: Line {
                id: Uuid::from_u128(id),
                name: name.to_string(),
                project: None,
                status: LineStatus::Open,
                created_at: now,
                updated_at: now,
                completed_at: None,
            },
            record_count: records.len(),
            open_task_count: 0,
            next_action: None,
            latest_record: records.first().cloned(),
            records,
        }
    }

    fn note_at(title: &str, year: i32, month: u32, day: u32, hour: u32) -> Record {
        let mut record = Record::new_note(title.to_string());
        record.created_at = Utc.with_ymd_and_hms(year, month, day, hour, 0, 0).unwrap();
        record
    }

    #[test]
    fn merged_timeline_items_include_unassigned_and_sort_newest_first() {
        let older = note_at("older", 2026, 6, 1, 10);
        let newer = note_at("newer", 2026, 6, 3, 10);
        let global = note_at("global", 2026, 6, 2, 10);
        let graph = LineGraphData {
            lines: vec![test_line(1, "事务", vec![older, newer])],
            unassigned_records: vec![global],
            stats: LineGraphStats {
                unassigned_record_count: 1,
                ..LineGraphStats::default()
            },
            ..LineGraphData::default()
        };

        let items = merged_timeline_items(&graph);

        assert_eq!(
            items
                .iter()
                .map(|item| item.record.title.as_deref().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["newer", "global", "older"]
        );
        assert_eq!(items[1].source.key, TimelineSourceKey::Global);
    }

    #[test]
    fn date_groups_keep_cross_transaction_records_in_same_day_bucket() {
        let left = note_at("left", 2026, 6, 3, 8);
        let right = note_at("right", 2026, 6, 3, 9);
        let graph = LineGraphData {
            lines: vec![
                test_line(1, "左事务", vec![left]),
                test_line(2, "右事务", vec![right]),
            ],
            ..LineGraphData::default()
        };

        let groups = group_items_by_date(merged_timeline_items(&graph));

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 2);
    }

    #[test]
    fn split_by_transaction_maps_unassigned_records_to_global_column() {
        let line_record = note_at("line", 2026, 6, 3, 10);
        let global_record = note_at("global", 2026, 6, 3, 11);
        let graph = LineGraphData {
            lines: vec![test_line(1, "事务", vec![line_record])],
            unassigned_records: vec![global_record],
            stats: LineGraphStats {
                unassigned_record_count: 1,
                ..LineGraphStats::default()
            },
            ..LineGraphData::default()
        };

        let groups = split_timeline_by_transaction(&graph);
        let global_column = groups[0]
            .columns
            .iter()
            .find(|column| column.source.key == TimelineSourceKey::Global)
            .expect("expected global column");

        assert_eq!(global_column.items.len(), 1);
        assert_eq!(
            global_column.items[0].record.title.as_deref(),
            Some("global")
        );
    }

    #[test]
    fn empty_graph_has_no_merged_items_or_split_columns() {
        let graph = LineGraphData::default();

        assert!(merged_timeline_items(&graph).is_empty());
        assert!(split_timeline_by_transaction(&graph).is_empty());
    }

    #[test]
    fn merged_timeline_rows_flatten_date_headers_and_records_for_virtualization() {
        let older = note_at("older", 2026, 6, 1, 10);
        let newer = note_at("newer", 2026, 6, 3, 10);
        let graph = LineGraphData {
            lines: vec![test_line(1, "事务", vec![older, newer])],
            ..LineGraphData::default()
        };

        let rows = merged_timeline_rows(&graph);

        assert!(matches!(rows[0], TimelineRow::DateHeader { .. }));
        assert!(matches!(rows[1], TimelineRow::Record(_)));
        assert!(matches!(rows[2], TimelineRow::DateHeader { .. }));
        assert!(matches!(rows[3], TimelineRow::Record(_)));
    }

    #[test]
    fn timeline_row_sizes_match_rows_and_allow_variable_heights() {
        let mut long = note_at("long", 2026, 6, 3, 10);
        long.content =
            "这是一条比较长的时间线记录，用来验证虚拟列表可以为内容更多的节点分配更高的行高"
                .to_string();
        let graph = LineGraphData {
            lines: vec![test_line(1, "事务", vec![long])],
            ..LineGraphData::default()
        };
        let rows = merged_timeline_rows(&graph);

        let sizes = timeline_row_sizes(&rows);

        assert_eq!(sizes.len(), rows.len());
        assert!(sizes[0].height < sizes[1].height);
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

fn compact_badge(label: impl Into<String>) -> Div {
    div()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(999.0))
        .bg(rgb(0xecfdf5))
        .border_1()
        .border_color(rgb(0xbbf7d0))
        .text_xs()
        .text_color(rgb(0x047857))
        .child(label.into())
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
