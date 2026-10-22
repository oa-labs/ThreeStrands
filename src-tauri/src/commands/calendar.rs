//! Calendar accounts, events, availability, and invitations.

use std::collections::HashSet;

use crate::auth::GoogleConnection;
use crate::db::Database;
use crate::models::{
    BusyInterval, CalendarAccount, CalendarOption, CheckProposedTimeRequest,
    CreateCalendarEventRequest, FindAvailabilityRequest, ProposedTimeCheck, ScheduleEvent,
    ScheduleResult, UpdateCalendarEventRequest,
};
use tauri::State;

use super::accounts::NOT_SYNC_ENROLLED;
use crate::{auth, availability, calendar, limits, models};
use crate::{authorize_interactively, database_result, record_synced_value, AppError, AppResult, AppState};

#[tauri::command(async)]
pub(crate) fn list_calendar_accounts(state: State<'_, AppState>) -> AppResult<Vec<CalendarAccount>> {
    let config = state.auth_config.google().ok();
    let mut accounts = state.database.list_calendar_accounts()?;
    for account in &mut accounts {
        if !config.is_some_and(|config| config.calendar_account(&account.email).available()) {
            account.status = "needs_reauth".to_string();
        }
    }
    Ok(accounts)
}

#[tauri::command]
pub(crate) async fn add_calendar_account(state: State<'_, AppState>) -> AppResult<CalendarAccount> {
    let config = state.auth_config.google()?;
    let auth = config.pending_calendar_account();
    let email = authorize_interactively(&state, &auth).await?;
    state.database.adopt_calendar_account(&email)?;
    let account = state
        .database
        .list_calendar_accounts()?
        .into_iter()
        .find(|account| account.email == email)
        .ok_or_else(|| "Calendar account was not saved".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::CalendarAccount, &email.to_ascii_lowercase(), &serde_json::json!({"email":email,"sortOrder":account.sort_order}), None)?;
    Ok(account)
}

#[tauri::command(async)]
pub(crate) fn reorder_calendar_accounts(emails: Vec<String>, state: State<'_, AppState>) -> AppResult<()> {
    state.database.reorder_calendar_accounts(&emails)?;
    for account in state.database.list_calendar_accounts()? {
        record_synced_value(&state, threestrands_sync_protocol::EntityType::CalendarAccount, &account.email.to_ascii_lowercase(),
            &serde_json::json!({"email":account.email,"sortOrder":account.sort_order}),
            Some(std::collections::BTreeSet::from(["sortOrder".to_string()])))?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn reconnect_calendar_account(
    email: String,
    state: State<'_, AppState>,
) -> AppResult<CalendarAccount> {
    let config = state.auth_config.google()?;
    let auth = config.calendar_account(&email);
    authorize_interactively(&state, &auth).await?;
    state.database.adopt_calendar_account(&email)?;
    let account = state
        .database
        .list_calendar_accounts()?
        .into_iter()
        .find(|account| account.email == email)
        .ok_or_else(|| "Calendar account was not saved".to_string())?;
    record_synced_value(&state, threestrands_sync_protocol::EntityType::CalendarAccount, &email.to_ascii_lowercase(), &serde_json::json!({"email":email}), None)?;
    Ok(account)
}

/// The calendar accounts this device can query. Replicated sync materializes
/// an account another device connected, and "Disconnect" on an enrolled
/// device keeps the row, so a row can exist without a credential here.
/// Reads that span every account skip those rows rather than letting one
/// account this device never authorized fail the rest; Settings lists them
/// with Reconnect.
fn connected_calendar_accounts(config: &auth::OAuthApp, database: &Database) -> AppResult<Vec<CalendarAccount>> {
    Ok(usable_calendar_accounts(database.list_calendar_accounts()?, |email| {
        config.calendar_account(email).available()
    }))
}

fn usable_calendar_accounts(
    accounts: Vec<CalendarAccount>,
    has_credential: impl Fn(&str) -> bool,
) -> Vec<CalendarAccount> {
    accounts
        .into_iter()
        .filter(|account| account.status == "connected" && has_credential(&account.email))
        .collect()
}

async fn calendar_options_for_account(
    config: &auth::OAuthApp,
    database: &Database,
    email: &str,
) -> AppResult<Vec<CalendarOption>> {
    let mut options =
        calendar::list_calendar_options(config.calendar_account(email), email).await?;
    let selected = match database.calendar_selection(email)? {
        Some(selected) => selected,
        None => {
            let defaults = options
                .iter()
                .filter(|option| option.primary)
                .map(|option| option.id.clone())
                .collect::<Vec<_>>();
            database.set_calendar_selection(email, &defaults)?;
            defaults
        }
    };
    let selected = selected.into_iter().collect::<HashSet<_>>();
    for option in &mut options {
        option.selected = selected.contains(&option.id);
    }
    Ok(options)
}

async fn selected_calendar_ids(
    config: &auth::OAuthApp,
    database: &Database,
    email: &str,
) -> AppResult<Vec<String>> {
    match database.calendar_selection(email)? {
        Some(selected) => Ok(selected),
        None => Ok(calendar_options_for_account(config, database, email)
            .await?
            .into_iter()
            .filter(|option| option.selected)
            .map(|option| option.id)
            .collect()),
    }
}

async fn freebusy_coverage(
    time_min: &str,
    time_max: &str,
    time_zone: &str,
    drive_time_buffer_minutes: u32,
    state: &AppState,
) -> AppResult<(Vec<BusyInterval>, usize, usize, Vec<String>)> {
    let Ok(config) = state.auth_config.google() else {
        return Ok((Vec::new(), 0, 0, Vec::new()));
    };
    let mut busy = Vec::new();
    let mut checked = 0;
    let mut total = 0;
    let mut errors = Vec::new();
    for account in connected_calendar_accounts(config, &state.database)? {
        let calendar_ids = match selected_calendar_ids(config, &state.database, &account.email).await {
            Ok(ids) => ids,
            Err(error) => {
                errors.push(format!("{}: {error}", account.email));
                continue;
            }
        };
        total += calendar_ids.len();
        if calendar_ids.is_empty() {
            continue;
        }
        match calendar::fetch_freebusy(
            config.calendar_account(&account.email),
            &calendar_ids,
            time_min,
            time_max,
            time_zone,
            drive_time_buffer_minutes,
        )
        .await
        {
            Ok(result) => {
                checked += result.checked_calendar_count;
                busy.extend(result.busy);
                errors.extend(result.errors.into_iter().map(|error| format!("{}: {error}", account.email)));
            }
            Err(error) => errors.push(format!("{}: {error}", account.email)),
        }
    }
    // Preserve an explicit partial state when an account/calendar discovery
    // call failed before it could tell us how many calendars were selected.
    if !errors.is_empty() && checked >= total {
        total = checked + 1;
    }
    Ok((busy, checked, total, errors))
}

#[tauri::command]
pub(crate) async fn list_calendar_options(
    state: State<'_, AppState>,
) -> AppResult<Vec<CalendarOption>> {
    let config = state.auth_config.google()?;
    let mut options = Vec::new();
    for account in connected_calendar_accounts(config, &state.database)? {
        options.extend(
            calendar_options_for_account(config, &state.database, &account.email).await?,
        );
    }
    Ok(options)
}

fn validate_calendar_event_request(request: &CreateCalendarEventRequest) -> AppResult<()> {
    if request.location.len() > 4096 {
        return Err("Event location is too long".into());
    }
    validate_calendar_event_fields(&request.title, &request.description, &request.start, &request.end, request.all_day, &request.attendees)
}

fn validate_calendar_event_update(request: &UpdateCalendarEventRequest) -> AppResult<()> {
    if request.location.len() > 4096 {
        return Err("Event location is too long".into());
    }
    validate_calendar_event_fields(&request.title, &request.description, &request.start, &request.end, request.all_day, &request.attendees)
}

/// All-day events use `YYYY-MM-DD` dates with an exclusive end; timed events use RFC 3339.
fn validate_calendar_event_fields(
    title: &str,
    description: &str,
    start: &str,
    end: &str,
    all_day: bool,
    attendees: &[String],
) -> AppResult<()> {
    if title.trim().is_empty() || title.len() > 1024 {
        return Err("Enter an event title of at most 1024 characters".into());
    }
    if description.len() > 32_768 {
        return Err("Event description is too long".into());
    }
    let ordered = if all_day {
        let start = chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d")
            .map_err(|_| "Enter a valid event start date".to_string())?;
        let end = chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d")
            .map_err(|_| "Enter a valid event end date".to_string())?;
        end > start
    } else {
        let start = chrono::DateTime::parse_from_rfc3339(start)
            .map_err(|_| "Enter a valid event start time".to_string())?;
        let end = chrono::DateTime::parse_from_rfc3339(end)
            .map_err(|_| "Enter a valid event end time".to_string())?;
        end > start
    };
    if !ordered {
        return Err("Event end must be after its start".into());
    }
    if attendees.len() > 100 || attendees.iter().any(|email| {
        email.len() > 254 || email.contains(char::is_whitespace)
            || !email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
    }) {
        return Err("Enter up to 100 valid attendee email addresses".into());
    }
    Ok(())
}

#[cfg(test)]
mod calendar_event_request_tests {
    use super::*;

    fn request() -> CreateCalendarEventRequest {
        CreateCalendarEventRequest {
            account_id: "work@example.com".into(), calendar_id: "primary".into(),
            title: "Planning".into(), start: "2026-09-22T09:00:00Z".into(),
            end: "2026-09-22T10:00:00Z".into(), all_day: false, location: String::new(),
            description: "Agenda".into(), attendees: vec!["guest@example.com".into()],
            source_message_id: None,
        }
    }

    #[test]
    fn validates_event_times_and_invitees_before_the_network_request() {
        assert!(validate_calendar_event_request(&request()).is_ok());
        let mut invalid = request();
        invalid.end = invalid.start.clone();
        assert!(validate_calendar_event_request(&invalid).is_err());
        invalid = request();
        invalid.attendees = vec!["not-an-email".into()];
        assert!(validate_calendar_event_request(&invalid).is_err());
        invalid = request();
        invalid.title = " ".into();
        assert!(validate_calendar_event_request(&invalid).is_err());
    }

    #[test]
    fn validates_all_day_events_and_locations_on_create() {
        let mut all_day = request();
        all_day.all_day = true;
        all_day.start = "2026-11-07".into();
        all_day.end = "2026-11-08".into();
        assert!(validate_calendar_event_request(&all_day).is_ok());
        // An all-day event must not carry timed values, and needs an exclusive end.
        let mut invalid = all_day.clone();
        invalid.start = "2026-11-07T09:00:00Z".into();
        assert!(validate_calendar_event_request(&invalid).is_err());
        invalid = all_day.clone();
        invalid.end = invalid.start.clone();
        assert!(validate_calendar_event_request(&invalid).is_err());
        let mut located = request();
        located.location = "x".repeat(4096);
        assert!(validate_calendar_event_request(&located).is_ok());
        located.location = "x".repeat(4097);
        assert!(validate_calendar_event_request(&located).is_err());
    }

    #[test]
    fn create_requests_from_earlier_clients_default_to_timed_without_location() {
        let request: CreateCalendarEventRequest = serde_json::from_value(serde_json::json!({
            "accountId": "work@example.com", "calendarId": "primary", "title": "Planning",
            "start": "2026-09-22T09:00:00Z", "end": "2026-09-22T10:00:00Z",
            "description": "", "attendees": [],
        })).unwrap();
        assert!(!request.all_day);
        assert!(request.location.is_empty());
        assert!(request.source_message_id.is_none());
        assert!(validate_calendar_event_request(&request).is_ok());
    }

    #[test]
    fn create_requests_carry_the_email_they_came_from() {
        let request: CreateCalendarEventRequest = serde_json::from_value(serde_json::json!({
            "accountId": "work@example.com", "calendarId": "primary", "title": "Planning",
            "start": "2026-09-22T09:00:00Z", "end": "2026-09-22T10:00:00Z",
            "description": "", "attendees": [], "sourceMessageId": "message-1",
        })).unwrap();
        assert_eq!(request.source_message_id.as_deref(), Some("message-1"));
        assert!(validate_calendar_event_request(&request).is_ok());
    }

    fn update(all_day: bool) -> UpdateCalendarEventRequest {
        UpdateCalendarEventRequest {
            account_id: "work@example.com".into(), calendar_id: "primary".into(), event_id: "primary:owned".into(),
            title: "Planning".into(),
            start: if all_day { "2026-09-22".into() } else { "2026-09-22T09:00:00Z".into() },
            end: if all_day { "2026-09-23".into() } else { "2026-09-22T10:00:00Z".into() },
            all_day, location: "Room 4B".into(), description: "Agenda".into(),
            attendees: vec!["guest@example.com".into()],
        }
    }

    #[test]
    fn validates_event_updates_for_timed_and_all_day_events() {
        assert!(validate_calendar_event_update(&update(false)).is_ok());
        assert!(validate_calendar_event_update(&update(true)).is_ok());
        let mut invalid = update(true);
        invalid.end = invalid.start.clone();
        assert!(validate_calendar_event_update(&invalid).is_err());
        // An all-day update must not carry timed values, and the reverse.
        invalid = update(true);
        invalid.start = "2026-09-22T09:00:00Z".into();
        assert!(validate_calendar_event_update(&invalid).is_err());
        invalid = update(false);
        invalid.start = "2026-09-22".into();
        assert!(validate_calendar_event_update(&invalid).is_err());
        invalid = update(false);
        invalid.location = "x".repeat(4097);
        assert!(validate_calendar_event_update(&invalid).is_err());
        invalid = update(false);
        invalid.location = "x".repeat(4096);
        assert!(validate_calendar_event_update(&invalid).is_ok());
        invalid = update(false);
        invalid.attendees = vec!["not-an-email".into()];
        assert!(validate_calendar_event_update(&invalid).is_err());
    }
}

#[tauri::command]
pub(crate) async fn create_calendar_event(
    request: CreateCalendarEventRequest,
    state: State<'_, AppState>,
) -> AppResult<ScheduleEvent> {
    validate_calendar_event_request(&request)?;
    let account = state.database.list_calendar_accounts()?.into_iter()
        .find(|account| account.email == request.account_id && account.status == "connected")
        .ok_or_else(|| "Connect this calendar account in Settings first".to_string())?;
    let config = state.auth_config.google()?;
    let options = calendar::list_calendar_options(config.calendar_account(&account.email), &account.email).await?;
    if !options.iter().any(|option| option.id == request.calendar_id && option.writable) {
        return Err("Choose a calendar where you can create events".into());
    }
    calendar::create_event(config.calendar_account(&account.email), &request).await
        .map_err(AppError::from)
}

/// Finds the connected account for an edit and confirms it can still write to
/// the event's calendar; `calendar::*_event` then confirms it organizes the event.
async fn writable_calendar_account(
    state: &State<'_, AppState>,
    account_id: &str,
    calendar_id: &str,
) -> AppResult<String> {
    let account = state.database.list_calendar_accounts()?.into_iter()
        .find(|account| account.email == account_id && account.status == "connected")
        .ok_or_else(|| "Connect this calendar account in Settings first".to_string())?;
    let config = state.auth_config.google()?;
    let options = calendar::list_calendar_options(config.calendar_account(&account.email), &account.email).await?;
    if !options.iter().any(|option| option.id == calendar_id && option.writable) {
        return Err("You can only change events on calendars you can edit".into());
    }
    Ok(account.email)
}

#[tauri::command]
pub(crate) async fn update_calendar_event(
    request: UpdateCalendarEventRequest,
    state: State<'_, AppState>,
) -> AppResult<ScheduleEvent> {
    validate_calendar_event_update(&request)?;
    let email = writable_calendar_account(&state, &request.account_id, &request.calendar_id).await?;
    let config = state.auth_config.google()?;
    calendar::update_event(config.calendar_account(&email), &request).await
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn delete_calendar_event(
    account_id: String,
    calendar_id: String,
    event_id: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let email = writable_calendar_account(&state, &account_id, &calendar_id).await?;
    let config = state.auth_config.google()?;
    calendar::delete_event(config.calendar_account(&email), &calendar_id, &event_id).await
        .map_err(AppError::from)
}

#[tauri::command]
pub(crate) async fn set_calendar_selection(
    account_id: String,
    calendar_ids: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<Vec<CalendarOption>> {
    let config = state.auth_config.google()?;
    let mut available =
        calendar::list_calendar_options(config.calendar_account(&account_id), &account_id).await?;
    let available_ids = available
        .iter()
        .map(|option| option.id.as_str())
        .collect::<HashSet<_>>();
    let unique = calendar_ids.iter().collect::<HashSet<_>>();
    if unique.len() != calendar_ids.len()
        || calendar_ids
            .iter()
            .any(|calendar_id| !available_ids.contains(calendar_id.as_str()))
    {
        return Err("Calendar selection contained an unknown or duplicate calendar".into());
    }
    state
        .database
        .set_calendar_selection(&account_id, &calendar_ids)?;
    record_synced_value(
        &state,
        threestrands_sync_protocol::EntityType::CalendarSelection,
        &account_id.to_ascii_lowercase(),
        &serde_json::json!({ "accountId": account_id, "calendarIds": calendar_ids }),
        None,
    )?;
    for option in &mut available {
        option.selected = unique.contains(&option.id);
    }
    Ok(available)
}

#[tauri::command]
pub(crate) async fn remove_calendar_account(email: String, state: State<'_, AppState>) -> AppResult<()> {
    let config = state.auth_config.google()?;
    let enrolled = state.database.cross_device_sync_enrolled()?;
    // Removing it from this device only keeps the grant other devices rely
    // on; see remove_account_internal.
    if !enrolled {
        state
            .auth_config
            .revoke_google_grant(&email, GoogleConnection::Calendar)
            .await?;
    }
    config.calendar_account(&email).disconnect()?;
    if enrolled {
        database_result(state.database.disconnect_calendar_account_locally(&email))
    } else {
        database_result(state.database.remove_calendar_account(&email))
    }
}

#[tauri::command]
pub(crate) async fn remove_synced_calendar_account(email: String, state: State<'_, AppState>) -> AppResult<()> {
    if !state.database.cross_device_sync_enrolled()? {
        return Err(NOT_SYNC_ENROLLED.into());
    }
    state
        .auth_config
        .revoke_google_grant(&email, GoogleConnection::Calendar)
        .await?;
    state.database.record_local_entity_deletion(threestrands_sync_protocol::EntityType::CalendarAccount, &email.to_ascii_lowercase())?;
    state.replicated_sync.sync_once().await?;
    let config = state.auth_config.google()?;
    config.calendar_account(&email).disconnect()?;
    database_result(state.database.remove_calendar_account(&email))
}

#[tauri::command]
pub(crate) async fn list_schedule_events(
    time_min: String,
    time_max: String,
    time_zone: String,
    state: State<'_, AppState>,
) -> AppResult<ScheduleResult> {
    let config = state.auth_config.google()?;
    let accounts = connected_calendar_accounts(config, &state.database)?;
    if accounts.is_empty() {
        return Err("Connect a Google Calendar account in Settings first.".into());
    }
    let mut merged = Vec::new();
    let mut errors = Vec::new();
    for account in accounts {
        let calendar_ids = match state.database.calendar_selection(&account.email)? {
            Some(selected) => selected,
            None => match calendar_options_for_account(
                config,
                &state.database,
                &account.email,
            )
            .await
            {
                Ok(options) => options
                    .into_iter()
                    .filter(|option| option.selected)
                    .map(|option| option.id)
                    .collect(),
                Err(error) => {
                    errors.push(format!("{}: {error}", account.email));
                    continue;
                }
            },
        };
        if calendar_ids.is_empty() {
            continue;
        }
        match calendar::fetch_schedule(
            config.calendar_account(&account.email),
            &account.email,
            &calendar_ids,
            &time_min,
            &time_max,
            &time_zone,
        )
        .await
        {
            Ok(mut events) => {
                merged.append(&mut events);
            }
            Err(error) => errors.push(format!("{}: {error}", account.email)),
        }
    }
    merged.sort_by(|left, right| left.start.cmp(&right.start));
    Ok(ScheduleResult {
        events: merged,
        errors,
    })
}

#[tauri::command]
pub(crate) async fn update_calendar_response(
    account_id: String,
    calendar_id: String,
    event_id: String,
    response_status: String,
    state: State<'_, AppState>,
) -> AppResult<models::ScheduleEvent> {
    let config = state.auth_config.google()?;
    if !state.database.list_calendar_accounts()?.iter().any(|account| account.email == account_id) {
        return Err("Calendar account is not connected".into());
    }
    calendar::update_response(
        config.calendar_account(&account_id),
        &account_id,
        &calendar_id,
        &event_id,
        &response_status,
    )
    .await
    .map_err(AppError::from)
}

/// Finds an opened invitation on the connected calendars by its iCalendar
/// UID, so the user can RSVP to the copy their calendar already holds. Ok(None)
/// means every connected calendar was checked and none has it.
#[tauri::command]
pub(crate) async fn find_calendar_invitation(
    uid: String,
    state: State<'_, AppState>,
) -> AppResult<Option<models::ScheduleEvent>> {
    let uid = uid.trim();
    if uid.is_empty() || uid.len() > limits::MAX_CALENDAR_UID_BYTES {
        return Err("This invitation has no usable event ID".into());
    }
    let config = state.auth_config.google()?;
    let mut errors = Vec::new();
    for account in connected_calendar_accounts(config, &state.database)? {
        match calendar::find_invitation(config.calendar_account(&account.email), &account.email, uid).await {
            Ok(Some(event)) => return Ok(Some(event)),
            Ok(None) => {}
            Err(error) => errors.push(format!("{}: {error}", account.email)),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(errors.join("; ").into())
    }
}

#[tauri::command]
pub(crate) async fn find_availability(
    request: FindAvailabilityRequest,
    state: State<'_, AppState>,
) -> AppResult<models::AvailabilityResult> {
    availability::validate_preferences(&request.preferences)?;
    availability::buffered_range(&request.range_start, &request.range_end, request.preferences.drive_time_buffer_minutes)?;
    let (busy, checked, total, errors) = freebusy_coverage(
        &request.range_start,
        &request.range_end,
        &request.preferences.time_zone,
        request.preferences.drive_time_buffer_minutes,
        &state,
    )
    .await?;
    let candidates = availability::find_candidates(
        &request.range_start,
        &request.range_end,
        &request.preferences,
        &busy,
        checked,
        total,
        request.max_per_day,
    )?;
    Ok(models::AvailabilityResult {
        candidates,
        checked_calendar_count: checked,
        total_calendar_count: total,
        errors,
    })
}

#[tauri::command]
pub(crate) async fn check_proposed_time(
    request: CheckProposedTimeRequest,
    state: State<'_, AppState>,
) -> AppResult<ProposedTimeCheck> {
    if request.time_zone.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!("Unknown IANA timezone: {}", request.time_zone).into());
    }
    availability::buffered_range(&request.start, &request.end, request.drive_time_buffer_minutes)?;
    let (busy, checked, total, errors) = freebusy_coverage(
        &request.start,
        &request.end,
        &request.time_zone,
        request.drive_time_buffer_minutes,
        &state,
    )
    .await?;
    let status = availability::check_time(&request.start, &request.end, &busy, checked, total)?;
    let start = chrono::DateTime::parse_from_rfc3339(&request.start)
        .map_err(|_| "Proposed time must be RFC3339 timestamps".to_string())?
        .with_timezone(&chrono::Utc);
    let end = chrono::DateTime::parse_from_rfc3339(&request.end)
        .map_err(|_| "Proposed time must be RFC3339 timestamps".to_string())?
        .with_timezone(&chrono::Utc);
    let conflicts = busy
        .into_iter()
        .filter(|interval| {
            let Ok(interval_start) = chrono::DateTime::parse_from_rfc3339(&interval.start) else { return false; };
            let Ok(interval_end) = chrono::DateTime::parse_from_rfc3339(&interval.end) else { return false; };
            interval_start.with_timezone(&chrono::Utc) < end && interval_end.with_timezone(&chrono::Utc) > start
        })
        .collect();
    Ok(ProposedTimeCheck {
        status,
        conflicts,
        checked_calendar_count: checked,
        total_calendar_count: total,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(email: &str, status: &str) -> CalendarAccount {
        CalendarAccount { email: email.into(), connected_at: "2026-10-19T00:00:00Z".into(), status: status.into(), sort_order: 0 }
    }

    #[test]
    fn multi_account_reads_skip_accounts_without_a_credential_on_this_device() {
        let accounts = vec![
            account("work@example.com", "connected"),
            // Materialized from another device; never authorized here.
            account("synced@example.com", "needs_reauth"),
            // Marked connected, but its keychain entry is missing.
            account("lost@example.com", "connected"),
            account("home@example.com", "connected"),
        ];
        let usable = usable_calendar_accounts(accounts, |email| email != "lost@example.com" && email != "synced@example.com");
        let emails = usable.iter().map(|account| account.email.as_str()).collect::<Vec<_>>();
        assert_eq!(emails, ["work@example.com", "home@example.com"]);
    }

    #[test]
    fn locally_disconnected_account_is_skipped_even_if_a_credential_lingers() {
        let usable = usable_calendar_accounts(vec![account("left@example.com", "needs_reauth")], |_| true);
        assert!(usable.is_empty());
    }
}
