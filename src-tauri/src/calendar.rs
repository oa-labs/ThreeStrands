use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarParameterName,
    ICalendarParameterValue, ICalendarProperty, ICalendarValue,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

use crate::{
    auth::GoogleAuth,
    models::{BusyInterval, CalendarOption, CreateCalendarEventRequest, ScheduleEvent},
};

const MAX_EVENTS: usize = 20;
const MAX_CALENDAR_BYTES: usize = 2 * 1024 * 1024;
const CALENDAR_LIST_URL: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";
const CALENDARS_URL: &str = "https://www.googleapis.com/calendar/v3/calendars/";
const FREEBUSY_URL: &str = "https://www.googleapis.com/calendar/v3/freeBusy";
const MAX_SCHEDULE_EVENTS: usize = 250;
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
    start: GoogleCreateEventTime<'a>,
    end: GoogleCreateEventTime<'a>,
    attendees: Vec<GoogleCreateAttendee<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCreateEventTime<'a> { date_time: &'a str }

#[derive(Serialize)]
struct GoogleCreateAttendee<'a> { email: &'a str }

#[derive(Deserialize)]
struct GoogleEvents {
    #[serde(default)]
    items: Vec<GoogleEvent>,
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

#[derive(Debug, Clone)]
pub struct FreeBusyResult {
    pub busy: Vec<BusyInterval>,
    pub checked_calendar_count: usize,
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
    let status = attendee.response_status.as_deref().filter(|status| matches!(*status, "accepted" | "declined" | "tentative" | "needsAction"));
    (status.map(str::to_string), attendee.email.is_some())
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
fn event_people(organizer: Option<GoogleAttendee>, attendees: Vec<GoogleAttendee>) -> Vec<String> {
    let mut people: Vec<String> = Vec::new();
    for person in organizer.into_iter().chain(attendees) {
        if person.is_self || person.resource {
            continue;
        }
        let Some(email) = person.email.map(|email| email.trim().to_ascii_lowercase()) else {
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
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|error| error.to_string())
}

pub async fn list_calendar_options(
    auth: GoogleAuth,
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
            writable: matches!(entry.access_role.as_str(), "owner" | "writer"),
        })
        .collect()
}

pub async fn create_event(
    auth: GoogleAuth,
    request: &CreateCalendarEventRequest,
) -> Result<ScheduleEvent, String> {
    let url = events_url(&request.calendar_id)?
        .ok_or_else(|| "Choose a calendar for the event".to_string())?;
    let token = auth.access_token().await.map_err(|error| error.to_string())?;
    let response = calendar_client()?
        .post(url)
        .bearer_auth(&token)
        .query(&[("sendUpdates", "all")])
        .json(&GoogleCreateEvent {
            summary: request.title.trim(),
            description: request.description.trim(),
            start: GoogleCreateEventTime { date_time: &request.start },
            end: GoogleCreateEventTime { date_time: &request.end },
            attendees: request.attendees.iter().map(|email| GoogleCreateAttendee { email }).collect(),
        })
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("Calendar write access was denied. Reconnect this calendar account in Settings to grant event access.".into());
    }
    let event: GoogleEvent = checked_json(response).await?;
    normalize_events(vec![event], &request.account_id, &request.calendar_id)
        .into_iter().next()
        .ok_or_else(|| "Google created an event but did not return its details. Refresh the calendar before trying again.".to_string())
}

pub async fn fetch_schedule(
    auth: GoogleAuth,
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
        schedule.extend(normalize_events(events.items, account_id, &calendar_id));
    }
    schedule.sort_by(|left, right| left.start.cmp(&right.start));
    Ok(schedule)
}

/// Queries Google's FreeBusy endpoint once for all selected calendars on an
/// account. Calendar-level errors are returned as partial failures so callers
/// never mistake an incomplete response for confirmed availability.
pub async fn fetch_freebusy(
    auth: GoogleAuth,
    calendar_ids: &[String],
    time_min: &str,
    time_max: &str,
    time_zone: &str,
) -> Result<FreeBusyResult, String> {
    if calendar_ids.is_empty() {
        return Ok(FreeBusyResult {
            busy: Vec::new(),
            checked_calendar_count: 0,
            errors: Vec::new(),
        });
    }
    let access_token = auth
        .access_token()
        .await
        .map_err(|error| error.to_string())?;
    let response = calendar_client()?
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
    let mut busy = Vec::new();
    let mut checked = 0;
    let mut errors = Vec::new();
    for calendar_id in calendar_ids {
        match freebusy.calendars.get(calendar_id) {
            Some(calendar) if calendar.errors.is_empty() => {
                checked += 1;
                busy.extend(calendar.busy.iter().map(|interval| BusyInterval {
                    start: interval.start.clone(),
                    end: interval.end.clone(),
                }));
            }
            Some(_calendar) => errors.push(format!("Calendar {calendar_id} returned a FreeBusy error")),
            None => errors.push(format!("Calendar {calendar_id} was not returned by FreeBusy")),
        }
    }
    busy.sort_by(|left, right| left.start.cmp(&right.start));
    Ok(FreeBusyResult {
        busy,
        checked_calendar_count: checked,
        errors,
    })
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
    auth: GoogleAuth,
    account_id: &str,
    calendar_id: &str,
    schedule_id: &str,
    response_status: &str,
) -> Result<ScheduleEvent, String> {
    if !matches!(response_status, "accepted" | "declined" | "tentative") {
        return Err("Choose Yes, No, or Maybe".into());
    }
    let event_id = schedule_id.strip_prefix(&format!("{calendar_id}:"))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Event does not belong to this calendar".to_string())?;
    let url = event_url(calendar_id, event_id)?;
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
    let mut updated = normalize_events(vec![event], account_id, calendar_id).into_iter().next()
        .ok_or_else(|| "Google changed the response but did not return event details. Refresh the calendar.".to_string())?;
    updated.response_status = Some(response_status.to_string());
    updated.can_respond = true;
    Ok(updated)
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

fn normalize_events(
    events: Vec<GoogleEvent>,
    account_id: &str,
    calendar_id: &str,
) -> Vec<ScheduleEvent> {
    events
        .into_iter()
        .filter(|event| event.status.as_deref() != Some("cancelled"))
        .filter_map(|event| {
            let all_day = event.start.date_time.is_none();
            let start = event.start.date_time.or(event.start.date)?;
            let end = event.end.date_time.or(event.end.date)?;
            let (response_status, can_respond) = self_response(&event.attendees, account_id);
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
                attendees: event_people(event.organizer, event.attendees),
                response_status,
                can_respond,
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
    let mut all_events = calendar
        .components
        .iter()
        .filter(|component| component.component_type == ICalendarComponentType::VEvent);
    let events = all_events
        .by_ref()
        .take(MAX_EVENTS)
        .map(event_preview)
        .collect::<Vec<_>>();
    let truncated = all_events.next().is_some();
    if events.is_empty() {
        return Err("Calendar attachment does not contain an event".into());
    }
    Ok(CalendarPreview { events, truncated })
}

fn event_preview(event: &ICalendarComponent) -> CalendarEventPreview {
    let start_entry = event.property(&ICalendarProperty::Dtstart);
    let start_value = start_entry.and_then(date_value);
    let time_zone =
        start_entry.and_then(|entry| parameter_text(entry, ICalendarParameterName::Tzid));
    CalendarEventPreview {
        uid: text_property(event, ICalendarProperty::Uid),
        title: text_property(event, ICalendarProperty::Summary)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Untitled event".into()),
        start: start_value.and_then(|value| display_date_time(value, time_zone.is_some())),
        end: event
            .property(&ICalendarProperty::Dtend)
            .and_then(date_value)
            .and_then(|value| display_date_time(value, time_zone.is_some())),
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

fn display_date_time(
    value: &calcard::common::PartialDateTime,
    has_named_time_zone: bool,
) -> Option<String> {
    let date = format!("{:04}-{:02}-{:02}", value.year?, value.month?, value.day?);
    let Some(hour) = value.hour else {
        return Some(date);
    };
    let mut result = format!(
        "{date}T{hour:02}:{:02}:{:02}",
        value.minute.unwrap_or(0),
        value.second.unwrap_or(0)
    );
    if !has_named_time_zone {
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
        assert_eq!(event.start.as_deref(), Some("2026-09-18T09:30:00"));
        assert_eq!(event.time_zone.as_deref(), Some("America/New_York"));
        assert_eq!(event.location.as_deref(), Some("Room 4B"));
        assert_eq!(event.organizer.as_deref(), Some("Jane Doe"));
        assert_eq!(event.attendee_count, 1);
        assert!(event.recurring);
        assert_eq!(event.status.as_deref(), Some("CONFIRMED"));
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
                        GoogleAttendee { email: Some("Jane@Example.com".into()), response_status: None, is_self: false, resource: false },
                        GoogleAttendee { email: Some("room-4b@resource.example.com".into()), response_status: None, is_self: false, resource: true },
                        GoogleAttendee { email: None, response_status: None, is_self: false, resource: false },
                    ],
                    organizer: Some(GoogleAttendee { email: Some("jane@example.com".into()), response_status: None, is_self: false, resource: false }),
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
                },
            ],
            "work@example.com",
            "team@example.com",
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
        assert_eq!(events[0].attendees, vec!["jane@example.com"]);
        assert_eq!(events[0].response_status.as_deref(), Some("needsAction"));
        assert!(events[0].can_respond);
        assert_eq!(events[0].calendar_id, "team@example.com");
        assert_eq!(events[1].title, "Untitled event");
        assert!(events[1].all_day);
        assert!(events[1].attendees.is_empty());
        assert_eq!(events[1].response_status, None);
        assert!(!events[1].can_respond);
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
        assert_eq!(event_people(event.organizer, event.attendees), vec!["bob@example.com"]);
        let without_people: GoogleEvent = serde_json::from_value(serde_json::json!({
            "id": "e2", "start": {"date": "2026-09-18"}, "end": {"date": "2026-09-19"},
        }))
        .unwrap();
        assert!(event_people(without_people.organizer, without_people.attendees).is_empty());

        let attendees = |count: usize| {
            (0..count)
                .map(|index| GoogleAttendee { email: Some(format!("person{index}@example.com")), response_status: None, is_self: false, resource: false })
                .collect::<Vec<_>>()
        };
        assert_eq!(event_people(None, attendees(MAX_EVENT_ATTENDEES - 1)).len(), MAX_EVENT_ATTENDEES - 1);
        assert_eq!(event_people(None, attendees(MAX_EVENT_ATTENDEES)).len(), MAX_EVENT_ATTENDEES);
        assert_eq!(event_people(None, attendees(MAX_EVENT_ATTENDEES + 1)).len(), MAX_EVENT_ATTENDEES);
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
            start: GoogleCreateEventTime { date_time: "2026-09-22T09:00:00Z" },
            end: GoogleCreateEventTime { date_time: "2026-09-22T10:00:00Z" },
            attendees: vec![GoogleCreateAttendee { email: "guest@example.com" }],
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
}
