use calcard::icalendar::{
    timezone::TzResolver, ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry,
    ICalendarParameterName, ICalendarParameterValue, ICalendarProperty, ICalendarValue,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use crate::{
    auth::OAuthCredential,
    models::{BusyInterval, CalendarOption, CreateCalendarEventRequest, ScheduleEvent, UpdateCalendarEventRequest},
};

const MAX_EVENTS: usize = 20;
pub(crate) const MAX_CALENDAR_BYTES: usize = 2 * 1024 * 1024;
const CALENDAR_LIST_URL: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";
const CALENDARS_URL: &str = "https://www.googleapis.com/calendar/v3/calendars/";
const FREEBUSY_URL: &str = "https://www.googleapis.com/calendar/v3/freeBusy";
const MAX_SCHEDULE_EVENTS: usize = 250;
/// The private extended property naming the email an event was added from.
/// Private properties live on this calendar's copy only: guests never see them.
const SOURCE_MESSAGE_PROPERTY: &str = "threestrandsSourceMessage";
/// Google's limit on an extended property value; a longer id is not stored.
const MAX_EXTENDED_PROPERTY_VALUE_BYTES: usize = 1024;
/// Bounds the attendee addresses kept per event; Google may list hundreds on
/// large meetings and the client only matches them against contacts.
const MAX_EVENT_ATTENDEES: usize = 100;
// Calendar IDs are untrusted path data. Encode every reserved URI character so
// IDs containing `@`, `#`, or `/` remain exactly one path segment.
const CALENDAR_ID_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

#[derive(Deserialize)]
struct GoogleCalendarList {
    #[serde(default)]
    items: Vec<GoogleCalendarListEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCalendarListEntry {
    id: String,
    summary: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    access_role: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCreateEvent<'a> {
    summary: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "is_empty_str")]
    location: &'a str,
    start: GoogleCreateEventTime<'a>,
    end: GoogleCreateEventTime<'a>,
    attendees: Vec<GoogleCreateAttendee<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extended_properties: Option<GoogleCreateExtendedProperties<'a>>,
}

#[derive(Serialize)]
struct GoogleCreateExtendedProperties<'a> {
    private: HashMap<&'static str, &'a str>,
}

fn is_empty_str(value: &&str) -> bool {
    value.is_empty()
}

/// All-day events set `date`; timed events set `dateTime`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCreateEventTime<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date_time: Option<&'a str>,
}

impl<'a> GoogleCreateEventTime<'a> {
    fn new(value: &'a str, all_day: bool) -> Self {
        if all_day {
            Self { date: Some(value), date_time: None }
        } else {
            Self { date: None, date_time: Some(value) }
        }
    }
}

fn create_body(request: &CreateCalendarEventRequest) -> GoogleCreateEvent<'_> {
    GoogleCreateEvent {
        summary: request.title.trim(),
        description: request.description.trim(),
        location: request.location.trim(),
        start: GoogleCreateEventTime::new(&request.start, request.all_day),
        end: GoogleCreateEventTime::new(&request.end, request.all_day),
        attendees: request.attendees.iter().map(|email| GoogleCreateAttendee { email }).collect(),
        extended_properties: request.source_message_id.as_deref().map(str::trim)
            .filter(|id| !id.is_empty() && id.len() <= MAX_EXTENDED_PROPERTY_VALUE_BYTES)
            .map(|id| GoogleCreateExtendedProperties { private: HashMap::from([(SOURCE_MESSAGE_PROPERTY, id)]) }),
    }
}

#[derive(Serialize)]
struct GoogleCreateAttendee<'a> { email: &'a str }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEvents {
    #[serde(default)]
    items: Vec<GoogleEvent>,
    /// The caller's access to the listed calendar, for example `owner` or `reader`.
    #[serde(default)]
    access_role: String,
}

fn writable_access_role(access_role: &str) -> bool {
    matches!(access_role, "owner" | "writer")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FreeBusyRequest<'a> {
    time_min: &'a str,
    time_max: &'a str,
    time_zone: &'a str,
    items: Vec<FreeBusyItem>,
}

#[derive(Serialize)]
struct FreeBusyItem {
    id: String,
}

#[derive(Deserialize)]
struct GoogleFreeBusy {
    #[serde(default)]
    calendars: HashMap<String, GoogleFreeBusyCalendar>,
}

#[derive(Deserialize)]
struct GoogleFreeBusyCalendar {
    #[serde(default)]
    busy: Vec<GoogleBusyInterval>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct GoogleBusyInterval {
    start: String,
    end: String,
}

/// Separate from the agenda model: availability only needs busy semantics,
/// locations and exact times, and must read every page of recurring instances.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveTimeEvents {
    #[serde(default)]
    items: Vec<DriveTimeEvent>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct DriveTimeEvent {
    status: Option<String>,
    transparency: Option<String>,
    location: Option<String>,
    start: Option<GoogleEventTime>,
    end: Option<GoogleEventTime>,
    #[serde(default)]
    attendees: Vec<GoogleAttendee>,
}

fn drive_time_interval(event: DriveTimeEvent, minutes: u32) -> Result<Option<BusyInterval>, String> {
    if minutes == 0 || event.status.as_deref() == Some("cancelled")
        || event.transparency.as_deref() == Some("transparent")
        || !event.location.as_deref().is_some_and(|location| !location.trim().is_empty())
        || event.attendees.iter().any(|attendee| attendee.is_self && attendee.response_status.as_deref() == Some("declined")) {
        return Ok(None);
    }
    let start = event.start.ok_or("Located calendar event has no start time")?;
    let end = event.end.ok_or("Located calendar event has no end time")?;
    // Date-only events have no departure or return time to buffer. FreeBusy
    // continues to determine whether their actual days block availability.
    if start.date.is_some() && end.date.is_some() { return Ok(None); }
    let start = start.date_time.ok_or("Located calendar event has no exact start time")?;
    let end = end.date_time.ok_or("Located calendar event has no exact end time")?;
    let (start, end) = crate::availability::buffered_range(&start, &end, minutes)?;
    Ok(Some(BusyInterval { start, end }))
}

/// Paginate independently of the agenda's display limit. The widened range
/// includes events just outside the requested slot whose travel overlaps it.
async fn fetch_drive_time_busy(
    client: &reqwest::Client,
    access_token: &str,
    calendar_id: &str,
    time_min: &str,
    time_max: &str,
    time_zone: &str,
    minutes: u32,
) -> Result<Vec<BusyInterval>, String> {
    let url = events_url(calendar_id)?.ok_or("Calendar has no ID")?;
    fetch_drive_time_pages(client, access_token, url, time_min, time_max, time_zone, minutes).await
}

async fn fetch_drive_time_pages(
    client: &reqwest::Client,
    access_token: &str,
    url: url::Url,
    time_min: &str,
    time_max: &str,
    time_zone: &str,
    minutes: u32,
) -> Result<Vec<BusyInterval>, String> {
    let (time_min, time_max) = crate::availability::buffered_range(time_min, time_max, minutes)?;
    let mut page_token = None;
    let mut seen_tokens = std::collections::HashSet::new();
    let mut busy = Vec::new();
    loop {
        let mut request = client.get(url.clone()).bearer_auth(access_token).query(&[
            ("timeMin", time_min.as_str()), ("timeMax", time_max.as_str()),
            ("timeZone", time_zone), ("singleEvents", "true"),
            ("maxResults", "2500"), ("showHiddenInvitations", "true"),
            ("fields", "nextPageToken,items(status,transparency,location,start,end,attendees(self,responseStatus))"),
        ]);
        if let Some(token) = &page_token { request = request.query(&[("pageToken", token)]); }
        let page: DriveTimeEvents = checked_json(request.send().await.map_err(|error| error.to_string())?).await?;
        for event in page.items {
            if let Some(interval) = drive_time_interval(event, minutes)? { busy.push(interval); }
        }
        match page.next_page_token.filter(|token| !token.is_empty()) {
            None => return Ok(busy),
            Some(token) => {
                if !seen_tokens.insert(token.clone()) { return Err("Calendar repeated an event page".into()); }
                page_token = Some(token);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct FreeBusyResult {
    pub busy: Vec<BusyInterval>,
    pub checked_calendar_count: usize,
    /// Selected calendars whose busy times could not be read.
    pub unchecked_calendar_ids: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEvent {
    id: String,
    summary: Option<String>,
    status: Option<String>,
    location: Option<String>,
    description: Option<String>,
    hangout_link: Option<String>,
    conference_data: Option<GoogleConferenceData>,
    start: GoogleEventTime,
    end: GoogleEventTime,
    #[serde(default)]
    attendees: Vec<GoogleAttendee>,
    organizer: Option<GoogleAttendee>,
    #[serde(default)]
    extended_properties: Option<GoogleExtendedProperties>,
}

#[derive(Deserialize)]
struct GoogleExtendedProperties {
    #[serde(default)]
    private: HashMap<String, String>,
}

#[derive(Deserialize)]
struct GoogleAttendee {
    email: Option<String>,
    #[serde(rename = "responseStatus")]
    response_status: Option<String>,
    #[serde(rename = "self", default)]
    is_self: bool,
    #[serde(default)]
    resource: bool,
}

fn self_response(attendees: &[GoogleAttendee], account_id: &str) -> (Option<String>, bool) {
    // `self` identifies this calendar's copy; a shared calendar's owner can
    // differ from the connected user whose RSVP the app is showing.
    let Some(attendee) = attendees.iter().find(|attendee| {
        attendee.is_self && attendee.email.as_deref().is_some_and(|email| email.eq_ignore_ascii_case(account_id))
    }) else {
        return (None, false);
    };
    (known_response(attendee).map(str::to_string), attendee.email.is_some())
}

fn known_response(attendee: &GoogleAttendee) -> Option<&str> {
    attendee.response_status.as_deref().filter(|status| matches!(*status, "accepted" | "declined" | "tentative" | "needsAction"))
}

/// The RSVP each of `people` gave. Google's organizer object carries no
/// response, so the organizer's comes from their attendee entry when listed.
fn attendee_responses(attendees: &[GoogleAttendee], people: &[String]) -> BTreeMap<String, String> {
    attendees
        .iter()
        .filter_map(|attendee| {
            let email = attendee.email.as_deref()?.trim().to_ascii_lowercase();
            let status = known_response(attendee)?;
            people.contains(&email).then(|| (email, status.to_string()))
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResponsePatch<'a> {
    attendees_omitted: bool,
    attendees: Vec<ResponseAttendee<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResponseAttendee<'a> {
    email: &'a str,
    response_status: &'a str,
}

/// The lowercased addresses of the other people on an event: the organizer
/// and attendees, excluding the calendar owner and rooms or other resources.
fn event_people(organizer: Option<&GoogleAttendee>, attendees: &[GoogleAttendee]) -> Vec<String> {
    let mut people: Vec<String> = Vec::new();
    for person in organizer.into_iter().chain(attendees) {
        if person.is_self || person.resource {
            continue;
        }
        let Some(email) = person.email.as_deref().map(|email| email.trim().to_ascii_lowercase()) else {
            continue;
        };
        if email.contains('@') && !people.contains(&email) {
            people.push(email);
            if people.len() == MAX_EVENT_ATTENDEES {
                break;
            }
        }
    }
    people
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleConferenceData {
    #[serde(default)]
    entry_points: Vec<GoogleConferenceEntryPoint>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleConferenceEntryPoint {
    entry_point_type: Option<String>,
    uri: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEventTime {
    date: Option<String>,
    date_time: Option<String>,
}

fn calendar_client() -> Result<reqwest::Client, String> {
    crate::http_client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|error| error.to_string())
}

pub async fn list_calendar_options(
    auth: OAuthCredential,
    account_id: &str,
) -> Result<Vec<CalendarOption>, String> {
    let access_token = auth
        .access_token()
        .await
        .map_err(|error| error.to_string())?;
    let max_results = MAX_SCHEDULE_EVENTS.to_string();
    let response = calendar_client()?
        .get(CALENDAR_LIST_URL)
        .bearer_auth(&access_token)
        .query(&[
            ("showHidden", "false"),
            ("minAccessRole", "reader"),
            ("maxResults", max_results.as_str()),
        ])
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let calendars: GoogleCalendarList = checked_json(response).await?;
    Ok(calendar_options(calendars.items, account_id))
}

fn calendar_options(
    entries: Vec<GoogleCalendarListEntry>,
    account_id: &str,
) -> Vec<CalendarOption> {
    entries
        .into_iter()
        .map(|entry| CalendarOption {
            id: entry.id,
            account_id: account_id.to_string(),
            name: entry.summary,
            primary: entry.primary,
            selected: false,
            writable: writable_access_role(&entry.access_role),
        })
        .collect()
}

pub async fn create_event(
    auth: OAuthCredential,
    request: &CreateCalendarEventRequest,
) -> Result<ScheduleEvent, String> {
    let url = events_url(&request.calendar_id)?
        .ok_or_else(|| "Choose a calendar for the event".to_string())?;
    let token = auth.access_token().await.map_err(|error| error.to_string())?;
    let response = calendar_client()?
        .post(url)
        .bearer_auth(&token)
        .query(&[("sendUpdates", "all")])
        .json(&create_body(request))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("Calendar write access was denied. Reconnect this calendar account in Settings to grant event access.".into());
    }
    let event: GoogleEvent = checked_json(response).await?;
    normalize_events(vec![event], &request.account_id, &request.calendar_id, true)
        .into_iter().next()
        .ok_or_else(|| "Google created an event but did not return its details. Refresh the calendar before trying again.".to_string())
}

pub async fn fetch_schedule(
    auth: OAuthCredential,
    account_id: &str,
    calendar_ids: &[String],
    time_min: &str,
    time_max: &str,
    time_zone: &str,
) -> Result<Vec<ScheduleEvent>, String> {
    let access_token = auth
        .access_token()
        .await
        .map_err(|error| error.to_string())?;
    let max_results = MAX_SCHEDULE_EVENTS.to_string();
    let client = calendar_client()?;
    let mut schedule = Vec::new();
    for calendar_id in calendar_ids {
        let Some(events_url) = events_url(calendar_id)? else {
            continue;
        };
        let response = client
            .get(events_url)
            .bearer_auth(&access_token)
            .query(&[
                ("timeMin", time_min),
                ("timeMax", time_max),
                ("timeZone", time_zone),
                ("singleEvents", "true"),
                ("orderBy", "startTime"),
                ("maxResults", max_results.as_str()),
            ])
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let events: GoogleEvents = checked_json(response).await?;
        let writable = writable_access_role(&events.access_role);
        schedule.extend(normalize_events(events.items, account_id, &calendar_id, writable));
    }
    schedule.sort_by(|left, right| left.start.cmp(&right.start));
    Ok(schedule)
}

/// Looks an invitation up by its iCalendar UID on an account's primary
/// calendar, where Google files the invitations it receives. Google's primary
/// calendar ID is the account address, which keeps the result's ID in the
/// same form the schedule uses.
pub async fn find_invitation(
    auth: OAuthCredential,
    account_id: &str,
    uid: &str,
) -> Result<Option<ScheduleEvent>, String> {
    let Some(url) = events_url(account_id)? else {
        return Ok(None);
    };
    let access_token = auth.access_token().await.map_err(|error| error.to_string())?;
    let response = calendar_client()?
        .get(url)
        .bearer_auth(&access_token)
        .query(&[("iCalUID", uid), ("maxResults", "1")])
        .send()
        .await
        .map_err(|error| error.to_string())?;
    Ok(invitation_from(checked_json(response).await?, account_id))
}

fn invitation_from(events: GoogleEvents, account_id: &str) -> Option<ScheduleEvent> {
    let writable = writable_access_role(&events.access_role);
    normalize_events(events.items, account_id, account_id, writable).into_iter().next()
}

/// Queries Google's FreeBusy endpoint once for all selected calendars on an
/// account. Calendar-level errors are returned as partial failures so callers
/// never mistake an incomplete response for confirmed availability.
pub async fn fetch_freebusy(
    auth: OAuthCredential,
    calendar_ids: &[String],
    time_min: &str,
    time_max: &str,
    time_zone: &str,
    drive_time_buffer_minutes: u32,
) -> Result<FreeBusyResult, String> {
    crate::availability::validate_drive_time_buffer(drive_time_buffer_minutes)?;
    if calendar_ids.is_empty() {
        return Ok(FreeBusyResult {
            busy: Vec::new(),
            checked_calendar_count: 0,
            unchecked_calendar_ids: Vec::new(),
            errors: Vec::new(),
        });
    }
    let access_token = auth
        .access_token()
        .await
        .map_err(|error| error.to_string())?;
    let client = calendar_client()?;
    let response = client
        .post(FREEBUSY_URL)
        .bearer_auth(&access_token)
        .json(&FreeBusyRequest {
            time_min: time_min,
            time_max,
            time_zone,
            items: calendar_ids
                .iter()
                .cloned()
                .map(|id| FreeBusyItem { id })
                .collect(),
        })
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let freebusy: GoogleFreeBusy = checked_json(response).await?;
    let mut result = freebusy_result(calendar_ids, freebusy);
    if drive_time_buffer_minutes > 0 {
        let readable = calendar_ids.iter().filter(|id| !result.unchecked_calendar_ids.contains(id));
        for calendar_id in readable {
            // FreeBusy already covered this calendar's events; drive time only
            // widens them, so a failure here keeps the calendar checked.
            match fetch_drive_time_busy(&client, &access_token, calendar_id, time_min, time_max, time_zone, drive_time_buffer_minutes).await {
                Ok(intervals) => result.busy.extend(intervals),
                Err(error) => log::warn!(target: "availability", "drive time for calendar {calendar_id} could not be read: {error}"),
            }
        }
        result.busy.sort_by(|left, right| left.start.cmp(&right.start));
    }
    Ok(result)
}

/// Busy times from a FreeBusy response, with each selected calendar it could
/// not answer for listed as unchecked.
fn freebusy_result(calendar_ids: &[String], freebusy: GoogleFreeBusy) -> FreeBusyResult {
    let mut busy = Vec::new();
    let mut checked = 0;
    let mut unchecked = Vec::new();
    let mut errors = Vec::new();
    for calendar_id in calendar_ids {
        match freebusy.calendars.get(calendar_id) {
            Some(calendar) if calendar.errors.is_empty() => {
                busy.extend(calendar.busy.iter().map(|interval| BusyInterval {
                    start: interval.start.clone(),
                    end: interval.end.clone(),
                }));
                checked += 1;
            }
            Some(_calendar) => {
                unchecked.push(calendar_id.clone());
                errors.push(format!("Calendar {calendar_id} returned a FreeBusy error"));
            }
            None => {
                unchecked.push(calendar_id.clone());
                errors.push(format!("Calendar {calendar_id} was not returned by FreeBusy"));
            }
        }
    }
    busy.sort_by(|left, right| left.start.cmp(&right.start));
    FreeBusyResult {
        busy,
        checked_calendar_count: checked,
        unchecked_calendar_ids: unchecked,
        errors,
    }
}

fn events_url(calendar_id: &str) -> Result<Option<url::Url>, String> {
    let calendar_id = calendar_id.trim();
    if calendar_id.is_empty() {
        return Ok(None);
    }
    let encoded_id = utf8_percent_encode(calendar_id, CALENDAR_ID_ENCODE_SET);
    url::Url::parse(&format!("{CALENDARS_URL}{encoded_id}/events"))
        .map(Some)
        .map_err(|error| error.to_string())
}

fn event_url(calendar_id: &str, event_id: &str) -> Result<url::Url, String> {
    let mut url = events_url(calendar_id)?
        .ok_or_else(|| "Choose a calendar for the event".to_string())?;
    if event_id.trim().is_empty() {
        return Err("Choose an event to respond to".into());
    }
    url.path_segments_mut()
        .map_err(|_| "Invalid calendar event URL".to_string())?
        .push(event_id);
    Ok(url)
}

pub async fn update_response(
    auth: OAuthCredential,
    account_id: &str,
    calendar_id: &str,
    schedule_id: &str,
    response_status: &str,
) -> Result<ScheduleEvent, String> {
    if !matches!(response_status, "accepted" | "declined" | "tentative") {
        return Err("Choose Yes, No, or Maybe".into());
    }
    let url = event_url(calendar_id, google_event_id(calendar_id, schedule_id)?)?;
    let token = auth.access_token().await.map_err(|error| error.to_string())?;
    let client = calendar_client()?;
    let current: GoogleEvent = checked_json(client.get(url.clone()).bearer_auth(&token).send().await.map_err(|error| error.to_string())?).await?;
    let email = current.attendees.iter().find(|attendee| {
        attendee.is_self && attendee.email.as_deref().is_some_and(|email| email.eq_ignore_ascii_case(account_id))
    })
        .and_then(|attendee| attendee.email.as_deref())
        .ok_or_else(|| "This event has no RSVP for your calendar".to_string())?;
    let response = client.patch(url).bearer_auth(&token)
        .json(&ResponsePatch {
            attendees_omitted: true,
            attendees: vec![ResponseAttendee { email, response_status }],
        })
        .send().await.map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("Calendar write access was denied. Reconnect this calendar account in Settings to grant event access.".into());
    }
    let event: GoogleEvent = checked_json(response).await?;
    // Google only accepts the patch on a calendar this account can write to.
    let mut updated = normalize_events(vec![event], account_id, calendar_id, true).into_iter().next()
        .ok_or_else(|| "Google changed the response but did not return event details. Refresh the calendar.".to_string())?;
    updated.response_status = Some(response_status.to_string());
    updated.can_respond = true;
    // The patch omits attendees, so the reply may too; an RSVP leaves the
    // other guests unchanged, so keep them from the event read above.
    updated.attendees = event_people(current.organizer.as_ref(), &current.attendees);
    updated.attendee_responses = attendee_responses(&current.attendees, &updated.attendees);
    Ok(updated)
}

/// Schedule IDs are `{calendar_id}:{google_event_id}` so events stay unique
/// across calendars; recover the Google ID and refuse IDs from another calendar.
fn google_event_id<'a>(calendar_id: &str, schedule_id: &'a str) -> Result<&'a str, String> {
    schedule_id.strip_prefix(&format!("{calendar_id}:"))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Event does not belong to this calendar".to_string())
}

/// Loads an event and refuses to continue unless this calendar organizes it,
/// so invitations on a writable calendar are never edited or deleted for
/// everyone else.
async fn owned_event(client: &reqwest::Client, token: &str, url: &url::Url) -> Result<serde_json::Value, String> {
    let current: serde_json::Value = checked_json(
        client.get(url.clone()).bearer_auth(token).send().await.map_err(|error| error.to_string())?,
    ).await?;
    if !organized_by_self(&current) {
        return Err("Only events you organize can be changed".into());
    }
    Ok(current)
}

fn organized_by_self(event: &serde_json::Value) -> bool {
    event.pointer("/organizer/self").and_then(serde_json::Value::as_bool) == Some(true)
}

/// Keeps the existing attendee records (and their RSVPs) for people who stay
/// invited, always keeps the calendar owner and booked rooms, and invites new
/// addresses.
fn merged_attendees(current: &[serde_json::Value], requested: &[String]) -> Vec<serde_json::Value> {
    let requested: Vec<String> = requested.iter().map(|email| email.trim().to_ascii_lowercase()).collect();
    let email_of = |attendee: &serde_json::Value| attendee.get("email").and_then(serde_json::Value::as_str).map(str::to_ascii_lowercase);
    let flag = |attendee: &serde_json::Value, name: &str| attendee.get(name).and_then(serde_json::Value::as_bool) == Some(true);
    let mut merged: Vec<serde_json::Value> = current.iter()
        .filter(|attendee| flag(attendee, "self") || flag(attendee, "resource")
            || email_of(attendee).is_some_and(|email| requested.contains(&email)))
        .cloned()
        .collect();
    for email in requested {
        if !merged.iter().any(|attendee| email_of(attendee).as_deref() == Some(email.as_str())) {
            merged.push(serde_json::json!({ "email": email }));
        }
    }
    merged
}

/// Google keeps `date` and `dateTime` side by side, so clear the other one
/// when an event moves between all-day and timed.
fn event_time_patch(value: &str, all_day: bool) -> serde_json::Value {
    if all_day {
        serde_json::json!({ "date": value, "dateTime": null })
    } else {
        serde_json::json!({ "dateTime": value, "date": null })
    }
}

fn update_patch(request: &UpdateCalendarEventRequest, current: &serde_json::Value) -> serde_json::Value {
    let current_attendees = current.get("attendees").and_then(serde_json::Value::as_array).map(Vec::as_slice).unwrap_or_default();
    serde_json::json!({
        "summary": request.title.trim(),
        "description": request.description.trim(),
        "location": request.location.trim(),
        "start": event_time_patch(&request.start, request.all_day),
        "end": event_time_patch(&request.end, request.all_day),
        "attendees": merged_attendees(current_attendees, &request.attendees),
    })
}

pub async fn update_event(
    auth: OAuthCredential,
    request: &UpdateCalendarEventRequest,
) -> Result<ScheduleEvent, String> {
    let url = event_url(&request.calendar_id, google_event_id(&request.calendar_id, &request.event_id)?)?;
    let token = auth.access_token().await.map_err(|error| error.to_string())?;
    let client = calendar_client()?;
    let current = owned_event(&client, &token, &url).await?;
    let response = client.patch(url).bearer_auth(&token)
        .query(&[("sendUpdates", "all")])
        .json(&update_patch(request, &current))
        .send().await.map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("Calendar write access was denied. Reconnect this calendar account in Settings to grant event access.".into());
    }
    let event: GoogleEvent = checked_json(response).await?;
    normalize_events(vec![event], &request.account_id, &request.calendar_id, true)
        .into_iter().next()
        .ok_or_else(|| "Google saved the event but did not return its details. Refresh the calendar.".to_string())
}

pub async fn delete_event(
    auth: OAuthCredential,
    calendar_id: &str,
    schedule_id: &str,
) -> Result<(), String> {
    let url = event_url(calendar_id, google_event_id(calendar_id, schedule_id)?)?;
    let token = auth.access_token().await.map_err(|error| error.to_string())?;
    let client = calendar_client()?;
    owned_event(&client, &token, &url).await?;
    let response = client.delete(url).bearer_auth(&token)
        .query(&[("sendUpdates", "all")])
        .send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    // Gone means another device already deleted it, which is the outcome asked for.
    if status.is_success() || status == reqwest::StatusCode::GONE {
        return Ok(());
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err("Calendar write access was denied. Reconnect this calendar account in Settings to grant event access.".into());
    }
    let body = response.text().await.unwrap_or_default();
    eprintln!("Google Calendar request failed with {status}: {body}");
    Err(format!("Google Calendar request failed ({status})."))
}

async fn checked_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    if response.status().is_success() {
        return response.json().await.map_err(|error| error.to_string());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    eprintln!("Google Calendar request failed with {status}: {body}");
    Err(format!("Google Calendar request failed ({status})."))
}

/// `writable` is whether this account can write to the calendar; only events
/// it also organizes become editable.
fn normalize_events(
    events: Vec<GoogleEvent>,
    account_id: &str,
    calendar_id: &str,
    writable: bool,
) -> Vec<ScheduleEvent> {
    events
        .into_iter()
        .filter(|event| event.status.as_deref() != Some("cancelled"))
        .filter_map(|event| {
            let all_day = event.start.date_time.is_none();
            let start = event.start.date_time.or(event.start.date)?;
            let end = event.end.date_time.or(event.end.date)?;
            let (response_status, can_respond) = self_response(&event.attendees, account_id);
            let organized_by_self = event.organizer.as_ref().is_some_and(|organizer| organizer.is_self);
            let attendees = event_people(event.organizer.as_ref(), &event.attendees);
            let attendee_responses = attendee_responses(&event.attendees, &attendees);
            Some(ScheduleEvent {
                id: format!("{calendar_id}:{}", event.id),
                account_id: account_id.to_string(),
                calendar_id: calendar_id.to_string(),
                title: event
                    .summary
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| "Untitled event".to_string()),
                start,
                end,
                all_day,
                location: event.location.filter(|value| !value.trim().is_empty()),
                description: event.description.filter(|value| !value.trim().is_empty()),
                attendees,
                attendee_responses,
                response_status,
                can_respond,
                can_edit: writable && organized_by_self,
                source_message_id: event.extended_properties
                    .and_then(|properties| properties.private.get(SOURCE_MESSAGE_PROPERTY).cloned())
                    .filter(|id| !id.trim().is_empty()),
                conference_url: event.hangout_link.or_else(|| {
                    event.conference_data.and_then(|conference| {
                        conference.entry_points.into_iter().find_map(|entry| {
                            (entry.entry_point_type.as_deref() == Some("video"))
                                .then_some(entry.uri)
                                .flatten()
                        })
                    })
                }),
            })
        })
        .collect()
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarPreview {
    pub events: Vec<CalendarEventPreview>,
    pub truncated: bool,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventPreview {
    pub uid: Option<String>,
    pub title: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub all_day: bool,
    /// Set only when the start's TZID could not be resolved. `start` and
    /// `end` are then wall-clock times in that zone, not exact moments, so
    /// the zone is shown beside them and they must not be scheduled as-is.
    /// A resolved zone instead gives `start` and `end` a UTC offset.
    pub time_zone: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub organizer: Option<String>,
    pub attendee_count: usize,
    pub recurring: bool,
    pub status: Option<String>,
}

pub fn parse(input: &[u8]) -> Result<CalendarPreview, String> {
    if input.len() > MAX_CALENDAR_BYTES {
        return Err("Calendar attachment is too large to preview".into());
    }
    let text = std::str::from_utf8(input).map_err(|_| "Calendar attachment is not valid UTF-8")?;
    let calendar = ICalendar::parse(text).map_err(|_| "Could not parse calendar attachment")?;
    // Resolves IANA and Windows zone names, and the file's own VTIMEZONE
    // definitions through their X-LIC-LOCATION or Exchange zone ID.
    let time_zones = calendar.build_tz_resolver();
    let mut all_events = calendar
        .components
        .iter()
        .filter(|component| component.component_type == ICalendarComponentType::VEvent);
    let events = all_events
        .by_ref()
        .take(MAX_EVENTS)
        .map(|event| event_preview(event, &time_zones))
        .collect::<Vec<_>>();
    let truncated = all_events.next().is_some();
    if events.is_empty() {
        return Err("Calendar attachment does not contain an event".into());
    }
    Ok(CalendarPreview { events, truncated })
}

fn event_preview(event: &ICalendarComponent, time_zones: &TzResolver<&str>) -> CalendarEventPreview {
    let start_entry = event.property(&ICalendarProperty::Dtstart);
    let start_value = start_entry.and_then(date_value);
    let (start, time_zone) = entry_time(start_entry, time_zones);
    let (end, _) = entry_time(event.property(&ICalendarProperty::Dtend), time_zones);
    CalendarEventPreview {
        uid: text_property(event, ICalendarProperty::Uid),
        title: text_property(event, ICalendarProperty::Summary)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Untitled event".into()),
        start,
        end,
        all_day: start_value.is_some_and(|value| value.hour.is_none()),
        time_zone,
        location: text_property(event, ICalendarProperty::Location),
        description: text_property(event, ICalendarProperty::Description),
        organizer: event
            .property(&ICalendarProperty::Organizer)
            .and_then(person_name),
        attendee_count: event.properties(&ICalendarProperty::Attendee).count(),
        recurring: event.is_recurrent(),
        status: text_property(event, ICalendarProperty::Status),
    }
}

/// A DTSTART or DTEND as display text: a date for all-day values, an exact
/// moment with a UTC offset for UTC, offset, or resolvable TZID times, and
/// otherwise local wall-clock time. The second value is a TZID that could not
/// be resolved, which leaves the time as wall-clock time in that zone.
fn entry_time(entry: Option<&ICalendarEntry>, time_zones: &TzResolver<&str>) -> (Option<String>, Option<String>) {
    let Some(entry) = entry else {
        return (None, None);
    };
    let Some(value) = date_value(entry) else {
        return (None, None);
    };
    let Some(tz_id) = parameter_text(entry, ICalendarParameterName::Tzid) else {
        return (display_date_time(value), None);
    };
    if value.hour.is_none() {
        return (display_date_time(value), None);
    }
    let resolved = time_zones
        .resolve(&tz_id)
        .filter(|zone| !zone.is_floating())
        .and_then(|zone| value.to_date_time_with_tz(zone));
    match resolved {
        Some(moment) => (
            Some(moment.fixed_offset().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            None,
        ),
        None => {
            let wall_clock = calcard::common::PartialDateTime { tz_hour: None, tz_minute: None, tz_minus: false, ..value.clone() };
            (display_date_time(&wall_clock), Some(tz_id))
        }
    }
}

fn display_date_time(value: &calcard::common::PartialDateTime) -> Option<String> {
    let date = format!("{:04}-{:02}-{:02}", value.year?, value.month?, value.day?);
    let Some(hour) = value.hour else {
        return Some(date);
    };
    let mut result = format!(
        "{date}T{hour:02}:{:02}:{:02}",
        value.minute.unwrap_or(0),
        value.second.unwrap_or(0)
    );
    if let Some(offset_hour) = value.tz_hour {
        if offset_hour == 0 && value.tz_minute.unwrap_or(0) == 0 && !value.tz_minus {
            result.push('Z');
        } else {
            result.push(if value.tz_minus { '-' } else { '+' });
            result.push_str(&format!(
                "{offset_hour:02}:{:02}",
                value.tz_minute.unwrap_or(0)
            ));
        }
    }
    Some(result)
}

fn text_property(event: &ICalendarComponent, property: ICalendarProperty) -> Option<String> {
    event
        .property(&property)
        .and_then(|entry| entry.values.first())
        .and_then(ICalendarValue::as_text)
        .map(str::to_string)
}

fn date_value(entry: &ICalendarEntry) -> Option<&calcard::common::PartialDateTime> {
    entry
        .values
        .first()
        .and_then(ICalendarValue::as_partial_date_time)
}

fn parameter_text(entry: &ICalendarEntry, name: ICalendarParameterName) -> Option<String> {
    match entry.parameter(&name) {
        Some(ICalendarParameterValue::Text(value)) => Some(value.clone()),
        Some(ICalendarParameterValue::Uri(value)) => value.as_str().map(str::to_string),
        _ => None,
    }
}

fn person_name(entry: &ICalendarEntry) -> Option<String> {
    parameter_text(entry, ICalendarParameterName::Cn).or_else(|| {
        entry
            .values
            .first()
            .and_then(ICalendarValue::as_text)
            .map(|value| {
                value
                    .strip_prefix("mailto:")
                    .or_else(|| value.strip_prefix("MAILTO:"))
                    .unwrap_or(value)
                    .to_string()
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_event_card_fields() {
        let preview = parse(
            concat!(
                "BEGIN:VCALENDAR\r\n",
                "VERSION:2.0\r\n",
                "BEGIN:VEVENT\r\n",
                "UID:planning@example.com\r\n",
                "SUMMARY:Quarterly planning\r\n",
                "DTSTART;TZID=America/New_York:20260918T093000\r\n",
                "DTEND;TZID=America/New_York:20260918T103000\r\n",
                "LOCATION:Room 4B\r\n",
                "DESCRIPTION:Review the roadmap\r\n",
                "ORGANIZER;CN=Jane Doe:mailto:jane@example.com\r\n",
                "ATTENDEE:mailto:alex@example.com\r\n",
                "RRULE:FREQ=MONTHLY\r\n",
                "STATUS:CONFIRMED\r\n",
                "END:VEVENT\r\n",
                "END:VCALENDAR\r\n"
            )
            .as_bytes(),
        )
        .unwrap();

        assert_eq!(preview.events.len(), 1);
        let event = &preview.events[0];
        assert_eq!(event.uid.as_deref(), Some("planning@example.com"));
        assert_eq!(event.title, "Quarterly planning");
        assert_eq!(event.start.as_deref(), Some("2026-09-18T09:30:00-04:00"));
        assert_eq!(event.end.as_deref(), Some("2026-09-18T10:30:00-04:00"));
        assert_eq!(event.time_zone, None);
        assert_eq!(event.location.as_deref(), Some("Room 4B"));
        assert_eq!(event.organizer.as_deref(), Some("Jane Doe"));
        assert_eq!(event.attendee_count, 1);
        assert!(event.recurring);
        assert_eq!(event.status.as_deref(), Some("CONFIRMED"));
    }

    fn times_of(ics: &str) -> (Option<String>, Option<String>, Option<String>) {
        let event = parse(ics.as_bytes()).unwrap().events.remove(0);
        (event.start, event.end, event.time_zone)
    }

    fn event_with(lines: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:zone@example.com\r\nSUMMARY:Zone check\r\n{lines}END:VEVENT\r\nEND:VCALENDAR\r\n")
    }

    #[test]
    fn converts_iana_and_windows_zone_names_to_exact_moments() {
        // Google and Apple send IANA names; Outlook and Exchange send Windows names.
        for zone in ["America/New_York", "Eastern Standard Time"] {
            assert_eq!(
                times_of(&event_with(&format!("DTSTART;TZID={zone}:20260918T093000\r\nDTEND;TZID={zone}:20260918T103000\r\n"))),
                (Some("2026-09-18T09:30:00-04:00".into()), Some("2026-09-18T10:30:00-04:00".into()), None),
                "{zone}"
            );
        }
    }

    #[test]
    fn applies_the_zone_offset_in_effect_on_the_event_date() {
        assert_eq!(
            times_of(&event_with("DTSTART;TZID=America/New_York:20261210T093000\r\n")).0.as_deref(),
            Some("2026-12-10T09:30:00-05:00")
        );
    }

    #[test]
    fn resolves_a_zone_defined_inside_the_file() {
        let ics = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
            "BEGIN:VTIMEZONE\r\nTZID:Office Time\r\nX-LIC-LOCATION:Europe/Berlin\r\n",
            "BEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\n",
            "BEGIN:DAYLIGHT\r\nDTSTART:19700329T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nEND:DAYLIGHT\r\n",
            "END:VTIMEZONE\r\n",
            "BEGIN:VEVENT\r\nUID:berlin@example.com\r\nSUMMARY:Standup\r\n",
            "DTSTART;TZID=Office Time:20260918T093000\r\nDTEND;TZID=Office Time:20260918T094500\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        assert_eq!(
            times_of(ics),
            (Some("2026-09-18T09:30:00+02:00".into()), Some("2026-09-18T09:45:00+02:00".into()), None)
        );
    }

    #[test]
    fn resolves_start_and_end_zones_independently() {
        assert_eq!(
            times_of(&event_with("DTSTART;TZID=America/New_York:20260918T090000\r\nDTEND;TZID=Europe/London:20260918T150000\r\n")),
            (Some("2026-09-18T09:00:00-04:00".into()), Some("2026-09-18T15:00:00+01:00".into()), None)
        );
    }

    #[test]
    fn keeps_wall_clock_time_and_reports_a_zone_it_cannot_resolve() {
        assert_eq!(
            times_of(&event_with("DTSTART;TZID=Olympus Mons Time:20260918T093000\r\nDTEND;TZID=Olympus Mons Time:20260918T103000\r\n")),
            (Some("2026-09-18T09:30:00".into()), Some("2026-09-18T10:30:00".into()), Some("Olympus Mons Time".into()))
        );
    }

    #[test]
    fn keeps_utc_floating_and_all_day_values_as_written() {
        assert_eq!(times_of(&event_with("DTSTART:20260918T133000Z\r\n")).0.as_deref(), Some("2026-09-18T13:30:00Z"));
        assert_eq!(times_of(&event_with("DTSTART:20260918T093000\r\n")), (Some("2026-09-18T09:30:00".into()), None, None));
        assert_eq!(
            times_of(&event_with("DTSTART;VALUE=DATE;TZID=America/New_York:20260918\r\n")),
            (Some("2026-09-18".into()), None, None)
        );
    }

    #[test]
    fn rejects_calendars_without_events() {
        assert_eq!(
            parse(b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n"),
            Err("Calendar attachment does not contain an event".into())
        );
    }

    #[test]
    fn normalizes_timed_and_all_day_google_events() {
        let events = normalize_events(
            vec![
                GoogleEvent {
                    id: "timed".into(),
                    summary: Some("Planning".into()),
                    status: Some("confirmed".into()),
                    location: Some("Room 4B".into()),
                    description: Some("Review the roadmap".into()),
                    hangout_link: Some("https://meet.google.com/abc-defg-hij".into()),
                    conference_data: None,
                    start: GoogleEventTime {
                        date: None,
                        date_time: Some("2026-09-18T09:30:00-07:00".into()),
                    },
                    end: GoogleEventTime {
                        date: None,
                        date_time: Some("2026-09-18T10:00:00-07:00".into()),
                    },
                    attendees: vec![
                        GoogleAttendee { email: Some("work@example.com".into()), response_status: Some("needsAction".into()), is_self: true, resource: false },
                        GoogleAttendee { email: Some("Jane@Example.com".into()), response_status: Some("accepted".into()), is_self: false, resource: false },
                        GoogleAttendee { email: Some("bob@example.com".into()), response_status: Some("declined".into()), is_self: false, resource: false },
                        GoogleAttendee { email: Some("kim@example.com".into()), response_status: Some("delegated".into()), is_self: false, resource: false },
                        GoogleAttendee { email: Some("room-4b@resource.example.com".into()), response_status: Some("accepted".into()), is_self: false, resource: true },
                        GoogleAttendee { email: None, response_status: None, is_self: false, resource: false },
                    ],
                    organizer: Some(GoogleAttendee { email: Some("jane@example.com".into()), response_status: None, is_self: false, resource: false }),
                    extended_properties: None,
                },
                GoogleEvent {
                    id: "all-day".into(),
                    summary: None,
                    status: None,
                    location: None,
                    description: None,
                    hangout_link: None,
                    conference_data: None,
                    start: GoogleEventTime {
                        date: Some("2026-09-18".into()),
                        date_time: None,
                    },
                    end: GoogleEventTime {
                        date: Some("2026-09-19".into()),
                        date_time: None,
                    },
                    attendees: Vec::new(),
                    organizer: None,
                    extended_properties: None,
                },
            ],
            "work@example.com",
            "team@example.com",
            true,
        );

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "team@example.com:timed");
        assert_eq!(events[0].title, "Planning");
        assert!(!events[0].all_day);
        assert_eq!(events[0].location.as_deref(), Some("Room 4B"));
        assert_eq!(events[0].description.as_deref(), Some("Review the roadmap"));
        assert_eq!(
            events[0].conference_url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert_eq!(events[0].attendees, vec!["jane@example.com", "bob@example.com", "kim@example.com"]);
        // The user's own and the room's RSVPs are left out, as are unknown statuses.
        assert_eq!(
            events[0].attendee_responses,
            BTreeMap::from([("bob@example.com".to_string(), "declined".to_string()), ("jane@example.com".to_string(), "accepted".to_string())])
        );
        assert_eq!(events[0].response_status.as_deref(), Some("needsAction"));
        assert!(events[0].can_respond);
        assert_eq!(events[0].calendar_id, "team@example.com");
        assert_eq!(events[1].title, "Untitled event");
        assert!(events[1].all_day);
        assert!(events[1].attendees.is_empty());
        assert!(events[1].attendee_responses.is_empty());
        assert_eq!(events[1].response_status, None);
        assert!(!events[1].can_respond);
        // An invitation and an event without a known organizer are not editable.
        assert!(!events[0].can_edit);
        assert!(!events[1].can_edit);
    }

    fn organized_event(organizer_self: bool) -> GoogleEvent {
        serde_json::from_value(serde_json::json!({
            "id": "owned",
            "summary": "Planning",
            "start": {"dateTime": "2026-09-18T09:30:00-07:00"},
            "end": {"dateTime": "2026-09-18T10:00:00-07:00"},
            "organizer": {"email": "work@example.com", "self": organizer_self}
        })).unwrap()
    }

    #[test]
    fn only_self_organized_events_on_writable_calendars_are_editable() {
        let editable = |organizer_self, writable| {
            normalize_events(vec![organized_event(organizer_self)], "work@example.com", "primary", writable)[0].can_edit
        };
        assert!(editable(true, true));
        assert!(!editable(true, false));
        assert!(!editable(false, true));
        assert!(!editable(false, false));
    }

    #[test]
    fn events_list_access_role_controls_editability() {
        let list: GoogleEvents = serde_json::from_value(serde_json::json!({ "accessRole": "reader", "items": [] })).unwrap();
        assert!(!writable_access_role(&list.access_role));
        let list: GoogleEvents = serde_json::from_value(serde_json::json!({ "accessRole": "writer" })).unwrap();
        assert!(writable_access_role(&list.access_role));
        let list: GoogleEvents = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!writable_access_role(&list.access_role));
    }

    #[test]
    fn an_invitation_found_by_uid_can_be_answered_on_the_primary_calendar() {
        let events: GoogleEvents = serde_json::from_value(serde_json::json!({
            "accessRole": "owner",
            "items": [{
                "id": "evt123",
                "summary": "Launch review",
                "start": { "dateTime": "2026-10-12T15:00:00Z" },
                "end": { "dateTime": "2026-10-12T16:00:00Z" },
                "organizer": { "email": "lead@example.com" },
                "attendees": [
                    { "email": "lead@example.com", "responseStatus": "accepted" },
                    { "email": "me@example.com", "self": true, "responseStatus": "needsAction" }
                ]
            }]
        })).unwrap();
        let event = invitation_from(events, "me@example.com").unwrap();
        assert_eq!(event.calendar_id, "me@example.com");
        assert_eq!(google_event_id(&event.calendar_id, &event.id), Ok("evt123"));
        assert_eq!(event.response_status.as_deref(), Some("needsAction"));
        assert!(event.can_respond);
        assert!(!event.can_edit);
    }

    #[test]
    fn a_cancelled_or_missing_invitation_is_not_found() {
        let missing: GoogleEvents = serde_json::from_value(serde_json::json!({ "accessRole": "owner", "items": [] })).unwrap();
        assert!(invitation_from(missing, "me@example.com").is_none());
        let cancelled: GoogleEvents = serde_json::from_value(serde_json::json!({
            "accessRole": "owner",
            "items": [{ "id": "gone", "status": "cancelled", "start": { "date": "2026-10-12" }, "end": { "date": "2026-10-13" } }]
        })).unwrap();
        assert!(invitation_from(cancelled, "me@example.com").is_none());
    }

    #[test]
    fn recognizes_only_events_this_calendar_organizes() {
        assert!(organized_by_self(&serde_json::json!({ "organizer": { "email": "me@example.com", "self": true } })));
        assert!(!organized_by_self(&serde_json::json!({ "organizer": { "email": "boss@example.com" } })));
        assert!(!organized_by_self(&serde_json::json!({ "organizer": { "self": "true" } })));
        assert!(!organized_by_self(&serde_json::json!({})));
    }

    #[test]
    fn schedule_ids_must_belong_to_the_named_calendar() {
        assert_eq!(google_event_id("primary", "primary:abc"), Ok("abc"));
        assert!(google_event_id("primary", "other:abc").is_err());
        assert!(google_event_id("primary", "primary:").is_err());
    }

    #[test]
    fn attendee_merge_keeps_rsvps_owner_and_rooms_and_invites_new_people() {
        let current = vec![
            serde_json::json!({ "email": "me@example.com", "self": true, "organizer": true, "responseStatus": "accepted" }),
            serde_json::json!({ "email": "Jane@Example.com", "responseStatus": "accepted", "comment": "See you" }),
            serde_json::json!({ "email": "removed@example.com", "responseStatus": "declined" }),
            serde_json::json!({ "email": "room@resource.example.com", "resource": true, "responseStatus": "accepted" }),
        ];
        let merged = merged_attendees(&current, &["jane@example.com".into(), " New@Example.com ".into()]);
        assert_eq!(merged, vec![
            current[0].clone(),
            current[1].clone(),
            current[3].clone(),
            serde_json::json!({ "email": "new@example.com" }),
        ]);
        assert!(merged_attendees(&[], &[]).is_empty());
    }

    fn update_request(all_day: bool) -> UpdateCalendarEventRequest {
        UpdateCalendarEventRequest {
            account_id: "work@example.com".into(), calendar_id: "primary".into(), event_id: "primary:owned".into(),
            title: " Planning ".into(),
            start: if all_day { "2026-09-18".into() } else { "2026-09-18T09:00:00Z".into() },
            end: if all_day { "2026-09-19".into() } else { "2026-09-18T10:00:00Z".into() },
            all_day, location: " Room 4B ".into(), description: " Agenda ".into(), attendees: vec!["jane@example.com".into()],
        }
    }

    fn create_request(all_day: bool, location: &str) -> CreateCalendarEventRequest {
        CreateCalendarEventRequest {
            account_id: "work@example.com".into(), calendar_id: "primary".into(),
            title: " Bonfire ".into(),
            start: if all_day { "2026-11-07".into() } else { "2026-10-17T18:45:00-04:00".into() },
            end: if all_day { "2026-11-08".into() } else { "2026-10-17T20:45:00-04:00".into() },
            all_day, location: location.into(), description: " From the email ".into(), attendees: Vec::new(),
            source_message_id: None,
        }
    }

    #[test]
    fn create_body_sends_one_time_kind_and_an_optional_location() {
        let timed = serde_json::to_value(create_body(&create_request(false, " 224 Fairoaks St "))).unwrap();
        assert_eq!(timed["summary"], "Bonfire");
        assert_eq!(timed["location"], "224 Fairoaks St");
        assert_eq!(timed["start"], serde_json::json!({ "dateTime": "2026-10-17T18:45:00-04:00" }));
        assert_eq!(timed["end"], serde_json::json!({ "dateTime": "2026-10-17T20:45:00-04:00" }));
        let all_day = serde_json::to_value(create_body(&create_request(true, " "))).unwrap();
        assert_eq!(all_day["start"], serde_json::json!({ "date": "2026-11-07" }));
        assert_eq!(all_day["end"], serde_json::json!({ "date": "2026-11-08" }));
        assert!(all_day.get("location").is_none());
    }

    #[test]
    fn create_body_tags_the_source_email_privately_within_googles_limit() {
        let tagged = |id: Option<&str>| {
            let request = CreateCalendarEventRequest { source_message_id: id.map(str::to_string), ..create_request(false, "") };
            serde_json::to_value(create_body(&request)).unwrap().get("extendedProperties").cloned()
        };
        assert_eq!(tagged(Some(" message-1 ")), Some(serde_json::json!({ "private": { "threestrandsSourceMessage": "message-1" } })));
        assert_eq!(tagged(None), None);
        assert_eq!(tagged(Some("  ")), None);
        let at_limit = "m".repeat(MAX_EXTENDED_PROPERTY_VALUE_BYTES);
        assert!(tagged(Some(&at_limit)).is_some());
        assert!(tagged(Some(&format!("{at_limit}m"))).is_none());
        assert!(tagged(Some(&"m".repeat(MAX_EXTENDED_PROPERTY_VALUE_BYTES - 1))).is_some());
    }

    #[test]
    fn reads_the_source_email_tag_and_ignores_other_properties() {
        let event = |properties: serde_json::Value| -> Option<String> {
            let event: GoogleEvent = serde_json::from_value(serde_json::json!({
                "id": "tagged", "summary": "Bonfire",
                "start": {"dateTime": "2026-10-17T18:45:00-04:00"}, "end": {"dateTime": "2026-10-17T20:45:00-04:00"},
                "extendedProperties": properties,
            })).unwrap();
            normalize_events(vec![event], "work@example.com", "primary", true).remove(0).source_message_id
        };
        assert_eq!(event(serde_json::json!({ "private": { "threestrandsSourceMessage": "imap:work@example.com:abc" } })).as_deref(), Some("imap:work@example.com:abc"));
        // Shared properties are visible to every guest; only the private one counts.
        assert_eq!(event(serde_json::json!({ "shared": { "threestrandsSourceMessage": "message-1" } })), None);
        assert_eq!(event(serde_json::json!({ "private": { "otherApp": "message-1" } })), None);
        assert_eq!(event(serde_json::json!({ "private": { "threestrandsSourceMessage": " " } })), None);
        assert!(normalize_events(vec![organized_event(true)], "work@example.com", "primary", true)[0].source_message_id.is_none());
    }

    #[test]
    fn update_patch_sets_fields_and_clears_the_other_time_kind() {
        let current = serde_json::json!({ "attendees": [{ "email": "jane@example.com", "responseStatus": "accepted" }] });
        let timed = update_patch(&update_request(false), &current);
        assert_eq!(timed["summary"], "Planning");
        assert_eq!(timed["location"], "Room 4B");
        assert_eq!(timed["description"], "Agenda");
        assert_eq!(timed["start"], serde_json::json!({ "dateTime": "2026-09-18T09:00:00Z", "date": null }));
        assert_eq!(timed["end"], serde_json::json!({ "dateTime": "2026-09-18T10:00:00Z", "date": null }));
        assert_eq!(timed["attendees"], serde_json::json!([{ "email": "jane@example.com", "responseStatus": "accepted" }]));
        let all_day = update_patch(&update_request(true), &serde_json::json!({}));
        assert_eq!(all_day["start"], serde_json::json!({ "date": "2026-09-18", "dateTime": null }));
        assert_eq!(all_day["end"], serde_json::json!({ "date": "2026-09-19", "dateTime": null }));
        assert_eq!(all_day["attendees"], serde_json::json!([{ "email": "jane@example.com" }]));
    }

    #[test]
    fn reads_only_the_calendar_owners_response_and_builds_a_scoped_patch() {
        for status in ["accepted", "declined", "tentative", "needsAction"] {
            let attendees: Vec<GoogleAttendee> = serde_json::from_value(serde_json::json!([
                {"email": "guest@example.com", "responseStatus": "accepted"},
                {"email": "me@example.com", "self": true, "responseStatus": status}
            ])).unwrap();
            assert_eq!(self_response(&attendees, "me@example.com"), (Some(status.into()), true));
            assert_eq!(self_response(&attendees, "other@example.com"), (None, false));
        }
        let attendees: Vec<GoogleAttendee> = serde_json::from_value(serde_json::json!([
            {"email": "guest@example.com", "responseStatus": "accepted"}
        ])).unwrap();
        assert_eq!(self_response(&attendees, "me@example.com"), (None, false));
        let patch = serde_json::to_value(ResponsePatch {
            attendees_omitted: true,
            attendees: vec![ResponseAttendee { email: "me@example.com", response_status: "tentative" }],
        }).unwrap();
        assert_eq!(patch, serde_json::json!({
            "attendeesOmitted": true,
            "attendees": [{"email": "me@example.com", "responseStatus": "tentative"}]
        }));
        assert_eq!(event_url("team@example.com", "event/one").unwrap().as_str(),
            "https://www.googleapis.com/calendar/v3/calendars/team%40example.com/events/event%2Fone");
    }

    #[test]
    fn reads_event_people_from_google_json_and_bounds_them() {
        let event: GoogleEvent = serde_json::from_value(serde_json::json!({
            "id": "e1",
            "start": {"dateTime": "2026-09-18T09:30:00Z"},
            "end": {"dateTime": "2026-09-18T10:00:00Z"},
            "organizer": {"email": "me@example.com", "self": true},
            "attendees": [{"email": "bob@example.com", "responseStatus": "accepted"}, {"email": "not-an-address"}],
        }))
        .unwrap();
        assert_eq!(event_people(event.organizer.as_ref(), &event.attendees), vec!["bob@example.com"]);
        let without_people: GoogleEvent = serde_json::from_value(serde_json::json!({
            "id": "e2", "start": {"date": "2026-09-18"}, "end": {"date": "2026-09-19"},
        }))
        .unwrap();
        assert!(event_people(without_people.organizer.as_ref(), &without_people.attendees).is_empty());

        let attendees = |count: usize| {
            (0..count)
                .map(|index| GoogleAttendee { email: Some(format!("person{index}@example.com")), response_status: None, is_self: false, resource: false })
                .collect::<Vec<_>>()
        };
        assert_eq!(event_people(None, &attendees(MAX_EVENT_ATTENDEES - 1)).len(), MAX_EVENT_ATTENDEES - 1);
        assert_eq!(event_people(None, &attendees(MAX_EVENT_ATTENDEES)).len(), MAX_EVENT_ATTENDEES);
        assert_eq!(event_people(None, &attendees(MAX_EVENT_ATTENDEES + 1)).len(), MAX_EVENT_ATTENDEES);
    }

    #[test]
    fn maps_calendar_list_entries_for_selection_ui() {
        let options = calendar_options(
            vec![
                GoogleCalendarListEntry {
                    id: "primary@example.com".into(),
                    summary: "My calendar".into(),
                    primary: true,
                    access_role: "owner".into(),
                },
                GoogleCalendarListEntry {
                    id: "team@example.com".into(),
                    summary: "Team".into(),
                    primary: false,
                    access_role: "reader".into(),
                },
            ],
            "work@example.com",
        );

        assert_eq!(options.len(), 2);
        assert_eq!(options[0].name, "My calendar");
        assert!(options[0].primary);
        assert!(options[0].writable);
        assert!(!options[1].writable);
        assert_eq!(options[1].account_id, "work@example.com");
    }

    #[test]
    fn google_calendar_access_role_controls_writable_options() {
        let entry: GoogleCalendarListEntry = serde_json::from_value(serde_json::json!({
            "id": "team@example.com", "summary": "Team", "accessRole": "writer"
        })).unwrap();
        assert!(calendar_options(vec![entry], "work@example.com")[0].writable);
    }

    #[test]
    fn create_payload_preserves_times_description_and_attendees() {
        let payload = GoogleCreateEvent {
            summary: "Planning",
            description: "Agenda",
            location: "",
            start: GoogleCreateEventTime::new("2026-09-22T09:00:00Z", false),
            end: GoogleCreateEventTime::new("2026-09-22T10:00:00Z", false),
            attendees: vec![GoogleCreateAttendee { email: "guest@example.com" }],
            extended_properties: None,
        };
        assert_eq!(serde_json::to_value(payload).unwrap(), serde_json::json!({
            "summary": "Planning", "description": "Agenda",
            "start": { "dateTime": "2026-09-22T09:00:00Z" },
            "end": { "dateTime": "2026-09-22T10:00:00Z" },
            "attendees": [{ "email": "guest@example.com" }],
        }));
    }

    #[test]
    fn builds_encoded_event_urls_without_empty_segments() {
        let url = events_url("person@example.com").unwrap().unwrap();
        assert_eq!(
            url.as_str(),
            "https://www.googleapis.com/calendar/v3/calendars/person%40example.com/events"
        );

        let group_url = events_url("team#contacts@group.v.calendar.google.com")
            .unwrap()
            .unwrap();
        assert!(group_url
            .as_str()
            .ends_with("team%23contacts%40group.v.calendar.google.com/events"));
        assert!(!group_url.path().contains("calendars//"));
    }

    #[test]
    fn does_not_build_an_event_url_for_an_empty_calendar_id() {
        assert_eq!(events_url("").unwrap(), None);
        assert_eq!(events_url("   ").unwrap(), None);
    }

    fn located_event() -> serde_json::Value {
        serde_json::json!({
            "location": "123 Main Street",
            "start": {"dateTime": "2099-09-21T14:00:00-04:00"},
            "end": {"dateTime": "2099-09-21T15:00:00-04:00"}
        })
    }

    #[test]
    fn drive_time_blocks_haircut_departure_and_return_with_exact_boundaries_free() {
        let event = serde_json::from_value(located_event()).unwrap();
        let interval = drive_time_interval(event, 30).unwrap().unwrap();
        assert_eq!(interval.start, "2099-09-21T17:30:00+00:00");
        assert_eq!(interval.end, "2099-09-21T19:30:00+00:00");
        let busy = [interval];
        for (start, end, expected) in [
            ("17:00", "17:30", "free"),
            ("17:30", "18:00", "conflicting"),
            ("19:00", "19:30", "conflicting"),
            ("19:30", "20:00", "free"),
            ("17:15", "17:45", "conflicting"),
        ] {
            assert_eq!(crate::availability::check_time(&format!("2099-09-21T{start}:00Z"), &format!("2099-09-21T{end}:00Z"), &busy, 1, 1).unwrap(), expected);
        }
        let preferences = crate::models::AvailabilityPreferences {
            time_zone: "America/New_York".into(),
            working_windows: vec![crate::models::AvailabilityWindow { weekday: 1, start: "13:00".into(), end: "16:00".into() }],
            default_duration_minutes: 30, slot_increment_minutes: 30, drive_time_buffer_minutes: 30,
        };
        let slots = crate::availability::find_candidates("2099-09-21T17:00:00Z", "2099-09-21T20:00:00Z", &preferences, &busy, 1, 1, None).unwrap();
        assert_eq!(slots.iter().map(|slot| slot.start.as_str()).collect::<Vec<_>>(), vec!["2099-09-21T17:00:00+00:00", "2099-09-21T19:30:00+00:00"]);
    }

    #[test]
    fn drive_time_ignores_unlocated_nonbusy_and_all_day_events() {
        let mut cases = vec![located_event(); 7];
        cases[0]["location"] = serde_json::Value::Null;
        cases[1]["location"] = serde_json::json!("  ");
        cases[2]["status"] = serde_json::json!("cancelled");
        cases[2].as_object_mut().unwrap().remove("start");
        cases[3]["transparency"] = serde_json::json!("transparent");
        cases[4]["attendees"] = serde_json::json!([{"self":true,"responseStatus":"declined"}]);
        cases[5]["start"] = serde_json::json!({"date":"2099-09-21"});
        cases[5]["end"] = serde_json::json!({"date":"2099-09-22"});
        for (index, case) in cases.into_iter().enumerate() {
            let minutes = if index == 6 { 0 } else { 30 };
            assert!(drive_time_interval(serde_json::from_value(case).unwrap(), minutes).unwrap().is_none());
        }
        let mut malformed = located_event();
        malformed["end"]["dateTime"] = serde_json::json!("invalid");
        assert!(drive_time_interval(serde_json::from_value(malformed).unwrap(), 30).is_err());
    }

    #[tokio::test]
    async fn drive_time_fetches_every_page_and_widens_the_query_for_neighboring_events() {
        use axum::{extract::Query, routing::get, Json, Router};
        async fn page(Query(query): Query<HashMap<String, String>>) -> Json<serde_json::Value> {
            assert_eq!(query["timeMin"], "2099-09-21T17:00:00+00:00");
            assert_eq!(query["timeMax"], "2099-09-21T20:00:00+00:00");
            assert_eq!(query["singleEvents"], "true");
            if query.contains_key("pageToken") {
                assert_eq!(query["pageToken"], "second");
                Json(serde_json::json!({"items":[located_event()]}))
            } else {
                // A short (even empty) page is not the end of the results.
                Json(serde_json::json!({"items":[],"nextPageToken":"second"}))
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = url::Url::parse(&format!("http://{}/events", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, Router::new().route("/events", get(page))).await.unwrap() });
        let busy = fetch_drive_time_pages(&reqwest::Client::new(), "test-token", url, "2099-09-21T17:30:00Z", "2099-09-21T19:30:00Z", "America/New_York", 30).await.unwrap();
        server.abort();
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].end, "2099-09-21T19:30:00+00:00");
    }

    #[test]
    fn freebusy_request_uses_google_field_names() {
        let request = FreeBusyRequest {
            time_min: "2026-09-21T00:00:00Z",
            time_max: "2026-09-22T00:00:00Z",
            time_zone: "America/New_York",
            items: vec![FreeBusyItem { id: "primary".to_string() }],
        };
        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["timeMin"], "2026-09-21T00:00:00Z");
        assert!(json.get("time_min").is_none());
    }

    #[test]
    fn freebusy_result_lists_calendars_it_could_not_read() {
        let freebusy: GoogleFreeBusy = serde_json::from_value(serde_json::json!({
            "calendars": {
                "me@example.com": { "busy": [{ "start": "2026-09-21T14:00:00Z", "end": "2026-09-21T15:00:00Z" }] },
                "team@group.calendar.google.com": { "errors": [{ "domain": "global", "reason": "notFound" }] }
            }
        }))
        .unwrap();
        let ids = ["me@example.com", "team@group.calendar.google.com", "missing@group.calendar.google.com"].map(String::from);
        let result = freebusy_result(&ids, freebusy);
        assert_eq!(result.checked_calendar_count, 1);
        assert_eq!(result.unchecked_calendar_ids, ["team@group.calendar.google.com", "missing@group.calendar.google.com"]);
        assert_eq!(result.busy.len(), 1);
        assert_eq!(result.errors.len(), 2);
    }

    #[test]
    fn freebusy_result_checks_every_calendar_that_answered() {
        let freebusy: GoogleFreeBusy = serde_json::from_value(serde_json::json!({
            "calendars": { "me@example.com": { "busy": [] }, "family@group.calendar.google.com": { "busy": [] } }
        }))
        .unwrap();
        let ids = ["me@example.com", "family@group.calendar.google.com"].map(String::from);
        let result = freebusy_result(&ids, freebusy);
        assert_eq!(result.checked_calendar_count, 2);
        assert!(result.unchecked_calendar_ids.is_empty());
        assert!(result.errors.is_empty());
    }
}
