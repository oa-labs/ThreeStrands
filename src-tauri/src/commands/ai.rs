//! AI features: summaries, analysis, thread chat, briefs, and reply help.

use std::collections::HashSet;

use crate::models::{
    ActionAnalysis, ActionProposal, AiActivity, AiUsageDay, ChatAttachmentRef,
    ChatAttachmentSource, ChatSource, ReplyAssistContext, ReplyAssistResult, SummaryResult,
    ThreadBriefResult, ThreadChatReply, ThreadChatRequest, ThreadDetail, ThreadTask,
};
use chrono::Utc;
use tauri::State;

use super::attachments::load_attachment;
use crate::{ai, attachment_text, db, pdf_worker};
use crate::{database_result, run_database_task, AppError, AppResult, AppState};

#[tauri::command]
pub(crate) fn ai_api_key_configured() -> bool {
    ai::configured()
}

#[tauri::command]
pub(crate) fn set_ai_api_key(key: String) -> AppResult<()> {
    ai::set(&key).map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn ai_test_connection(
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<()> {
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    let mut purpose = ai::RequestPurpose::new("connection");
    purpose.control = control;
    ai::with_purpose(
        purpose,
        ai::test_connection(provider, &model, endpoint.as_deref(), &api_key),
    )
    .await
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn ai_list_models(provider: ai::AiProvider) -> AppResult<Vec<ai::ModelInfo>> {
    // `list_models` decides per provider whether the key is sent: never to
    // OpenRouter or Straitly, whose catalogs are public, and required by the others. A
    // missing key is passed through rather than rejected here.
    let api_key = ai::get_key()?;
    ai::list_models(provider, api_key.as_deref())
        .await
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn ai_summarize_thread(
    thread_id: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    reasoning: Option<ai::Reasoning>,
    state: State<'_, AppState>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<SummaryResult> {
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    let detail = state.database.get_thread(&thread_id)?;
    let revision = detail.thread.last_message_at.clone();
    let user_addresses = account_addresses(&state, &detail.thread.account_id)?;
    let focus_from = brief_focus(&state, &detail, &user_addresses)?;
    let mut purpose = ai::RequestPurpose::about(
        "summary",
        &detail.thread.subject,
        Some(ai::sent_message_count(detail.messages.len())),
    );
    purpose.control = control;
    purpose.scope = ai::manifest::SourceScope {
        account_ids: vec![detail.thread.account_id.clone()],
        record_id: Some(thread_id.clone()),
        revision: Some(revision.clone()),
    };
    let request = ai::SummarizeRequest {
        provider,
        model,
        endpoint,
        reasoning: reasoning.unwrap_or_default(),
        subject: detail.thread.subject,
        messages: detail
            .messages
            .into_iter()
            .map(|message| ai::ThreadMessageInput {
                sender: message.sender,
                sent_at: message.sent_at,
                body_text: message.body_text,
            })
            .collect(),
        focus_from: focus_from.clone(),
    };
    let (summary, provenance) =
        ai::with_purpose_recorded(purpose, ai::summarize(request, &api_key)).await?;
    save_thread_summary(
        &state,
        &thread_id,
        summary,
        &revision,
        focus_from.as_deref(),
        provenance.as_ref(),
        Vec::new(),
    )
}

/// The account's own sending addresses, bounded for a prompt.
fn account_addresses(state: &AppState, account_id: &str) -> AppResult<Vec<String>> {
    Ok(state
        .database
        .account_sending_identities(account_id)?
        .into_iter()
        .take(ai::MAX_PROMPT_USER_ADDRESSES)
        .map(|identity| identity.address)
        .collect())
}

/// The names the user goes by in this thread's account, so a brief can tell
/// when a message names the reader; see `ai::user_names`.
fn account_names(
    state: &AppState,
    detail: &ThreadDetail,
    user_addresses: &[String],
) -> AppResult<Vec<String>> {
    let account = state.database.get_account(&detail.thread.account_id)?;
    let identities = state
        .database
        .account_sending_identities(&detail.thread.account_id)?;
    let known = account
        .iter()
        .filter_map(|account| account.display_name.as_deref())
        .chain(
            identities
                .iter()
                .filter_map(|identity| identity.display_name.as_deref()),
        );
    let senders = detail
        .messages
        .iter()
        .map(|message| message.sender.as_str());
    Ok(ai::user_names(known, senders, user_addresses))
}

/// Where the messages a brief, summary, or suggestions should focus on
/// start; see `ai::brief_focus_start`.
fn brief_focus(
    state: &AppState,
    detail: &ThreadDetail,
    user_addresses: &[String],
) -> AppResult<Option<String>> {
    let analysis = state.database.thread_analysis_coverage(&detail.thread.id)?;
    let mut prior = Vec::new();
    if let Some(revision) = detail.thread.summary_revision.as_deref() {
        prior.push(ai::PriorCoverage {
            revision,
            focus_from: detail.thread.summary_focus_from.as_deref(),
        });
    }
    if let Some((revision, focus_from)) = &analysis {
        prior.push(ai::PriorCoverage {
            revision,
            focus_from: focus_from.as_deref(),
        });
    }
    let messages: Vec<(&str, &str)> = detail
        .messages
        .iter()
        .map(|message| (message.sent_at.as_str(), message.sender.as_str()))
        .collect();
    Ok(ai::brief_focus_start(
        &messages,
        user_addresses,
        &detail.thread.last_message_at,
        &prior,
    ))
}

/// Saves a summary written from the thread as of `revision`. When a request
/// for a newer revision already saved its summary, that one stays and is
/// returned instead, so the caller shows what is stored.
fn save_thread_summary(
    state: &AppState,
    thread_id: &str,
    summary: String,
    revision: &str,
    focus_from: Option<&str>,
    provenance: Option<&ai::requests::RequestRecord>,
    evidence: Vec<crate::models::BriefEvidence>,
) -> AppResult<SummaryResult> {
    let generated_at = Utc::now().to_rfc3339();
    if !state.database.set_thread_summary_with_provenance(
        thread_id,
        &summary,
        &generated_at,
        revision,
        focus_from,
        provenance,
        &evidence,
    )? {
        if let Ok(detail) = state.database.get_thread(thread_id) {
            let thread = detail.thread;
            if let (Some(summary), Some(generated_at), Some(revision)) = (
                thread.summary,
                thread.summary_generated_at,
                thread.summary_revision,
            ) {
                return Ok(SummaryResult {
                    summary,
                    generated_at,
                    revision,
                    focus_from: thread.summary_focus_from,
                    evidence: thread.summary_evidence,
                });
            }
        }
    }
    Ok(SummaryResult {
        summary,
        generated_at,
        revision: revision.to_string(),
        focus_from: focus_from.map(str::to_string),
        evidence,
    })
}

/// Validates the timezone and loads the bounded analysis input shared by
/// thread analysis and the combined brief. Returns the thread's current
/// revision (its newest message time), which keys saved suggestions.
async fn thread_analysis_request(
    thread_id: &str,
    user_time_zone: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: &AppState,
) -> AppResult<(String, ai::AnalyzeRequest)> {
    if user_time_zone.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!("Unknown IANA timezone: {user_time_zone}").into());
    }
    if user_time_zone.chars().count() > 100 {
        return Err("Timezone value is too long".into());
    }
    let database = state.database.clone();
    let database_thread_id = thread_id.to_string();
    let detail = run_database_task(move || database.get_thread(&database_thread_id)).await?;
    let revision = detail.thread.last_message_at.clone();
    let goals = state
        .database
        .list_goals(Some(&detail.thread.account_id))?
        .into_iter()
        .filter(|goal| goal.status == "active")
        .take(ai::MAX_PROMPT_GOALS)
        .map(|goal| ai::GoalPromptInput {
            id: goal.id,
            title: goal.title,
            horizon: goal.horizon,
            period: goal.period,
        })
        .collect();
    let user_addresses = account_addresses(state, &detail.thread.account_id)?;
    let focus_from = brief_focus(state, &detail, &user_addresses)?;
    let user_names = account_names(state, &detail, &user_addresses)?;
    let request = ai::AnalyzeRequest {
        provider,
        model,
        endpoint,
        subject: detail.thread.subject,
        messages: detail
            .messages
            .into_iter()
            .map(|message| ai::ActionMessageInput {
                id: message.id,
                sender: message.sender,
                recipients: message.recipients,
                sent_at: message.sent_at,
                body_text: message.body_text,
            })
            .collect(),
        current_time: Utc::now().to_rfc3339(),
        user_time_zone,
        user_addresses,
        user_names,
        goals,
        focus_from,
    };
    Ok((revision, request))
}

/// Saves the suggestions for `revision`, replacing any saved for an older
/// one, and returns the suggestions now saved for the thread. Suggestions
/// already saved for a newer revision are kept. `replace_same_revision` is
/// false for a cache fill: a concurrent request for the same revision has
/// already saved its result, possibly with suggestions the user has since
/// handled, and that saved set is returned instead.
fn cache_thread_analysis(
    state: &AppState,
    thread_id: &str,
    revision: &str,
    analysis: ActionAnalysis,
    replace_same_revision: bool,
    focus_from: Option<&str>,
    provenance: Option<&ai::requests::RequestRecord>,
) -> AppResult<ActionAnalysis> {
    let messages = state.database.get_thread(thread_id)?.messages;
    let tasks = state.database.list_tasks_for_thread(thread_id)?;
    let analysis = without_tracked_tasks(without_invitation_meetings(analysis, &messages), &tasks);
    let saved = database_result(state.database.save_thread_analysis_with_provenance(
        thread_id,
        revision,
        &analysis,
        &Utc::now().to_rfc3339(),
        replace_same_revision,
        focus_from,
        provenance,
    ))?;
    if saved {
        return Ok(analysis);
    }
    let saved = state
        .database
        .thread_analysis(thread_id, revision)?
        .unwrap_or(analysis);
    Ok(without_tracked_tasks(
        without_invitation_meetings(saved, &messages),
        &tasks,
    ))
}

fn without_tracked_tasks(mut analysis: ActionAnalysis, tasks: &[ThreadTask]) -> ActionAnalysis {
    let normalized = |title: &str| {
        title
            .trim_end_matches(['.', '!', '?'])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    analysis.proposals.retain(|proposal| {
        let crate::models::ActionProposal::Task(proposal) = proposal else {
            return true;
        };
        let title = normalized(proposal.title.trim());
        !tasks.iter().any(|task| {
            task.kind == proposal.kind
                && task.source_message_id.as_deref() == Some(&proposal.evidence.source_message_id)
                && normalized(task.title.trim()) == title
        })
    });
    analysis
}

/// A calendar invitation already defines its event. This applies to fresh
/// briefs, saved analyses from older builds, and chat, independently of what
/// the model calls the event. Tasks and meetings from other messages remain.
fn without_invitation_meetings(
    mut analysis: ActionAnalysis,
    messages: &[crate::models::Message],
) -> ActionAnalysis {
    let invitations: HashSet<&str> = messages
        .iter()
        .filter(|message| {
            message.attachments.iter().any(|attachment| {
                attachment
                    .mime_type
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("text/calendar")
                    || attachment.filename.to_ascii_lowercase().ends_with(".ics")
            })
        })
        .map(|message| message.id.as_str())
        .collect();
    analysis.proposals.retain(|proposal| match proposal {
        ActionProposal::Meeting(meeting) => {
            !invitations.contains(meeting.evidence.source_message_id.as_str())
        }
        ActionProposal::Task(_) => true,
    });
    analysis
}

#[tauri::command]
pub(crate) async fn ai_analyze_thread(
    thread_id: String,
    user_time_zone: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<ActionAnalysis> {
    let (revision, request) = thread_analysis_request(
        &thread_id,
        user_time_zone,
        provider,
        model.clone(),
        endpoint,
        &state,
    )
    .await?;
    if let Some(saved) = state.database.thread_analysis(&thread_id, &revision)? {
        return Ok(without_tracked_tasks(
            without_invitation_meetings(saved, &state.database.get_thread(&thread_id)?.messages),
            &state.database.list_tasks_for_thread(&thread_id)?,
        ));
    }
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    log::info!(
        target: "ai_analyze_thread",
        "starting analysis for thread {thread_id} with provider {provider:?} model {model}"
    );
    let focus_from = request.focus_from.clone();
    let mut purpose = ai::RequestPurpose::about(
        "suggestions",
        &request.subject,
        Some(ai::sent_message_count(request.messages.len())),
    );
    purpose.control = control;
    purpose.scope = ai::manifest::SourceScope {
        account_ids: vec![state.database.get_thread(&thread_id)?.thread.account_id],
        record_id: Some(thread_id.clone()),
        revision: Some(revision.clone()),
    };
    let (analysis, provenance) = match ai::diagnostics::with_request(
        "suggestions",
        &thread_id,
        provider,
        &model,
        ai::with_purpose_recorded(purpose, ai::analyze(request, &api_key)),
    )
    .await
    {
        Ok(analysis) => analysis,
        Err(error) => {
            log::error!(target: "ai_analyze_thread", "analysis failed for thread {thread_id}: {error}");
            return Err(error.into());
        }
    };
    log::info!(
        target: "ai_analyze_thread",
        "analysis succeeded for thread {thread_id} with {} proposal(s), {} withheld",
        analysis.proposals.len(),
        analysis.hidden_count
    );
    cache_thread_analysis(
        &state,
        &thread_id,
        &revision,
        analysis,
        false,
        focus_from.as_deref(),
        provenance.as_ref(),
    )
}

/// Drops a handled suggestion from the thread's saved suggestions for
/// `revision` (the thread's newest message time when it was shown).
#[tauri::command]
pub(crate) async fn remove_thread_suggestion(
    thread_id: String,
    revision: String,
    proposal: ActionProposal,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let database = state.database.clone();
    run_database_task(move || database.remove_thread_suggestion(&thread_id, &revision, &proposal))
        .await
}

/// One line per open task for chat context: its title and due value.
fn chat_task_line(task: &ThreadTask) -> String {
    match &task.due_value {
        Some(due) => format!("{} (due {due})", task.title),
        None => task.title.clone(),
    }
}

/// The attachments a question shares, once each, after checking each belongs
/// to a message in the open conversation.
fn chat_attachment_refs(
    requested: &[ChatAttachmentRef],
    thread_message_ids: &[&str],
) -> AppResult<Vec<ChatAttachmentRef>> {
    let mut unique: Vec<ChatAttachmentRef> = Vec::new();
    for reference in requested {
        if !unique.contains(reference) {
            unique.push(reference.clone());
        }
    }
    if unique.len() > attachment_text::MAX_CHAT_ATTACHMENTS {
        return Err(format!(
            "Share up to {} attachments at a time",
            attachment_text::MAX_CHAT_ATTACHMENTS
        )
        .into());
    }
    if unique
        .iter()
        .any(|reference| !thread_message_ids.contains(&reference.message_id.as_str()))
    {
        return Err("That attachment isn't in this conversation".into());
    }
    Ok(unique)
}

/// Extracts an attachment's text off the async runtime, failing if the
/// parser runs past the timeout or panics on a hostile file. PDFs are parsed
/// by `pdf_command` (a killable worker process on desktop).
async fn extract_chat_attachment(
    filename: &str,
    mime_type: &str,
    bytes: Vec<u8>,
    timeout: std::time::Duration,
    pdf_command: &pdf_worker::WorkerCommand,
) -> AppResult<attachment_text::ExtractedText> {
    let kind = attachment_text::kind_for(filename, mime_type).ok_or_else(|| {
        format!("Thread Chat can't read {filename}; it reads text, PDF, Word, Excel, and PowerPoint files")
    })?;
    if kind == attachment_text::DocumentKind::Pdf {
        if bytes.len() > crate::limits::MAX_ATTACHMENT_BYTES {
            return Err(format!("Couldn't read {filename}: it is too large to read").into());
        }
        return pdf_worker::pdf_pages(pdf_command, bytes, timeout)
            .await
            .and_then(|pages| attachment_text::extract_pdf_pages(&pages))
            .map_err(|reason| format!("Couldn't read {filename}: {reason}").into());
    }
    run_attachment_extraction(filename, timeout, move || {
        attachment_text::extract(kind, &bytes)
    })
    .await
}

async fn run_attachment_extraction(
    filename: &str,
    timeout: std::time::Duration,
    job: impl FnOnce() -> Result<attachment_text::ExtractedText, String> + Send + 'static,
) -> AppResult<attachment_text::ExtractedText> {
    let task = tokio::task::spawn_blocking(job);
    let reason = match tokio::time::timeout(timeout, task).await {
        Ok(Ok(Ok(text))) => return Ok(text),
        Ok(Ok(Err(reason))) => reason,
        Ok(Err(_)) => "it isn't a readable file".to_string(),
        Err(_) => "it took too long to read".to_string(),
    };
    Err(format!("Couldn't read {filename}: {reason}").into())
}

/// Loads and extracts the shared attachments, reusing recent extractions.
async fn chat_attachments(
    references: Vec<ChatAttachmentRef>,
    state: &AppState,
) -> AppResult<(Vec<ChatAttachmentSource>, Vec<ai::ChatAttachmentInput>)> {
    let mut sources = Vec::new();
    let mut inputs = Vec::new();
    let pdf_command = pdf_worker::WorkerCommand::current_exe()
        .map_err(|_| "Thread Chat couldn't start its document reader")?;
    for reference in references {
        // A file that already failed or timed out is not run again.
        if let Some(error) = state
            .attachment_text
            .failure(&reference.message_id, &reference.attachment_id)
        {
            return Err(error.into());
        }
        let (filename, mime_type, extracted) = match state
            .attachment_text
            .get(&reference.message_id, &reference.attachment_id)
        {
            Some(cached) => cached,
            None => {
                let (filename, mime_type, bytes) =
                    load_attachment(&reference.message_id, &reference.attachment_id, state).await?;
                let text = match extract_chat_attachment(
                    &filename,
                    &mime_type,
                    bytes,
                    attachment_text::ATTACHMENT_EXTRACT_TIMEOUT,
                    &pdf_command,
                )
                .await
                {
                    Ok(text) => text,
                    Err(error) => {
                        state.attachment_text.insert_failure(
                            &reference.message_id,
                            &reference.attachment_id,
                            error.to_string(),
                        );
                        return Err(error);
                    }
                };
                let entry = (filename, mime_type, text);
                state.attachment_text.insert(
                    &reference.message_id,
                    &reference.attachment_id,
                    entry.clone(),
                );
                entry
            }
        };
        sources.push(ChatAttachmentSource {
            message_id: reference.message_id.clone(),
            attachment_id: reference.attachment_id,
            filename: filename.clone(),
            truncated: extracted.truncated,
        });
        inputs.push(ai::ChatAttachmentInput {
            message_id: reference.message_id,
            filename,
            mime_type,
            text: extracted.text,
            truncated: extracted.truncated,
        });
    }
    Ok((sources, inputs))
}

#[cfg(test)]
mod chat_attachment_tests {
    use super::*;
    use std::time::Duration;

    fn reference(message_id: &str, attachment_id: &str) -> ChatAttachmentRef {
        ChatAttachmentRef {
            message_id: message_id.into(),
            attachment_id: attachment_id.into(),
        }
    }

    #[test]
    fn shares_each_attachment_once_and_only_from_the_open_conversation() {
        let ids = ["m1", "m2"];
        let shared = chat_attachment_refs(
            &[
                reference("m1", "a"),
                reference("m2", "b"),
                reference("m1", "a"),
            ],
            &ids,
        )
        .unwrap();
        assert_eq!(shared, vec![reference("m1", "a"), reference("m2", "b")]);
        assert_eq!(
            chat_attachment_refs(&[reference("m1", "a"), reference("elsewhere", "a")], &ids)
                .unwrap_err()
                .to_string(),
            "That attachment isn't in this conversation"
        );
        assert!(chat_attachment_refs(&[], &ids).unwrap().is_empty());
    }

    #[test]
    fn limits_how_many_attachments_one_question_shares() {
        let ids = ["m1"];
        let refs: Vec<ChatAttachmentRef> = (0..=attachment_text::MAX_CHAT_ATTACHMENTS)
            .map(|index| reference("m1", &index.to_string()))
            .collect();
        assert_eq!(
            chat_attachment_refs(&refs[..attachment_text::MAX_CHAT_ATTACHMENTS - 1], &ids)
                .unwrap()
                .len(),
            attachment_text::MAX_CHAT_ATTACHMENTS - 1
        );
        assert_eq!(
            chat_attachment_refs(&refs[..attachment_text::MAX_CHAT_ATTACHMENTS], &ids)
                .unwrap()
                .len(),
            attachment_text::MAX_CHAT_ATTACHMENTS
        );
        assert_eq!(
            chat_attachment_refs(&refs, &ids).unwrap_err().to_string(),
            format!(
                "Share up to {} attachments at a time",
                attachment_text::MAX_CHAT_ATTACHMENTS
            )
        );
    }

    #[tokio::test]
    async fn names_the_file_when_it_cannot_be_read() {
        let text = extract_chat_attachment(
            "notes.txt",
            "text/plain",
            b"Ship Friday".to_vec(),
            Duration::from_secs(5),
            &pdf_worker::WorkerCommand::current_exe().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(text.text, "Ship Friday");
        assert_eq!(
            extract_chat_attachment("photo.jpg", "image/jpeg", vec![1, 2, 3], Duration::from_secs(5), &pdf_worker::WorkerCommand::current_exe().unwrap()).await.unwrap_err().to_string(),
            "Thread Chat can't read photo.jpg; it reads text, PDF, Word, Excel, and PowerPoint files"
        );
        assert_eq!(
            extract_chat_attachment(
                "contract.docx",
                "",
                b"not a zip".to_vec(),
                Duration::from_secs(5),
                &pdf_worker::WorkerCommand::current_exe().unwrap()
            )
            .await
            .unwrap_err()
            .to_string(),
            "Couldn't read contract.docx: it isn't a readable Office file"
        );
    }

    #[tokio::test]
    async fn a_parser_that_hangs_or_panics_fails_only_the_question() {
        let slow = run_attachment_extraction("slow.pdf", Duration::from_millis(10), || {
            std::thread::sleep(Duration::from_millis(300));
            Ok(attachment_text::ExtractedText {
                text: "late".into(),
                truncated: false,
            })
        })
        .await;
        assert_eq!(
            slow.unwrap_err().to_string(),
            "Couldn't read slow.pdf: it took too long to read"
        );

        let panicked =
            run_attachment_extraction("bad.pdf", Duration::from_secs(5), || panic!("hostile file"))
                .await;
        assert_eq!(
            panicked.unwrap_err().to_string(),
            "Couldn't read bad.pdf: it isn't a readable file"
        );
    }

    /// A stand-in worker: `/bin/sh -c script`, run like the real worker.
    #[cfg(unix)]
    fn shell_worker(script: &str) -> pdf_worker::WorkerCommand {
        pdf_worker::WorkerCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env: Vec::new(),
        }
    }

    #[cfg(unix)]
    async fn read_pdf(
        script: &str,
        timeout: Duration,
    ) -> Result<attachment_text::ExtractedText, String> {
        extract_chat_attachment(
            "a.pdf",
            "",
            b"%PDF-1.4".to_vec(),
            timeout,
            &shell_worker(script),
        )
        .await
        .map_err(|error| error.to_string())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pdfs_are_read_from_the_worker_with_the_same_messages() {
        // One page "Hello" as the worker frames it.
        let text = read_pdf(
            "cat >/dev/null; printf '\\0\\0\\0\\5Hello'",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(text.text, "Hello");
        assert_eq!(
            read_pdf("exit 10", Duration::from_secs(5))
                .await
                .unwrap_err(),
            "Couldn't read a.pdf: it is password protected"
        );
        assert_eq!(
            read_pdf("exit 11", Duration::from_secs(5))
                .await
                .unwrap_err(),
            "Couldn't read a.pdf: it isn't a readable PDF"
        );
        // A worker with only blank pages is the scanned-image message.
        assert_eq!(
            read_pdf("printf '\\0\\0\\0\\0'", Duration::from_secs(5))
                .await
                .unwrap_err(),
            "Couldn't read a.pdf: it has no readable text (it may be a scanned image)"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_pdf_worker_that_hangs_is_killed_at_the_timeout() {
        let started = std::time::Instant::now();
        assert_eq!(
            read_pdf("exec sleep 30", Duration::from_millis(200))
                .await
                .unwrap_err(),
            "Couldn't read a.pdf: it took too long to read"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_pdf_worker_that_crashes_fails_cleanly() {
        assert_eq!(
            read_pdf("kill -SEGV $$", Duration::from_secs(5))
                .await
                .unwrap_err(),
            "Couldn't read a.pdf: it isn't a readable PDF"
        );
    }

    #[test]
    fn failed_attachments_are_remembered_for_the_session() {
        let cache = attachment_text::ExtractCache::default();
        assert_eq!(cache.failure("m", "a"), None);
        cache.insert_failure(
            "m",
            "a",
            "Couldn't read a.pdf: it took too long to read".into(),
        );
        assert_eq!(
            cache.failure("m", "a").as_deref(),
            Some("Couldn't read a.pdf: it took too long to read")
        );
        assert_eq!(cache.failure("m", "b"), None);
        assert_eq!(cache.failure("n", "a"), None);
    }
}

#[cfg(test)]
mod invitation_proposal_tests {
    use super::*;
    use crate::models::{Message, MessageAttachment};
    use serde_json::json;

    fn message(id: &str, filename: &str, mime_type: &str) -> Message {
        Message {
            sent_by_account: None,
            id: id.into(),
            thread_id: "thread".into(),
            sender: "organizer@example.com".into(),
            recipients: vec!["you@example.com".into()],
            cc: vec![],
            sent_at: "2026-10-23T12:00:00Z".into(),
            body_html: String::new(),
            body_text: "You are invited".into(),
            unread: false,
            unsubscribe: None,
            attachments: vec![MessageAttachment {
                id: "attachment".into(),
                filename: filename.into(),
                mime_type: mime_type.into(),
                size: 100,
                content_id: None,
                inline: false,
            }],
        }
    }

    fn analysis() -> ActionAnalysis {
        serde_json::from_value(json!({
            "proposals": [
                {"type": "meeting", "intent": "attend", "title": "Invitation", "participants": [],
                 "rawTimeLanguage": "Tuesday at 1pm", "confidence": 1,
                 "evidence": {"sourceMessageId": "invite", "excerpt": "You are invited"}},
                {"type": "task", "kind": "action", "title": "Prepare agenda", "confidence": 1,
                 "evidence": {"sourceMessageId": "invite", "excerpt": "Prepare agenda"}},
                {"type": "meeting", "intent": "schedule", "title": "Separate follow-up", "participants": [],
                 "rawTimeLanguage": "Next week", "confidence": 1,
                 "evidence": {"sourceMessageId": "follow-up", "excerpt": "Next week"}}
            ], "hiddenCount": 2, "extraCount": 1
        })).unwrap()
    }

    #[test]
    fn invitations_never_produce_ai_events_but_tasks_and_other_messages_remain() {
        for (filename, mime_type) in [
            ("INVITE.ICS", "application/octet-stream"),
            ("", " TEXT/CALENDAR ; method=REQUEST"),
        ] {
            let messages = [
                message("invite", filename, mime_type),
                message("follow-up", "notes.txt", "text/plain"),
            ];
            let filtered = without_invitation_meetings(analysis(), &messages);
            assert_eq!(filtered.proposals.len(), 2);
            assert!(
                matches!(&filtered.proposals[0], ActionProposal::Task(task) if task.title == "Prepare agenda")
            );
            assert!(
                matches!(&filtered.proposals[1], ActionProposal::Meeting(meeting) if meeting.title == "Separate follow-up")
            );
            assert_eq!(filtered.hidden_count, 2);
            assert_eq!(filtered.extra_count, 1);
            // The same rule applies when an old cached analysis is filtered again.
            assert_eq!(
                without_invitation_meetings(filtered, &messages)
                    .proposals
                    .len(),
                2
            );
        }
    }

    #[test]
    fn ordinary_email_keeps_its_meeting_suggestions() {
        let filtered = without_invitation_meetings(
            analysis(),
            &[message("invite", "invite.txt", "text/plain")],
        );
        assert_eq!(filtered.proposals.len(), 3);
    }
}

/// Answers a question about the open conversation. The context is the
/// conversation itself, open tasks linked to it or to the selected person,
/// the text of any attachments the user shared for the question, and, only
/// when `search_mailbox` is set, the best-matching other conversations from
/// local search.
#[tauri::command]
pub(crate) async fn ai_thread_chat(
    request: ThreadChatRequest,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<ThreadChatReply> {
    let question = request.question.trim().to_string();
    if question.is_empty() {
        return Err("Ask a question first".into());
    }
    if question.chars().count() > ai::MAX_CHAT_TURN_CHARS {
        return Err(format!(
            "Questions can be up to {} characters",
            ai::MAX_CHAT_TURN_CHARS
        )
        .into());
    }
    let thread_id = request.thread_id.clone();
    let (revision, analysis_request) = thread_analysis_request(
        &thread_id,
        request.user_time_zone.clone(),
        provider,
        model.clone(),
        endpoint.clone(),
        &state,
    )
    .await?;
    let message_ids: Vec<&str> = analysis_request
        .messages
        .iter()
        .map(|message| message.id.as_str())
        .collect();
    let attachment_refs = chat_attachment_refs(&request.attachments, &message_ids)?;
    let (shared_attachments, attachment_inputs) = chat_attachments(attachment_refs, &state).await?;

    let database = state.database.clone();
    let contact_id = request.contact_id.clone();
    let search_terms = request
        .search_mailbox
        .then(|| ai::chat_search_terms(&question));
    let lookup_thread_id = thread_id.clone();
    let account_id = state.database.get_thread(&thread_id)?.thread.account_id;
    let task_account = account_id.clone();
    let scoped_account = request
        .search_current_account_only
        .then(|| account_id.clone());
    let (open_tasks, other_threads) = run_database_task(move || {
        let mut tasks: Vec<ThreadTask> = database
            .list_tasks_for_thread(&lookup_thread_id)?
            .into_iter()
            .filter(|task| matches!(task.status.as_str(), "open" | "in_progress"))
            .collect();
        if let Some(contact_id) = contact_id {
            for task in database.list_contact_tasks(&contact_id)? {
                if task.account_id == task_account
                    && !tasks.iter().any(|existing| existing.id == task.id)
                {
                    tasks.push(task);
                }
            }
        }
        let mut others = Vec::new();
        if let Some(terms) = search_terms {
            for id in database.chat_search_thread_ids(
                &terms,
                &lookup_thread_id,
                ai::MAX_MAILBOX_CHAT_THREADS,
                scoped_account.as_deref(),
            )? {
                others.push(database.get_thread(&id)?);
            }
        }
        Ok::<_, db::DatabaseError>((tasks, others))
    })
    .await?;

    let searched: Vec<ChatSource> = other_threads
        .iter()
        .map(|detail| ChatSource {
            thread_id: detail.thread.id.clone(),
            account_id: detail.thread.account_id.clone(),
            subject: detail.thread.subject.clone(),
            last_message_at: detail.thread.last_message_at.clone(),
        })
        .collect();
    let chat_request = ai::ChatRequest {
        provider,
        model: model.clone(),
        endpoint,
        question,
        history: request.history,
        subject: analysis_request.subject,
        messages: analysis_request.messages,
        open_tasks: open_tasks.iter().map(chat_task_line).collect(),
        other_threads: other_threads
            .into_iter()
            .map(|detail| ai::ChatThreadInput {
                thread_id: detail.thread.id,
                subject: detail.thread.subject,
                messages: detail
                    .messages
                    .into_iter()
                    .map(|message| ai::ActionMessageInput {
                        id: message.id,
                        recipients: Vec::new(),
                        sender: message.sender,
                        sent_at: message.sent_at,
                        body_text: message.body_text,
                    })
                    .collect(),
            })
            .collect(),
        attachments: attachment_inputs,
        proposals_allowed: request.include_proposals,
        current_time: analysis_request.current_time,
        user_time_zone: analysis_request.user_time_zone,
    };
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    log::info!(
        target: "ai_thread_chat",
        "asking about thread {thread_id} with provider {provider:?} model {model}, {} other thread(s), {} attachment(s)",
        searched.len(),
        shared_attachments.len()
    );
    let mut purpose = ai::RequestPurpose::about(
        "chat",
        &chat_request.subject,
        Some(ai::sent_message_count(chat_request.messages.len())),
    );
    purpose.control = control;
    purpose.scope = ai::manifest::SourceScope {
        account_ids: std::iter::once(account_id)
            .chain(searched.iter().map(|source| source.account_id.clone()))
            .collect(),
        record_id: Some(thread_id.clone()),
        revision: Some(revision.clone()),
    };
    let answer = match ai::diagnostics::with_request(
        "chat",
        &thread_id,
        provider,
        &model,
        ai::with_purpose(purpose, ai::chat(chat_request, &api_key)),
    )
    .await
    {
        Ok(answer) => answer,
        Err(error) => {
            log::error!(target: "ai_thread_chat", "chat failed for thread {thread_id}: {error}");
            return Err(error.into());
        }
    };
    let sources = answer
        .source_thread_ids
        .iter()
        .filter_map(|id| {
            searched
                .iter()
                .find(|source| &source.thread_id == id)
                .cloned()
        })
        .collect();
    Ok(ThreadChatReply {
        answer: answer.answer,
        analysis: without_invitation_meetings(
            answer.analysis,
            &state.database.get_thread(&thread_id)?.messages,
        ),
        reply_draft: answer.reply_draft,
        sources,
        evidence: answer.evidence,
        searched,
        attachments: shared_attachments,
        availability: answer.availability,
    })
}

/// Most recent days of AI usage the settings screen may request.
const MAX_AI_USAGE_SUMMARY_DAYS: u32 = 31;

/// AI provider usage for the last `days` local days, including today.
#[tauri::command(async)]
pub(crate) fn ai_usage_summary(
    days: u32,
    state: State<'_, AppState>,
) -> AppResult<Vec<AiUsageDay>> {
    if days == 0 || days > MAX_AI_USAGE_SUMMARY_DAYS {
        return Err(format!("Usage can be shown for 1 to {MAX_AI_USAGE_SUMMARY_DAYS} days").into());
    }
    let since = chrono::Local::now().date_naive() - chrono::Days::new(u64::from(days - 1));
    database_result(
        state
            .database
            .ai_usage_since(&since.format("%Y-%m-%d").to_string()),
    )
}

/// Summarizes the thread and extracts proposals in one provider call. Always
/// calls the provider; the summary is persisted like `ai_summarize_thread`
/// and the proposals replace the thread's cached analysis.
#[tauri::command]
pub(crate) async fn ai_brief_thread(
    thread_id: String,
    user_time_zone: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<ThreadBriefResult> {
    let (revision, request) = thread_analysis_request(
        &thread_id,
        user_time_zone,
        provider,
        model.clone(),
        endpoint,
        &state,
    )
    .await?;
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    log::info!(
        target: "ai_analyze_thread",
        "starting brief for thread {thread_id} with provider {provider:?} model {model}"
    );
    let focus_from = request.focus_from.clone();
    let mut purpose = ai::RequestPurpose::about(
        "brief",
        &request.subject,
        Some(ai::sent_message_count(request.messages.len())),
    );
    purpose.control = control;
    purpose.scope = ai::manifest::SourceScope {
        account_ids: vec![state.database.get_thread(&thread_id)?.thread.account_id],
        record_id: Some(thread_id.clone()),
        revision: Some(revision.clone()),
    };
    let (brief, provenance) = match ai::diagnostics::with_request(
        "brief",
        &thread_id,
        provider,
        &model,
        ai::with_purpose_recorded(purpose, ai::brief(request, &api_key)),
    )
    .await
    {
        Ok(brief) => brief,
        Err(error) => {
            log::error!(target: "ai_analyze_thread", "brief failed for thread {thread_id}: {error}");
            return Err(error.into());
        }
    };
    log::info!(
        target: "ai_analyze_thread",
        "brief succeeded for thread {thread_id} with {} proposal(s), {} withheld",
        brief.analysis.proposals.len(),
        brief.analysis.hidden_count
    );
    let summary = save_thread_summary(
        &state,
        &thread_id,
        brief.summary,
        &revision,
        focus_from.as_deref(),
        provenance.as_ref(),
        brief.evidence,
    )?;
    let analysis = cache_thread_analysis(
        &state,
        &thread_id,
        &revision,
        brief.analysis,
        true,
        focus_from.as_deref(),
        provenance.as_ref(),
    )?;
    Ok(ThreadBriefResult {
        summary,
        analysis,
        offered_times: brief.offered_times,
        offered_times_error: brief.offered_times_error,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ai_enrich_contact(
    id: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    empty_fields: Option<Vec<String>>,
    search_more: Option<bool>,
    account_id: Option<String>,
    reasoning: Option<ai::Reasoning>,
    state: State<'_, AppState>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<ai::ContactEnrichmentResult> {
    let profile = state
        .database
        .get_contact_profile(&id)?
        .ok_or_else(|| "Contact could not be found".to_string())?;
    // Inspect more conversations locally so the first provider batch can
    // favor messages that the contact opened, even if those are less recent.
    let timeline =
        state
            .database
            .contact_timeline_for_account(&id, 0, 30, account_id.as_deref())?;
    let addresses = profile
        .addresses
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut messages = Vec::new();
    let mut source_accounts = HashSet::new();
    for item in timeline {
        let detail = state.database.get_thread(&item.thread_id)?;
        for (index, message) in detail.messages.into_iter().enumerate() {
            let sender_matches = crate::correspondence::stored_addresses(&message.sender)
                .iter()
                .any(|(_, email)| addresses.contains(&email.to_ascii_lowercase()));
            let recipient_matches = message.recipients.iter().any(|raw| {
                crate::correspondence::stored_addresses(raw)
                    .iter()
                    .any(|(_, email)| addresses.contains(&email.to_ascii_lowercase()))
            });
            if sender_matches || recipient_matches {
                source_accounts.insert(detail.thread.account_id.clone());
                messages.push(ai::ContactMessageInput {
                    id: message.id,
                    thread_id: item.thread_id.clone(),
                    sender: message.sender,
                    sent_at: message.sent_at,
                    subject: detail.thread.subject.clone(),
                    body_text: message.body_text,
                    from_contact: sender_matches,
                    is_thread_starter: index == 0,
                });
            }
        }
    }
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    let mut purpose = ai::RequestPurpose {
        message_count: Some(messages.len()),
        ..ai::RequestPurpose::new("contact")
    };
    purpose.control = control;
    purpose.scope.account_ids = source_accounts.into_iter().collect();
    purpose.scope.record_id = Some(id);
    ai::with_purpose(
        purpose,
        ai::enrich_contact(
            ai::ContactEnrichmentRequest {
                provider,
                model,
                endpoint,
                reasoning: reasoning.unwrap_or_default(),
                profile,
                messages,
                search_more: search_more.unwrap_or(false),
                empty_fields,
            },
            &api_key,
        ),
    )
    .await
    .map_err(AppError::from)
}

#[tauri::command(async)]
pub(crate) fn ai_reply_assist_context(
    draft_id: String,
    state: State<'_, AppState>,
) -> AppResult<ReplyAssistContext> {
    let draft = state.correspondence.database.draft(&draft_id)?;
    if !matches!(draft.mode.as_str(), "reply" | "replyAll") {
        return Err("Reply Assist is only available for reply drafts".into());
    }
    let source_id = draft
        .source_id
        .ok_or_else(|| "Reply draft has no source message".to_string())?;
    let detail = state.database.get_thread_for_message(&source_id)?;
    let source_index = detail
        .messages
        .iter()
        .position(|message| message.id == source_id);
    Ok(ai::reply_context_for_message(
        detail.thread.subject,
        detail
            .messages
            .into_iter()
            .map(|message| ai::ThreadMessageInput {
                sender: message.sender,
                sent_at: message.sent_at,
                body_text: message.body_text,
            })
            .collect(),
        source_index,
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FindTimesInput {
    /// The message the draft replies to or forwards; its conversation is read for timing.
    source_message_id: Option<String>,
    draft_subject: String,
    draft_text: String,
    /// The user's adjustment to `current`, such as "afternoons only".
    instruction: Option<String>,
    current: Option<ai::find_times::TimeSearch>,
    user_time_zone: String,
}

/// Reads when a meeting should happen from the draft's conversation and the
/// draft itself, and returns the days and hours to search. The calendar is
/// searched by the app afterwards; the provider never sees it.
#[tauri::command]
pub(crate) async fn ai_find_times(
    request: FindTimesInput,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    state: State<'_, AppState>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<ai::find_times::TimeSearch> {
    let zone = (request.user_time_zone.chars().count() <= 100)
        .then(|| request.user_time_zone.parse::<chrono_tz::Tz>().ok())
        .flatten()
        .ok_or_else(|| format!("Unknown IANA timezone: {}", request.user_time_zone))?;
    let instruction = ai::find_times::validate_instruction(request.instruction.as_deref())?;
    if instruction.is_some() != request.current.is_some() {
        return Err("An adjustment needs the search it changes".into());
    }
    let (subject, messages, user_addresses) = match request.source_message_id {
        Some(source_id) => {
            let database = state.database.clone();
            let detail =
                run_database_task(move || database.get_thread_for_message(&source_id)).await?;
            let user_addresses = account_addresses(&state, &detail.thread.account_id)?;
            let messages = detail
                .messages
                .into_iter()
                .map(|message| ai::ActionMessageInput {
                    id: message.id,
                    sender: message.sender,
                    recipients: message.recipients,
                    sent_at: message.sent_at,
                    body_text: message.body_text,
                })
                .collect();
            (detail.thread.subject, messages, user_addresses)
        }
        None => (request.draft_subject, Vec::new(), Vec::new()),
    };
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    log::info!(target: "ai_find_times", "finding times with provider {provider:?} model {model}, {} message(s)", messages.len());
    let mut purpose = ai::RequestPurpose::about("find_times", &subject, Some(messages.len()));
    purpose.control = control;
    ai::with_purpose(
        purpose,
        ai::find_times::find_times(
            ai::find_times::FindTimesRequest {
                provider,
                model,
                endpoint,
                subject,
                messages,
                draft_text: request.draft_text,
                instruction,
                current: request.current,
                current_time: Utc::now().with_timezone(&zone).to_rfc3339(),
                user_time_zone: request.user_time_zone,
                user_addresses,
            },
            &api_key,
        ),
    )
    .await
    .inspect_err(|error| log::error!(target: "ai_find_times", "find times failed: {error}"))
    .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn ai_review_draft(
    request: ai::DraftReviewRequest,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<ai::DraftReviewResult> {
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    let mut purpose = ai::RequestPurpose::about("draft_review", &request.subject, None);
    purpose.control = control;
    ai::with_purpose(
        purpose,
        ai::review_draft(request, provider, &model, endpoint.as_deref(), &api_key),
    )
    .await
    .map_err(AppError::from)
}

/// Recent AI provider requests, newest first, for Settings → AI.
#[tauri::command(async)]
pub(crate) fn ai_activity(
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> AppResult<Vec<AiActivity>> {
    let limit = limit.unwrap_or(100).min(500) as usize;
    Ok(state.database.ai_activity(limit)?)
}

#[tauri::command]
pub(crate) async fn ai_generate_reply(
    context: ReplyAssistContext,
    instruction: String,
    provider: ai::AiProvider,
    model: String,
    endpoint: Option<String>,
    reasoning: Option<ai::Reasoning>,
    profile: Option<ai::ReplyWritingProfile>,
    control: Option<ai::requests::RequestControl>,
) -> AppResult<ReplyAssistResult> {
    let api_key = ai::get_key()?.ok_or_else(|| "No AI API key configured".to_string())?;
    // Re-apply the native bounds even though normal callers received this
    // context from `ai_reply_assist_context`; the webview is not trusted to
    // enforce request size or cost limits.
    let bounded = ai::reply_context_for_message(
        context.subject,
        context
            .messages
            .into_iter()
            .map(|message| ai::ThreadMessageInput {
                sender: message.sender,
                sent_at: message.sent_at,
                body_text: message.body_text,
            })
            .collect(),
        context.reply_to_message_index,
    );
    let mut purpose =
        ai::RequestPurpose::about("reply", &bounded.subject, Some(bounded.messages.len()));
    purpose.control = control;
    let body = ai::with_purpose(
        purpose,
        ai::generate_reply(
            &bounded,
            &instruction,
            &profile.unwrap_or_default(),
            provider,
            &model,
            endpoint.as_deref(),
            reasoning.unwrap_or_default(),
            &api_key,
        ),
    )
    .await?;
    Ok(ReplyAssistResult { body })
}

#[tauri::command(async)]
pub(crate) fn ai_diagnostics() -> ai::diagnostics::Snapshot {
    ai::diagnostics::snapshot()
}

#[tauri::command(async)]
pub(crate) fn set_ai_diagnostic_capture(enabled: bool) -> ai::diagnostics::Snapshot {
    ai::diagnostics::set_capture(enabled)
}

#[tauri::command(async)]
pub(crate) fn clear_ai_diagnostics() -> AppResult<ai::diagnostics::Snapshot> {
    ai::diagnostics::clear().map_err(AppError::from)
}

#[tauri::command(async)]
pub(crate) fn open_logs_folder(app: tauri::AppHandle) -> AppResult<()> {
    use tauri::Manager;
    use tauri_plugin_opener::OpenerExt;
    let path = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn ai_request_snapshot(
    include_history: Option<bool>,
) -> AppResult<ai::requests::RequestSnapshot> {
    ai::requests::RUNTIME
        .snapshot_limit(if include_history.unwrap_or(false) {
            20
        } else {
            0
        })
        .map_err(AppError::from)
}
#[tauri::command(async)]
pub(crate) fn set_ai_request_policy(policy: ai::requests::RequestPolicy) -> AppResult<()> {
    ai::requests::RUNTIME
        .configure(policy)
        .map_err(AppError::from)
}
#[tauri::command(async)]
pub(crate) fn cancel_ai_request(request_id: String) -> AppResult<()> {
    ai::requests::RUNTIME
        .cancel(&request_id)
        .map_err(AppError::from)
}

#[tauri::command(async)]
pub(crate) fn recover_ai_requests() -> AppResult<()> {
    ai::requests::RUNTIME.recover().map_err(AppError::from)
}
