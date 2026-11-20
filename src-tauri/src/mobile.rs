//! Android's Tauri entry point. The command surface starts only the local
//! database and replication engine, with the same schema and merge rules as desktop.

use std::{
    io::Write,
    sync::{Arc, OnceLock},
};

use jni::{errors::ThrowRuntimeExAndDefault, jni_mangle, objects::JObject, EnvUnowned};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{plugin::PluginHandle, Manager, State, Wry};
use tauri_plugin_opener::OpenerExt;
use threestrands_sync_protocol::EntityType;

use crate::{
    db::{mobile_drafts::MobileDraft, Database, NoteDocument, NoteTask},
    enrollment::{self, EnrollmentStatus, JoinCodePreview, JoinCredentialsChoice},
    models::{
        CreateTaskRequest, Note, NoteLinks, NoteSave, NoteSearchHit, NoteSearchScope, NoteSummary,
        NoteTitle, ThreadTask, UpdateTaskRequest,
    },
    replicated_sync::{FrontierConflict, ReplicatedSync},
    run_database_task,
    s3_transport::S3Credentials,
    sync_connectors::ConnectorCredentials,
    AppError, AppResult,
};

static NATIVE: OnceLock<PluginHandle<Wry>> = OnceLock::new();

pub(crate) fn native() -> Result<&'static PluginHandle<Wry>, String> {
    NATIVE
        .get()
        .ok_or_else(|| "Android services are unavailable".into())
}

// The Kotlin plugin passes ApplicationContext before the webview can start
// sync. Reqwest's existing verifier then uses Android's trust store.
#[jni_mangle("app.threestrands.mail.NativeServices")]
pub fn initialize_tls<'caller>(
    mut env: EnvUnowned<'caller>,
    _plugin: JObject<'caller>,
    context: JObject<'caller>,
) {
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
        .resolve::<ThrowRuntimeExAndDefault>();
}

struct MobileState {
    database: Arc<Database>,
    engine: ReplicatedSync,
    sync_gate: tokio::sync::Mutex<()>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Target {
    Daily { date: String },
    Note { id: String },
}

impl Target {
    fn load(&self, database: &Database) -> AppResult<Option<Note>> {
        Ok(match self {
            Self::Daily { date } => database.get_daily_note(date)?,
            Self::Note { id } => database.get_note(id)?,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenNote {
    note: Option<Note>,
    document: Option<NoteDocument>,
    links: Option<NoteLinks>,
    conflicts: Vec<FrontierConflict>,
}

#[tauri::command]
async fn mobile_draft(
    key: String,
    state: State<'_, MobileState>,
) -> AppResult<Option<MobileDraft>> {
    let db = state.database.clone();
    run_database_task(move || db.mobile_draft(&key)).await
}

#[tauri::command]
async fn mobile_write_draft(
    key: String,
    draft: Option<MobileDraft>,
    expected: Option<MobileDraft>,
    state: State<'_, MobileState>,
) -> AppResult<()> {
    let db = state.database.clone();
    run_database_task(move || db.write_mobile_draft(&key, draft, expected)).await
}

#[tauri::command]
async fn mobile_note(target: Target, state: State<'_, MobileState>) -> AppResult<OpenNote> {
    let db = state.database.clone();
    run_database_task(move || {
        let note = target.load(&db)?;
        let mut document = None;
        let mut links = None;
        let mut conflicts = Vec::new();
        if let Some(note) = &note {
            document = Some(NoteDocument::from_note(note));
            links = Some(db.note_links(&note.id)?);
            conflicts = db
                .list_frontier_conflicts()?
                .into_iter()
                .filter(|field| field.entity_type == "note" && field.entity_id == note.id)
                .collect();
        }
        Ok::<_, AppError>(OpenNote {
            note,
            document,
            links,
            conflicts,
        })
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum NoteView {
    Recent,
    Starred,
    Trash,
}

#[derive(Serialize)]
#[serde(untagged)]
enum NoteResults {
    List(Vec<NoteSummary>),
    Search(Vec<NoteSearchHit>),
}

#[tauri::command]
async fn mobile_notes(
    view: NoteView,
    query: String,
    state: State<'_, MobileState>,
) -> AppResult<NoteResults> {
    let db = state.database.clone();
    run_database_task(move || {
        if !query.trim().is_empty() {
            let scope = match view {
                NoteView::Recent => NoteSearchScope::Active,
                NoteView::Starred => NoteSearchScope::Starred,
                NoteView::Trash => NoteSearchScope::Trash,
            };
            return Ok(NoteResults::Search(
                db.search_notes_in_scope(&query, 100, scope)?,
            ));
        }
        Ok::<_, AppError>(NoteResults::List(match view {
            NoteView::Recent => db.list_recent_notes(100)?,
            NoteView::Starred => db.list_starred_notes(100)?,
            NoteView::Trash => db.list_trashed_notes(100)?,
        }))
    })
    .await
}

#[tauri::command]
async fn mobile_note_titles(
    query: String,
    state: State<'_, MobileState>,
) -> AppResult<Vec<NoteTitle>> {
    let db = state.database.clone();
    run_database_task(move || db.suggest_note_titles(&query, 24)).await
}

#[tauri::command]
async fn mobile_find_note(
    title: String,
    state: State<'_, MobileState>,
) -> AppResult<Option<NoteTitle>> {
    let db = state.database.clone();
    run_database_task(move || db.find_note_by_title(&title)).await
}

#[tauri::command]
async fn mobile_save_note(
    target: Target,
    title: String,
    body: String,
    expected: Option<String>,
    state: State<'_, MobileState>,
) -> AppResult<NoteSave> {
    let db = state.database.clone();
    run_database_task(move || match target {
        Target::Daily { date } => db.save_daily_note_edit(&date, &body, expected.as_deref()),
        Target::Note { id } => {
            db.save_note(&id, &title, &body, expected.as_deref().unwrap_or_default())
        }
    })
    .await
}

#[tauri::command]
async fn mobile_create_note(
    title: String,
    body: String,
    state: State<'_, MobileState>,
) -> AppResult<Note> {
    let db = state.database.clone();
    run_database_task(move || db.create_note(&title, &body)).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum NoteAction {
    Star,
    Unstar,
    Trash,
    Restore,
}

#[tauri::command]
async fn mobile_note_action(
    id: String,
    action: NoteAction,
    state: State<'_, MobileState>,
) -> AppResult<Note> {
    let db = state.database.clone();
    run_database_task(move || match action {
        NoteAction::Star => db.set_note_starred(&id, true),
        NoteAction::Unstar => db.set_note_starred(&id, false),
        NoteAction::Trash => db.trash_note(&id),
        NoteAction::Restore => db.restore_note(&id),
    })
    .await
}

#[tauri::command]
async fn mobile_resolve_note(
    id: String,
    field: String,
    operation: String,
    state: State<'_, MobileState>,
) -> AppResult<()> {
    let db = state.database.clone();
    run_database_task(move || {
        db.resolve_frontier_conflict(EntityType::Note, &id, &field, &operation)
    })
    .await
}

#[tauri::command]
async fn mobile_check_note_item(
    id: String,
    marker: String,
    expected: String,
    checked: bool,
    state: State<'_, MobileState>,
) -> AppResult<Note> {
    let db = state.database.clone();
    run_database_task(move || db.patch_note_item(&id, &marker, &expected, Some(checked), None))
        .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MobileTasks {
    standalone: Vec<ThreadTask>,
    from_notes: Vec<NoteTask>,
}

#[tauri::command]
async fn mobile_tasks(state: State<'_, MobileState>) -> AppResult<MobileTasks> {
    let db = state.database.clone();
    run_database_task(move || {
        Ok::<_, AppError>(MobileTasks {
            standalone: db.list_tasks(None, None)?,
            from_notes: db.list_note_tasks()?,
        })
    })
    .await
}

#[tauri::command]
async fn mobile_create_task(
    title: String,
    notes: Option<String>,
    due_date: Option<String>,
    state: State<'_, MobileState>,
) -> AppResult<ThreadTask> {
    let db = state.database.clone();
    run_database_task(move || {
        db.create_task(&CreateTaskRequest {
            account_id: String::new(),
            thread_id: None,
            source_message_id: None,
            subject_snapshot: None,
            title,
            notes,
            kind: "action".into(),
            due_kind: if due_date.is_some() { "date" } else { "none" }.into(),
            due_value: due_date,
            time_zone: None,
            repeat_interval_days: None,
            evidence_text: None,
            goal_id: None,
        })
    })
    .await
}

#[tauri::command]
async fn mobile_update_task(
    request: UpdateTaskRequest,
    state: State<'_, MobileState>,
) -> AppResult<ThreadTask> {
    let db = state.database.clone();
    run_database_task(move || db.update_task(&request)).await
}

#[tauri::command]
async fn mobile_check_task(
    id: String,
    checked: bool,
    state: State<'_, MobileState>,
) -> AppResult<ThreadTask> {
    let db = state.database.clone();
    let status = if checked { "completed" } else { "open" };
    run_database_task(move || db.set_task_status(&id, status, "user")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStatus {
    enrolled: bool,
    revoked: bool,
    awaiting_admission_from: Option<String>,
    last_success_at: Option<String>,
    last_error: Option<String>,
}

fn sync_status(db: &Database) -> AppResult<SyncStatus> {
    let enrollment = db.enrollment_status()?;
    let (enrolled, awaiting_admission_from) = match enrollment {
        EnrollmentStatus::Enrolled {
            awaiting_admission_from,
            ..
        } => (true, awaiting_admission_from),
        _ => (false, None),
    };
    let revoked = db
        .device_roster()?
        .into_iter()
        .any(|device| device.is_self && device.status == "revoked");
    let transport = db.configured_transports()?.into_iter().next();
    Ok(SyncStatus {
        enrolled,
        revoked,
        awaiting_admission_from,
        last_success_at: transport
            .as_ref()
            .and_then(|row| row.last_success_at.clone()),
        last_error: transport.and_then(|row| row.last_error),
    })
}

#[tauri::command]
async fn mobile_sync_status(state: State<'_, MobileState>) -> AppResult<SyncStatus> {
    let db = state.database.clone();
    run_database_task(move || sync_status(&db)).await
}

#[tauri::command]
async fn mobile_sync_now(state: State<'_, MobileState>) -> AppResult<SyncStatus> {
    let _cycle = state.sync_gate.lock().await;
    let status = sync_status(&state.database)?;
    if status.enrolled && !status.revoked {
        // Enrollment metadata can survive the loss of this installation's keys.
        let db = state.database.clone();
        run_database_task(move || db.require_mobile_sync_keys().map_err(|_| {
            AppError::message("Sync keys are unavailable. Export your work before disconnecting and pairing again.")
        })).await?;
        let connector = state.database.configured_transports()?.into_iter().next();
        let Some(connector) = connector else {
            return Err("The sync group has no connector on this phone.".into());
        };
        if connector.open_connector().await.is_none() {
            return Err("The S3 connector could not be opened. Check its credentials or pair the phone again.".into());
        }
        state.engine.sync_once().await?;
    }
    sync_status(&state.database)
}

fn preview_android_join(code: &str) -> AppResult<JoinCodePreview> {
    let preview = enrollment::preview_join_code(code, chrono::Utc::now().timestamp_millis())?;
    let [connector] = preview.connectors.as_slice() else {
        return Err(
            "Create a join code on desktop with only its HTTPS S3 connector selected.".into(),
        );
    };
    if connector.kind != "s3" || !connector.supported {
        return Err(
            "Create a join code on desktop with only its HTTPS S3 connector selected.".into(),
        );
    }
    // Preview locations use the validated endpoint followed by the bucket.
    if !connector.location.starts_with("https://") {
        return Err("Android sync requires an HTTPS S3 connector.".into());
    }
    if preview.expired {
        return Err("This join code expired. Create another on desktop.".into());
    }
    Ok(preview)
}

#[tauri::command]
fn mobile_preview_join(code: String) -> AppResult<JoinCodePreview> {
    preview_android_join(code.trim())
}

#[tauri::command]
async fn mobile_join(
    code: String,
    credentials: Option<S3Credentials>,
    state: State<'_, MobileState>,
) -> AppResult<SyncStatus> {
    let _cycle = state.sync_gate.lock().await;
    let code = code.trim();
    let preview = preview_android_join(code)?;
    if sync_status(&state.database)?.enrolled {
        return Err("Disconnect this phone before pairing with another group.".into());
    }
    let connector = &preview.connectors[0];
    if connector.needs_credentials && credentials.is_none() {
        return Err("Enter the credentials for this connector.".into());
    }
    let credentials = credentials
        .into_iter()
        .map(|credentials| JoinCredentialsChoice {
            connector_index: connector.index,
            credentials: ConnectorCredentials::S3(credentials),
        })
        .collect();
    for row in state.database.configured_transports()? {
        state.database.remove_transport(&row.instance_id)?;
    }
    // Enable recording only after the enrollment has adopted its keys.
    state.engine.join_with_code(code, &[], credentials).await?;
    state.database.set_beta_features_enabled(true)?;
    sync_status(&state.database)
}

#[tauri::command]
async fn mobile_disconnect(state: State<'_, MobileState>) -> AppResult<SyncStatus> {
    let _cycle = state.sync_gate.lock().await;
    state.database.set_beta_features_enabled(false)?;
    let leave = state.engine.leave_sync_space().await;
    for row in state.database.configured_transports()? {
        state.database.remove_transport(&row.instance_id)?;
    }
    leave?;
    sync_status(&state.database)
}

#[tauri::command]
async fn mobile_export(app: tauri::AppHandle, state: State<'_, MobileState>) -> AppResult<bool> {
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(AppError::message)?
        .join("mobile-exports");
    let db = state.database.clone();
    let path = run_database_task(move || -> Result<_, String> {
        let notes = db.notes_for_mobile_export()?;
        let tasks = db.list_tasks(None, None)?;
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let path = dir.join(format!("{}.zip", uuid::Uuid::new_v4()));
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            let file = std::fs::File::options()
                .create_new(true)
                .write(true)
                .open(&path)?;
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            for note in &notes {
                let stem = crate::notes_export::file_stem(&note.title);
                let folder = if note.trashed {
                    "trash"
                } else if note.daily_date.is_some() {
                    "daily"
                } else {
                    "notes"
                };
                let name = match &note.daily_date {
                    Some(date) => date.clone(),
                    None => format!("{stem}-{}", crate::notes_export::file_stem(&note.id)),
                };
                zip.start_file(format!("{folder}/{name}.md"), options)?;
                zip.write_all(crate::notes_export::file_text(note).as_bytes())?;
            }
            zip.start_file("tasks.json", options)?;
            serde_json::to_writer_pretty(&mut zip, &tasks)?;
            zip.finish()?;
            Ok(())
        };
        if let Err(error) = write() {
            let _ = std::fs::remove_file(&path);
            return Err(error.to_string());
        }
        Ok(path)
    })
    .await?;
    let result = native()?
        .run_mobile_plugin_async("exportFile", json!({ "path": path }))
        .await;
    let _ = std::fs::remove_file(path);
    result.map_err(AppError::message)
}

#[tauri::command]
fn mobile_open_url(url: String, app: tauri::AppHandle) -> AppResult<()> {
    let url = url::Url::parse(&url).map_err(AppError::message)?;
    if !matches!(url.scheme(), "http" | "https" | "mailto") {
        return Err("That link type cannot be opened.".into());
    }
    app.opener()
        .open_url(url.to_string(), None::<&str>)
        .map_err(AppError::message)
}

#[tauri::mobile_entry_point]
pub fn run() {
    let context = tauri::generate_context!();
    let dev_origin = context
        .config()
        .build
        .dev_url
        .as_ref()
        .map(url::Url::origin);
    let guard = tauri::plugin::Builder::<Wry>::new("mobile-navigation")
        .on_navigation(move |_, url| {
            (url.host_str() == Some("tauri.localhost") && matches!(url.scheme(), "http" | "https"))
                || (url.host_str() == Some("localhost") && url.scheme() == "tauri")
                || cfg!(debug_assertions) && Some(url.origin()) == dev_origin
        })
        .build();
    tauri::Builder::default()
        .plugin(guard)
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri::plugin::Builder::<Wry>::new("native-services")
                .setup(|_, api| {
                    let native =
                        api.register_android_plugin("app.threestrands.mail", "NativeServices")?;
                    let _ = NATIVE.set(native);
                    Ok(())
                })
                .build(),
        )
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let database = Arc::new(
                Database::open(&dir.join("threestrands.sqlite"))
                    .map_err(|error| std::io::Error::other(error.to_string()))?,
            );
            database.initialize_mobile_drafts()?;
            app.manage(MobileState {
                engine: ReplicatedSync::new(database.clone()),
                database,
                sync_gate: tokio::sync::Mutex::new(()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            mobile_draft,
            mobile_write_draft,
            mobile_note,
            mobile_notes,
            mobile_note_titles,
            mobile_find_note,
            mobile_save_note,
            mobile_create_note,
            mobile_note_action,
            mobile_resolve_note,
            mobile_check_note_item,
            mobile_tasks,
            mobile_create_task,
            mobile_update_task,
            mobile_check_task,
            mobile_sync_status,
            mobile_sync_now,
            mobile_preview_join,
            mobile_join,
            mobile_disconnect,
            mobile_export,
            mobile_open_url,
        ])
        .run(context)
        .expect("error while running ThreeStrands on Android");
}
