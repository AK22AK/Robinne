use crate::models::{LineRef, Record};
use crate::store::{Store, StoreEvent};
use crate::ui::parsing;
use crate::ui::record_detail_sidebar::{RecordDetailSidebar, SavePayload};
use crate::ui::tokenized_text::{
    render_metadata_chip, render_tokenized_text, MetadataChipKind, TokenTextStyle,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::scroll::ScrollableElement;
use uuid::Uuid;

const NOTE_TITLE_LIMIT: usize = 24;
const NOTE_PREVIEW_LIMIT: usize = 44;
#[derive(Clone)]
struct PendingDeletion {
    id: Uuid,
    record_label: &'static str,
    display_title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NoteDisplayMetadata {
    tags: Vec<String>,
    persons: Vec<String>,
    line: Option<LineRef>,
}

pub struct NotePanel {
    store: Store,
    notes: Vec<Record>,
    focus_handle: FocusHandle,
    _window_activation_subscription: Subscription,
    pending_deletion: Option<PendingDeletion>,
    record_detail_sidebar: Entity<RecordDetailSidebar>,
}

impl NotePanel {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        let mut panel = Self {
            store: store.clone(),
            notes: Vec::new(),
            focus_handle,
            _window_activation_subscription: cx.observe_window_activation(
                window,
                |this, window, cx| {
                    if window.is_window_active() {
                        this.load_notes(cx);
                    }
                },
            ),
            pending_deletion: None,
            record_detail_sidebar: cx.new(|cx| RecordDetailSidebar::new(store.clone(), window, cx)),
        };

        let handle = cx.entity().clone();
        panel.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_save(move |payload, cx| {
                handle.update(cx, |panel, cx| {
                    panel.handle_sidebar_save(&payload, cx);
                });
            });
        });
        let handle = cx.entity().clone();
        panel.record_detail_sidebar.update(cx, |sidebar, _cx| {
            sidebar.on_delete(move |record_id, cx| {
                if let Ok(record_id) = Uuid::parse_str(&record_id) {
                    handle.update(cx, |panel, cx| {
                        panel.request_delete_note(record_id, cx);
                    });
                }
            });
        });

        panel.install_data_change_listener(cx);
        panel.refresh_data(cx);
        panel
    }

    pub fn refresh_data(&mut self, cx: &mut Context<Self>) {
        self.load_notes(cx);
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

    fn handle_sidebar_save(&mut self, payload: &SavePayload, cx: &mut Context<Self>) {
        if let Some(note) = self
            .notes
            .iter_mut()
            .find(|n| n.id.to_string() == payload.record_id)
        {
            note.title = payload.title.clone();
            note.content = payload.content.clone();
            note.tags = payload.tags.clone();
            note.persons = payload.persons.clone();
            note.updated_at = chrono::Utc::now();

            let updated_note = note.clone();
            let line_ref = payload.line.clone();
            let store = self.store.clone();
            let sidebar = self.record_detail_sidebar.clone();
            cx.spawn(async move |_view, cx| {
                if let Err(e) = store.update_record_with_line(updated_note, line_ref).await {
                    eprintln!("[NotePanel] Failed to update note: {}", e);
                } else {
                    let _ = sidebar.update(cx, |sidebar, cx| {
                        sidebar.load_metadata_catalog(cx);
                    });
                }
            })
            .detach();

            cx.notify();
        }
    }

    fn select_record(&mut self, record: &Record, window: &mut Window, cx: &mut Context<Self>) {
        self.record_detail_sidebar.update(cx, |sidebar, cx| {
            sidebar.show_record(record, window, cx);
        });
        cx.notify();
    }

    fn load_notes(&mut self, cx: &mut Context<Self>) {
        eprintln!("[NotePanel] load_notes called");
        let store = self.store.clone();
        cx.spawn(async move |view, cx| {
            let mut retries = 0;
            let notes = loop {
                eprintln!(
                    "[NotePanel] Fetching notes from store... (attempt {})",
                    retries + 1
                );
                match store.get_notes().await {
                    Ok(notes) => break notes,
                    Err(e) => {
                        eprintln!("[NotePanel] Failed to load notes: {}, retrying...", e);
                        retries += 1;
                        if retries >= 3 {
                            eprintln!("[NotePanel] Max retries reached, giving up");
                            break Vec::new();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            };

            eprintln!("[NotePanel] Loaded {} notes", notes.len());
            let update_result = view.update(cx, |panel, cx| {
                panel.notes = notes;
                cx.notify();
                eprintln!(
                    "[NotePanel] Notes updated and notified, panel now has {} notes",
                    panel.notes.len()
                );
            });
            if let Err(e) = update_result {
                eprintln!("[NotePanel] Failed to update view: {:?}", e);
            }
        })
        .detach();
    }

    fn request_delete_note(&mut self, note_id: Uuid, cx: &mut Context<Self>) {
        if let Some(note) = self.notes.iter().find(|n| n.id == note_id) {
            self.pending_deletion = Some(PendingDeletion {
                id: note_id,
                record_label: "记录",
                display_title: Self::get_note_display(note).0,
            });
            cx.notify();
        }
    }

    fn cancel_delete_confirmation(&mut self, cx: &mut Context<Self>) {
        self.pending_deletion = None;
        cx.notify();
    }

    fn confirm_delete_note(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_deletion.clone() else {
            return;
        };

        self.perform_delete_note(pending.id, true, cx);
    }

    fn perform_delete_note(
        &mut self,
        note_id: Uuid,
        clear_confirmation: bool,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let note_id_string = note_id.to_string();
        cx.spawn(
            async move |view, cx| match store.delete_record(note_id).await {
                Ok(_) => {
                    view.update(cx, |panel, cx| {
                        if clear_confirmation {
                            panel.pending_deletion = None;
                        }
                        if panel.record_detail_sidebar.read(cx).current_record_id()
                            == Some(note_id_string.as_str())
                        {
                            panel.record_detail_sidebar.update(cx, |sidebar, cx| {
                                sidebar.dismiss(cx);
                            });
                        }
                        panel.load_notes(cx);
                    })
                    .ok();
                }
                Err(e) => eprintln!("[NotePanel] Failed to delete note: {}", e),
            },
        )
        .detach();
    }

    // 从 Record 获取显示标题和预览
    // 优先使用 title 字段，如果没有则从 content 提取
    fn get_note_display(note: &Record) -> (String, String) {
        let title = note
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| {
                note.content
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_string())
                    .unwrap_or_else(|| "无标题".to_string())
            });

        let preview = note
            .content
            .lines()
            .skip(1)
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
            .unwrap_or_default();

        (
            Self::truncate_text(&title, NOTE_TITLE_LIMIT),
            Self::truncate_text(&preview, NOTE_PREVIEW_LIMIT),
        )
    }

    fn note_display_metadata(note: &Record) -> NoteDisplayMetadata {
        let inline_fields = parsing::parse_record_fields(note.title.as_deref(), &note.content);
        NoteDisplayMetadata {
            tags: parsing::merge_inline_metadata(&note.tags, &inline_fields.tags),
            persons: parsing::merge_inline_metadata(&note.persons, &inline_fields.people),
            line: inline_fields.line,
        }
    }

    fn normalize_text(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn truncate_text(text: &str, limit: usize) -> String {
        let normalized = Self::normalize_text(text);
        if normalized.chars().count() <= limit {
            normalized
        } else {
            format!("{}...", normalized.chars().take(limit).collect::<String>())
        }
    }

    fn render_delete_confirmation(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.pending_deletion.as_ref()?;
        let title = pending.display_title.clone();
        let record_label = pending.record_label;

        Some(
            div()
                .id("note-delete-confirm-overlay")
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
                                    Button::new("note-delete-confirm-cancel")
                                        .child("取消")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.cancel_delete_confirmation(cx);
                                        })),
                                )
                                .child(
                                    Button::new("note-delete-confirm-submit")
                                        .child("确认删除")
                                        .text_color(rgb(0xff4d4f))
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.confirm_delete_note(cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for NotePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_deletion.is_some() {
            self.focus_handle.focus(window, cx);
        }

        eprintln!("[NotePanel] Rendering {} notes", self.notes.len());

        let sidebar_task_id = self
            .record_detail_sidebar
            .read(cx)
            .current_record_id()
            .map(|s| s.to_string());

        div()
            .size_full()
            .flex()
            .flex_row()
            .relative()
            .track_focus(&self.focus_handle(cx))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.pending_deletion.is_none() {
                    return;
                }

                match event.keystroke.key.as_str() {
                    "enter" => {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.confirm_delete_note(cx);
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
                    .id("note-panel-main")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .p(px(16.0))
                    .on_click(cx.listener(|this, _event: &ClickEvent, window, cx| {
                        if this.record_detail_sidebar.read(cx).current_record_id().is_some() {
                            this.record_detail_sidebar.update(cx, |sidebar, cx| {
                                sidebar.close(window, cx);
                            });
                        }
                    }))
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("记录 ({})", self.notes.len()))
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("note-list")
                                    .size_full()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .pr(px(16.0))
                                    .overflow_y_scrollbar()
                                    .children(self.notes.clone().into_iter().enumerate().map(|(idx, note)| {
                                let note_id = note.id;
                                let is_selected = sidebar_task_id.as_ref() == Some(&note_id.to_string());
                                let (title, preview) = Self::get_note_display(&note);
                                let metadata = Self::note_display_metadata(&note);
                                let has_metadata = !metadata.tags.is_empty()
                                    || !metadata.persons.is_empty()
                                    || metadata.line.is_some();

                                div()
                                    .id(idx)
                                    .w_full()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .p(px(12.0))
                                    .rounded(px(6.0))
                                    .bg(if is_selected { rgb(0xe6f7ff) } else { rgb(0xffffff) })
                                    .border_1()
                                    .border_color(if is_selected { rgb(0x1890ff) } else { rgb(0xe8e8e8) })
                                    .text_color(rgb(0x000000))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(if is_selected { rgb(0xe6f7ff) } else { rgb(0xf6ffed) }))
                                    .on_click(cx.listener({
                                        let note = note.clone();
                                        move |this, _event: &ClickEvent, window, cx| {
                                            this.select_record(&note, window, cx);
                                            cx.stop_propagation();
                                        }
                                    }))
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w(px(0.0))
                                            .flex()
                                            .gap(px(8.0))
                                            .items_start()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .flex()
                                                    .flex_col()
                                                    .overflow_hidden()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .min_w(px(0.0))
                                                            .overflow_hidden()
                                                            .text_sm()
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(rgb(0x333333))
                                                            .child(render_tokenized_text(
                                                                &title,
                                                                TokenTextStyle::new(
                                                                    rgb(0x333333),
                                                                    FontWeight::MEDIUM,
                                                                ),
                                                            ))
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w(px(0.0))
                                                            .text_sm()
                                                            .text_color(rgb(0x888888))
                                                            .line_height(relative(1.35))
                                                            .child(render_tokenized_text(
                                                                if preview.is_empty() {
                                                                    "..."
                                                                } else {
                                                                    &preview
                                                                },
                                                                TokenTextStyle::new(
                                                                    rgb(0x888888),
                                                                    FontWeight::NORMAL,
                                                                ),
                                                            ))
                                                    )
                                                    .when(has_metadata, |el| {
                                                        el.child(
                                                            div()
                                                                .flex()
                                                                .min_w(px(0.0))
                                                                .gap(px(8.0))
                                                                .flex_wrap()
                                                                .text_xs()
                                                                .text_color(rgb(0xbbbbbb))
                                                                .when_some(metadata.line.as_ref(), |el, line| {
                                                                    el.child(
                                                                        div()
                                                                            .id("note-line")
                                                                            .child(render_metadata_chip(
                                                                                MetadataChipKind::Line,
                                                                                &format_line_ref(line),
                                                                            ))
                                                                    )
                                                                })
                                                                .children(metadata.tags.iter().enumerate().map(|(tag_idx, tag)| {
                                                                    div()
                                                                        .id(("note-tag", tag_idx))
                                                                        .child(render_metadata_chip(
                                                                            MetadataChipKind::Tag,
                                                                            tag,
                                                                        ))
                                                                }))
                                                                .children(metadata.persons.iter().enumerate().map(|(person_idx, person)| {
                                                                    div()
                                                                        .id(("note-person", person_idx))
                                                                        .child(render_metadata_chip(
                                                                            MetadataChipKind::Person,
                                                                            person,
                                                                        ))
                                                                }))
                                                        )
                                                    })
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_start()
                                                    .justify_end()
                                                    .child(
                                                        div()
                                                            .cursor_pointer()
                                                            .px(px(4.0))
                                                            .text_color(rgb(0x888888))
                                                            .hover(|style| style.text_color(rgb(0xff4d4f)))
                                                            .child("×")
                                                            .id(("delete", idx))
                                                            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                                                                this.request_delete_note(note_id, cx);
                                                                cx.stop_propagation();
                                                            }))
                                                    )
                                            )
                                    )
                                    .into_any_element()
                            }))
                        )
                    )
            )
            .child(self.record_detail_sidebar.clone())
            .children(self.render_delete_confirmation(cx))
            .into_any_element()
    }
}

impl Focusable for NotePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn format_line_ref(line: &LineRef) -> String {
    match line.project.as_deref() {
        Some(project) => format!("~{project}/{}", line.name),
        None => format!("~{}", line.name),
    }
}

#[cfg(test)]
mod tests {
    use super::NotePanel;
    use crate::models::Record;

    #[test]
    fn note_display_metadata_merges_inline_people_and_line() {
        let mut note = Record::new_note(
            "~微信冻屏问题关键进展 #关键进展 刚用 glm 写了个初稿，发给了@沈慧海 @宋亚南"
                .to_string(),
        );
        note.tags = vec!["关键进展".to_string()];
        note.persons = vec!["宋亚南".to_string()];

        let metadata = NotePanel::note_display_metadata(&note);

        assert_eq!(metadata.tags, vec!["关键进展"]);
        assert_eq!(metadata.persons, vec!["沈慧海", "宋亚南"]);
        let line = metadata.line.expect("expected inline transaction");
        assert_eq!(line.name, "微信冻屏问题关键进展");
    }
}
