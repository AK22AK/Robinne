use crate::models::{Record, RecordType, TaskStatus};
use crate::store::{Store, StoreEvent};
use crate::ui::record_detail_sidebar::{RecordDetailSidebar, SavePayload as RecordSavePayload};
use crate::ui::task_detail_sidebar::{SavePayload as TaskSavePayload, TaskDetailSidebar};
use crate::ui::tokenized_text::{
    render_inline_token_text, render_metadata_chip, tokenize_text, MetadataChipKind,
    TextTokenSegment, TokenTextStyle,
};
use chrono::{DateTime, Datelike, Duration, Local, Utc, Weekday};
use gpui::prelude::FluentBuilder as _;
use gpui::StatefulInteractiveElement as _;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::h_flex;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::v_flex;
use std::collections::BTreeSet;
use std::time::Duration as StdDuration;
use uuid::Uuid;

const SEARCH_DEBOUNCE_MS: u64 = 300;
const SEARCH_TITLE_PREVIEW_LIMIT: usize = 48;
const SEARCH_BODY_PREVIEW_LIMIT: usize = 96;

#[derive(Clone, PartialEq, Eq, Debug)]
enum BrowseFilter {
    Tag(String),
    Person(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AdvancedFilterMode {
    And,
    Or,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SearchDetailTarget {
    Task,
    Record,
}

#[derive(Clone)]
struct PendingDeletion {
    id: Uuid,
    record_label: &'static str,
    display_title: String,
}

impl AdvancedFilterMode {
    fn label(&self) -> &'static str {
        match self {
            AdvancedFilterMode::And => "AND",
            AdvancedFilterMode::Or => "OR",
        }
    }

    fn toggle(&self) -> Self {
        match self {
            AdvancedFilterMode::And => AdvancedFilterMode::Or,
            AdvancedFilterMode::Or => AdvancedFilterMode::And,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchFilterType {
    All,
    Task,
    Note,
    Idea,
}

impl SearchFilterType {
    fn label(&self) -> &'static str {
        match self {
            SearchFilterType::All => "全部",
            SearchFilterType::Task => "任务",
            SearchFilterType::Note => "记录",
            SearchFilterType::Idea => "想法",
        }
    }

    fn matches(&self, record_type: &RecordType) -> bool {
        match self {
            SearchFilterType::All => true,
            SearchFilterType::Task => matches!(record_type, RecordType::Task),
            SearchFilterType::Note => matches!(record_type, RecordType::Note),
            SearchFilterType::Idea => matches!(record_type, RecordType::Idea),
        }
    }
}

pub struct SearchPanel {
    store: Store,
    query: String,
    results: Vec<Record>,
    filter_type: SearchFilterType,
    show_completed_tasks: bool,
    browse_filter: Option<BrowseFilter>,
    advanced_filter_enabled: bool,
    selected_tags: BTreeSet<String>,
    selected_persons: BTreeSet<String>,
    filter_mode: AdvancedFilterMode,
    #[allow(dead_code)]
    available_tags: Vec<String>,
    #[allow(dead_code)]
    available_persons: Vec<String>,
    input_state: Entity<InputState>,
    focus_handle: FocusHandle,
    _search_subscription: Subscription,
    is_searching: bool,
    search_generation: usize,
    pending_deletion: Option<PendingDeletion>,
    task_detail_sidebar: Entity<TaskDetailSidebar>,
    record_detail_sidebar: Entity<RecordDetailSidebar>,
}

impl SearchPanel {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input_state = cx.new(|cx| InputState::new(window, cx).placeholder("🔍 搜索内容..."));

        let _subscription = cx.subscribe_in(
            &input_state,
            window,
            |this, input_state, _event: &InputEvent, _window, cx| {
                let text = input_state.read(cx).text().to_string();
                this.on_query_change(text, cx);
            },
        );

        let focus_handle = cx.focus_handle();
        let task_sidebar_store = store.clone();
        let record_sidebar_store = store.clone();

        let mut panel = Self {
            store,
            query: String::new(),
            results: Vec::new(),
            filter_type: SearchFilterType::All,
            show_completed_tasks: false,
            browse_filter: None,
            advanced_filter_enabled: false,
            selected_tags: BTreeSet::new(),
            selected_persons: BTreeSet::new(),
            filter_mode: AdvancedFilterMode::And,
            available_tags: Vec::new(),
            available_persons: Vec::new(),
            input_state,
            focus_handle,
            _search_subscription: _subscription,
            is_searching: false,
            search_generation: 0,
            pending_deletion: None,
            task_detail_sidebar: cx
                .new(|cx| TaskDetailSidebar::new(task_sidebar_store.clone(), window, cx)),
            record_detail_sidebar: cx
                .new(|cx| RecordDetailSidebar::new(record_sidebar_store.clone(), window, cx)),
        };

        let handle = cx.entity().clone();
        panel.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_save(move |payload, cx| {
                handle.update(cx, |panel, cx| {
                    panel.handle_task_sidebar_save(&payload, cx);
                });
            });
        });
        let handle = cx.entity().clone();
        panel.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_delete(move |task_id, cx| {
                if let Ok(task_id) = Uuid::parse_str(&task_id) {
                    handle.update(cx, |panel, cx| {
                        panel.request_delete_record(task_id, cx);
                    });
                }
            });
        });
        let handle = cx.entity().clone();
        panel.task_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_close(move |cx| {
                handle.update(cx, |panel, cx| {
                    panel.handle_detail_sidebar_close(cx);
                });
            });
        });

        let handle = cx.entity().clone();
        panel.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_save(move |payload, cx| {
                handle.update(cx, |panel, cx| {
                    panel.handle_record_sidebar_save(&payload, cx);
                });
            });
        });
        let handle = cx.entity().clone();
        panel.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_delete(move |record_id, cx| {
                if let Ok(record_id) = Uuid::parse_str(&record_id) {
                    handle.update(cx, |panel, cx| {
                        panel.request_delete_record(record_id, cx);
                    });
                }
            });
        });
        let handle = cx.entity().clone();
        panel.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_close(move |cx| {
                handle.update(cx, |panel, cx| {
                    panel.handle_detail_sidebar_close(cx);
                });
            });
        });

        panel.install_data_change_listener(cx);
        panel.refresh_data(cx);
        panel
    }

    pub fn refresh_data(&mut self, cx: &mut Context<Self>) {
        self.refresh_available_metadata(cx);
        self.refresh_results(cx);
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

    fn next_browse_filter(
        current_filter: Option<&BrowseFilter>,
        clicked_filter: BrowseFilter,
    ) -> Option<BrowseFilter> {
        match current_filter {
            Some(filter) if *filter == clicked_filter => None,
            _ => Some(clicked_filter),
        }
    }

    pub fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_deletion.is_some() || self.sidebar_visible(cx) {
            self.focus_handle.focus(window, cx);
            return;
        }

        self.focus_handle.focus(window, cx);
        self.input_state.update(cx, |state, cx| {
            state.focus(window, cx);
        });
    }

    fn load_available_tags(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |view, cx| match store.get_all_tags().await {
            Ok(tags) => {
                let _ = view.update(cx, |panel, cx| {
                    panel.available_tags = tags.into_iter().map(|t| t.name).collect();
                    cx.notify();
                });
            }
            Err(e) => {
                eprintln!("[SearchPanel] Failed to load tags: {}", e);
            }
        })
        .detach();
    }

    fn on_query_change(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.refresh_results(cx);
    }

    #[allow(dead_code)]
    fn perform_search(&mut self, cx: &mut Context<Self>) {
        self.refresh_results(cx);
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.clear();
        self.input_state.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        self.refresh_results(cx);
    }

    fn set_filter_type(
        &mut self,
        filter_type: SearchFilterType,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.filter_type = filter_type;
        self.sync_open_detail_visibility(cx);
        cx.notify();
    }

    fn toggle_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        if self.advanced_filter_enabled {
            Self::toggle_multi_select(&mut self.selected_tags, tag);
        } else {
            self.browse_filter = Self::next_browse_filter(
                self.browse_filter.as_ref(),
                BrowseFilter::Tag(tag.to_string()),
            );
        }
        self.refresh_results(cx);
    }

    fn toggle_person(&mut self, person: &str, cx: &mut Context<Self>) {
        if self.advanced_filter_enabled {
            Self::toggle_multi_select(&mut self.selected_persons, person);
        } else {
            self.browse_filter = Self::next_browse_filter(
                self.browse_filter.as_ref(),
                BrowseFilter::Person(person.to_string()),
            );
        }
        self.refresh_results(cx);
    }

    fn clear_tag_filters(&mut self, cx: &mut Context<Self>) {
        self.browse_filter = None;
        self.selected_tags.clear();
        self.selected_persons.clear();
        self.refresh_results(cx);
    }

    fn set_advanced_filter_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled == self.advanced_filter_enabled {
            return;
        }

        let (browse_filter, selected_tags, selected_persons, filter_mode) =
            Self::apply_advanced_filter_toggle(
                self.browse_filter.as_ref(),
                enabled,
                &self.selected_tags,
                &self.selected_persons,
                self.filter_mode,
            );

        self.advanced_filter_enabled = enabled;
        self.browse_filter = browse_filter;
        self.selected_tags = selected_tags;
        self.selected_persons = selected_persons;
        self.filter_mode = filter_mode;
        self.refresh_results(cx);
    }

    fn toggle_advanced_filter_mode(&mut self, cx: &mut Context<Self>) {
        self.filter_mode = self.filter_mode.toggle();
        self.refresh_results(cx);
    }

    fn toggle_show_completed_tasks(&mut self, cx: &mut Context<Self>) {
        self.show_completed_tasks = !self.show_completed_tasks;
        self.sync_open_detail_visibility(cx);
        cx.notify();
    }

    fn matches_completion_visibility(show_completed_tasks: bool, record: &Record) -> bool {
        !matches!(record.record_type, RecordType::Task)
            || record.completed_at.is_none()
            || show_completed_tasks
    }

    fn should_show_record(&self, record: &Record) -> bool {
        if !self.filter_type.matches(&record.record_type) {
            return false;
        }

        if !Self::matches_completion_visibility(self.show_completed_tasks, record) {
            return false;
        }

        if self.advanced_filter_enabled {
            Self::matches_advanced_filter(
                record,
                &self.selected_tags,
                &self.selected_persons,
                self.filter_mode,
            )
        } else {
            self.browse_filter
                .as_ref()
                .map(|browse_filter| Self::matches_single_browse_filter(record, browse_filter))
                .unwrap_or(true)
        }
    }

    fn get_filtered_results(&self) -> Vec<Record> {
        let mut results: Vec<Record> = self
            .results
            .iter()
            .filter(|r| self.should_show_record(r))
            .cloned()
            .collect();

        Self::sort_results_by_created_at_desc(&mut results);
        results
    }

    fn sort_results_by_created_at_desc(results: &mut [Record]) {
        results.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| right.id.cmp(&left.id))
        });
    }

    fn load_available_persons(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |view, cx| match store.get_all_persons().await {
            Ok(persons) => {
                let _ = view.update(cx, |panel, cx| {
                    panel.available_persons =
                        persons.into_iter().map(|person| person.name).collect();
                    cx.notify();
                });
            }
            Err(e) => {
                eprintln!("[SearchPanel] Failed to load persons: {}", e);
            }
        })
        .detach();
    }

    fn refresh_results(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        let query = self.query.trim().to_string();
        let browse_filter = self.browse_filter.clone();
        let advanced_filter_enabled = self.advanced_filter_enabled;
        let has_advanced_filters = self.has_advanced_filters();

        if query.is_empty() && !advanced_filter_enabled && browse_filter.is_none() {
            self.results.clear();
            self.is_searching = false;
            self.sync_open_detail_visibility(cx);
            cx.notify();
            return;
        }

        if query.is_empty() && advanced_filter_enabled && !has_advanced_filters {
            self.results.clear();
            self.is_searching = false;
            self.sync_open_detail_visibility(cx);
            cx.notify();
            return;
        }

        let store = self.store.clone();
        self.is_searching = true;
        cx.notify();

        if !query.is_empty() {
            cx.spawn(async move |view, cx| {
                cx.background_executor()
                    .timer(StdDuration::from_millis(SEARCH_DEBOUNCE_MS))
                    .await;

                match store.search_records(&query).await {
                    Ok(records) => {
                        let _ = view.update(cx, |panel, cx| {
                            if panel.search_generation != generation {
                                return;
                            }
                            panel.results = records;
                            panel.is_searching = false;
                            panel.sync_open_detail_visibility(cx);
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        eprintln!("[SearchPanel] Search failed: {}", e);
                        let _ = view.update(cx, |panel, cx| {
                            if panel.search_generation != generation {
                                return;
                            }
                            panel.is_searching = false;
                            cx.notify();
                        });
                    }
                }
            })
            .detach();
            return;
        }

        if advanced_filter_enabled {
            cx.spawn(async move |view, cx| match store.get_all_records().await {
                Ok(records) => {
                    let _ = view.update(cx, |panel, cx| {
                        if panel.search_generation != generation {
                            return;
                        }
                        panel.results = records;
                        panel.is_searching = false;
                        panel.sync_open_detail_visibility(cx);
                        cx.notify();
                    });
                }
                Err(e) => {
                    eprintln!("[SearchPanel] Advanced filter records load failed: {}", e);
                    let _ = view.update(cx, |panel, cx| {
                        if panel.search_generation != generation {
                            return;
                        }
                        panel.is_searching = false;
                        cx.notify();
                    });
                }
            })
            .detach();
            return;
        }

        if let Some(browse_filter) = browse_filter {
            match browse_filter {
                BrowseFilter::Tag(tag) => {
                    cx.spawn(
                        async move |view, cx| match store.get_records_by_tag(&tag).await {
                            Ok(records) => {
                                let _ = view.update(cx, |panel, cx| {
                                    if panel.search_generation != generation {
                                        return;
                                    }
                                    panel.results = records;
                                    panel.is_searching = false;
                                    panel.sync_open_detail_visibility(cx);
                                    cx.notify();
                                });
                            }
                            Err(e) => {
                                eprintln!("[SearchPanel] Tagged records load failed: {}", e);
                                let _ = view.update(cx, |panel, cx| {
                                    if panel.search_generation != generation {
                                        return;
                                    }
                                    panel.is_searching = false;
                                    cx.notify();
                                });
                            }
                        },
                    )
                    .detach();
                }
                BrowseFilter::Person(person) => {
                    cx.spawn(async move |view, cx| {
                        match store.get_records_by_person(&person).await {
                            Ok(records) => {
                                let _ = view.update(cx, |panel, cx| {
                                    if panel.search_generation != generation {
                                        return;
                                    }
                                    panel.results = records;
                                    panel.is_searching = false;
                                    panel.sync_open_detail_visibility(cx);
                                    cx.notify();
                                });
                            }
                            Err(e) => {
                                eprintln!("[SearchPanel] Person records load failed: {}", e);
                                let _ = view.update(cx, |panel, cx| {
                                    if panel.search_generation != generation {
                                        return;
                                    }
                                    panel.is_searching = false;
                                    cx.notify();
                                });
                            }
                        }
                    })
                    .detach();
                }
            }
        }
    }

    fn toggle_multi_select(values: &mut BTreeSet<String>, value: &str) {
        if !values.remove(value) {
            values.insert(value.to_string());
        }
    }

    fn matches_single_browse_filter(record: &Record, browse_filter: &BrowseFilter) -> bool {
        match browse_filter {
            BrowseFilter::Tag(tag) => record.tags.iter().any(|record_tag| record_tag == tag),
            BrowseFilter::Person(person) => record
                .persons
                .iter()
                .any(|record_person| record_person == person),
        }
    }

    fn matches_advanced_filter(
        record: &Record,
        selected_tags: &BTreeSet<String>,
        selected_persons: &BTreeSet<String>,
        filter_mode: AdvancedFilterMode,
    ) -> bool {
        if selected_tags.is_empty() && selected_persons.is_empty() {
            return true;
        }

        match filter_mode {
            AdvancedFilterMode::And => {
                selected_tags
                    .iter()
                    .all(|tag| record.tags.iter().any(|record_tag| record_tag == tag))
                    && selected_persons.iter().all(|person| {
                        record
                            .persons
                            .iter()
                            .any(|record_person| record_person == person)
                    })
            }
            AdvancedFilterMode::Or => {
                selected_tags
                    .iter()
                    .any(|tag| record.tags.iter().any(|record_tag| record_tag == tag))
                    || selected_persons.iter().any(|person| {
                        record
                            .persons
                            .iter()
                            .any(|record_person| record_person == person)
                    })
            }
        }
    }

    fn apply_advanced_filter_toggle(
        browse_filter: Option<&BrowseFilter>,
        enabled: bool,
        selected_tags: &BTreeSet<String>,
        selected_persons: &BTreeSet<String>,
        filter_mode: AdvancedFilterMode,
    ) -> (
        Option<BrowseFilter>,
        BTreeSet<String>,
        BTreeSet<String>,
        AdvancedFilterMode,
    ) {
        if enabled {
            let mut next_tags = selected_tags.clone();
            let mut next_persons = selected_persons.clone();
            if let Some(browse_filter) = browse_filter {
                match browse_filter {
                    BrowseFilter::Tag(tag) => {
                        next_tags.insert(tag.clone());
                    }
                    BrowseFilter::Person(person) => {
                        next_persons.insert(person.clone());
                    }
                }
            }
            (None, next_tags, next_persons, filter_mode)
        } else {
            (
                None,
                BTreeSet::new(),
                BTreeSet::new(),
                AdvancedFilterMode::And,
            )
        }
    }

    fn has_advanced_filters(&self) -> bool {
        !self.selected_tags.is_empty() || !self.selected_persons.is_empty()
    }

    fn has_active_browse_filters(&self) -> bool {
        if self.advanced_filter_enabled {
            self.has_advanced_filters()
        } else {
            self.browse_filter.is_some()
        }
    }

    fn format_date_group(dt: DateTime<Utc>) -> String {
        let local = dt.with_timezone(&Local);
        let now = Local::now();
        let today = now.date_naive();
        let yesterday = today - Duration::days(1);
        let date = local.date_naive();

        if date == today {
            "今天".to_string()
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

    fn format_time(dt: DateTime<Utc>) -> String {
        let local = dt.with_timezone(&Local);
        local.format("%H:%M").to_string()
    }

    fn get_node_icon(record: &Record) -> &'static str {
        match record.record_type {
            RecordType::Task => match record.status {
                Some(TaskStatus::InProgress) => "▶",
                Some(TaskStatus::Done) => "☑",
                Some(TaskStatus::Cancelled) => "✕",
                _ => "◎",
            },
            RecordType::Event => "●",
            RecordType::Idea => "💡",
            RecordType::Note => "📝",
        }
    }

    fn get_node_color(record: &Record) -> Rgba {
        match record.record_type {
            RecordType::Task => match record.status {
                Some(TaskStatus::InProgress) => rgb(0x1890ff),
                Some(TaskStatus::Done) => rgb(0x52c41a),
                Some(TaskStatus::Cancelled) => rgb(0xff4d4f),
                _ => rgb(0x1890ff),
            },
            RecordType::Event => rgb(0x8c8c8c),
            RecordType::Idea => rgb(0xfaad14),
            RecordType::Note => rgb(0x722ed1),
        }
    }

    fn get_record_type_label(record: &Record) -> &'static str {
        match record.record_type {
            RecordType::Task => "任务",
            RecordType::Event => "事件",
            RecordType::Idea => "想法",
            RecordType::Note => "笔记",
        }
    }

    fn detail_target_for(record: &Record) -> SearchDetailTarget {
        match record.record_type {
            RecordType::Task => SearchDetailTarget::Task,
            RecordType::Note | RecordType::Idea | RecordType::Event => SearchDetailTarget::Record,
        }
    }

    fn sidebar_visible(&self, cx: &App) -> bool {
        self.task_detail_sidebar
            .read(cx)
            .current_task_id()
            .is_some()
            || self
                .record_detail_sidebar
                .read(cx)
                .current_record_id()
                .is_some()
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
            SearchDetailTarget::Task => selected_task_id == Some(record.id.to_string().as_str()),
            SearchDetailTarget::Record => {
                selected_record_id == Some(record.id.to_string().as_str())
            }
        }
    }

    fn handle_detail_sidebar_close(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn refresh_available_metadata(&mut self, cx: &mut Context<Self>) {
        self.load_available_tags(cx);
        self.load_available_persons(cx);
    }

    fn select_result(&mut self, record: &Record, window: &mut Window, cx: &mut Context<Self>) {
        match Self::detail_target_for(record) {
            SearchDetailTarget::Task => {
                self.record_detail_sidebar.update(cx, |sidebar, cx| {
                    sidebar.dismiss(cx);
                });
                self.task_detail_sidebar.update(cx, |sidebar, cx| {
                    sidebar.show_task(record, window, cx);
                });
            }
            SearchDetailTarget::Record => {
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

    fn sync_open_detail_visibility(&mut self, cx: &mut Context<Self>) {
        let filtered_results = self.get_filtered_results();
        let current_task_id = self.selected_task_id(cx);
        let current_record_id = self.selected_record_id(cx);
        let should_keep_task = current_task_id.as_ref().is_some_and(|task_id| {
            filtered_results.iter().any(|record| {
                matches!(Self::detail_target_for(record), SearchDetailTarget::Task)
                    && record.id.to_string() == *task_id
            })
        });
        let should_keep_record = current_record_id.as_ref().is_some_and(|record_id| {
            filtered_results.iter().any(|record| {
                matches!(Self::detail_target_for(record), SearchDetailTarget::Record)
                    && record.id.to_string() == *record_id
            })
        });

        let mut changed = false;
        if current_task_id.is_some() && !should_keep_task {
            self.task_detail_sidebar.update(cx, |sidebar, cx| {
                sidebar.dismiss(cx);
            });
            changed = true;
        }
        if current_record_id.is_some() && !should_keep_record {
            self.record_detail_sidebar.update(cx, |sidebar, cx| {
                sidebar.dismiss(cx);
            });
            changed = true;
        }

        if changed {
            cx.notify();
        }
    }

    fn handle_task_sidebar_save(&mut self, payload: &TaskSavePayload, cx: &mut Context<Self>) {
        if let Some(task) = self
            .results
            .iter_mut()
            .find(|record| record.id.to_string() == payload.task_id)
        {
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
            let now = chrono::Utc::now();
            task.updated_at = now;
            task.sync_task_lifecycle_fields(previous_status, now);

            let updated_task = task.clone();
            let line_ref = payload.line.clone();
            let store = self.store.clone();
            cx.spawn(async move |_view, _cx| {
                if let Err(e) = store.update_record_with_line(updated_task, line_ref).await {
                    eprintln!("[SearchPanel] Failed to update task: {}", e);
                }
            })
            .detach();

            self.refresh_available_metadata(cx);
            self.sync_open_detail_visibility(cx);
            cx.notify();
        }
    }

    fn handle_record_sidebar_save(&mut self, payload: &RecordSavePayload, cx: &mut Context<Self>) {
        if let Some(record) = self
            .results
            .iter_mut()
            .find(|item| item.id.to_string() == payload.record_id)
        {
            record.title = payload.title.clone();
            record.content = payload.content.clone();
            record.tags = payload.tags.clone();
            record.persons = payload.persons.clone();
            record.updated_at = chrono::Utc::now();

            let updated_record = record.clone();
            let line_ref = payload.line.clone();
            let store = self.store.clone();
            cx.spawn(async move |_view, _cx| {
                if let Err(e) = store
                    .update_record_with_line(updated_record, line_ref)
                    .await
                {
                    eprintln!("[SearchPanel] Failed to update record: {}", e);
                }
            })
            .detach();

            self.refresh_available_metadata(cx);
            self.sync_open_detail_visibility(cx);
            cx.notify();
        }
    }

    fn request_delete_record(&mut self, record_id: Uuid, cx: &mut Context<Self>) {
        if let Some(record) = self.results.iter().find(|record| record.id == record_id) {
            self.pending_deletion = Some(PendingDeletion {
                id: record_id,
                record_label: match Self::detail_target_for(record) {
                    SearchDetailTarget::Task => "任务",
                    SearchDetailTarget::Record => "记录",
                },
                display_title: record.display_title(),
            });
            cx.notify();
        }
    }

    fn cancel_delete_confirmation(&mut self, cx: &mut Context<Self>) {
        self.pending_deletion = None;
        cx.notify();
    }

    fn confirm_delete_record(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_deletion.clone() else {
            return;
        };

        self.perform_delete_record(pending.id, true, cx);
    }

    fn remove_record_from_results(results: &mut Vec<Record>, record_id: Uuid) -> bool {
        let original_len = results.len();
        results.retain(|record| record.id != record_id);
        results.len() != original_len
    }

    fn perform_delete_record(
        &mut self,
        record_id: Uuid,
        clear_confirmation: bool,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let record_id_string = record_id.to_string();
        cx.spawn(
            async move |view, cx| match store.delete_record(record_id).await {
                Ok(_) => {
                    view.update(cx, |panel, cx| {
                        if clear_confirmation {
                            panel.pending_deletion = None;
                        }

                        Self::remove_record_from_results(&mut panel.results, record_id);

                        if panel.task_detail_sidebar.read(cx).current_task_id()
                            == Some(record_id_string.as_str())
                        {
                            panel.task_detail_sidebar.update(cx, |sidebar, cx| {
                                sidebar.dismiss(cx);
                            });
                        }

                        if panel.record_detail_sidebar.read(cx).current_record_id()
                            == Some(record_id_string.as_str())
                        {
                            panel.record_detail_sidebar.update(cx, |sidebar, cx| {
                                sidebar.dismiss(cx);
                            });
                        }

                        panel.refresh_available_metadata(cx);
                        panel.sync_open_detail_visibility(cx);
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => eprintln!("[SearchPanel] Failed to delete record: {}", e),
            },
        )
        .detach();
    }

    fn query_terms(&self) -> Vec<String> {
        self.query
            .split_whitespace()
            .filter(|term| !term.is_empty())
            .map(|term| term.to_string())
            .collect()
    }

    fn text_match_ranges(&self, content: &str) -> Vec<(usize, usize)> {
        let content_lower = content.to_lowercase();
        let mut ranges = Vec::new();

        for term in self.query_terms() {
            let term_lower = term.to_lowercase();
            for (start, _) in content_lower.match_indices(&term_lower) {
                ranges.push((start, start + term_lower.len()));
            }
        }

        ranges.sort_by_key(|(start, end)| (*start, *end));

        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in ranges {
            if let Some(last) = merged.last_mut() {
                if start <= last.1 {
                    last.1 = last.1.max(end);
                    continue;
                }
            }
            merged.push((start, end));
        }

        merged
    }

    fn highlight_match_text(
        &self,
        content: &str,
        base_color: Rgba,
        base_weight: FontWeight,
    ) -> AnyElement {
        let ranges = self.text_match_ranges(content);
        if ranges.is_empty() {
            return div()
                .text_color(base_color)
                .font_weight(base_weight)
                .child(content.to_string())
                .into_any_element();
        }

        let mut elements: Vec<AnyElement> = Vec::new();
        let mut last_end = 0;

        for (start, end) in ranges {
            if start > last_end {
                elements.push(
                    div()
                        .text_color(base_color)
                        .font_weight(base_weight)
                        .child(content[last_end..start].to_string())
                        .into_any_element(),
                );
            }

            elements.push(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0x1890ff))
                    .child(content[start..end].to_string())
                    .into_any_element(),
            );
            last_end = end;
        }

        if last_end < content.len() {
            elements.push(
                div()
                    .text_color(base_color)
                    .font_weight(base_weight)
                    .child(content[last_end..].to_string())
                    .into_any_element(),
            );
        }

        h_flex().flex_wrap().children(elements).into_any_element()
    }

    fn render_search_tokenized_text(
        &self,
        content: &str,
        base_color: Rgba,
        base_weight: FontWeight,
    ) -> AnyElement {
        let lines = tokenize_text(content);
        let render_line = |line: &[TextTokenSegment]| {
            h_flex()
                .w_full()
                .gap(px(0.0))
                .flex_wrap()
                .items_center()
                .children(line.iter().map(|segment| match segment {
                    TextTokenSegment::Plain(text) => {
                        self.highlight_match_text(text, base_color, base_weight)
                    }
                    TextTokenSegment::Line(_)
                    | TextTokenSegment::Tag(_)
                    | TextTokenSegment::Person(_) => render_inline_token_text(
                        segment,
                        TokenTextStyle::new(base_color, base_weight),
                    ),
                }))
        };

        if lines.len() == 1 {
            return render_line(&lines[0]).into_any_element();
        }

        v_flex()
            .w_full()
            .gap(px(4.0))
            .children(lines.iter().map(|line| render_line(line)))
            .into_any_element()
    }

    fn first_match_char_index(&self, content: &str) -> Option<usize> {
        let content_lower = content.to_lowercase();
        self.query_terms()
            .into_iter()
            .filter_map(|term| {
                let term_lower = term.to_lowercase();
                content_lower
                    .find(&term_lower)
                    .map(|byte_idx| content[..byte_idx].chars().count())
            })
            .min()
    }

    fn excerpt_around_match(&self, content: &str, max_chars: usize) -> String {
        let chars: Vec<char> = content.chars().collect();
        if chars.len() <= max_chars {
            return content.to_string();
        }

        let match_start = self.first_match_char_index(content).unwrap_or(0);
        let preferred_start = match_start.saturating_sub(max_chars / 3);
        let max_start = chars.len().saturating_sub(max_chars);
        let start = preferred_start.min(max_start);
        let end = (start + max_chars).min(chars.len());

        let mut excerpt: String = chars[start..end].iter().collect();
        if start > 0 {
            excerpt = format!("…{}", excerpt);
        }
        if end < chars.len() {
            excerpt.push('…');
        }
        excerpt
    }

    fn body_preview(&self, record: &Record) -> Option<String> {
        let body = if let Some(title) = record.title.as_deref() {
            let title = title.trim();
            let mut skipped_title = false;
            record
                .content
                .lines()
                .filter_map(|line| {
                    if !skipped_title && !title.is_empty() && line.trim() == title {
                        skipped_title = true;
                        None
                    } else {
                        Some(line)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            record.content.clone()
        };

        let body = body.trim();
        if body.is_empty() {
            return None;
        }

        Some(self.excerpt_around_match(body, SEARCH_BODY_PREVIEW_LIMIT))
    }

    fn render_search_input(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap(px(8.0))
            .items_center()
            .child(div().flex_1().child(Input::new(&self.input_state)))
            .when(!self.query.is_empty(), |el| {
                el.child(
                    Button::new("clear-search")
                        .child("清除")
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.clear_search(window, cx);
                        })),
                )
            })
    }

    fn render_type_filter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filters = vec![
            SearchFilterType::All,
            SearchFilterType::Task,
            SearchFilterType::Note,
            SearchFilterType::Idea,
        ];

        h_flex()
            .gap(px(8.0))
            .flex_wrap()
            .items_center()
            .child(
                h_flex()
                    .gap(px(4.0))
                    .children(filters.into_iter().enumerate().map(|(idx, filter)| {
                        let is_selected = self.filter_type == filter;
                        let filter_clone = filter;

                        div()
                            .id(("filter", idx))
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(16.0))
                            .cursor_pointer()
                            .border_1()
                            .border_color(if is_selected {
                                rgb(0x1890ff)
                            } else {
                                rgb(0xd9d9d9)
                            })
                            .bg(if is_selected {
                                rgb(0xe6f7ff)
                            } else {
                                rgb(0xffffff)
                            })
                            .text_color(if is_selected {
                                rgb(0x1890ff)
                            } else {
                                rgb(0x595959)
                            })
                            .text_sm()
                            .hover(|s| {
                                s.bg(if is_selected {
                                    rgb(0xbae7ff)
                                } else {
                                    rgb(0xf5f5f5)
                                })
                            })
                            .child(filter.label())
                            .on_click(cx.listener(move |this, _event, window, cx| {
                                this.set_filter_type(filter_clone, window, cx);
                            }))
                    })),
            )
            .child(
                div()
                    .id("search-show-completed-tasks")
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(16.0))
                    .cursor_pointer()
                    .border_1()
                    .border_color(if self.show_completed_tasks {
                        rgb(0x1890ff)
                    } else {
                        rgb(0xd9d9d9)
                    })
                    .bg(if self.show_completed_tasks {
                        rgb(0xe6f7ff)
                    } else {
                        rgb(0xffffff)
                    })
                    .text_color(if self.show_completed_tasks {
                        rgb(0x1890ff)
                    } else {
                        rgb(0x595959)
                    })
                    .text_sm()
                    .hover(|s| {
                        s.bg(if self.show_completed_tasks {
                            rgb(0xbae7ff)
                        } else {
                            rgb(0xf5f5f5)
                        })
                    })
                    .child(format!(
                        "{} 已完成任务",
                        if self.show_completed_tasks {
                            "☑"
                        } else {
                            "☐"
                        }
                    ))
                    .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.toggle_show_completed_tasks(cx);
                    })),
            )
    }

    fn render_tag_filter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_selected = self.has_active_browse_filters();
        let show_filter_row =
            !self.available_tags.is_empty() || !self.available_persons.is_empty() || has_selected;

        v_flex().gap(px(8.0)).when(show_filter_row, |el| {
            el.child(
                h_flex()
                    .gap(px(4.0))
                    .flex_wrap()
                    .items_center()
                    .child(
                        div()
                            .id("advanced-filter-toggle")
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(12.0))
                            .cursor_pointer()
                            .border_1()
                            .border_color(if self.advanced_filter_enabled {
                                rgb(0x1890ff)
                            } else {
                                rgb(0xd9d9d9)
                            })
                            .bg(if self.advanced_filter_enabled {
                                rgb(0xe6f7ff)
                            } else {
                                rgb(0xffffff)
                            })
                            .text_color(if self.advanced_filter_enabled {
                                rgb(0x1890ff)
                            } else {
                                rgb(0x595959)
                            })
                            .text_sm()
                            .hover(|s| {
                                s.bg(if self.advanced_filter_enabled {
                                    rgb(0xbae7ff)
                                } else {
                                    rgb(0xf5f5f5)
                                })
                            })
                            .child(format!(
                                "{} 高级筛选",
                                if self.advanced_filter_enabled {
                                    "☑"
                                } else {
                                    "☐"
                                }
                            ))
                            .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                                this.set_advanced_filter_enabled(!this.advanced_filter_enabled, cx);
                            })),
                    )
                    .children(self.available_tags.iter().enumerate().map(|(idx, tag)| {
                        let is_selected = if self.advanced_filter_enabled {
                            self.selected_tags.contains(tag)
                        } else {
                            self.browse_filter.as_ref() == Some(&BrowseFilter::Tag(tag.clone()))
                        };
                        let tag_clone = tag.clone();
                        div()
                            .id(("search-tag-filter", idx))
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(12.0))
                            .cursor_pointer()
                            .border_1()
                            .border_color(if is_selected {
                                rgb(0x1890ff)
                            } else {
                                rgb(0xd9d9d9)
                            })
                            .bg(if is_selected {
                                rgb(0xe6f7ff)
                            } else {
                                rgb(0xffffff)
                            })
                            .text_color(if is_selected {
                                rgb(0x1890ff)
                            } else {
                                rgb(0x595959)
                            })
                            .text_sm()
                            .hover(|s| {
                                s.bg(if is_selected {
                                    rgb(0xbae7ff)
                                } else {
                                    rgb(0xf5f5f5)
                                })
                            })
                            .child(format!("#{}", tag))
                            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                                this.toggle_tag(&tag_clone, cx);
                            }))
                    }))
                    .children(
                        self.available_persons
                            .iter()
                            .enumerate()
                            .map(|(idx, person)| {
                                let is_selected = if self.advanced_filter_enabled {
                                    self.selected_persons.contains(person)
                                } else {
                                    self.browse_filter.as_ref()
                                        == Some(&BrowseFilter::Person(person.clone()))
                                };
                                let person_clone = person.clone();
                                div()
                                    .id(("search-person-filter", idx))
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(12.0))
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(if is_selected {
                                        rgb(0x1890ff)
                                    } else {
                                        rgb(0xd9d9d9)
                                    })
                                    .bg(if is_selected {
                                        rgb(0xe6f7ff)
                                    } else {
                                        rgb(0xffffff)
                                    })
                                    .text_color(if is_selected {
                                        rgb(0x1890ff)
                                    } else {
                                        rgb(0x595959)
                                    })
                                    .text_sm()
                                    .hover(|s| {
                                        s.bg(if is_selected {
                                            rgb(0xbae7ff)
                                        } else {
                                            rgb(0xf5f5f5)
                                        })
                                    })
                                    .child(format!("@{}", person))
                                    .on_click(cx.listener(
                                        move |this, _event: &ClickEvent, _window, cx| {
                                            this.toggle_person(&person_clone, cx);
                                        },
                                    ))
                            }),
                    )
                    .when(
                        self.advanced_filter_enabled && self.has_advanced_filters(),
                        |el| {
                            el.child(
                                Button::new("search-toggle-mode")
                                    .child(self.filter_mode.label().to_string())
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.toggle_advanced_filter_mode(cx);
                                    })),
                            )
                        },
                    )
                    .when(has_selected, |el| {
                        el.child(Button::new("clear-search-tags").child("清除筛选").on_click(
                            cx.listener(|this, _event, _window, cx| {
                                this.clear_tag_filters(cx);
                            }),
                        ))
                    }),
            )
        })
    }

    fn render_search_result_item(
        &self,
        record: &Record,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let time_str = Self::format_time(record.created_at);
        let icon = Self::get_node_icon(record);
        let icon_color = Self::get_node_color(record);
        let record_type_label = Self::get_record_type_label(record);
        let title = self.excerpt_around_match(&record.display_title(), SEARCH_TITLE_PREVIEW_LIMIT);
        let body_preview = self.body_preview(record);
        let tags = record.tags.clone();
        let persons = record.persons.clone();

        div()
            .id(record.id)
            .w_full()
            .min_w(px(0.0))
            .p(px(12.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(if is_selected {
                rgb(0x1890ff)
            } else {
                rgb(0xe8e8e8)
            })
            .bg(if is_selected {
                rgb(0xe6f7ff)
            } else {
                rgb(0xffffff)
            })
            .cursor_pointer()
            .hover(|style| {
                style.bg(if is_selected {
                    rgb(0xe6f7ff)
                } else {
                    rgb(0xf6ffed)
                })
            })
            .on_click(cx.listener({
                let record = record.clone();
                move |this, _event: &ClickEvent, window, cx| {
                    this.select_result(&record, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(12.0))
                    .items_start()
                    .child(
                        div()
                            .w(px(60.0))
                            .text_sm()
                            .text_color(rgb(0x8c8c8c))
                            .child(time_str),
                    )
                    .child(div().text_color(icon_color).text_sm().child(icon))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                h_flex().gap(px(8.0)).items_center().child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(0xf0f0f0))
                                        .text_xs()
                                        .text_color(rgb(0x8c8c8c))
                                        .child(record_type_label),
                                ),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x262626))
                                    .child(self.render_search_tokenized_text(
                                        &title,
                                        rgb(0x262626),
                                        FontWeight::SEMIBOLD,
                                    )),
                            )
                            .when_some(body_preview, |el, body| {
                                el.child(div().text_sm().text_color(rgb(0x8c8c8c)).child(
                                    self.render_search_tokenized_text(
                                        &body,
                                        rgb(0x8c8c8c),
                                        FontWeight::NORMAL,
                                    ),
                                ))
                            })
                            .child(h_flex().gap(px(6.0)).flex_wrap().children(
                                tags.into_iter().enumerate().map(|(idx, tag)| {
                                    div()
                                        .id(("result-tag", idx))
                                        .child(render_metadata_chip(MetadataChipKind::Tag, &tag))
                                }),
                            ))
                            .when(!persons.is_empty(), |el| {
                                el.child(h_flex().gap(px(6.0)).flex_wrap().children(
                                    persons.into_iter().enumerate().map(|(idx, person)| {
                                        div().id(("result-person", idx)).child(
                                            render_metadata_chip(MetadataChipKind::Person, &person),
                                        )
                                    }),
                                ))
                            }),
                    ),
            )
    }

    fn group_results_by_date(&self, results: &[Record]) -> Vec<(String, Vec<Record>)> {
        let mut groups: Vec<(String, Vec<Record>)> = Vec::new();
        let mut current_group: Option<(String, Vec<Record>)> = None;

        for record in results {
            let date_group = Self::format_date_group(record.created_at);

            match &mut current_group {
                Some((group_date, group_records)) if group_date == &date_group => {
                    group_records.push(record.clone());
                }
                _ => {
                    if let Some(group) = current_group.take() {
                        groups.push(group);
                    }
                    current_group = Some((date_group, vec![record.clone()]));
                }
            }
        }

        if let Some(group) = current_group {
            groups.push(group);
        }

        groups
    }

    fn render_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filtered_results = self.get_filtered_results();
        let result_count = filtered_results.len();
        let grouped_results = self.group_results_by_date(&filtered_results);
        let is_searching = self.is_searching;
        let has_query = !self.query.trim().is_empty();
        let browse_active = !has_query && self.has_active_browse_filters();
        let browse_filter = self.browse_filter.clone();
        let selected_task_id = self.selected_task_id(cx);
        let selected_record_id = self.selected_record_id(cx);

        div()
            .id("search-results")
            .size_full()
            .flex()
            .flex_col()
            .pr(px(16.0))
            .overflow_y_scrollbar()
            .child(div().py(px(8.0)).child(if is_searching {
                div()
                    .text_sm()
                    .text_color(rgb(0x8c8c8c))
                    .child(if has_query {
                        "搜索中..."
                    } else {
                        "加载中..."
                    })
            } else if has_query {
                div()
                    .text_sm()
                    .text_color(rgb(0x595959))
                    .child(format!("找到 {} 个结果：", result_count))
            } else if self.advanced_filter_enabled && self.has_advanced_filters() {
                div()
                    .text_sm()
                    .text_color(rgb(0x595959))
                    .child(format!("高级筛选下共 {} 个结果：", result_count))
            } else if let Some(browse_filter) = browse_filter.as_ref() {
                div()
                    .text_sm()
                    .text_color(rgb(0x595959))
                    .child(match browse_filter {
                        BrowseFilter::Tag(tag) => {
                            format!("#{} 下共 {} 个结果：", tag, result_count)
                        }
                        BrowseFilter::Person(person) => {
                            format!("@{} 下共 {} 个结果：", person, result_count)
                        }
                    })
            } else {
                div()
                    .text_sm()
                    .text_color(rgb(0x8c8c8c))
                    .child("输入关键词开始搜索")
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(grouped_results.into_iter().map(|(date_group, records)| {
                        v_flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .pt(px(16.0))
                                    .pb(px(8.0))
                                    .border_b_1()
                                    .border_color(rgb(0xe8e8e8))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0x262626))
                                            .child(date_group),
                                    ),
                            )
                            .children(records.iter().enumerate().map(|(idx, record)| {
                                let is_selected = Self::is_record_selected(
                                    record,
                                    selected_task_id.as_deref(),
                                    selected_record_id.as_deref(),
                                );
                                div()
                                    .id(("result", idx))
                                    .child(self.render_search_result_item(record, is_selected, cx))
                            }))
                    }))
                    .when(
                        filtered_results.is_empty()
                            && (has_query || browse_active)
                            && !is_searching,
                        |el| {
                            el.child(
                                div()
                                    .py(px(32.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(div().text_sm().text_color(rgb(0xbfbfbf)).child(
                                        if has_query {
                                            "未找到匹配的结果"
                                        } else {
                                            "该筛选下暂无内容"
                                        },
                                    )),
                            )
                        },
                    ),
            )
    }

    fn render_delete_confirmation(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.pending_deletion.as_ref()?;
        let title = pending.display_title.clone();
        let record_label = pending.record_label;

        Some(
            div()
                .id("search-delete-confirm-overlay")
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0xf5f5f5))
                .on_click(cx.listener(|_this, _event: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(360.0))
                        .max_w(px(360.0))
                        .p(px(20.0))
                        .rounded(px(12.0))
                        .bg(rgb(0xffffff))
                        .border_1()
                        .border_color(rgb(0xe8e8e8))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .cursor_default()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("删除{}", record_label)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x666666))
                                .child(format!("确认删除“{}”？删除后无法恢复。", title)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x999999))
                                .child("按 Enter 确认，按 Esc 取消"),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(
                                    Button::new("search-delete-confirm-cancel")
                                        .child("取消")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.cancel_delete_confirmation(cx);
                                        })),
                                )
                                .child(
                                    Button::new("search-delete-confirm-submit")
                                        .child("确认删除")
                                        .text_color(rgb(0xff4d4f))
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.confirm_delete_record(cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for SearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_deletion.is_some() {
            self.focus_handle.focus(window, cx);
        }

        v_flex()
            .size_full()
            .overflow_hidden()
            .relative()
            .p(px(24.0))
            .gap(px(18.0))
            .track_focus(&self.focus_handle(cx))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.pending_deletion.is_none() {
                    return;
                }

                match event.keystroke.key.as_str() {
                    "enter" => {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.confirm_delete_record(cx);
                    }
                    "escape" => {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.cancel_delete_confirmation(cx);
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x262626))
                    .child("搜索"),
            )
            .child(self.render_search_input(cx))
            .child(self.render_type_filter(cx))
            .child(self.render_tag_filter(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_results(cx)),
            )
            .child(self.task_detail_sidebar.clone())
            .child(self.record_detail_sidebar.clone())
            .children(self.render_delete_confirmation(cx))
    }
}

impl Focusable for SearchPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{AdvancedFilterMode, BrowseFilter, SearchDetailTarget, SearchPanel};
    use crate::models::{Priority, Record, RecordType};
    use chrono::{Duration, Utc};
    use std::collections::BTreeSet;
    use uuid::Uuid;

    #[test]
    fn test_next_browse_filter_supports_single_select_toggle() {
        assert_eq!(
            SearchPanel::next_browse_filter(None, BrowseFilter::Tag("开发".to_string())),
            Some(BrowseFilter::Tag("开发".to_string()))
        );
        assert_eq!(
            SearchPanel::next_browse_filter(
                Some(&BrowseFilter::Tag("开发".to_string())),
                BrowseFilter::Tag("开发".to_string())
            ),
            None
        );
        assert_eq!(
            SearchPanel::next_browse_filter(
                Some(&BrowseFilter::Tag("开发".to_string())),
                BrowseFilter::Tag("测试".to_string())
            ),
            Some(BrowseFilter::Tag("测试".to_string()))
        );
        assert_eq!(
            SearchPanel::next_browse_filter(
                Some(&BrowseFilter::Tag("开发".to_string())),
                BrowseFilter::Person("张三".to_string())
            ),
            Some(BrowseFilter::Person("张三".to_string()))
        );
    }

    #[test]
    fn test_matches_advanced_filter_in_and_mode() {
        let mut record = Record::new_task(
            "搜索高级筛选".to_string(),
            "测试内容".to_string(),
            Priority::Medium,
        );
        record.tags = vec!["开发".to_string(), "测试".to_string()];
        record.persons = vec!["张三".to_string()];

        let selected_tags = BTreeSet::from(["开发".to_string(), "测试".to_string()]);
        let selected_persons = BTreeSet::from(["张三".to_string()]);
        assert!(SearchPanel::matches_advanced_filter(
            &record,
            &selected_tags,
            &selected_persons,
            AdvancedFilterMode::And,
        ));

        let missing_persons = BTreeSet::from(["李四".to_string()]);
        assert!(!SearchPanel::matches_advanced_filter(
            &record,
            &selected_tags,
            &missing_persons,
            AdvancedFilterMode::And,
        ));
    }

    #[test]
    fn test_matches_advanced_filter_in_or_mode() {
        let mut record = Record::new_task(
            "搜索高级筛选".to_string(),
            "测试内容".to_string(),
            Priority::Medium,
        );
        record.tags = vec!["开发".to_string()];
        record.persons = vec!["张三".to_string()];

        let selected_tags = BTreeSet::from(["测试".to_string()]);
        let selected_persons = BTreeSet::from(["张三".to_string()]);
        assert!(SearchPanel::matches_advanced_filter(
            &record,
            &selected_tags,
            &selected_persons,
            AdvancedFilterMode::Or,
        ));

        let unmatched_persons = BTreeSet::from(["李四".to_string()]);
        assert!(!SearchPanel::matches_advanced_filter(
            &record,
            &selected_tags,
            &unmatched_persons,
            AdvancedFilterMode::Or,
        ));
    }

    #[test]
    fn test_apply_advanced_filter_toggle_promotes_single_filter_and_clears_on_disable() {
        let (browse_filter, selected_tags, selected_persons, filter_mode) =
            SearchPanel::apply_advanced_filter_toggle(
                Some(&BrowseFilter::Tag("开发".to_string())),
                true,
                &BTreeSet::new(),
                &BTreeSet::new(),
                AdvancedFilterMode::Or,
            );
        assert_eq!(browse_filter, None);
        assert_eq!(selected_tags, BTreeSet::from(["开发".to_string()]));
        assert!(selected_persons.is_empty());
        assert_eq!(filter_mode, AdvancedFilterMode::Or);

        let (browse_filter, selected_tags, selected_persons, filter_mode) =
            SearchPanel::apply_advanced_filter_toggle(
                None,
                false,
                &selected_tags,
                &selected_persons,
                filter_mode,
            );
        assert_eq!(browse_filter, None);
        assert!(selected_tags.is_empty());
        assert!(selected_persons.is_empty());
        assert_eq!(filter_mode, AdvancedFilterMode::And);
    }

    #[test]
    fn test_sort_results_by_created_at_desc_keeps_latest_first() {
        let now = Utc::now();

        let mut oldest =
            Record::new_task("最早".to_string(), "最早内容".to_string(), Priority::Low);
        oldest.created_at = now - Duration::days(3);
        oldest.updated_at = oldest.created_at;

        let mut newest =
            Record::new_task("最新".to_string(), "最新内容".to_string(), Priority::High);
        newest.created_at = now;
        newest.updated_at = newest.created_at;

        let mut middle =
            Record::new_task("中间".to_string(), "中间内容".to_string(), Priority::Medium);
        middle.created_at = now - Duration::days(1);
        middle.updated_at = middle.created_at;

        let mut results = vec![middle.clone(), oldest.clone(), newest.clone()];
        SearchPanel::sort_results_by_created_at_desc(&mut results);

        assert_eq!(
            results
                .iter()
                .map(|record| record.title.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["最新", "中间", "最早"]
        );
    }

    #[test]
    fn test_detail_target_routes_tasks_and_non_tasks_to_correct_sidebar() {
        let task = Record::new_task("任务".to_string(), "内容".to_string(), Priority::Medium);
        assert_eq!(
            SearchPanel::detail_target_for(&task),
            SearchDetailTarget::Task
        );

        for record_type in [RecordType::Note, RecordType::Idea, RecordType::Event] {
            let mut record = Record::new_note("记录".to_string());
            record.record_type = record_type;
            assert_eq!(
                SearchPanel::detail_target_for(&record),
                SearchDetailTarget::Record
            );
        }
    }

    #[test]
    fn test_remove_record_from_results_removes_matching_record_only() {
        let mut records = vec![
            Record::new_task("任务 A".to_string(), "内容".to_string(), Priority::Low),
            Record::new_task("任务 B".to_string(), "内容".to_string(), Priority::Medium),
        ];
        let removed_id = records[0].id;
        let kept_id = records[1].id;

        assert!(SearchPanel::remove_record_from_results(
            &mut records,
            removed_id
        ));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, kept_id);
        assert!(!SearchPanel::remove_record_from_results(
            &mut records,
            Uuid::new_v4()
        ));
    }

    #[test]
    fn test_matches_completion_visibility_hides_completed_tasks_by_default() {
        let mut completed_task =
            Record::new_task("已完成".to_string(), "内容".to_string(), Priority::Low);
        completed_task.completed_at = Some(Utc::now());
        completed_task.status = Some(crate::models::TaskStatus::Done);

        let active_task =
            Record::new_task("进行中".to_string(), "内容".to_string(), Priority::Medium);
        let note = Record::new_note("普通记录".to_string());

        assert!(!SearchPanel::matches_completion_visibility(
            false,
            &completed_task
        ));
        assert!(SearchPanel::matches_completion_visibility(
            true,
            &completed_task
        ));
        assert!(SearchPanel::matches_completion_visibility(
            false,
            &active_task
        ));
        assert!(SearchPanel::matches_completion_visibility(false, &note));
    }
}
