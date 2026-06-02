use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use gpui::{prelude::*, *};
use gpui_component::{
    button::Button,
    date_picker::{DatePicker, DatePickerState},
    h_flex,
    input::{Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Paste},
    scroll::ScrollableElement,
    v_flex, Sizable,
};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use crate::models::{Attachment, AttachmentStatus, LineRef, Priority, Record, TaskStatus};
use crate::platform::{open_saved_attachment, pick_image_files, ParentWindowHint};
use crate::store::Store;
use crate::ui::metadata_autocomplete::{
    apply_completion_to_input, autocomplete_item, render_autocomplete_menu,
    MetadataAutocompleteAction, MetadataAutocompleteState, MetadataCatalog,
};
use crate::ui::parsing;
use crate::ui::style::{ClaudeLikeColors, TaskTypography};
use crate::ui::tokenized_text::{render_tokenized_text, MetadataChipKind, TokenTextStyle};
use std::time::Duration;

const TASK_DETAIL_DOCKED_WIDTH: Pixels = px(340.0);
const TASK_DETAIL_DOCKED_CONTAINER_WIDTH: Pixels = px(340.0);

#[derive(Clone)]
struct AttachmentPreview {
    attachment: Attachment,
    preview_image: Option<Arc<Image>>,
}

pub struct TaskDetailSidebar {
    store: Store,
    current_task_id: Option<String>,
    task_title: Option<String>,
    task_content: String,
    priority: Option<Priority>,
    status: Option<TaskStatus>,
    due_date: Option<DateTime<Utc>>,
    scheduled_for: Option<DateTime<Utc>>,
    cancel_reason: Option<String>,
    line: Option<LineRef>,
    tags: Vec<String>,
    persons: Vec<String>,
    inline_tags: Vec<String>,
    inline_persons: Vec<String>,
    date_picker: Option<Entity<DatePickerState>>,
    time_input: Option<Entity<InputState>>,
    reminder_date_picker: Option<Entity<DatePickerState>>,
    reminder_time_input: Option<Entity<InputState>>,
    title_input: Option<Entity<InputState>>,
    content_input: Option<Entity<InputState>>,
    title_input_subscription: Option<Subscription>,
    content_input_subscription: Option<Subscription>,
    title_metadata_autocomplete: MetadataAutocompleteState,
    content_metadata_autocomplete: MetadataAutocompleteState,
    editing_title: bool,
    editing_content: bool,
    content_expanded: bool,
    attachments: Vec<AttachmentPreview>,
    active_attachment_preview: Option<AttachmentPreview>,
    attachments_loading: bool,
    attachment_error: Option<String>,
    on_save: Option<Box<dyn Fn(SavePayload, &mut Context<Self>) + Send + Sync>>,
    on_delete: Option<Box<dyn Fn(String, &mut Context<Self>) + Send + Sync>>,
    on_reopen: Option<Box<dyn Fn(String, &mut Context<Self>) + Send + Sync>>,
    on_close: Option<Box<dyn Fn(&mut Context<Self>) + Send + Sync>>,
    presentation: TaskDetailPresentation,
}

/// 保存时的数据载荷
#[derive(Debug, Clone)]
pub struct SavePayload {
    pub task_id: String,
    pub title: Option<String>,
    pub content: String,
    pub priority: Priority,
    pub status: TaskStatus,
    pub due_date: Option<DateTime<Utc>>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub cancel_reason: Option<String>,
    pub tags: Vec<String>,
    pub persons: Vec<String>,
    pub line: Option<LineRef>,
}

/// 侧边栏显示状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SidebarState {
    Hidden,
    Visible,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskDetailPresentation {
    Overlay,
    Docked,
}

impl TaskDetailSidebar {
    pub fn new(store: Store, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            store,
            current_task_id: None,
            task_title: None,
            task_content: String::new(),
            priority: None,
            status: None,
            due_date: None,
            scheduled_for: None,
            cancel_reason: None,
            line: None,
            tags: Vec::new(),
            persons: Vec::new(),
            inline_tags: Vec::new(),
            inline_persons: Vec::new(),
            date_picker: None,
            time_input: None,
            reminder_date_picker: None,
            reminder_time_input: None,
            title_input: None,
            content_input: None,
            title_input_subscription: None,
            content_input_subscription: None,
            title_metadata_autocomplete: MetadataAutocompleteState::default(),
            content_metadata_autocomplete: MetadataAutocompleteState::default(),
            editing_title: false,
            editing_content: false,
            content_expanded: false,
            attachments: Vec::new(),
            active_attachment_preview: None,
            attachments_loading: false,
            attachment_error: None,
            on_save: None,
            on_delete: None,
            on_reopen: None,
            on_close: None,
            presentation: TaskDetailPresentation::Overlay,
        }
    }

    pub fn with_presentation(mut self, presentation: TaskDetailPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn on_save<F>(&mut self, callback: F)
    where
        F: Fn(SavePayload, &mut Context<Self>) + Send + Sync + 'static,
    {
        self.on_save = Some(Box::new(callback));
    }

    pub fn on_delete<F>(&mut self, callback: F)
    where
        F: Fn(String, &mut Context<Self>) + Send + Sync + 'static,
    {
        self.on_delete = Some(Box::new(callback));
    }

    pub fn on_reopen<F>(&mut self, callback: F)
    where
        F: Fn(String, &mut Context<Self>) + Send + Sync + 'static,
    {
        self.on_reopen = Some(Box::new(callback));
    }

    pub fn on_close<F>(&mut self, callback: F)
    where
        F: Fn(&mut Context<Self>) + Send + Sync + 'static,
    {
        self.on_close = Some(Box::new(callback));
    }

    /// 显示任务详情 - 只在 task_id 变化时才重建 UI 状态
    pub fn show_task(&mut self, task: &Record, window: &mut Window, cx: &mut Context<Self>) {
        let task_id = task.id.to_string();

        // 如果已经在显示同一个任务，只刷新展示数据，不重建输入和附件状态。
        if self.current_task_id.as_ref() == Some(&task_id) {
            self.refresh_task_view_state(task);
            cx.notify();
            return;
        }

        // 更新任务数据
        self.current_task_id = Some(task_id);
        self.refresh_task_view_state(task);
        self.editing_title = false;
        self.editing_content = false;
        self.attachments.clear();
        self.active_attachment_preview = None;
        self.attachments_loading = true;
        self.attachment_error = None;

        // 初始化或更新截止日期选择器状态
        let (init_date, init_time_str) = if let Some(due) = self.due_date {
            let local = due.with_timezone(&Local);
            (
                Some(local.naive_local().date()),
                local.format("%H:%M").to_string(),
            )
        } else {
            (None, String::new())
        };

        // 初始化或更新提醒时间选择器状态
        let (reminder_init_date, reminder_init_time_str) =
            if let Some(scheduled) = self.scheduled_for {
                let local = scheduled.with_timezone(&Local);
                (
                    Some(local.naive_local().date()),
                    local.format("%H:%M").to_string(),
                )
            } else {
                (None, String::new())
            };

        // 处理截止日期选择器：如果存在则更新，不存在则创建
        // 重要：如果任务没有截止日期，需要重置选择器状态
        match (&self.date_picker, &self.time_input, init_date) {
            (Some(_), Some(_), None) => {
                // 之前有选择器但现在任务无日期，销毁并重建空的
                self.date_picker = None;
                self.time_input = None;
                self.init_picker_states_empty(window, cx);
            }
            (Some(dp), Some(ti), Some(date)) => {
                // 更新现有选择器
                dp.update(cx, |state, cx| {
                    state.set_date(date, window, cx);
                });
                ti.update(cx, |state, cx| {
                    state.set_value(&init_time_str, window, cx);
                });
            }
            (None, None, Some(date)) => {
                // 创建新的状态
                self.init_picker_states(date, &init_time_str, window, cx);
            }
            _ => {
                // 创建空的选择器状态
                self.init_picker_states_empty(window, cx);
            }
        }

        // 处理提醒时间选择器
        match (
            &self.reminder_date_picker,
            &self.reminder_time_input,
            reminder_init_date,
        ) {
            (Some(_), Some(_), None) => {
                // 之前有选择器但现在任务无提醒时间，销毁并重建空的
                self.reminder_date_picker = None;
                self.reminder_time_input = None;
                self.init_reminder_picker_states_empty(window, cx);
            }
            (Some(rdp), Some(rti), Some(date)) => {
                // 更新现有选择器
                rdp.update(cx, |state, cx| {
                    state.set_date(date, window, cx);
                });
                rti.update(cx, |state, cx| {
                    state.set_value(&reminder_init_time_str, window, cx);
                });
            }
            (None, None, Some(date)) => {
                // 创建新的提醒时间选择器状态
                self.init_reminder_picker_states(date, &reminder_init_time_str, window, cx);
            }
            _ => {
                // 创建空的提醒时间选择器状态
                self.init_reminder_picker_states_empty(window, cx);
            }
        }

        // 初始化或更新标题输入框
        let title_value = task.title.clone().unwrap_or_default();
        if let Some(ref input) = self.title_input {
            input.update(cx, |state, cx| {
                state.set_value(&title_value, window, cx);
            });
        } else {
            let title_input = cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_value(&title_value, window, cx);
                input
            });
            let subscription = cx.subscribe_in(
                &title_input,
                window,
                |this, _state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change | InputEvent::Focus => {
                        this.sync_inline_metadata_from_inputs(cx);
                        this.sync_title_metadata_autocomplete(cx);
                    }
                    InputEvent::Blur => {
                        this.clear_title_metadata_autocomplete(cx);
                        this.cancel_title_edit(window, cx);
                    }
                    InputEvent::PressEnter { .. } => {}
                },
            );
            self.title_input = Some(title_input);
            self.title_input_subscription = Some(subscription);
        }

        // 初始化或更新内容输入框（多行文本区域）
        let content_value = task.content.clone();
        if let Some(ref input) = self.content_input {
            input.update(cx, |state, cx| {
                state.set_value(&content_value, window, cx);
            });
        } else {
            let content_input = cx.new(|cx| {
                let mut input = InputState::new(window, cx).multi_line(true).auto_grow(1, 6);
                input.set_value(&content_value, window, cx);
                input
            });
            let subscription = cx.subscribe_in(
                &content_input,
                window,
                |this, _state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change | InputEvent::Focus => {
                        this.sync_inline_metadata_from_inputs(cx);
                        this.sync_content_metadata_autocomplete(cx);
                    }
                    InputEvent::Blur => {
                        this.clear_content_metadata_autocomplete(cx);
                        this.cancel_content_edit(window, cx);
                    }
                    InputEvent::PressEnter { .. } => {}
                },
            );
            self.content_input = Some(content_input);
            self.content_input_subscription = Some(subscription);
        }

        self.reload_attachments(cx);
        self.title_metadata_autocomplete.clear();
        self.content_metadata_autocomplete.clear();
        self.load_metadata_catalog(cx);
        cx.notify();
    }

    fn refresh_task_view_state(&mut self, task: &Record) {
        self.task_title = task.title.clone();
        self.task_content = task.content.clone();
        self.priority = task.priority.clone();
        self.status = task.status.clone();
        self.due_date = task.due_date;
        self.scheduled_for = task.scheduled_for;
        self.cancel_reason = task.cancelled_reason.clone();
        let inline_fields = parsing::parse_record_fields(task.title.as_deref(), &task.content);
        self.tags = parsing::merge_inline_metadata(&task.tags, &inline_fields.tags);
        self.persons = parsing::merge_inline_metadata(&task.persons, &inline_fields.people);
        self.line = inline_fields.line.clone();
        self.inline_tags = inline_fields.tags;
        self.inline_persons = inline_fields.people;
    }

    fn sync_inline_metadata_from_inputs(&mut self, cx: &mut Context<Self>) {
        let raw_title = self
            .title_input
            .as_ref()
            .map(|input| {
                let value = input.read(cx).value().to_string();
                if value.trim().is_empty() {
                    None
                } else {
                    Some(value)
                }
            })
            .unwrap_or_else(|| self.task_title.clone());
        let raw_content = self
            .content_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_else(|| self.task_content.clone());
        let parsed_fields = parsing::parse_record_fields(raw_title.as_deref(), &raw_content);
        self.tags = parsing::reconcile_metadata(&self.tags, &self.inline_tags, &parsed_fields.tags);
        self.persons =
            parsing::reconcile_metadata(&self.persons, &self.inline_persons, &parsed_fields.people);
        self.line = parsed_fields.line.clone();
        self.inline_tags = parsed_fields.tags;
        self.inline_persons = parsed_fields.people;
    }

    /// 关闭侧边栏
    pub fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.current_task_id = None;
        self.line = None;
        self.attachments.clear();
        self.active_attachment_preview = None;
        self.attachments_loading = false;
        self.attachment_error = None;
        cx.notify();
    }

    /// 获取当前状态
    pub fn state(&self) -> SidebarState {
        if self.current_task_id.is_some() {
            SidebarState::Visible
        } else {
            SidebarState::Hidden
        }
    }

    /// 获取当前任务 ID
    pub fn current_task_id(&self) -> Option<&str> {
        self.current_task_id.as_deref()
    }

    fn init_picker_states(
        &mut self,
        init_date: NaiveDate,
        init_time_str: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let date_picker = cx.new(|cx| {
            let mut picker = DatePickerState::new(window, cx).date_format("%Y-%m-%d");
            picker.set_date(init_date, window, cx);
            picker
        });

        let time_str = init_time_str.to_string();
        let time_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(&time_str, window, cx);
            input
        });

        self.date_picker = Some(date_picker);
        self.time_input = Some(time_input);
    }

    fn init_picker_states_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let date_picker = cx.new(|cx| DatePickerState::new(window, cx).date_format("%Y-%m-%d"));

        let time_input = cx.new(|cx| InputState::new(window, cx));

        self.date_picker = Some(date_picker);
        self.time_input = Some(time_input);
    }

    fn init_reminder_picker_states(
        &mut self,
        init_date: NaiveDate,
        init_time_str: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reminder_date_picker = cx.new(|cx| {
            let mut picker = DatePickerState::new(window, cx).date_format("%Y-%m-%d");
            picker.set_date(init_date, window, cx);
            picker
        });

        let time_str = init_time_str.to_string();
        let reminder_time_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(&time_str, window, cx);
            input
        });

        self.reminder_date_picker = Some(reminder_date_picker);
        self.reminder_time_input = Some(reminder_time_input);
    }

    fn init_reminder_picker_states_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reminder_date_picker =
            cx.new(|cx| DatePickerState::new(window, cx).date_format("%Y-%m-%d"));

        let reminder_time_input = cx.new(|cx| InputState::new(window, cx));

        self.reminder_date_picker = Some(reminder_date_picker);
        self.reminder_time_input = Some(reminder_time_input);
    }

    fn save_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(ref task_id), Some(ref priority), Some(ref status)) =
            (&self.current_task_id, &self.priority, &self.status)
        {
            let raw_title = self
                .title_input
                .as_ref()
                .map(|input| {
                    let val = input.read(cx).value().to_string();
                    if val.trim().is_empty() {
                        None
                    } else {
                        Some(val)
                    }
                })
                .unwrap_or_else(|| self.task_title.clone());

            let raw_content = self
                .content_input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_else(|| self.task_content.clone());
            let parsed_fields = parsing::parse_record_fields(raw_title.as_deref(), &raw_content);
            let next_tags =
                parsing::reconcile_metadata(&self.tags, &self.inline_tags, &parsed_fields.tags);
            let next_persons = parsing::reconcile_metadata(
                &self.persons,
                &self.inline_persons,
                &parsed_fields.people,
            );

            self.task_title = parsed_fields.title.clone();
            self.task_content = parsed_fields.content.clone();
            self.tags = next_tags.clone();
            self.persons = next_persons.clone();
            self.line = parsed_fields.line.clone();
            self.inline_tags = parsed_fields.tags.clone();
            self.inline_persons = parsed_fields.people.clone();
            self.editing_title = false;
            self.editing_content = false;

            let due_date = self.date_picker.as_ref().and_then(|dp| {
                let date_range = dp.read(cx).date();
                let start_date = date_range.start();
                start_date.and_then(|d| {
                    let time_str = self
                        .time_input
                        .as_ref()
                        .map(|ti| ti.read(cx).value().to_string())
                        .unwrap_or_else(|| "00:00".to_string());

                    self.parse_datetime(d, &time_str)
                })
            });

            let scheduled_for = self.reminder_date_picker.as_ref().and_then(|rdp| {
                let date_range = rdp.read(cx).date();
                let start_date = date_range.start();
                start_date.and_then(|d| {
                    let time_str = self
                        .reminder_time_input
                        .as_ref()
                        .map(|rti| rti.read(cx).value().to_string())
                        .unwrap_or_else(|| "00:00".to_string());

                    self.parse_datetime(d, &time_str)
                })
            });

            let payload = SavePayload {
                task_id: task_id.clone(),
                title: parsed_fields.title,
                content: parsed_fields.content,
                priority: priority.clone(),
                status: status.clone(),
                due_date,
                scheduled_for,
                cancel_reason: self.cancel_reason.clone(),
                tags: next_tags,
                persons: next_persons,
                line: parsed_fields.line,
            };

            if let Some(ref callback) = self.on_save {
                callback(payload, cx);
            }
        }
    }

    fn parse_datetime(&self, date: NaiveDate, time_str: &str) -> Option<DateTime<Utc>> {
        let time_parts: Vec<&str> = time_str.split(':').collect();
        let hour = time_parts
            .first()
            .and_then(|h| h.parse().ok())
            .unwrap_or(0u32);
        let minute = time_parts
            .get(1)
            .and_then(|m| m.parse().ok())
            .unwrap_or(0u32);

        date.and_hms_opt(hour, minute, 0)
            .and_then(|dt| Local.from_local_datetime(&dt).single())
            .map(|dt| dt.with_timezone(&Utc))
    }

    fn set_status(&mut self, status: TaskStatus, _window: &mut Window, cx: &mut Context<Self>) {
        self.status = Some(status);
        cx.notify();
    }

    fn set_priority(&mut self, priority: Priority, _window: &mut Window, cx: &mut Context<Self>) {
        self.priority = Some(priority);
        cx.notify();
    }

    /// 切换内容输入框的展开/收起状态
    fn toggle_content_expanded(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.content_expanded = !self.content_expanded;
        cx.notify();
    }

    fn begin_title_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_title = true;
        if let Some(ref input) = self.title_input {
            input.update(cx, |state, cx| {
                state.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn begin_content_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_content = true;
        if let Some(ref input) = self.content_input {
            input.update(cx, |state, cx| {
                state.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn cancel_title_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_title = false;
        if let Some(ref input) = self.title_input {
            let title_value = self.task_title.clone().unwrap_or_default();
            input.update(cx, |state, cx| {
                state.set_value(&title_value, window, cx);
            });
        }
        cx.notify();
    }

    fn cancel_content_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_content = false;
        if let Some(ref input) = self.content_input {
            let content_value = self.task_content.clone();
            input.update(cx, |state, cx| {
                state.set_value(&content_value, window, cx);
            });
        }
        cx.notify();
    }

    pub fn load_metadata_catalog(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            let tags = store.get_tag_catalog().await.unwrap_or_default();
            let persons = store.get_person_catalog().await.unwrap_or_default();
            let lines = store.get_line_catalog().await.unwrap_or_default();
            let _ = view.update(cx, |this, cx| {
                let catalog = MetadataCatalog {
                    tags,
                    persons,
                    lines,
                };
                this.title_metadata_autocomplete
                    .set_catalog(catalog.clone());
                this.content_metadata_autocomplete.set_catalog(catalog);
                this.sync_title_metadata_autocomplete(cx);
                this.sync_content_metadata_autocomplete(cx);
            });
        })
        .detach();
    }

    fn sync_title_metadata_autocomplete(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.title_input.as_ref() {
            self.title_metadata_autocomplete
                .sync_from_input(&input.read(cx));
        } else {
            self.title_metadata_autocomplete.clear();
        }
        cx.notify();
    }

    fn sync_content_metadata_autocomplete(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.content_input.as_ref() {
            self.content_metadata_autocomplete
                .sync_from_input(&input.read(cx));
        } else {
            self.content_metadata_autocomplete.clear();
        }
        cx.notify();
    }

    fn clear_title_metadata_autocomplete(&mut self, cx: &mut Context<Self>) {
        self.title_metadata_autocomplete.clear();
        cx.notify();
    }

    fn clear_content_metadata_autocomplete(&mut self, cx: &mut Context<Self>) {
        self.content_metadata_autocomplete.clear();
        cx.notify();
    }

    fn handle_metadata_keydown(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.title_metadata_autocomplete.is_open() {
            return self.handle_title_metadata_keydown(event, window, cx);
        }
        if self.content_metadata_autocomplete.is_open() {
            return self.handle_content_metadata_keydown(event, window, cx);
        }
        false
    }

    fn handle_title_metadata_keydown(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(input) = self.title_input.as_ref() else {
            return false;
        };
        let text = input.read(cx).text().to_string();
        match self
            .title_metadata_autocomplete
            .handle_key(event.keystroke.key.as_str(), &text)
        {
            MetadataAutocompleteAction::Ignored => false,
            MetadataAutocompleteAction::Moved | MetadataAutocompleteAction::Dismissed => {
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                true
            }
            MetadataAutocompleteAction::Applied(edit) => {
                window.prevent_default();
                cx.stop_propagation();
                apply_completion_to_input(input, &edit, window, cx);
                self.sync_title_metadata_autocomplete(cx);
                true
            }
        }
    }

    fn handle_metadata_action(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.title_metadata_autocomplete.is_open() {
            let Some(input) = self.title_input.as_ref() else {
                return false;
            };
            let text = input.read(cx).text().to_string();
            match self.title_metadata_autocomplete.handle_key(key, &text) {
                MetadataAutocompleteAction::Ignored => {}
                MetadataAutocompleteAction::Moved | MetadataAutocompleteAction::Dismissed => {
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                    return true;
                }
                MetadataAutocompleteAction::Applied(edit) => {
                    window.prevent_default();
                    cx.stop_propagation();
                    apply_completion_to_input(input, &edit, window, cx);
                    self.sync_title_metadata_autocomplete(cx);
                    return true;
                }
            }
        }

        if self.content_metadata_autocomplete.is_open() {
            let Some(input) = self.content_input.as_ref() else {
                return false;
            };
            let text = input.read(cx).text().to_string();
            match self.content_metadata_autocomplete.handle_key(key, &text) {
                MetadataAutocompleteAction::Ignored => false,
                MetadataAutocompleteAction::Moved | MetadataAutocompleteAction::Dismissed => {
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                    true
                }
                MetadataAutocompleteAction::Applied(edit) => {
                    window.prevent_default();
                    cx.stop_propagation();
                    apply_completion_to_input(input, &edit, window, cx);
                    self.sync_content_metadata_autocomplete(cx);
                    true
                }
            }
        } else {
            false
        }
    }

    fn handle_content_metadata_keydown(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(input) = self.content_input.as_ref() else {
            return false;
        };
        let text = input.read(cx).text().to_string();
        match self
            .content_metadata_autocomplete
            .handle_key(event.keystroke.key.as_str(), &text)
        {
            MetadataAutocompleteAction::Ignored => false,
            MetadataAutocompleteAction::Moved | MetadataAutocompleteAction::Dismissed => {
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                true
            }
            MetadataAutocompleteAction::Applied(edit) => {
                window.prevent_default();
                cx.stop_propagation();
                apply_completion_to_input(input, &edit, window, cx);
                self.sync_content_metadata_autocomplete(cx);
                true
            }
        }
    }

    fn apply_title_metadata_candidate(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.title_input.as_ref() else {
            return;
        };
        let text = input.read(cx).text().to_string();
        if let Some(edit) = self.title_metadata_autocomplete.apply_index(&text, index) {
            apply_completion_to_input(input, &edit, window, cx);
            self.sync_title_metadata_autocomplete(cx);
        }
    }

    fn apply_content_metadata_candidate(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.content_input.as_ref() else {
            return;
        };
        let text = input.read(cx).text().to_string();
        if let Some(edit) = self.content_metadata_autocomplete.apply_index(&text, index) {
            apply_completion_to_input(input, &edit, window, cx);
            self.sync_content_metadata_autocomplete(cx);
        }
    }

    fn render_title_metadata_autocomplete_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        render_autocomplete_menu(
            &self.title_metadata_autocomplete,
            "task-title-metadata-autocomplete",
            cx,
            |idx, candidate, selected| {
                autocomplete_item(
                    ("task-title-metadata-candidate", idx),
                    &candidate.name,
                    candidate.usage_count,
                    selected,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                        this.apply_title_metadata_candidate(idx, window, cx);
                        cx.stop_propagation();
                    }),
                )
                .into_any_element()
            },
        )
    }

    fn render_content_metadata_autocomplete_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        render_autocomplete_menu(
            &self.content_metadata_autocomplete,
            "task-content-metadata-autocomplete",
            cx,
            |idx, candidate, selected| {
                autocomplete_item(
                    ("task-content-metadata-candidate", idx),
                    &candidate.name,
                    candidate.usage_count,
                    selected,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                        this.apply_content_metadata_candidate(idx, window, cx);
                        cx.stop_propagation();
                    }),
                )
                .into_any_element()
            },
        )
    }

    const APPROX_CHARS_PER_LINE: usize = 45;

    fn estimate_line_count(content: &str) -> usize {
        if content.is_empty() {
            return 1;
        }

        let newline_count = content.matches('\n').count();

        // 估算基于字符宽度的额外行数
        // 中文字符按1.5个宽度计算，英文字母按1个宽度计算
        let total_width: usize = content
            .chars()
            .map(|c| if c.is_ascii() { 1 } else { 2 })
            .sum();

        let estimated_lines_from_width =
            (total_width + Self::APPROX_CHARS_PER_LINE - 1) / Self::APPROX_CHARS_PER_LINE;

        // 取换行符行数和字符宽度行数的较大值
        let estimated_lines = newline_count + estimated_lines_from_width;
        estimated_lines.max(1)
    }
    /// 清除截止日期
    fn clear_due_date(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.due_date = None;
        // 重新初始化空的日期选择器状态
        self.init_picker_states_empty(window, cx);
        cx.notify();
    }

    /// 清除提醒时间
    fn clear_reminder_time(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scheduled_for = None;
        // 重新初始化空的提醒时间选择器状态
        self.init_reminder_picker_states_empty(window, cx);
        cx.notify();
    }

    fn render_status_button(&self, status: TaskStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, color) = match status {
            TaskStatus::Todo => ("待办", ClaudeLikeColors::text_tertiary()),
            TaskStatus::InProgress => ("进行中", ClaudeLikeColors::accent()),
            TaskStatus::Done => ("已完成", rgb(0x5f8f63).into()),
            TaskStatus::Cancelled => ("已取消", ClaudeLikeColors::danger()),
        };

        let is_selected = self.status == Some(status.clone());

        div()
            .id(format!("sidebar-status-{:?}", status))
            .px(px(9.0))
            .py(px(4.0))
            .rounded(px(999.0))
            .border_1()
            .border_color(if is_selected {
                ClaudeLikeColors::stronger_separator()
            } else {
                ClaudeLikeColors::separator()
            })
            .bg(if is_selected {
                ClaudeLikeColors::selected_surface()
            } else {
                ClaudeLikeColors::app_background()
            })
            .text_size(px(TaskTypography::META_SIZE))
            .font_weight(TaskTypography::task_title_font_weight(is_selected))
            .text_color(if is_selected {
                ClaudeLikeColors::text_primary()
            } else {
                color
            })
            .cursor_pointer()
            .hover(|style| style.bg(ClaudeLikeColors::hover_surface()))
            .child(label)
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.set_status(status.clone(), window, cx);
                cx.stop_propagation();
            }))
    }

    fn render_priority_button(
        &self,
        priority: Priority,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (label, color) = match priority {
            Priority::High => ("高", ClaudeLikeColors::danger()),
            Priority::Medium => ("中", ClaudeLikeColors::accent()),
            Priority::Low => ("低", rgb(0x5f8f63).into()),
        };

        let is_selected = self.priority == Some(priority.clone());

        div()
            .id(format!("sidebar-priority-{:?}", priority))
            .px(px(9.0))
            .py(px(4.0))
            .rounded(px(999.0))
            .border_1()
            .border_color(if is_selected {
                ClaudeLikeColors::stronger_separator()
            } else {
                ClaudeLikeColors::separator()
            })
            .bg(if is_selected {
                ClaudeLikeColors::selected_surface()
            } else {
                ClaudeLikeColors::app_background()
            })
            .text_size(px(TaskTypography::META_SIZE))
            .font_weight(TaskTypography::task_title_font_weight(is_selected))
            .text_color(if is_selected {
                ClaudeLikeColors::text_primary()
            } else {
                color
            })
            .cursor_pointer()
            .hover(|style| style.bg(ClaudeLikeColors::hover_surface()))
            .child(label)
            .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.set_priority(priority.clone(), window, cx);
                cx.stop_propagation();
            }))
    }

    fn render_sidebar_metadata_chip(
        &self,
        kind: MetadataChipKind,
        label: &str,
    ) -> impl IntoElement {
        let text_color = match kind {
            MetadataChipKind::Line => rgb(0x047857).into(),
            MetadataChipKind::Tag => ClaudeLikeColors::text_secondary(),
            MetadataChipKind::Person => rgb(0x6f6254).into(),
        };

        div()
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(999.0))
            .border_1()
            .border_color(ClaudeLikeColors::separator())
            .bg(ClaudeLikeColors::task_paper_background())
            .text_size(px(TaskTypography::META_SIZE))
            .font_weight(FontWeight::NORMAL)
            .text_color(text_color)
            .child(label.to_string())
    }

    fn reload_attachments(&mut self, cx: &mut Context<Self>) {
        let Some(task_id) = self
            .current_task_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            self.attachments.clear();
            self.attachments_loading = false;
            self.attachment_error = None;
            cx.notify();
            return;
        };

        self.attachments_loading = true;
        self.attachment_error = None;
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            let result = async {
                let attachments = store.get_attachments(task_id).await?;
                let mut previews = Vec::with_capacity(attachments.len());
                for attachment in attachments {
                    let preview_image = match store.get_attachment_bytes(&attachment.id).await? {
                        Some(bytes) => preview_image_from_bytes(&attachment.mime_type, bytes),
                        None => None,
                    };
                    previews.push(AttachmentPreview {
                        attachment,
                        preview_image,
                    });
                }
                Ok::<_, String>(previews)
            }
            .await;

            let _ = view.update(cx, |this, cx| {
                this.attachments_loading = false;
                match result {
                    Ok(previews) => {
                        let should_poll = previews.iter().any(|preview| {
                            preview.attachment.status == AttachmentStatus::Processing
                        });
                        this.attachments = previews;
                        this.attachment_error = None;
                        if should_poll {
                            this.schedule_attachment_reload(cx);
                        }
                    }
                    Err(err) => {
                        this.attachments.clear();
                        this.attachment_error = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn schedule_attachment_reload(&mut self, cx: &mut Context<Self>) {
        let Some(current_task_id) = self.current_task_id.clone() else {
            return;
        };

        cx.spawn(async move |view, cx| {
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(800));
                let _ = tx.send_blocking(());
            });

            if rx.recv().await.is_err() {
                return;
            }

            let _ = view.update(cx, |this, cx| {
                if this.current_task_id.as_deref() == Some(current_task_id.as_str()) {
                    this.reload_attachments(cx);
                }
            });
        })
        .detach();
    }

    fn import_attachments(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(task_id) = self
            .current_task_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            return;
        };

        let picker = pick_image_files(ParentWindowHint::from_window(window));
        cx.spawn(async move |view, cx| {
            let Some(paths) = picker.await else {
                return;
            };
            let _ = view.update(cx, |this, cx| {
                this.import_attachment_paths(task_id, paths, cx);
            });
        })
        .detach();
    }

    fn import_attachment_paths(
        &mut self,
        task_id: Uuid,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.attachments_loading = true;
        self.attachment_error = None;
        cx.notify();

        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            let result = store.import_image_attachments(task_id, paths).await;
            let _ = view.update(cx, |this, cx| match result {
                Ok(_) => this.reload_attachments(cx),
                Err(err) => {
                    this.attachments_loading = false;
                    this.attachment_error = Some(err);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn paste_attachments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task_id) = self
            .current_task_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            return;
        };
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };
        if !crate::clipboard_attachment::clipboard_has_image_candidate(&clipboard) {
            return;
        }

        window.prevent_default();
        cx.stop_propagation();
        self.attachments_loading = true;
        self.attachment_error = None;
        cx.notify();

        cx.spawn(async move |view, cx| {
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let result =
                    crate::clipboard_attachment::extract_image_paths_from_clipboard(&clipboard);
                let _ = tx.send_blocking(result);
            });

            let result = rx
                .recv()
                .await
                .map_err(|err| format!("剪贴板图片处理任务失败: {}", err))
                .and_then(|result| result);

            let _ = view.update(cx, |this, cx| match result {
                Ok(paths) if !paths.is_empty() => this.import_attachment_paths(task_id, paths, cx),
                Ok(_) => {
                    this.attachments_loading = false;
                    this.attachment_error = None;
                    cx.notify();
                }
                Err(err) => {
                    this.attachments_loading = false;
                    this.attachment_error = Some(err);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn delete_attachment(&mut self, attachment_id: String, cx: &mut Context<Self>) {
        self.attachments_loading = true;
        self.attachment_error = None;
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            let result = store.delete_attachment(&attachment_id).await;
            let _ = view.update(cx, |this, cx| match result {
                Ok(_) => this.reload_attachments(cx),
                Err(err) => {
                    this.attachments_loading = false;
                    this.attachment_error = Some(err);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_attachment_card(
        &self,
        idx: usize,
        preview: &AttachmentPreview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let attachment_id = preview.attachment.id.clone();
        let lightbox_preview = preview.clone();
        let meta = format_attachment_meta(&preview.attachment);
        let (preview_width, preview_height) = attachment_preview_size(&preview.attachment);
        let preview_content = match preview.attachment.status {
            AttachmentStatus::Ready => preview
                .preview_image
                .clone()
                .map(|image| {
                    div()
                        .w_full()
                        .min_h(px(156.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(rgb(0xf5f5f5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                                this.open_attachment_preview(lightbox_preview.clone(), cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(img(image).w(preview_width).h(preview_height))
                        .into_any_element()
                })
                .unwrap_or_else(|| {
                    div()
                        .w_full()
                        .min_h(px(156.0))
                        .rounded(px(8.0))
                        .bg(rgb(0xf5f5f5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(rgb(0x999999))
                        .child("图片不可预览")
                        .into_any_element()
                }),
            AttachmentStatus::Processing => div()
                .w_full()
                .min_h(px(156.0))
                .rounded(px(8.0))
                .bg(rgb(0xf5f5f5))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(rgb(0x999999))
                .child("图片处理中…")
                .into_any_element(),
            AttachmentStatus::Failed => div()
                .w_full()
                .min_h(px(156.0))
                .rounded(px(8.0))
                .bg(rgb(0xfff2f0))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(rgb(0xff4d4f))
                .child(
                    preview
                        .attachment
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "图片不可用".to_string()),
                )
                .into_any_element(),
        };

        v_flex()
            .id(("task-attachment", idx))
            .gap(px(8.0))
            .p(px(8.0))
            .border_1()
            .border_color(rgb(0xf0f0f0))
            .rounded(px(10.0))
            .bg(rgb(0xfcfcfc))
            .child(preview_content)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().text_xs().text_color(rgb(0x999999)).child(meta))
                    .child(
                        Button::new(format!("task-attachment-delete-{}", idx))
                            .child("删除")
                            .text_color(rgb(0xff4d4f))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.delete_attachment(attachment_id.clone(), cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn open_attachment_preview(&mut self, preview: AttachmentPreview, cx: &mut Context<Self>) {
        self.active_attachment_preview = None;
        self.attachment_error = None;

        let attachment = preview.attachment;
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            let result = store.get_attachment_bytes(&attachment.id).await;
            let _ = view.update(cx, |this, cx| {
                match result {
                    Ok(file_data) => {
                        if let Err(err) = open_saved_attachment(&attachment, file_data) {
                            this.attachment_error = Some(err);
                        } else {
                            this.attachment_error = None;
                        }
                    }
                    Err(err) => {
                        this.attachment_error = Some(format!("读取预览图片失败：{}", err));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn close_attachment_preview(&mut self, cx: &mut Context<Self>) {
        self.active_attachment_preview = None;
        cx.notify();
    }

    fn render_attachment_lightbox(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.active_attachment_preview.as_ref()?;
        let image = preview.preview_image.clone()?;
        let meta = format_attachment_meta(&preview.attachment);
        let (lightbox_width, lightbox_height) = attachment_lightbox_size(&preview.attachment);

        Some(
            div()
                .id("task-attachment-lightbox")
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000061))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        this.close_attachment_preview(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    v_flex()
                        .w(px(960.0))
                        .max_w(relative(0.9))
                        .gap(px(12.0))
                        .p(px(16.0))
                        .rounded(px(14.0))
                        .bg(rgb(0xffffff))
                        .border_1()
                        .border_color(rgb(0xe8e8e8))
                        .shadow_lg()
                        .cursor_default()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _event: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(div().text_sm().text_color(rgb(0x666666)).child(meta))
                                .child(
                                    Button::new("task-attachment-lightbox-close")
                                        .child("关闭")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.close_attachment_preview(cx);
                                            cx.stop_propagation();
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_h(px(240.0))
                                .max_h(px(760.0))
                                .py(px(8.0))
                                .rounded(px(10.0))
                                .bg(rgb(0xf5f5f5))
                                .flex()
                                .items_center()
                                .justify_center()
                                .overflow_hidden()
                                .child(img(image).w(lightbox_width).h(lightbox_height)),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn preview_image_from_bytes(mime_type: &str, bytes: Vec<u8>) -> Option<Arc<Image>> {
    let format = ImageFormat::from_mime_type(mime_type)?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

fn format_attachment_meta(attachment: &Attachment) -> String {
    format!(
        "{} × {} · {}",
        attachment.width,
        attachment.height,
        format_file_size(attachment.file_size)
    )
}

fn attachment_preview_size(attachment: &Attachment) -> (Pixels, Pixels) {
    const MAX_WIDTH: f32 = 280.0;
    const MAX_HEIGHT: f32 = 180.0;

    let width = attachment.width.max(1) as f32;
    let height = attachment.height.max(1) as f32;
    let scale = (MAX_WIDTH / width).min(MAX_HEIGHT / height).min(1.0);

    (px(width * scale), px(height * scale))
}

fn attachment_lightbox_size(attachment: &Attachment) -> (Pixels, Pixels) {
    const MAX_WIDTH: f32 = 880.0;
    const MAX_HEIGHT: f32 = 720.0;

    let width = attachment.width.max(1) as f32;
    let height = attachment.height.max(1) as f32;
    let scale = (MAX_WIDTH / width).min(MAX_HEIGHT / height).min(1.0);

    (px(width * scale), px(height * scale))
}

fn format_file_size(file_size: usize) -> String {
    if file_size >= 1024 * 1024 {
        format!("{:.1} MB", file_size as f64 / (1024.0 * 1024.0))
    } else if file_size >= 1024 {
        format!("{:.1} KB", file_size as f64 / 1024.0)
    } else {
        format!("{} B", file_size)
    }
}

impl Render for TaskDetailSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_visible = self.current_task_id.is_some();
        if !is_visible {
            return div().into_any_element();
        }

        let dp_clone = self.date_picker.clone();
        let ti_clone = self.time_input.clone();
        let rdp_clone = self.reminder_date_picker.clone();
        let rti_clone = self.reminder_time_input.clone();
        let content_input_clone = self.content_input.clone();
        let content_expanded = self.content_expanded;
        let title_display = self.task_title.clone().unwrap_or_default();
        let content_display = self.task_content.clone();
        let is_overlay = self.presentation == TaskDetailPresentation::Overlay;

        div()
            .id("task-detail-sidebar")
            .when(is_overlay, |el| {
                el.absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
            })
            .when(!is_overlay, |el| {
                el.w(TASK_DETAIL_DOCKED_CONTAINER_WIDTH)
                    .h_full()
                    .border_l_1()
                    .border_color(ClaudeLikeColors::separator())
            })
            .flex()
            .flex_row()
            .justify_end()
            .cursor_default()
            .capture_action(cx.listener(|this, _action: &Paste, window, cx| {
                this.paste_attachments(window, cx);
            }))
            .when(is_overlay, |el| el.child(
                div()
                    .id("task-detail-sidebar-dismiss-area")
                    .flex_1()
                    .h_full(),
            ))
            .child(
                div()
                    .id("task-detail-sidebar-pane")
                    .w(TASK_DETAIL_DOCKED_WIDTH)
                    .h_full()
                    .flex()
                    .flex_col()
                    .occlude()
                    .overflow_hidden()
                    .when(is_overlay, |pane| {
                        pane.border_l_1()
                            .border_color(ClaudeLikeColors::separator())
                            .bg(ClaudeLikeColors::detail_background())
                    })
                    .when(!is_overlay, |pane| {
                        pane.rounded_tr(px(18.0))
                            .rounded_br(px(18.0))
                            .bg(ClaudeLikeColors::detail_background())
                    })
                    .font_family(TaskTypography::SYSTEM_FONT_FAMILY)
                    .cursor_default()
                    .capture_action(cx.listener(|this, _action: &MoveUp, window, cx| {
                        let _ = this.handle_metadata_action("up", window, cx);
                    }))
                    .capture_action(cx.listener(|this, _action: &MoveDown, window, cx| {
                        let _ = this.handle_metadata_action("down", window, cx);
                    }))
                    .capture_action(cx.listener(|this, _action: &IndentInline, window, cx| {
                        let _ = this.handle_metadata_action("tab", window, cx);
                    }))
                    .capture_action(cx.listener(|this, _action: &Escape, window, cx| {
                        let _ = this.handle_metadata_action("escape", window, cx);
                    }))
                    .on_click(cx.listener(|_this, _event: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        let _ = this.handle_metadata_keydown(event, window, cx);
                    }))
                    .child(
                        div()
                            .px(px(20.0))
                            .pt(px(20.0))
                            .pb(px(12.0))
                            .cursor_default()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(15.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(ClaudeLikeColors::text_primary())
                                            .child("任务详情"),
                                    )
                                    .child(
                                        div()
                                            .id("sidebar-close-detail")
                                            .w(px(28.0))
                                            .h(px(28.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(8.0))
                                            .text_size(px(18.0))
                                            .text_color(ClaudeLikeColors::text_secondary())
                                            .cursor_pointer()
                                            .hover(|style| style.bg(ClaudeLikeColors::hover_surface()))
                                            .child("×")
                                            .on_click(cx.listener(|this, _event: &ClickEvent, window, cx| {
                                                this.close(window, cx);
                                                if let Some(ref callback) = this.on_close {
                                                    callback(cx);
                                                }
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .cursor_default()
                            .child(
                                v_flex()
                                    .px(px(20.0))
                                    .py(px(16.0))
                                    .gap(px(14.0))
                                    .overflow_y_scrollbar()
                                    // 标题输入
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(ClaudeLikeColors::text_tertiary())
                                                    .child("标题"),
                                            )
                                            .child(if self.editing_title {
                                                self.title_input
                                                    .clone()
                                                    .map(|input| {
                                                        v_flex()
                                                            .gap(px(0.0))
                                                            .on_mouse_down(
                                                                gpui::MouseButton::Left,
                                                                cx.listener(
                                                                    |_this, _event, _window, cx| {
                                                                        cx.stop_propagation();
                                                                    },
                                                                ),
                                                            )
                                                            .child(
                                                                Input::new(&input)
                                                                    .appearance(false)
                                                                    .text_size(px(
                                                                        TaskTypography::DETAIL_TITLE_SIZE,
                                                                    ))
                                                                    .font_weight(
                                                                        gpui::FontWeight::MEDIUM,
                                                                    ),
                                                            )
                                                            .when(
                                                                self.title_metadata_autocomplete
                                                                    .is_open(),
                                                                |el| {
                                                                    el.child(
                                                                        self.render_title_metadata_autocomplete_menu(
                                                                            cx,
                                                                        ),
                                                                    )
                                                                },
                                                            )
                                                            .into_any_element()
                                                    })
                                                    .unwrap_or_else(|| div().into_any_element())
                                            } else {
                                                div()
                                                    .w_full()
                                                    .min_h(px(36.0))
                                                    .px(px(2.0))
                                                    .py(px(4.0))
                                                    .rounded(px(8.0))
                                                    .cursor_text()
                                                    .hover(|style| {
                                                        style.bg(ClaudeLikeColors::hover_surface())
                                                    })
                                                    .on_mouse_down(
                                                        gpui::MouseButton::Left,
                                                        cx.listener(
                                                        |this, _event: &MouseDownEvent, window, cx| {
                                                            this.begin_title_edit(window, cx);
                                                            cx.stop_propagation();
                                                        },
                                                        ),
                                                    )
                                                    .child(render_tokenized_text(
                                                        if title_display.is_empty() {
                                                            "\u{00a0}"
                                                        } else {
                                                            &title_display
                                                        },
                                                        TokenTextStyle::new(
                                                            rgb(0x242421),
                                                            gpui::FontWeight::MEDIUM,
                                                        ),
                                                    ))
                                                    .into_any_element()
                                            }),
                                    )
                                    // 内容输入（多行文本区域）
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(
                                                                ClaudeLikeColors::text_tertiary(),
                                                            )
                                                            .child("内容/详情"),
                                                    )
                                                    .when({
                                                        let content = if self.editing_content {
                                                            self.content_input
                                                                .as_ref()
                                                                .map(|input| input.read(cx).value().to_string())
                                                                .unwrap_or_else(|| content_display.clone())
                                                        } else {
                                                            content_display.clone()
                                                        };
                                                        Self::estimate_line_count(&content) > 6
                                                    }, |el| {
                                                        el.child(
                                                            Button::new("toggle-content-expand")
                                                                .child(if content_expanded { "收起" } else { "展开" })
                                                                .text_color(ClaudeLikeColors::accent())
                                                                .on_click(cx.listener(move |this, _event: &ClickEvent, window, cx| {
                                                                    this.toggle_content_expanded(window, cx);
                                                                    cx.stop_propagation();
                                                                })),
                                                        )
                                                    }),
                                            )
                                            .child(if self.editing_content {
                                                content_input_clone
                                                    .clone()
                                                    .map(|input| {
                                                        let content = input.read(cx).value();
                                                        let line_count = Self::estimate_line_count(&content);
                                                        let needs_scroll = line_count > 6 && !content_expanded;
                                                        let is_expanded = content_expanded;

                                                        v_flex()
                                                            .gap(px(0.0))
                                                            .on_mouse_down(
                                                                gpui::MouseButton::Left,
                                                                cx.listener(
                                                                    |_this, _event, _window, cx| {
                                                                        cx.stop_propagation();
                                                                    },
                                                                ),
                                                            )
                                                            .when(!needs_scroll && !is_expanded, |d| {
                                                                d.h_auto()
                                                            })
                                                            .when(needs_scroll, |d| {
                                                                d.h(px(144.0))
                                                            })
                                                            .when(is_expanded, |d| {
                                                                let total_height = ((line_count as f32) * 20.0 + 16.0).max(144.0);
                                                                d.h(px(total_height))
                                                            })
                                                            .child(
                                                                Input::new(&input)
                                                                    .appearance(false)
                                                                    .text_size(px(13.0))
                                                                    .when(needs_scroll || is_expanded, |i| i.h_full()),
                                                            )
                                                            .when(
                                                                self.content_metadata_autocomplete
                                                                    .is_open(),
                                                                |el| {
                                                                    el.child(
                                                                        self.render_content_metadata_autocomplete_menu(
                                                                            cx,
                                                                        ),
                                                                    )
                                                                },
                                                            )
                                                            .into_any_element()
                                                    })
                                                    .unwrap_or_else(|| div().into_any_element())
                                            } else {
                                                let line_count = Self::estimate_line_count(&content_display);
                                                let needs_scroll = line_count > 6 && !content_expanded;
                                                let is_expanded = content_expanded;
                                                let mut display = div()
                                                    .w_full()
                                                    .min_h(px(32.0))
                                                    .px(px(2.0))
                                                    .py(px(4.0))
                                                    .rounded(px(8.0))
                                                    .cursor_text()
                                                    .hover(|style| {
                                                        style.bg(ClaudeLikeColors::hover_surface())
                                                    })
                                                    .on_mouse_down(
                                                        gpui::MouseButton::Left,
                                                        cx.listener(
                                                        |this, _event: &MouseDownEvent, window, cx| {
                                                            this.begin_content_edit(window, cx);
                                                            cx.stop_propagation();
                                                        },
                                                        ),
                                                    );

                                                if !needs_scroll && !is_expanded {
                                                    display = display.h_auto();
                                                } else if needs_scroll {
                                                    display = display.h(px(144.0)).overflow_hidden();
                                                } else if is_expanded {
                                                    let total_height =
                                                        ((line_count as f32) * 20.0 + 16.0).max(144.0);
                                                    display = display.h(px(total_height));
                                                }

                                                display
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(
                                                                ClaudeLikeColors::text_secondary(),
                                                            )
                                                            .line_height(relative(1.55))
                                                            .child(render_tokenized_text(
                                                                if content_display.is_empty() {
                                                                    "\u{00a0}"
                                                                } else {
                                                                    &content_display
                                                                },
                                                                TokenTextStyle::new(
                                                                    rgb(0x555550),
                                                                    FontWeight::NORMAL,
                                                                ),
                                                            )),
                                                    )
                                                    .into_any_element()
                                            }),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .pt(px(12.0))
                                            .border_t_1()
                                            .border_color(ClaudeLikeColors::separator())
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(ClaudeLikeColors::text_tertiary())
                                                    .child("状态"),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap(px(6.0))
                                                    .child(
                                                        self.render_status_button(
                                                            TaskStatus::Todo,
                                                            cx,
                                                        ),
                                                    )
                                                    .child(self.render_status_button(
                                                        TaskStatus::InProgress,
                                                        cx,
                                                    ))
                                                    .child(
                                                        self.render_status_button(
                                                            TaskStatus::Done,
                                                            cx,
                                                        ),
                                                    )
                                                    .child(self.render_status_button(
                                                        TaskStatus::Cancelled,
                                                        cx,
                                                    )),
                                            ),
                                    )
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(ClaudeLikeColors::text_tertiary())
                                                    .child("优先级"),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap(px(6.0))
                                                    .child(
                                                        self.render_priority_button(
                                                            Priority::High,
                                                            cx,
                                                        ),
                                                    )
                                                    .child(self.render_priority_button(
                                                        Priority::Medium,
                                                        cx,
                                                    ))
                                                    .child(
                                                        self.render_priority_button(
                                                            Priority::Low,
                                                            cx,
                                                        ),
                                                    ),
                                            ),
                                    )
                                    // 截止日期：日期选择器 + 时间输入框 + 清除按钮
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .pt(px(12.0))
                                            .border_t_1()
                                            .border_color(ClaudeLikeColors::separator())
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(ClaudeLikeColors::text_tertiary())
                                                    .child("截止日期"),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap(px(5.0))
                                                    .items_center()
                                                    .when_some(dp_clone.clone(), |el, dp| {
                                                        el.child(
                                                            div()
                                                                .flex_1()
                                                                .h(px(28.0))
                                                                .px(px(8.0))
                                                                .rounded(px(999.0))
                                                                .border_1()
                                                                .border_color(
                                                                    ClaudeLikeColors::separator(),
                                                                )
                                                                .bg(ClaudeLikeColors::task_paper_background())
                                                                .child(
                                                                    DatePicker::new(&dp)
                                                                        .xsmall()
                                                                        .appearance(false)
                                                                        .placeholder("日期")
                                                                        .cleanable(false)
                                                                        .number_of_months(1),
                                                                ),
                                                        )
                                                    })
                                                    .when_some(ti_clone.clone(), |el, ti| {
                                                        el.child(
                                                            div()
                                                                .w(px(54.0))
                                                                .h(px(28.0))
                                                                .px(px(8.0))
                                                                .rounded(px(999.0))
                                                                .border_1()
                                                                .border_color(
                                                                    ClaudeLikeColors::separator(),
                                                                )
                                                                .bg(ClaudeLikeColors::task_paper_background())
                                                                .child(
                                                                    Input::new(&ti)
                                                                        .xsmall()
                                                                        .appearance(false)
                                                                        .focus_bordered(false)
                                                                        .text_size(px(12.0)),
                                                                ),
                                                        )
                                                    })
                                                    .child(
                                                        div()
                                                            .id("clear-due-date")
                                                            .px(px(5.0))
                                                            .py(px(4.0))
                                                            .rounded(px(7.0))
                                                            .text_size(px(12.0))
                                                            .child("清除")
                                                            .text_color(ClaudeLikeColors::text_tertiary())
                                                            .cursor_pointer()
                                                            .hover(|style| {
                                                                style.bg(ClaudeLikeColors::hover_surface())
                                                            })
                                                            .on_click(cx.listener(
                                                                |this, _event: &ClickEvent, window, cx| {
                                                                    this.clear_due_date(window, cx);
                                                                    cx.stop_propagation();
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    )
                                    // 提醒时间：日期选择器 + 时间输入框 + 清除按钮
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(ClaudeLikeColors::text_tertiary())
                                                    .child("提醒时间"),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap(px(5.0))
                                                    .items_center()
                                                    .when_some(rdp_clone.clone(), |el, rdp| {
                                                        el.child(
                                                            div()
                                                                .flex_1()
                                                                .h(px(28.0))
                                                                .px(px(8.0))
                                                                .rounded(px(999.0))
                                                                .border_1()
                                                                .border_color(
                                                                    ClaudeLikeColors::separator(),
                                                                )
                                                                .bg(ClaudeLikeColors::task_paper_background())
                                                                .child(
                                                                    DatePicker::new(&rdp)
                                                                        .xsmall()
                                                                        .appearance(false)
                                                                        .placeholder("日期")
                                                                        .cleanable(false)
                                                                        .number_of_months(1),
                                                                ),
                                                        )
                                                    })
                                                    .when_some(rti_clone.clone(), |el, rti| {
                                                        el.child(
                                                            div()
                                                                .w(px(54.0))
                                                                .h(px(28.0))
                                                                .px(px(8.0))
                                                                .rounded(px(999.0))
                                                                .border_1()
                                                                .border_color(
                                                                    ClaudeLikeColors::separator(),
                                                                )
                                                                .bg(ClaudeLikeColors::task_paper_background())
                                                                .child(
                                                                    Input::new(&rti)
                                                                        .xsmall()
                                                                        .appearance(false)
                                                                        .focus_bordered(false)
                                                                        .text_size(px(12.0)),
                                                                ),
                                                        )
                                                    })
                                                    .child(
                                                        div()
                                                            .id("clear-reminder-time")
                                                            .px(px(5.0))
                                                            .py(px(4.0))
                                                            .rounded(px(7.0))
                                                            .text_size(px(12.0))
                                                            .child("清除")
                                                            .text_color(ClaudeLikeColors::text_tertiary())
                                                            .cursor_pointer()
                                                            .hover(|style| {
                                                                style.bg(ClaudeLikeColors::hover_surface())
                                                            })
                                                            .on_click(cx.listener(
                                                                |this, _event: &ClickEvent, window, cx| {
                                                                    this.clear_reminder_time(
                                                                        window, cx,
                                                                    );
                                                                    cx.stop_propagation();
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .when(self.status == Some(TaskStatus::Cancelled), |el| {
                                        el.child(
                                            v_flex()
                                                .gap(px(6.0))
                                                .pt(px(12.0))
                                                .border_t_1()
                                                .border_color(ClaudeLikeColors::separator())
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(ClaudeLikeColors::text_tertiary())
                                                        .child("取消原因"),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0x999999))
                                                        .child("（此处可添加原因输入框）"),
                                                ),
                                        )
                                    })
                                    .when_some(self.line.as_ref(), |el, line| {
                                        el.child(
                                            v_flex()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(ClaudeLikeColors::text_tertiary())
                                                        .child("相关时间线"),
                                                )
                                                .child(
                                                    h_flex().gap(px(6.0)).flex_wrap().child(
                                                        self.render_sidebar_metadata_chip(
                                                            MetadataChipKind::Line,
                                                            &format_line_ref(line),
                                                        ),
                                                    ),
                                                ),
                                        )
                                    })
                                    .when(!self.tags.is_empty(), |el| {
                                        el.child(
                                            v_flex()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(ClaudeLikeColors::text_tertiary())
                                                        .child("标签"),
                                                )
                                                .child(h_flex().gap(px(6.0)).flex_wrap().children(
                                                    self.tags.iter().enumerate().map(
                                                        |(idx, tag)| {
                                                            div()
                                                                .id(("sidebar-tag", idx))
                                                                .child(self.render_sidebar_metadata_chip(
                                                                    MetadataChipKind::Tag,
                                                                    tag,
                                                                ))
                                                        },
                                                    ),
                                                )),
                                        )
                                    })
                                    .when(!self.persons.is_empty(), |el| {
                                        el.child(
                                            v_flex()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(ClaudeLikeColors::text_tertiary())
                                                        .child("相关人物"),
                                                )
                                                .child(h_flex().gap(px(6.0)).flex_wrap().children(
                                                    self.persons.iter().enumerate().map(
                                                        |(idx, person)| {
                                                            div()
                                                                .id(("sidebar-person", idx))
                                                                .child(self.render_sidebar_metadata_chip(
                                                                    MetadataChipKind::Person,
                                                                    person,
                                                                ))
                                                        },
                                                    ),
                                                )),
                                        )
                                    })
                                    .child(
                                        v_flex()
                                            .gap(px(8.0))
                                            .pt(px(12.0))
                                            .border_t_1()
                                            .border_color(ClaudeLikeColors::separator())
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(
                                                                ClaudeLikeColors::text_tertiary(),
                                                            )
                                                            .child("附件"),
                                                    )
                                                    .child(
                                                        div()
                                                            .id("task-sidebar-add-attachment")
                                                            .px(px(8.0))
                                                            .py(px(4.0))
                                                            .rounded(px(999.0))
                                                            .border_1()
                                                            .border_color(ClaudeLikeColors::separator())
                                                            .bg(ClaudeLikeColors::task_paper_background())
                                                            .text_size(px(TaskTypography::META_SIZE))
                                                            .font_weight(FontWeight::NORMAL)
                                                            .text_color(ClaudeLikeColors::text_secondary())
                                                            .cursor_pointer()
                                                            .hover(|style| {
                                                                style.bg(ClaudeLikeColors::hover_surface())
                                                            })
                                                            .child("添加图片")
                                                            .on_click(cx.listener(
                                                                |this, _event: &ClickEvent, window, cx| {
                                                                    this.import_attachments(window, cx);
                                                                    cx.stop_propagation();
                                                                },
                                                            )),
                                                    ),
                                            )
                                            .when(self.attachments_loading, |el| {
                                                el.child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(0x999999))
                                                        .child("正在加载图片…"),
                                                )
                                            })
                                            .when_some(self.attachment_error.clone(), |el, err| {
                                                el.child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(0xff4d4f))
                                                        .child(err),
                                                )
                                            })
                                            .when(
                                                !self.attachments_loading
                                                    && self.attachment_error.is_none()
                                                    && self.attachments.is_empty(),
                                                |el| {
                                                    el.child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(rgb(0x999999))
                                                            .child("暂无图片"),
                                                    )
                                                },
                                            )
                                            .children(self.attachments.iter().enumerate().map(
                                                |(idx, preview)| {
                                                    self.render_attachment_card(idx, preview, cx)
                                                },
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px(px(18.0))
                            .py(px(13.0))
                            .cursor_default()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .id("task-sidebar-delete-detail")
                                            .px(px(6.0))
                                            .py(px(5.0))
                                            .rounded(px(7.0))
                                            .text_size(px(TaskTypography::META_SIZE))
                                            .font_weight(FontWeight::NORMAL)
                                            .text_color(ClaudeLikeColors::danger())
                                            .cursor_pointer()
                                            .hover(|style| {
                                                style.bg(ClaudeLikeColors::hover_surface())
                                            })
                                            .child("删除")
                                            .on_click(cx.listener(
                                                |this, _event: &ClickEvent, _window, cx| {
                                                    if let Some(ref task_id) = this.current_task_id {
                                                        if let Some(ref callback) = this.on_delete {
                                                            callback(task_id.clone(), cx);
                                                        }
                                                    }
                                                },
                                            )),
                                    )
                                    .when(
                                        matches!(self.status, Some(TaskStatus::Done) | Some(TaskStatus::Cancelled)),
                                        |el| {
                                            el.child(
                                                div().flex_1().child(
                                                    Button::new("task-sidebar-reopen-detail")
                                                        .w_full()
                                                        .child("重新打开")
                                                        .on_click(cx.listener(
                                                            |this, _event, _window, cx| {
                                                                if let Some(ref task_id) =
                                                                    this.current_task_id
                                                                {
                                                                    if let Some(ref callback) =
                                                                        this.on_reopen
                                                                    {
                                                                        callback(task_id.clone(), cx);
                                                                    }
                                                                }
                                                            },
                                                        )),
                                                ),
                                            )
                                        },
                                    )
                                    .child(
                                        div()
                                            .id("sidebar-save-detail")
                                            .px(px(18.0))
                                            .py(px(7.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(999.0))
                                            .bg(ClaudeLikeColors::text_primary())
                                            .text_size(px(TaskTypography::META_SIZE))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ClaudeLikeColors::app_background())
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x3a3935)))
                                            .child("保存")
                                            .on_click(cx.listener(
                                                |this, _event: &ClickEvent, window, cx| {
                                                    this.save_changes(window, cx);
                                                },
                                            )),
                                    ),
                            ),
                    ),
            )
            .when_some(self.render_attachment_lightbox(cx), |el, overlay| {
                el.child(overlay)
            })
            .into_any_element()
    }
}

fn format_line_ref(line: &LineRef) -> String {
    match line.project.as_deref() {
        Some(project) => format!("~{project}/{}", line.name),
        None => format!("~{}", line.name),
    }
}
