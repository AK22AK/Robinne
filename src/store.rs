use crate::attachment_image::{
    build_attachment_import_jobs, prepare_attachment_for_existing_id, prepare_image_attachments,
    PreparedImageAttachment,
};
use crate::data_management::{
    app_data_dir, apply_import_archive, export_archive, export_archive_to_bytes,
    preview_import_archive, AttachmentHealthSummary, AttachmentListItem, ConflictResolution,
    ExportResult, ImportMode, ImportPreview, ImportResult, StorageUsageSummary,
};
use crate::db::Database;
use crate::git_sync::{
    build_upload_payload, GitRemoteSyncClient, GitRemoteSyncConfig, GitRemoteSyncMetadata,
    GitRemoteSyncPullResult, GitRemoteSyncPushResult, GitRemoteSyncState, GitRemoteVerification,
};
use crate::models::{
    Attachment, Line, LineGraphData, LineRef, MetadataCatalogEntry, Person, Record, Tag,
    TaskStatus, TimelineQuery,
};
use crate::settings::{load_app_settings, save_app_settings};
use async_channel::{unbounded, Receiver, Sender};
use chrono::{DateTime, Duration, Local};
use std::path::PathBuf;
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub struct DashboardData {
    pub in_progress: Vec<Record>,
    pub today_tasks: Vec<Record>,
    pub due_today_count: usize,
    pub due_tomorrow_count: usize,
    pub overdue_count: usize,
    pub high_priority_open_count: usize,
    pub total_open_count: usize,
    pub total_in_progress: usize,
    pub completed_today_count: usize,
    pub recent_review_items: Vec<Record>,
    pub common_tags: Vec<String>,
    pub common_persons: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct StatsData {
    pub total_open_count: usize,
    pub total_in_progress: usize,
    pub completed_today_count: usize,
    pub overdue_count: usize,
    pub due_today_count: usize,
    pub due_tomorrow_count: usize,
    pub high_priority_open_count: usize,
    pub last_7_days_completed: Vec<DailyCompletedCount>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DailyCompletedCount {
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct GitRemoteSyncPullPreview {
    pub preview: ImportPreview,
    pub remote_commit: String,
    pub metadata: GitRemoteSyncMetadata,
}

#[derive(Clone)]
pub struct Store {
    sender: Sender<StoreCommand>,
    event_sender: broadcast::Sender<StoreEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreEvent {
    DataChanged,
}

pub type StoreEventReceiver = broadcast::Receiver<StoreEvent>;

pub enum StoreCommand {
    GetTasks {
        completed: bool,
        respond_to: Sender<Result<Vec<Record>, String>>,
    },
    GetNotes {
        respond_to: Sender<Result<Vec<Record>, String>>,
    },
    CreateRecord {
        record: Record,
        respond_to: Sender<Result<(), String>>,
    },
    CreateRecordWithLine {
        record: Record,
        line_ref: Option<LineRef>,
        respond_to: Sender<Result<(), String>>,
    },
    UpdateRecord {
        record: Record,
        respond_to: Sender<Result<(), String>>,
    },
    UpdateRecordWithLine {
        record: Record,
        line_ref: Option<LineRef>,
        respond_to: Sender<Result<(), String>>,
    },
    DeleteRecord {
        id: uuid::Uuid,
        respond_to: Sender<Result<(), String>>,
    },
    // 历史记录查询（保留给搜索/兼容入口）
    GetTimeline {
        query: TimelineQuery,
        respond_to: Sender<Result<Vec<Record>, String>>,
    },
    GetLineGraph {
        project: Option<String>,
        respond_to: Sender<Result<LineGraphData, String>>,
    },
    GetLineCatalog {
        respond_to: Sender<Result<Vec<MetadataCatalogEntry>, String>>,
    },
    CompleteLine {
        line_id: uuid::Uuid,
        respond_to: Sender<Result<(), String>>,
    },
    DeleteLine {
        line_id: uuid::Uuid,
        respond_to: Sender<Result<(), String>>,
    },
    GetOrCreateLine {
        line_ref: LineRef,
        respond_to: Sender<Result<Line, String>>,
    },
    // 全文搜索
    SearchRecords {
        query: String,
        respond_to: Sender<Result<Vec<Record>, String>>,
    },
    GetAllRecords {
        respond_to: Sender<Result<Vec<Record>, String>>,
    },
    GetRecordById {
        id: uuid::Uuid,
        respond_to: Sender<Result<Option<Record>, String>>,
    },
    GetRecordsByTag {
        tag: String,
        respond_to: Sender<Result<Vec<Record>, String>>,
    },
    GetRecordsByPerson {
        person: String,
        respond_to: Sender<Result<Vec<Record>, String>>,
    },
    // 看板数据
    GetDashboard {
        respond_to: Sender<Result<DashboardData, String>>,
    },
    GetStats {
        respond_to: Sender<Result<StatsData, String>>,
    },
    // 任务状态变更
    StartTask {
        id: uuid::Uuid,
        respond_to: Sender<Result<(), String>>,
    },
    CompleteTask {
        id: uuid::Uuid,
        respond_to: Sender<Result<(), String>>,
    },
    CancelTask {
        id: uuid::Uuid,
        reason: Option<String>,
        respond_to: Sender<Result<(), String>>,
    },
    ReopenTask {
        id: uuid::Uuid,
        respond_to: Sender<Result<(), String>>,
    },
    // 标签操作
    GetAllTags {
        respond_to: Sender<Result<Vec<Tag>, String>>,
    },
    GetTagCatalog {
        respond_to: Sender<Result<Vec<MetadataCatalogEntry>, String>>,
    },
    CreateTag {
        name: String,
        respond_to: Sender<Result<i64, String>>,
    },
    AddTagToRecord {
        record_id: uuid::Uuid,
        tag_id: i64,
        respond_to: Sender<Result<(), String>>,
    },
    // 人物操作
    GetAllPersons {
        respond_to: Sender<Result<Vec<Person>, String>>,
    },
    GetPersonCatalog {
        respond_to: Sender<Result<Vec<MetadataCatalogEntry>, String>>,
    },
    CreatePerson {
        name: String,
        respond_to: Sender<Result<i64, String>>,
    },
    AddPersonToRecord {
        record_id: uuid::Uuid,
        person_id: i64,
        respond_to: Sender<Result<(), String>>,
    },
    GetAttachments {
        record_id: uuid::Uuid,
        respond_to: Sender<Result<Vec<Attachment>, String>>,
    },
    ImportImageAttachments {
        prepared: Vec<PreparedImageAttachment>,
        respond_to: Sender<Result<Vec<Attachment>, String>>,
    },
    CreateAttachmentPlaceholders {
        attachments: Vec<Attachment>,
        respond_to: Sender<Result<(), String>>,
    },
    MarkAttachmentReady {
        prepared: PreparedImageAttachment,
    },
    MarkAttachmentFailed {
        attachment_id: String,
        error_message: String,
    },
    GetAttachmentBytes {
        attachment_id: String,
        respond_to: Sender<Result<Option<Vec<u8>>, String>>,
    },
    DeleteAttachment {
        attachment_id: String,
        respond_to: Sender<Result<(), String>>,
    },
    GetStorageUsageSummary {
        respond_to: Sender<Result<StorageUsageSummary, String>>,
    },
    GetAttachmentHealthSummary {
        respond_to: Sender<Result<AttachmentHealthSummary, String>>,
    },
    GetAllAttachments {
        respond_to: Sender<Result<Vec<AttachmentListItem>, String>>,
    },
    ExportData {
        destination: PathBuf,
        respond_to: Sender<Result<ExportResult, String>>,
    },
    PreviewImport {
        archive_path: PathBuf,
        respond_to: Sender<Result<ImportPreview, String>>,
    },
    ApplyImport {
        archive_path: PathBuf,
        mode: ImportMode,
        resolutions: Vec<ConflictResolution>,
        respond_to: Sender<Result<ImportResult, String>>,
    },
    GetGitRemoteSyncConfig {
        respond_to: Sender<Result<GitRemoteSyncState, String>>,
    },
    SaveGitRemoteSyncConfig {
        config: GitRemoteSyncConfig,
        respond_to: Sender<Result<GitRemoteSyncState, String>>,
    },
    VerifyGitRemote {
        config: GitRemoteSyncConfig,
        respond_to: Sender<Result<GitRemoteVerification, String>>,
    },
    PushSnapshotToGitRemote {
        config: GitRemoteSyncConfig,
        respond_to: Sender<Result<GitRemoteSyncPushResult, String>>,
    },
    PullSnapshotFromGitRemote {
        config: GitRemoteSyncConfig,
        respond_to: Sender<Result<GitRemoteSyncPullPreview, String>>,
    },
}

pub struct StoreRuntime {
    receiver: Receiver<StoreCommand>,
    event_sender: broadcast::Sender<StoreEvent>,
    db: Option<Database>,
    db_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
struct DerivedTaskStats {
    total_open_count: usize,
    total_in_progress: usize,
    completed_today_count: usize,
    overdue_count: usize,
    due_today_count: usize,
    due_tomorrow_count: usize,
    high_priority_open_count: usize,
    last_7_days_completed: Vec<DailyCompletedCount>,
}

fn is_open_task(record: &Record) -> bool {
    matches!(
        record.status,
        Some(TaskStatus::Todo) | Some(TaskStatus::InProgress)
    ) && record.completed_at.is_none()
}

fn derive_task_stats(tasks: &[Record], now: DateTime<Local>) -> DerivedTaskStats {
    let today = now.date_naive();
    let tomorrow = today + Duration::days(1);
    let mut last_7_days_completed = (0..7)
        .map(|offset| {
            let date = today - Duration::days((6 - offset) as i64);
            DailyCompletedCount {
                label: if date == today {
                    "今天".to_string()
                } else {
                    date.format("%m/%d").to_string()
                },
                count: 0,
            }
        })
        .collect::<Vec<_>>();

    let mut stats = DerivedTaskStats::default();

    for task in tasks {
        if is_open_task(task) {
            stats.total_open_count += 1;

            if task.status == Some(TaskStatus::InProgress) {
                stats.total_in_progress += 1;
            }

            if matches!(task.priority, Some(crate::models::Priority::High)) {
                stats.high_priority_open_count += 1;
            }

            if let Some(due_date) = task
                .due_date
                .map(|due| due.with_timezone(&Local).date_naive())
            {
                if due_date < today {
                    stats.overdue_count += 1;
                } else if due_date == today {
                    stats.due_today_count += 1;
                } else if due_date == tomorrow {
                    stats.due_tomorrow_count += 1;
                }
            }
        }

        if task.status == Some(TaskStatus::Done) {
            if let Some(completed_at) = task.completed_at {
                let completed_date = completed_at.with_timezone(&Local).date_naive();
                if completed_date == today {
                    stats.completed_today_count += 1;
                }

                let days_from_start =
                    completed_date.signed_duration_since(today - Duration::days(6));
                if (0..=6).contains(&days_from_start.num_days()) {
                    let idx = days_from_start.num_days() as usize;
                    if let Some(bucket) = last_7_days_completed.get_mut(idx) {
                        bucket.count += 1;
                    }
                }
            }
        }
    }

    stats.last_7_days_completed = last_7_days_completed;
    stats
}

impl StoreRuntime {
    pub fn new(
        receiver: Receiver<StoreCommand>,
        event_sender: broadcast::Sender<StoreEvent>,
    ) -> Self {
        Self {
            receiver,
            event_sender,
            db: None,
            db_path: None,
        }
    }

    fn publish_data_changed(&self) {
        let _ = self.event_sender.send(StoreEvent::DataChanged);
    }

    async fn send_result<T>(
        &self,
        respond_to: Sender<Result<T, String>>,
        result: Result<T, String>,
        publish_on_success: bool,
    ) {
        let should_publish = publish_on_success && result.is_ok();
        let _ = respond_to.send(result).await;
        if should_publish {
            self.publish_data_changed();
        }
    }

    fn normalize_record_for_persistence(
        &self,
        mut record: Record,
        previous_status: Option<TaskStatus>,
    ) -> Result<Record, String> {
        if record.record_type != crate::models::RecordType::Task {
            return Ok(record);
        }

        record.sync_task_lifecycle_fields(previous_status, chrono::Utc::now());
        Ok(record)
    }

    pub async fn run(&mut self, db_path: PathBuf) {
        // 初始化数据库连接
        match Database::new(&db_path) {
            Ok(db) => {
                self.db = Some(db);
                self.db_path = Some(db_path.clone());
                println!("[Store] Database initialized at {:?}", db_path);
            }
            Err(e) => {
                eprintln!("[Store] Failed to initialize database: {}", e);
                return; // 数据库初始化失败，退出 runtime
            }
        }

        // Start background reminder loop
        let db_path_clone = db_path.clone();
        if let Some(db_clone) = Database::new(&db_path_clone).ok() {
            eprintln!("[Store] Background reminder thread started");
            std::thread::spawn(move || loop {
                match db_clone.get_pending_reminders() {
                    Ok(records) => {
                        if !records.is_empty() {
                            eprintln!("[Store] Found {} pending reminders", records.len());
                        }
                        let notifications_enabled = load_app_settings()
                            .map(|settings| settings.reminders.notifications_enabled)
                            .unwrap_or(true);
                        for record in records {
                            eprintln!("[Store] Processing reminder for task: {}", record.id);
                            if !notifications_enabled {
                                continue;
                            }
                            match crate::platform::send_reminder(&record) {
                                Ok(_) => {
                                    eprintln!("[Store] Notification sent successfully");
                                    let now = chrono::Utc::now();
                                    if let Err(e) =
                                        db_clone.update_record_notified_at(record.id, now)
                                    {
                                        eprintln!("[Store] Failed to update notified_at: {}", e);
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[Store] Failed to send notification: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[Store] get_pending_reminders failed: {}", e);
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(10));
            });
        } else {
            eprintln!("[Store] Failed to open background DB connection");
        }

        // 处理命令循环
        while let Ok(cmd) = self.receiver.recv().await {
            match cmd {
                StoreCommand::GetTasks {
                    completed,
                    respond_to,
                } => {
                    let result = self.handle_get_tasks(completed).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetNotes { respond_to } => {
                    let result = self.handle_get_notes().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::CreateRecord { record, respond_to } => {
                    let result = self.handle_create_record(record).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::CreateRecordWithLine {
                    record,
                    line_ref,
                    respond_to,
                } => {
                    let result = self.handle_create_record_with_line(record, line_ref).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::UpdateRecord { record, respond_to } => {
                    let result = self.handle_update_record(record).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::UpdateRecordWithLine {
                    record,
                    line_ref,
                    respond_to,
                } => {
                    let result = self.handle_update_record_with_line(record, line_ref).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::DeleteRecord { id, respond_to } => {
                    let result = self.handle_delete_record(id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::GetTimeline { query, respond_to } => {
                    let result = self.handle_get_timeline(query).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetLineGraph {
                    project,
                    respond_to,
                } => {
                    let result = self.handle_get_line_graph(project).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetLineCatalog { respond_to } => {
                    let result = self.handle_get_line_catalog().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::CompleteLine {
                    line_id,
                    respond_to,
                } => {
                    let result = self.handle_complete_line(line_id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::DeleteLine {
                    line_id,
                    respond_to,
                } => {
                    let result = self.handle_delete_line(line_id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::GetOrCreateLine {
                    line_ref,
                    respond_to,
                } => {
                    let result = self.handle_get_or_create_line(line_ref).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::SearchRecords { query, respond_to } => {
                    let result = self.handle_search_records(query).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetAllRecords { respond_to } => {
                    let result = self.handle_get_all_records().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetRecordById { id, respond_to } => {
                    let result = self.handle_get_record_by_id(id).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetRecordsByTag { tag, respond_to } => {
                    let result = self.handle_get_records_by_tag(tag).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetRecordsByPerson { person, respond_to } => {
                    let result = self.handle_get_records_by_person(person).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetDashboard { respond_to } => {
                    let result = self.handle_get_dashboard().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetStats { respond_to } => {
                    let result = self.handle_get_stats().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::StartTask { id, respond_to } => {
                    let result = self.handle_start_task(id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::CompleteTask { id, respond_to } => {
                    let result = self.handle_complete_task(id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::CancelTask {
                    id,
                    reason,
                    respond_to,
                } => {
                    let result = self.handle_cancel_task(id, reason).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::ReopenTask { id, respond_to } => {
                    let result = self.handle_reopen_task(id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::GetAllTags { respond_to } => {
                    let result = self.handle_get_all_tags().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetTagCatalog { respond_to } => {
                    let result = self.handle_get_tag_catalog().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::CreateTag { name, respond_to } => {
                    let result = self.handle_create_tag(name).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::AddTagToRecord {
                    record_id,
                    tag_id,
                    respond_to,
                } => {
                    let result = self.handle_add_tag_to_record(record_id, tag_id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::GetAllPersons { respond_to } => {
                    let result = self.handle_get_all_persons().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetPersonCatalog { respond_to } => {
                    let result = self.handle_get_person_catalog().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::CreatePerson { name, respond_to } => {
                    let result = self.handle_create_person(name).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::AddPersonToRecord {
                    record_id,
                    person_id,
                    respond_to,
                } => {
                    let result = self.handle_add_person_to_record(record_id, person_id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::GetAttachments {
                    record_id,
                    respond_to,
                } => {
                    let result = self.handle_get_attachments(record_id).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::ImportImageAttachments {
                    prepared,
                    respond_to,
                } => {
                    let result = self.handle_import_image_attachments(prepared).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::CreateAttachmentPlaceholders {
                    attachments,
                    respond_to,
                } => {
                    let result = self
                        .handle_create_attachment_placeholders(attachments)
                        .await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::MarkAttachmentReady { prepared } => {
                    if let Err(err) = self.handle_mark_attachment_ready(prepared).await {
                        eprintln!("[Store] Failed to mark attachment ready: {}", err);
                    } else {
                        self.publish_data_changed();
                    }
                }
                StoreCommand::MarkAttachmentFailed {
                    attachment_id,
                    error_message,
                } => {
                    if let Err(err) = self
                        .handle_mark_attachment_failed(attachment_id, error_message)
                        .await
                    {
                        eprintln!("[Store] Failed to mark attachment failed: {}", err);
                    } else {
                        self.publish_data_changed();
                    }
                }
                StoreCommand::GetAttachmentBytes {
                    attachment_id,
                    respond_to,
                } => {
                    let result = self.handle_get_attachment_bytes(attachment_id).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::DeleteAttachment {
                    attachment_id,
                    respond_to,
                } => {
                    let result = self.handle_delete_attachment(attachment_id).await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::GetStorageUsageSummary { respond_to } => {
                    let result = self.handle_get_storage_usage_summary().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetAttachmentHealthSummary { respond_to } => {
                    let result = self.handle_get_attachment_health_summary().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::GetAllAttachments { respond_to } => {
                    let result = self.handle_get_all_attachments().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::ExportData {
                    destination,
                    respond_to,
                } => {
                    let result = self.handle_export_data(destination).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::PreviewImport {
                    archive_path,
                    respond_to,
                } => {
                    let result = self.handle_preview_import(archive_path).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::ApplyImport {
                    archive_path,
                    mode,
                    resolutions,
                    respond_to,
                } => {
                    let result = self
                        .handle_apply_import(archive_path, mode, resolutions)
                        .await;
                    self.send_result(respond_to, result, true).await;
                }
                StoreCommand::GetGitRemoteSyncConfig { respond_to } => {
                    let result = self.handle_get_git_remote_sync_config().await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::SaveGitRemoteSyncConfig { config, respond_to } => {
                    let result = self.handle_save_git_remote_sync_config(config).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::VerifyGitRemote { config, respond_to } => {
                    let result = self.handle_verify_git_remote(config).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::PushSnapshotToGitRemote { config, respond_to } => {
                    let result = self.handle_push_snapshot_to_git_remote(config).await;
                    let _ = respond_to.send(result).await;
                }
                StoreCommand::PullSnapshotFromGitRemote { config, respond_to } => {
                    let result = self.handle_pull_snapshot_from_git_remote(config).await;
                    let _ = respond_to.send(result).await;
                }
            }
        }
    }

    async fn handle_get_tasks(&self, _completed: bool) -> Result<Vec<Record>, String> {
        eprintln!("[Store] handle_get_tasks called");
        match &self.db {
            Some(db) => {
                eprintln!("[Store] Database exists, querying...");
                match db.get_tasks() {
                    Ok(tasks) => {
                        eprintln!("[Store] Found {} tasks", tasks.len());
                        Ok(tasks)
                    }
                    Err(e) => {
                        eprintln!("[Store] Query failed: {}", e);
                        Err(format!("Database error: {}", e))
                    }
                }
            }
            None => {
                eprintln!("[Store] Database not initialized!");
                Err("Database not initialized".to_string())
            }
        }
    }

    async fn handle_create_record(&self, record: Record) -> Result<(), String> {
        match &self.db {
            Some(db) => {
                let record = self.normalize_record_for_persistence(record, None)?;
                db.create_record(&record)
                    .map_err(|e| format!("Database error: {}", e))
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_create_record_with_line(
        &self,
        mut record: Record,
        line_ref: Option<LineRef>,
    ) -> Result<(), String> {
        match &self.db {
            Some(db) => {
                if let Some(line_ref) = line_ref {
                    let line = db
                        .get_or_create_line(&line_ref)
                        .map_err(|e| format!("Database error: {}", e))?;
                    record.line_id = Some(line.id);
                }
                let record = self.normalize_record_for_persistence(record, None)?;
                db.create_record(&record)
                    .map_err(|e| format!("Database error: {}", e))
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_update_record(&self, record: Record) -> Result<(), String> {
        match &self.db {
            Some(db) => {
                let previous_status = db
                    .get_record_by_id(record.id)
                    .ok()
                    .flatten()
                    .and_then(|r| r.status);
                let record = self.normalize_record_for_persistence(record, previous_status)?;
                db.create_record(&record)
                    .map_err(|e| format!("Database error: {}", e))
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_update_record_with_line(
        &self,
        mut record: Record,
        line_ref: Option<LineRef>,
    ) -> Result<(), String> {
        match &self.db {
            Some(db) => {
                record.line_id = match line_ref {
                    Some(line_ref) => Some(
                        db.get_or_create_line(&line_ref)
                            .map_err(|e| format!("Database error: {}", e))?
                            .id,
                    ),
                    None => record.line_id,
                };
                let previous_status = db
                    .get_record_by_id(record.id)
                    .ok()
                    .flatten()
                    .and_then(|r| r.status);
                let record = self.normalize_record_for_persistence(record, previous_status)?;
                db.create_record(&record)
                    .map_err(|e| format!("Database error: {}", e))
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_notes(&self) -> Result<Vec<Record>, String> {
        eprintln!("[Store] handle_get_notes called");
        match &self.db {
            Some(db) => {
                eprintln!("[Store] Database exists, querying notes...");
                match db.get_notes() {
                    Ok(notes) => {
                        eprintln!("[Store] Found {} notes", notes.len());
                        Ok(notes)
                    }
                    Err(e) => {
                        eprintln!("[Store] Query failed: {}", e);
                        Err(format!("Database error: {}", e))
                    }
                }
            }
            None => {
                eprintln!("[Store] Database not initialized!");
                Err("Database not initialized".to_string())
            }
        }
    }

    async fn handle_delete_record(&self, id: uuid::Uuid) -> Result<(), String> {
        eprintln!("[Store] handle_delete_record called for id: {}", id);
        match &self.db {
            Some(db) => db
                .delete_record(id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_timeline(&self, query: TimelineQuery) -> Result<Vec<Record>, String> {
        eprintln!(
            "[Store] handle_get_timeline called with limit={}, offset={}, tags={:?}, persons={:?}",
            query.limit, query.offset, query.tags, query.persons
        );
        match &self.db {
            Some(db) => match db.get_timeline(&query) {
                Ok(records) => {
                    eprintln!("[Store] Found {} timeline records", records.len());
                    Ok(records)
                }
                Err(e) => {
                    eprintln!("[Store] Timeline query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_line_graph(
        &self,
        project: Option<String>,
    ) -> Result<LineGraphData, String> {
        match &self.db {
            Some(db) => db
                .get_line_graph(project.as_deref())
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_line_catalog(&self) -> Result<Vec<MetadataCatalogEntry>, String> {
        match &self.db {
            Some(db) => db
                .get_line_catalog()
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_complete_line(&self, line_id: uuid::Uuid) -> Result<(), String> {
        match &self.db {
            Some(db) => db
                .complete_line(line_id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_delete_line(&self, line_id: uuid::Uuid) -> Result<(), String> {
        match &self.db {
            Some(db) => db
                .delete_line(line_id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_or_create_line(&self, line_ref: LineRef) -> Result<Line, String> {
        match &self.db {
            Some(db) => db
                .get_or_create_line(&line_ref)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_search_records(&self, query: String) -> Result<Vec<Record>, String> {
        eprintln!(
            "[Store] handle_search_records called with query='{}'",
            query
        );
        match &self.db {
            Some(db) => match db.search_records(&query) {
                Ok(records) => {
                    eprintln!("[Store] Found {} search results", records.len());
                    Ok(records)
                }
                Err(e) => {
                    eprintln!("[Store] Search query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_all_records(&self) -> Result<Vec<Record>, String> {
        eprintln!("[Store] handle_get_all_records called");
        match &self.db {
            Some(db) => match db.get_all_records() {
                Ok(records) => {
                    eprintln!("[Store] Found {} total records", records.len());
                    Ok(records)
                }
                Err(e) => {
                    eprintln!("[Store] All records query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_record_by_id(&self, id: uuid::Uuid) -> Result<Option<Record>, String> {
        match &self.db {
            Some(db) => db
                .get_record_by_id(id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_records_by_tag(&self, tag: String) -> Result<Vec<Record>, String> {
        eprintln!(
            "[Store] handle_get_records_by_tag called with tag='{}'",
            tag
        );
        match &self.db {
            Some(db) => match db.get_records_by_tag(&tag) {
                Ok(records) => {
                    eprintln!("[Store] Found {} tagged records", records.len());
                    Ok(records)
                }
                Err(e) => {
                    eprintln!("[Store] Tagged records query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_records_by_person(&self, person: String) -> Result<Vec<Record>, String> {
        eprintln!(
            "[Store] handle_get_records_by_person called with person='{}'",
            person
        );
        match &self.db {
            Some(db) => match db.get_records_by_person(&person) {
                Ok(records) => {
                    eprintln!("[Store] Found {} person-linked records", records.len());
                    Ok(records)
                }
                Err(e) => {
                    eprintln!("[Store] Person records query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_dashboard(&self) -> Result<DashboardData, String> {
        eprintln!("[Store] handle_get_dashboard called");
        match &self.db {
            Some(db) => {
                let tasks = match db.get_tasks() {
                    Ok(tasks) => tasks,
                    Err(e) => return Err(format!("Database error: {}", e)),
                };

                let in_progress = db
                    .get_dashboard_in_progress_tasks()
                    .map_err(|e| format!("Database error: {}", e))?;

                let today_tasks = db
                    .get_dashboard_pending_tasks()
                    .map_err(|e| format!("Database error: {}", e))?
                    .into_iter()
                    .filter(|task| task.status == Some(TaskStatus::Todo))
                    .take(5)
                    .collect::<Vec<_>>();

                let recent_review_items = db
                    .get_dashboard_recent_records(2)
                    .map_err(|e| format!("Database error: {}", e))?;

                let common_tags = db
                    .get_dashboard_common_tags(5)
                    .map_err(|e| format!("Database error: {}", e))?;

                let common_persons = db
                    .get_dashboard_common_persons(5)
                    .map_err(|e| format!("Database error: {}", e))?;

                let stats = derive_task_stats(&tasks, Local::now());

                Ok(DashboardData {
                    in_progress,
                    today_tasks,
                    due_today_count: stats.due_today_count,
                    due_tomorrow_count: stats.due_tomorrow_count,
                    overdue_count: stats.overdue_count,
                    high_priority_open_count: stats.high_priority_open_count,
                    total_open_count: stats.total_open_count,
                    total_in_progress: stats.total_in_progress,
                    completed_today_count: stats.completed_today_count,
                    recent_review_items,
                    common_tags,
                    common_persons,
                })
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_stats(&self) -> Result<StatsData, String> {
        match &self.db {
            Some(db) => {
                let tasks = db
                    .get_tasks()
                    .map_err(|e| format!("Database error: {}", e))?;
                let derived = derive_task_stats(&tasks, Local::now());
                Ok(StatsData {
                    total_open_count: derived.total_open_count,
                    total_in_progress: derived.total_in_progress,
                    completed_today_count: derived.completed_today_count,
                    overdue_count: derived.overdue_count,
                    due_today_count: derived.due_today_count,
                    due_tomorrow_count: derived.due_tomorrow_count,
                    high_priority_open_count: derived.high_priority_open_count,
                    last_7_days_completed: derived.last_7_days_completed,
                })
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_start_task(&self, id: uuid::Uuid) -> Result<(), String> {
        eprintln!("[Store] handle_start_task called for id: {}", id);
        match &self.db {
            Some(db) => {
                let mut record = match db.get_record_by_id(id) {
                    Ok(Some(record)) => record,
                    Ok(None) => return Err("Task not found".to_string()),
                    Err(e) => return Err(format!("Database error: {}", e)),
                };

                let previous_status = record.status.clone();
                record.status = Some(TaskStatus::InProgress);
                record.updated_at = chrono::Utc::now();
                record = self.normalize_record_for_persistence(record, previous_status)?;
                db.create_record(&record)
                    .map_err(|e| format!("Database error: {}", e))
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_complete_task(&self, id: uuid::Uuid) -> Result<(), String> {
        eprintln!("[Store] handle_complete_task called for id: {}", id);
        match &self.db {
            Some(db) => {
                let mut record = match db.get_record_by_id(id) {
                    Ok(Some(record)) => record,
                    Ok(None) => return Err("Task not found".to_string()),
                    Err(e) => return Err(format!("Database error: {}", e)),
                };

                let previous_status = record.status.clone();
                record.status = Some(TaskStatus::Done);
                record.updated_at = chrono::Utc::now();
                record = self.normalize_record_for_persistence(record, previous_status)?;
                db.create_record(&record)
                    .map_err(|e| format!("Database error: {}", e))
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_cancel_task(
        &self,
        id: uuid::Uuid,
        reason: Option<String>,
    ) -> Result<(), String> {
        eprintln!("[Store] handle_cancel_task called for id: {}", id);
        match &self.db {
            Some(db) => {
                let mut record = match db.get_record_by_id(id) {
                    Ok(Some(record)) => record,
                    Ok(None) => return Err("Task not found".to_string()),
                    Err(e) => return Err(format!("Database error: {}", e)),
                };

                let previous_status = record.status.clone();
                record.status = Some(TaskStatus::Cancelled);
                record.cancelled_reason = reason;
                record.updated_at = chrono::Utc::now();
                record = self.normalize_record_for_persistence(record, previous_status)?;
                db.create_record(&record)
                    .map_err(|e| format!("Database error: {}", e))
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_reopen_task(&self, id: uuid::Uuid) -> Result<(), String> {
        eprintln!("[Store] handle_reopen_task called for id: {}", id);
        match &self.db {
            Some(db) => {
                let mut record = match db.get_record_by_id(id) {
                    Ok(Some(record)) => record,
                    Ok(None) => return Err("Task not found".to_string()),
                    Err(e) => return Err(format!("Database error: {}", e)),
                };

                let previous_status = record.status.clone();
                record.status = Some(TaskStatus::Todo);
                record.completed_at = None;
                record.cancelled_reason = None;
                record.updated_at = chrono::Utc::now();
                record = self.normalize_record_for_persistence(record, previous_status)?;
                db.create_record(&record)
                    .map_err(|e| format!("Database error: {}", e))
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_all_tags(&self) -> Result<Vec<Tag>, String> {
        eprintln!("[Store] handle_get_all_tags called");
        match &self.db {
            Some(db) => match db.get_tags() {
                Ok(tags) => {
                    eprintln!("[Store] Found {} tags", tags.len());
                    Ok(tags)
                }
                Err(e) => {
                    eprintln!("[Store] Get tags query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_create_tag(&self, name: String) -> Result<i64, String> {
        eprintln!("[Store] handle_create_tag called with name='{}'", name);
        match &self.db {
            Some(db) => match db.create_tag(&name, None) {
                Ok(tag) => {
                    eprintln!("[Store] Created tag with id: {}", tag.id);
                    Ok(tag.id)
                }
                Err(e) => {
                    eprintln!("[Store] Create tag failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_tag_catalog(&self) -> Result<Vec<MetadataCatalogEntry>, String> {
        eprintln!("[Store] handle_get_tag_catalog called");
        match &self.db {
            Some(db) => match db.get_tag_catalog() {
                Ok(entries) => {
                    eprintln!("[Store] Found {} tag catalog entries", entries.len());
                    Ok(entries)
                }
                Err(e) => {
                    eprintln!("[Store] Get tag catalog query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_add_tag_to_record(
        &self,
        record_id: uuid::Uuid,
        tag_id: i64,
    ) -> Result<(), String> {
        eprintln!(
            "[Store] handle_add_tag_to_record called for record_id: {}, tag_id: {}",
            record_id, tag_id
        );
        match &self.db {
            Some(db) => db
                .add_tag_to_record(record_id, tag_id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_all_persons(&self) -> Result<Vec<Person>, String> {
        eprintln!("[Store] handle_get_all_persons called");
        match &self.db {
            Some(db) => match db.get_persons() {
                Ok(persons) => {
                    eprintln!("[Store] Found {} persons", persons.len());
                    Ok(persons)
                }
                Err(e) => {
                    eprintln!("[Store] Get persons query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_create_person(&self, name: String) -> Result<i64, String> {
        eprintln!("[Store] handle_create_person called with name='{}'", name);
        match &self.db {
            Some(db) => match db.create_person(&name) {
                Ok(person) => {
                    eprintln!("[Store] Created person with id: {}", person.id);
                    Ok(person.id)
                }
                Err(e) => {
                    eprintln!("[Store] Create person failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_person_catalog(&self) -> Result<Vec<MetadataCatalogEntry>, String> {
        eprintln!("[Store] handle_get_person_catalog called");
        match &self.db {
            Some(db) => match db.get_person_catalog() {
                Ok(entries) => {
                    eprintln!("[Store] Found {} person catalog entries", entries.len());
                    Ok(entries)
                }
                Err(e) => {
                    eprintln!("[Store] Get person catalog query failed: {}", e);
                    Err(format!("Database error: {}", e))
                }
            },
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_add_person_to_record(
        &self,
        record_id: uuid::Uuid,
        person_id: i64,
    ) -> Result<(), String> {
        eprintln!(
            "[Store] handle_add_person_to_record called for record_id: {}, person_id: {}",
            record_id, person_id
        );
        match &self.db {
            Some(db) => db
                .add_person_to_record(record_id, person_id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_attachments(
        &self,
        record_id: uuid::Uuid,
    ) -> Result<Vec<Attachment>, String> {
        match &self.db {
            Some(db) => db
                .get_attachments(record_id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_import_image_attachments(
        &self,
        prepared: Vec<PreparedImageAttachment>,
    ) -> Result<Vec<Attachment>, String> {
        match &self.db {
            Some(db) => {
                let mut attachments = Vec::with_capacity(prepared.len());
                for prepared_attachment in prepared {
                    db.create_attachment_with_data(
                        &prepared_attachment.attachment,
                        Some(&prepared_attachment.file_data),
                    )
                    .map_err(|e| format!("Database error: {}", e))?;
                    attachments.push(prepared_attachment.attachment);
                }
                Ok(attachments)
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_create_attachment_placeholders(
        &self,
        attachments: Vec<Attachment>,
    ) -> Result<(), String> {
        match &self.db {
            Some(db) => {
                for attachment in attachments {
                    db.create_attachment_placeholder(&attachment)
                        .map_err(|e| format!("Database error: {}", e))?;
                }
                Ok(())
            }
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_mark_attachment_ready(
        &self,
        prepared: PreparedImageAttachment,
    ) -> Result<(), String> {
        match &self.db {
            Some(db) => db
                .mark_attachment_ready(&prepared.attachment, &prepared.file_data)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_mark_attachment_failed(
        &self,
        attachment_id: String,
        error_message: String,
    ) -> Result<(), String> {
        match &self.db {
            Some(db) => db
                .mark_attachment_failed(&attachment_id, &error_message)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_attachment_bytes(
        &self,
        attachment_id: String,
    ) -> Result<Option<Vec<u8>>, String> {
        match &self.db {
            Some(db) => db
                .get_attachment_bytes(&attachment_id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_delete_attachment(&self, attachment_id: String) -> Result<(), String> {
        match &self.db {
            Some(db) => db
                .delete_attachment(&attachment_id)
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_storage_usage_summary(&self) -> Result<StorageUsageSummary, String> {
        match &self.db {
            Some(db) => db
                .get_storage_usage_summary()
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_attachment_health_summary(
        &self,
    ) -> Result<AttachmentHealthSummary, String> {
        match &self.db {
            Some(db) => db
                .get_attachment_health_summary()
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_all_attachments(&self) -> Result<Vec<AttachmentListItem>, String> {
        match &self.db {
            Some(db) => db
                .get_all_attachment_items()
                .map_err(|e| format!("Database error: {}", e)),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_export_data(&self, destination: PathBuf) -> Result<ExportResult, String> {
        match &self.db {
            Some(db) => export_archive(db, &destination),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_preview_import(&self, archive_path: PathBuf) -> Result<ImportPreview, String> {
        match &self.db {
            Some(db) => preview_import_archive(db, &archive_path),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_apply_import(
        &self,
        archive_path: PathBuf,
        mode: ImportMode,
        resolutions: Vec<ConflictResolution>,
    ) -> Result<ImportResult, String> {
        let backup_dir = self
            .db_path
            .as_ref()
            .and_then(|db_path| db_path.parent().map(|path| path.join("backups")))
            .unwrap_or_else(|| app_data_dir().join("backups"));

        match &self.db {
            Some(db) => apply_import_archive(db, &archive_path, mode, &resolutions, &backup_dir),
            None => Err("Database not initialized".to_string()),
        }
    }

    async fn handle_get_git_remote_sync_config(&self) -> Result<GitRemoteSyncState, String> {
        load_git_remote_sync_state()
    }

    async fn handle_save_git_remote_sync_config(
        &self,
        config: GitRemoteSyncConfig,
    ) -> Result<GitRemoteSyncState, String> {
        persist_git_remote_sync_state(config)
    }

    async fn handle_verify_git_remote(
        &self,
        config: GitRemoteSyncConfig,
    ) -> Result<GitRemoteVerification, String> {
        let state = persist_git_remote_sync_state(config)?;
        let client = GitRemoteSyncClient::new()?;
        client.verify_remote(&state.config)
    }

    async fn handle_push_snapshot_to_git_remote(
        &self,
        config: GitRemoteSyncConfig,
    ) -> Result<GitRemoteSyncPushResult, String> {
        let state = persist_git_remote_sync_state(config)?;
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| "Database not initialized".to_string())?;
        let client = GitRemoteSyncClient::new()?;
        let (snapshot_bytes, summary) = export_archive_to_bytes(db)?;
        let payload = build_upload_payload(&summary, snapshot_bytes)?;
        let result = client.push_snapshot(&state.config, payload)?;

        let mut updated_config = state.config.clone();
        updated_config.last_seen_remote_commit = Some(result.remote_commit.clone());
        updated_config.last_sync_at = Some(chrono::Utc::now());
        persist_git_remote_sync_state(updated_config)?;

        Ok(result)
    }

    async fn handle_pull_snapshot_from_git_remote(
        &self,
        config: GitRemoteSyncConfig,
    ) -> Result<GitRemoteSyncPullPreview, String> {
        let state = persist_git_remote_sync_state(config)?;
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| "Database not initialized".to_string())?;
        let client = GitRemoteSyncClient::new()?;
        let result: GitRemoteSyncPullResult = client.pull_snapshot(&state.config)?;
        let archive_path = write_temp_git_remote_snapshot_archive(&result.archive_bytes)?;
        let preview = preview_import_archive(db, &archive_path)?;

        Ok(GitRemoteSyncPullPreview {
            preview,
            remote_commit: result.remote_commit,
            metadata: result.metadata,
        })
    }
}

fn load_git_remote_sync_state() -> Result<GitRemoteSyncState, String> {
    let settings = load_app_settings()?;
    let config = settings.git_sync.normalized();
    Ok(GitRemoteSyncState { config })
}

fn persist_git_remote_sync_state(
    config: GitRemoteSyncConfig,
) -> Result<GitRemoteSyncState, String> {
    let normalized = config.normalized();
    let mut settings = load_app_settings()?;
    settings.git_sync = normalized.clone();
    save_app_settings(&settings)?;
    Ok(GitRemoteSyncState { config: normalized })
}

fn write_temp_git_remote_snapshot_archive(bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("robinne-git-sync");
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("创建临时同步目录失败 {}: {}", dir.display(), err))?;
    let path = dir.join(format!("snapshot-{}.zip", uuid::Uuid::new_v4()));
    std::fs::write(&path, bytes)
        .map_err(|err| format!("写入临时同步归档失败 {}: {}", path.display(), err))?;
    Ok(path)
}

pub fn create_store() -> (Store, StoreRuntime) {
    let (sender, receiver) = unbounded();
    let (event_sender, _) = broadcast::channel(64);
    let store = Store {
        sender,
        event_sender: event_sender.clone(),
    };
    let runtime = StoreRuntime::new(receiver, event_sender);
    (store, runtime)
}

impl Store {
    pub fn subscribe_data_changes(&self) -> StoreEventReceiver {
        self.event_sender.subscribe()
    }

    pub async fn get_tasks(&self) -> Result<Vec<Record>, String> {
        eprintln!("[Store] get_tasks called");
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetTasks {
                completed: false,
                respond_to: tx,
            })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_tasks returning: {:?} records",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn create_record(&self, record: Record) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::CreateRecord {
                record,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn create_record_with_line(
        &self,
        record: Record,
        line_ref: Option<LineRef>,
    ) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::CreateRecordWithLine {
                record,
                line_ref,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn update_record(&self, record: Record) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::UpdateRecord {
                record,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn update_record_with_line(
        &self,
        record: Record,
        line_ref: Option<LineRef>,
    ) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::UpdateRecordWithLine {
                record,
                line_ref,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn get_notes(&self) -> Result<Vec<Record>, String> {
        eprintln!("[Store] get_notes called");
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetNotes { respond_to: tx })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_notes returning: {:?} records",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn delete_record(&self, id: uuid::Uuid) -> Result<(), String> {
        eprintln!("[Store] delete_record called for id: {}", id);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::DeleteRecord { id, respond_to: tx })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn get_timeline(&self, query: TimelineQuery) -> Result<Vec<Record>, String> {
        eprintln!(
            "[Store] get_timeline called with limit={}, offset={}, tags={:?}, persons={:?}",
            query.limit, query.offset, query.tags, query.persons
        );
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetTimeline {
                query,
                respond_to: tx,
            })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_timeline returning: {:?} records",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn get_line_graph(&self, project: Option<String>) -> Result<LineGraphData, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetLineGraph {
                project,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Ok(LineGraphData::default()))
    }

    pub async fn get_line_catalog(&self) -> Result<Vec<MetadataCatalogEntry>, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetLineCatalog { respond_to: tx })
            .await;
        rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()))
    }

    pub async fn complete_line(&self, line_id: uuid::Uuid) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::CompleteLine {
                line_id,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn delete_line(&self, line_id: uuid::Uuid) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::DeleteLine {
                line_id,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn get_or_create_line(&self, line_ref: LineRef) -> Result<Line, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetOrCreateLine {
                line_ref,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to get or create line".to_string()))
    }

    pub async fn search_records(&self, query: &str) -> Result<Vec<Record>, String> {
        eprintln!("[Store] search_records called with query='{}'", query);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::SearchRecords {
                query: query.to_string(),
                respond_to: tx,
            })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] search_records returning: {:?} records",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn get_all_records(&self) -> Result<Vec<Record>, String> {
        eprintln!("[Store] get_all_records called");
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetAllRecords { respond_to: tx })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_all_records returning: {:?} records",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn get_record_by_id(&self, id: uuid::Uuid) -> Result<Option<Record>, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetRecordById { id, respond_to: tx })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to get record".to_string()))
    }

    pub async fn get_records_by_tag(&self, tag: &str) -> Result<Vec<Record>, String> {
        eprintln!("[Store] get_records_by_tag called with tag='{}'", tag);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetRecordsByTag {
                tag: tag.to_string(),
                respond_to: tx,
            })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_records_by_tag returning: {:?} records",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn get_records_by_person(&self, person: &str) -> Result<Vec<Record>, String> {
        eprintln!(
            "[Store] get_records_by_person called with person='{}'",
            person
        );
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetRecordsByPerson {
                person: person.to_string(),
                respond_to: tx,
            })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_records_by_person returning: {:?} records",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn get_dashboard(&self) -> Result<DashboardData, String> {
        eprintln!("[Store] get_dashboard called");
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetDashboard { respond_to: tx })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to get dashboard".to_string()))
    }

    pub async fn get_stats(&self) -> Result<StatsData, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetStats { respond_to: tx })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to get stats".to_string()))
    }

    pub async fn get_attachments(&self, record_id: uuid::Uuid) -> Result<Vec<Attachment>, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetAttachments {
                record_id,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()))
    }

    pub async fn import_image_attachments(
        &self,
        record_id: uuid::Uuid,
        paths: Vec<PathBuf>,
    ) -> Result<Vec<Attachment>, String> {
        let (prep_tx, prep_rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = prepare_image_attachments(record_id, paths);
            let _ = prep_tx.send_blocking(result);
        });
        let prepared = prep_rx
            .recv()
            .await
            .map_err(|err| format!("图片处理任务失败: {}", err))??;
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::ImportImageAttachments {
                prepared,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()))
    }

    pub async fn enqueue_record_attachment_import(
        &self,
        record_id: uuid::Uuid,
        paths: Vec<PathBuf>,
    ) -> Result<(), String> {
        let (jobs_tx, jobs_rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = build_attachment_import_jobs(record_id, paths);
            let _ = jobs_tx.send_blocking(result);
        });
        let jobs = jobs_rx
            .recv()
            .await
            .map_err(|err| format!("图片处理任务失败: {}", err))??;

        if jobs.is_empty() {
            return Ok(());
        }

        let attachments = jobs.iter().map(|job| job.attachment.clone()).collect();
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::CreateAttachmentPlaceholders {
                attachments,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))?;

        let sender = self.sender.clone();
        std::thread::spawn(move || {
            for job in jobs {
                let result = prepare_attachment_for_existing_id(
                    &job.attachment.record_id,
                    &job.attachment.id,
                    job.path,
                );
                let send_result = match result {
                    Ok(prepared) => {
                        sender.send_blocking(StoreCommand::MarkAttachmentReady { prepared })
                    }
                    Err(error_message) => {
                        sender.send_blocking(StoreCommand::MarkAttachmentFailed {
                            attachment_id: job.attachment.id,
                            error_message,
                        })
                    }
                };

                if let Err(err) = send_result {
                    eprintln!(
                        "[Store] Failed to dispatch attachment background result: {}",
                        err
                    );
                    break;
                }
            }
        });

        Ok(())
    }

    pub async fn get_attachment_bytes(
        &self,
        attachment_id: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetAttachmentBytes {
                attachment_id: attachment_id.to_string(),
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(None))
    }

    pub async fn delete_attachment(&self, attachment_id: &str) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::DeleteAttachment {
                attachment_id: attachment_id.to_string(),
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn get_storage_usage_summary(&self) -> Result<StorageUsageSummary, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetStorageUsageSummary { respond_to: tx })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to get storage usage summary".to_string()))
    }

    pub async fn get_attachment_health_summary(&self) -> Result<AttachmentHealthSummary, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetAttachmentHealthSummary { respond_to: tx })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to get attachment health summary".to_string()))
    }

    pub async fn get_all_attachments(&self) -> Result<Vec<AttachmentListItem>, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetAllAttachments { respond_to: tx })
            .await;
        rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()))
    }

    pub async fn export_data(&self, destination: PathBuf) -> Result<ExportResult, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::ExportData {
                destination,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to export data".to_string()))
    }

    pub async fn preview_import(&self, archive_path: PathBuf) -> Result<ImportPreview, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::PreviewImport {
                archive_path,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to preview import".to_string()))
    }

    pub async fn apply_import(
        &self,
        archive_path: PathBuf,
        mode: ImportMode,
        resolutions: Vec<ConflictResolution>,
    ) -> Result<ImportResult, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::ApplyImport {
                archive_path,
                mode,
                resolutions,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to apply import".to_string()))
    }

    pub async fn get_git_remote_sync_config(&self) -> Result<GitRemoteSyncState, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetGitRemoteSyncConfig { respond_to: tx })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to load git sync config".to_string()))
    }

    pub async fn save_git_remote_sync_config(
        &self,
        config: GitRemoteSyncConfig,
    ) -> Result<GitRemoteSyncState, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::SaveGitRemoteSyncConfig {
                config,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to save git sync config".to_string()))
    }

    pub async fn verify_git_remote(
        &self,
        config: GitRemoteSyncConfig,
    ) -> Result<GitRemoteVerification, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::VerifyGitRemote {
                config,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to verify git remote".to_string()))
    }

    pub async fn push_snapshot_to_git_remote(
        &self,
        config: GitRemoteSyncConfig,
    ) -> Result<GitRemoteSyncPushResult, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::PushSnapshotToGitRemote {
                config,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to push git sync snapshot".to_string()))
    }

    pub async fn pull_snapshot_from_git_remote(
        &self,
        config: GitRemoteSyncConfig,
    ) -> Result<GitRemoteSyncPullPreview, String> {
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::PullSnapshotFromGitRemote {
                config,
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to pull git sync snapshot".to_string()))
    }

    pub async fn start_task(&self, id: uuid::Uuid) -> Result<(), String> {
        eprintln!("[Store] start_task called for id: {}", id);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::StartTask { id, respond_to: tx })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn complete_task(&self, id: uuid::Uuid) -> Result<(), String> {
        eprintln!("[Store] complete_task called for id: {}", id);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::CompleteTask { id, respond_to: tx })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn cancel_task(&self, id: uuid::Uuid, reason: Option<String>) -> Result<(), String> {
        eprintln!("[Store] cancel_task called for id: {}", id);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::CancelTask {
                id,
                reason,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn reopen_task(&self, id: uuid::Uuid) -> Result<(), String> {
        eprintln!("[Store] reopen_task called for id: {}", id);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::ReopenTask { id, respond_to: tx })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn get_all_tags(&self) -> Result<Vec<Tag>, String> {
        eprintln!("[Store] get_all_tags called");
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetAllTags { respond_to: tx })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_all_tags returning: {:?} tags",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn create_tag(&self, name: &str) -> Result<i64, String> {
        eprintln!("[Store] create_tag called with name='{}'", name);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::CreateTag {
                name: name.to_string(),
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to create tag".to_string()))
    }

    pub async fn get_tag_catalog(&self) -> Result<Vec<MetadataCatalogEntry>, String> {
        eprintln!("[Store] get_tag_catalog called");
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetTagCatalog { respond_to: tx })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_tag_catalog returning: {:?} entries",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn add_tag_to_record(
        &self,
        record_id: uuid::Uuid,
        tag_id: i64,
    ) -> Result<(), String> {
        eprintln!(
            "[Store] add_tag_to_record called for record_id: {}, tag_id: {}",
            record_id, tag_id
        );
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::AddTagToRecord {
                record_id,
                tag_id,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }

    pub async fn get_all_persons(&self) -> Result<Vec<Person>, String> {
        eprintln!("[Store] get_all_persons called");
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetAllPersons { respond_to: tx })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_all_persons returning: {:?} persons",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn create_person(&self, name: &str) -> Result<i64, String> {
        eprintln!("[Store] create_person called with name='{}'", name);
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::CreatePerson {
                name: name.to_string(),
                respond_to: tx,
            })
            .await;
        rx.recv()
            .await
            .unwrap_or_else(|_| Err("Failed to create person".to_string()))
    }

    pub async fn get_person_catalog(&self) -> Result<Vec<MetadataCatalogEntry>, String> {
        eprintln!("[Store] get_person_catalog called");
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::GetPersonCatalog { respond_to: tx })
            .await;
        let result = rx.recv().await.unwrap_or_else(|_| Ok(Vec::new()));
        eprintln!(
            "[Store] get_person_catalog returning: {:?} entries",
            result.as_ref().map(|v| v.len())
        );
        result
    }

    pub async fn add_person_to_record(
        &self,
        record_id: uuid::Uuid,
        person_id: i64,
    ) -> Result<(), String> {
        eprintln!(
            "[Store] add_person_to_record called for record_id: {}, person_id: {}",
            record_id, person_id
        );
        let (tx, rx) = async_channel::unbounded();
        let _ = self
            .sender
            .send(StoreCommand::AddPersonToRecord {
                record_id,
                person_id,
                respond_to: tx,
            })
            .await;
        rx.recv().await.unwrap_or(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::derive_task_stats;
    use crate::models::{Priority, Record, RecordType, TaskStatus};
    use chrono::{Duration, Local, TimeZone, Utc};
    use uuid::Uuid;

    fn make_task(
        status: TaskStatus,
        priority: Option<Priority>,
        due_offset_days: Option<i64>,
        completed_offset_days: Option<i64>,
        now_local: chrono::DateTime<Local>,
    ) -> Record {
        let now_utc = now_local.with_timezone(&Utc);
        let due_date = due_offset_days.map(|offset| {
            now_local
                .date_naive()
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_local_timezone(Local)
                .single()
                .unwrap()
                .checked_add_signed(Duration::days(offset))
                .unwrap()
                .with_timezone(&Utc)
        });
        let completed_at = completed_offset_days
            .map(|offset| now_local + Duration::days(offset))
            .map(|dt| dt.with_timezone(&Utc));

        Record {
            id: Uuid::new_v4(),
            line_id: None,
            title: Some("task".to_string()),
            content: String::new(),
            priority,
            status: Some(status),
            created_at: now_utc,
            updated_at: now_utc,
            started_at: None,
            completed_at,
            scheduled_for: None,
            due_date,
            notified_at: None,
            cancelled_reason: None,
            record_type: RecordType::Task,
            tags: Vec::new(),
            persons: Vec::new(),
        }
    }

    #[test]
    fn derive_task_stats_counts_open_risk_and_completion_buckets() {
        let now = Local.with_ymd_and_hms(2026, 4, 8, 9, 30, 0).unwrap();
        let tasks = vec![
            make_task(TaskStatus::Todo, Some(Priority::High), Some(0), None, now),
            make_task(
                TaskStatus::InProgress,
                Some(Priority::Medium),
                Some(1),
                None,
                now,
            ),
            make_task(TaskStatus::Todo, Some(Priority::Low), Some(-1), None, now),
            make_task(
                TaskStatus::Done,
                Some(Priority::High),
                Some(0),
                Some(0),
                now,
            ),
            make_task(
                TaskStatus::Done,
                Some(Priority::Medium),
                None,
                Some(-3),
                now,
            ),
            make_task(
                TaskStatus::Cancelled,
                Some(Priority::High),
                Some(2),
                Some(-6),
                now,
            ),
        ];

        let stats = derive_task_stats(&tasks, now);

        assert_eq!(stats.total_open_count, 3);
        assert_eq!(stats.total_in_progress, 1);
        assert_eq!(stats.due_today_count, 1);
        assert_eq!(stats.due_tomorrow_count, 1);
        assert_eq!(stats.overdue_count, 1);
        assert_eq!(stats.high_priority_open_count, 1);
        assert_eq!(stats.completed_today_count, 1);
        assert_eq!(stats.last_7_days_completed.len(), 7);
        assert_eq!(stats.last_7_days_completed[0].count, 0);
        assert_eq!(stats.last_7_days_completed[3].count, 1);
        assert_eq!(stats.last_7_days_completed[6].count, 1);
    }

    #[test]
    fn derive_task_stats_excludes_done_and_cancelled_from_open_counts() {
        let now = Local.with_ymd_and_hms(2026, 4, 8, 9, 30, 0).unwrap();
        let tasks = vec![
            make_task(
                TaskStatus::Done,
                Some(Priority::High),
                Some(-2),
                Some(-1),
                now,
            ),
            make_task(
                TaskStatus::Cancelled,
                Some(Priority::High),
                Some(0),
                Some(0),
                now,
            ),
        ];

        let stats = derive_task_stats(&tasks, now);

        assert_eq!(stats.total_open_count, 0);
        assert_eq!(stats.total_in_progress, 0);
        assert_eq!(stats.due_today_count, 0);
        assert_eq!(stats.due_tomorrow_count, 0);
        assert_eq!(stats.overdue_count, 0);
        assert_eq!(stats.high_priority_open_count, 0);
        assert_eq!(stats.completed_today_count, 0);
    }
}
