use chrono::{DateTime, Utc};
use gpui::{prelude::*, *};
use gpui_component::{
    button::Button,
    h_flex,
    input::{Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, Paste},
    scroll::ScrollableElement,
    v_flex,
};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use crate::models::{Attachment, AttachmentStatus, LineRef, Record};
use crate::platform::{open_saved_attachment, pick_image_files, ParentWindowHint};
use crate::store::Store;
use crate::ui::metadata_autocomplete::{
    apply_completion_to_input, autocomplete_item, render_autocomplete_menu,
    MetadataAutocompleteAction, MetadataAutocompleteState, MetadataCatalog,
};
use crate::ui::parsing;
use crate::ui::tokenized_text::{
    render_metadata_chip, render_tokenized_text, MetadataChipKind, TokenTextStyle,
};
use std::time::Duration;

#[derive(Clone)]
struct AttachmentPreview {
    attachment: Attachment,
    preview_image: Option<Arc<Image>>,
}

pub struct RecordDetailSidebar {
    store: Store,
    current_record_id: Option<String>,
    record_title: Option<String>,
    record_content: String,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    line: Option<LineRef>,
    tags: Vec<String>,
    persons: Vec<String>,
    inline_tags: Vec<String>,
    inline_persons: Vec<String>,
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
    on_close: Option<Box<dyn Fn(&mut Context<Self>) + Send + Sync>>,
}

/// 保存时的数据载荷
#[derive(Debug, Clone)]
pub struct SavePayload {
    pub record_id: String,
    pub title: Option<String>,
    pub content: String,
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

impl RecordDetailSidebar {
    pub fn new(store: Store, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            store,
            current_record_id: None,
            record_title: None,
            record_content: String::new(),
            created_at: None,
            updated_at: None,
            line: None,
            tags: Vec::new(),
            persons: Vec::new(),
            inline_tags: Vec::new(),
            inline_persons: Vec::new(),
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
            on_close: None,
        }
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

    pub fn on_close<F>(&mut self, callback: F)
    where
        F: Fn(&mut Context<Self>) + Send + Sync + 'static,
    {
        self.on_close = Some(Box::new(callback));
    }

    /// 显示记录详情 - 只在 record_id 变化时才重建 UI 状态
    pub fn show_record(&mut self, record: &Record, window: &mut Window, cx: &mut Context<Self>) {
        let record_id = record.id.to_string();

        // 如果已经在显示同一个记录，只刷新展示数据，不重建输入和附件状态。
        if self.current_record_id.as_ref() == Some(&record_id) {
            self.refresh_record_view_state(record);
            cx.notify();
            return;
        }

        // 更新记录数据
        self.current_record_id = Some(record_id);
        self.record_title = record.title.clone();
        self.record_content = record.content.clone();
        self.created_at = Some(record.created_at);
        self.updated_at = Some(record.updated_at);
        self.refresh_record_view_state(record);
        self.editing_title = false;
        self.editing_content = false;
        self.attachments.clear();
        self.active_attachment_preview = None;
        self.attachments_loading = true;
        self.attachment_error = None;

        // 初始化或更新标题输入框
        let title_value = record.title.clone().unwrap_or_default();
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
        let content_value = record.content.clone();
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

    fn refresh_record_view_state(&mut self, record: &Record) {
        self.record_title = record.title.clone();
        self.record_content = record.content.clone();
        self.created_at = Some(record.created_at);
        self.updated_at = Some(record.updated_at);
        let inline_fields = parsing::parse_record_fields(record.title.as_deref(), &record.content);
        self.tags = parsing::merge_inline_metadata(&record.tags, &inline_fields.tags);
        self.persons = parsing::merge_inline_metadata(&record.persons, &inline_fields.people);
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
            .unwrap_or_else(|| self.record_title.clone());
        let raw_content = self
            .content_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_else(|| self.record_content.clone());
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
        self.current_record_id = None;
        self.line = None;
        self.attachments.clear();
        self.active_attachment_preview = None;
        self.attachments_loading = false;
        self.attachment_error = None;
        cx.notify();
    }

    /// 获取当前状态
    pub fn state(&self) -> SidebarState {
        if self.current_record_id.is_some() {
            SidebarState::Visible
        } else {
            SidebarState::Hidden
        }
    }

    /// 获取当前记录 ID
    pub fn current_record_id(&self) -> Option<&str> {
        self.current_record_id.as_deref()
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
            let title_value = self.record_title.clone().unwrap_or_default();
            input.update(cx, |state, cx| {
                state.set_value(&title_value, window, cx);
            });
        }
        cx.notify();
    }

    fn cancel_content_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_content = false;
        if let Some(ref input) = self.content_input {
            let content_value = self.record_content.clone();
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
            "record-title-metadata-autocomplete",
            cx,
            |idx, candidate, selected| {
                autocomplete_item(
                    ("record-title-metadata-candidate", idx),
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
            "record-content-metadata-autocomplete",
            cx,
            |idx, candidate, selected| {
                autocomplete_item(
                    ("record-content-metadata-candidate", idx),
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

        // Estimate additional lines based on character width
        // Chinese characters count as 2 units, ASCII characters count as 1 unit
        let total_width: usize = content
            .chars()
            .map(|c| if c.is_ascii() { 1 } else { 2 })
            .sum();

        let estimated_lines_from_width =
            (total_width + Self::APPROX_CHARS_PER_LINE - 1) / Self::APPROX_CHARS_PER_LINE;

        let estimated_lines = newline_count + estimated_lines_from_width;
        estimated_lines.max(1)
    }

    fn save_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ref record_id) = self.current_record_id {
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
                .unwrap_or_else(|| self.record_title.clone());

            let raw_content = self
                .content_input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_else(|| self.record_content.clone());
            let parsed_fields = parsing::parse_record_fields(raw_title.as_deref(), &raw_content);
            let next_tags =
                parsing::reconcile_metadata(&self.tags, &self.inline_tags, &parsed_fields.tags);
            let next_persons = parsing::reconcile_metadata(
                &self.persons,
                &self.inline_persons,
                &parsed_fields.people,
            );
            self.record_title = parsed_fields.title.clone();
            self.record_content = parsed_fields.content.clone();
            self.tags = next_tags.clone();
            self.persons = next_persons.clone();
            self.line = parsed_fields.line.clone();
            self.inline_tags = parsed_fields.tags.clone();
            self.inline_persons = parsed_fields.people.clone();
            self.editing_title = false;
            self.editing_content = false;

            let payload = SavePayload {
                record_id: record_id.clone(),
                title: parsed_fields.title,
                content: parsed_fields.content,
                tags: next_tags,
                persons: next_persons,
                line: parsed_fields.line,
            };

            if let Some(ref callback) = self.on_save {
                callback(payload, cx);
            }
        }
    }

    fn reload_attachments(&mut self, cx: &mut Context<Self>) {
        let Some(record_id) = self
            .current_record_id
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
                let attachments = store.get_attachments(record_id).await?;
                let mut previews = Vec::with_capacity(attachments.len());
                for attachment in attachments {
                    let preview_data_uri = match store.get_attachment_bytes(&attachment.id).await? {
                        Some(bytes) => preview_image_from_bytes(&attachment.mime_type, bytes),
                        None => None,
                    };
                    previews.push(AttachmentPreview {
                        attachment,
                        preview_image: preview_data_uri,
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
        let Some(current_record_id) = self.current_record_id.clone() else {
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
                if this.current_record_id.as_deref() == Some(current_record_id.as_str()) {
                    this.reload_attachments(cx);
                }
            });
        })
        .detach();
    }

    fn import_attachments(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(record_id) = self
            .current_record_id
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
                this.import_attachment_paths(record_id, paths, cx);
            });
        })
        .detach();
    }

    fn import_attachment_paths(
        &mut self,
        record_id: Uuid,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.attachments_loading = true;
        self.attachment_error = None;
        cx.notify();

        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            let result = store.import_image_attachments(record_id, paths).await;
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
        let Some(record_id) = self
            .current_record_id
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
                Ok(paths) if !paths.is_empty() => {
                    this.import_attachment_paths(record_id, paths, cx)
                }
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
            .id(("record-attachment", idx))
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
                        Button::new(format!("record-attachment-delete-{}", idx))
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
                .id("record-attachment-lightbox")
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
                                    Button::new("record-attachment-lightbox-close")
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

impl Render for RecordDetailSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_visible = self.current_record_id.is_some();
        if !is_visible {
            return div().into_any_element();
        }

        let content_input_clone = self.content_input.clone();
        let content_expanded = self.content_expanded;
        let title_display = self.record_title.clone().unwrap_or_default();
        let content_display = self.record_content.clone();

        div()
            .id("record-detail-sidebar")
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .flex()
            .flex_row()
            .justify_end()
            .cursor_default()
            .capture_action(cx.listener(|this, _action: &Paste, window, cx| {
                this.paste_attachments(window, cx);
            }))
            .child(
                div()
                    .id("record-detail-sidebar-dismiss-area")
                    .flex_1()
                    .h_full(),
            )
            .child(
                div()
                    .id("record-detail-sidebar-pane")
                    .w(px(360.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .occlude()
                    .overflow_hidden()
                    .border_l_1()
                    .border_color(rgb(0xe8e8e8))
                    .bg(rgb(0xffffff))
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
                            .p(px(12.0))
                            .border_b_1()
                            .border_color(rgb(0xe8e8e8))
                            .cursor_default()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child("记录详情"),
                                    )
                                    .child(
                                        Button::new("sidebar-close-detail").child("✕").on_click(
                                            cx.listener(|this, _event, window, cx| {
                                                this.close(window, cx);
                                                if let Some(ref callback) = this.on_close {
                                                    callback(cx);
                                                }
                                            }),
                                        ),
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
                                    .p(px(12.0))
                                    .gap(px(12.0))
                                    .overflow_y_scrollbar()
                                    // 标题输入
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x666666))
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
                                                                    .text_size(px(16.0))
                                                                    .font_weight(
                                                                        gpui::FontWeight::SEMIBOLD,
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
                                                    .hover(|style| style.bg(rgb(0xfafafa)))
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
                                                            rgb(0x262626),
                                                            gpui::FontWeight::SEMIBOLD,
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
                                                            .text_color(rgb(0x666666))
                                                            .child("内容"),
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
                                                                .text_color(rgb(0x1890ff))
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
                                                                    .text_size(px(14.0))
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
                                                    .hover(|style| style.bg(rgb(0xfafafa)))
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
                                                            .text_color(rgb(0x595959))
                                                            .line_height(relative(1.45))
                                                            .child(render_tokenized_text(
                                                                if content_display.is_empty() {
                                                                    "\u{00a0}"
                                                                } else {
                                                                    &content_display
                                                                },
                                                                TokenTextStyle::new(
                                                                    rgb(0x595959),
                                                                    FontWeight::NORMAL,
                                                                ),
                                                            )),
                                                    )
                                                    .into_any_element()
                                            }),
                                    )
                                    // 创建时间
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x666666))
                                                    .child("创建时间"),
                                            )
                                            .child(
                                                div().text_sm().text_color(rgb(0x999999)).child(
                                                    self.created_at
                                                        .map(|dt| {
                                                            dt.with_timezone(&chrono::Local)
                                                                .format("%Y-%m-%d %H:%M")
                                                                .to_string()
                                                        })
                                                        .unwrap_or_else(|| "-".to_string()),
                                                ),
                                            ),
                                    )
                                    // 更新时间
                                    .child(
                                        v_flex()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x666666))
                                                    .child("更新时间"),
                                            )
                                            .child(
                                                div().text_sm().text_color(rgb(0x999999)).child(
                                                    self.updated_at
                                                        .map(|dt| {
                                                            dt.with_timezone(&chrono::Local)
                                                                .format("%Y-%m-%d %H:%M")
                                                                .to_string()
                                                        })
                                                        .unwrap_or_else(|| "-".to_string()),
                                                ),
                                            ),
                                    )
                                    .when_some(self.line.as_ref(), |el, line| {
                                        el.child(
                                            v_flex()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0x666666))
                                                        .child("相关时间线"),
                                                )
                                                .child(
                                                    h_flex().gap(px(6.0)).flex_wrap().child(
                                                        render_metadata_chip(
                                                            MetadataChipKind::Line,
                                                            &format_line_ref(line),
                                                        ),
                                                    ),
                                                ),
                                        )
                                    })
                                    // 标签
                                    .when(!self.tags.is_empty(), |el| {
                                        el.child(
                                            v_flex()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0x666666))
                                                        .child("标签"),
                                                )
                                                .child(h_flex().gap(px(6.0)).flex_wrap().children(
                                                    self.tags.iter().enumerate().map(
                                                        |(idx, tag)| {
                                                            div()
                                                                .id(("record-sidebar-tag", idx))
                                                                .child(render_metadata_chip(
                                                                    MetadataChipKind::Tag,
                                                                    tag,
                                                                ))
                                                        },
                                                    ),
                                                )),
                                        )
                                    })
                                    // 相关人物
                                    .when(!self.persons.is_empty(), |el| {
                                        el.child(
                                            v_flex()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0x666666))
                                                        .child("相关人物"),
                                                )
                                                .child(h_flex().gap(px(6.0)).flex_wrap().children(
                                                    self.persons.iter().enumerate().map(
                                                        |(idx, person)| {
                                                            div()
                                                                .id(("record-sidebar-person", idx))
                                                                .child(render_metadata_chip(
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
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(0x666666))
                                                            .child("附件"),
                                                    )
                                                    .child(
                                                        Button::new("record-sidebar-add-attachment")
                                                            .child("添加图片")
                                                            .on_click(cx.listener(
                                                                |this, _event, window, cx| {
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
                            .p(px(12.0))
                            .border_t_1()
                            .border_color(rgb(0xe8e8e8))
                            .cursor_default()
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .child(
                                        div().flex_1().child(
                                            Button::new("record-sidebar-delete-detail")
                                                .w_full()
                                                .child("删除")
                                                .text_color(rgb(0xff4d4f))
                                                .on_click(cx.listener(
                                                    |this, _event, _window, cx| {
                                                        if let Some(ref record_id) =
                                                            this.current_record_id
                                                        {
                                                            if let Some(ref callback) =
                                                                this.on_delete
                                                            {
                                                                callback(record_id.clone(), cx);
                                                            }
                                                        }
                                                    },
                                                )),
                                        ),
                                    )
                                    .child(
                                        div().flex_1().child(
                                            Button::new("sidebar-save-detail")
                                                .w_full()
                                                .child("保存修改")
                                                .on_click(cx.listener(
                                                    |this, _event, window, cx| {
                                                        this.save_changes(window, cx);
                                                    },
                                                )),
                                        ),
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
