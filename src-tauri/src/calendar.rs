use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarParameterName,
    ICalendarParameterValue, ICalendarProperty, ICalendarValue,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::{
    auth::GoogleAuth,
    models::{CalendarOption, ScheduleEvent},
};

const MAX_EVENTS: usize = 20;
const MAX_CALENDAR_BYTES: usize = 2 * 1024 * 1024;
const CALENDAR_LIST_URL: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";
const CALENDARS_URL: &str = "https://www.googleapis.com/calendar/v3/calendars/";
const MAX_SCHEDULE_EVENTS: usize = 250;

#[derive(Deserialize)]
struct GoogleCalendarList {
    #[serde(default)]
    items: Vec<GoogleCalendarListEntry>,
}

#[derive(Deserialize)]
struct GoogleCalendarListEntry {
    id: String,
    summary: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
struct GoogleEvents {
    #[serde(default)]
    items: Vec<GoogleEvent>,
}

#[derive(Deserialize)]
struct GoogleEvent {
    id: String,
    summary: Option<String>,
    status: Option<String>,
    start: GoogleEventTime,
    end: GoogleEventTime,
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
        })
        .collect()
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
        let mut events_url = url::Url::parse(CALENDARS_URL).map_err(|error| error.to_string())?;
        events_url
            .path_segments_mut()
            .map_err(|_| "Google Calendar URL cannot be extended".to_string())?
            .push(&calendar_id)
            .push("events");
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

async fn checked_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, String> {
    if response.status().is_success() {
        return response.json().await.map_err(|error| error.to_string());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(format!("Google Calendar returned {status}: {body}"))
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
            Some(ScheduleEvent {
                id: format!("{calendar_id}:{}", event.id),
                account_id: account_id.to_string(),
                title: event
                    .summary
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| "Untitled event".to_string()),
                start,
                end,
                all_day,
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
                    start: GoogleEventTime {
                        date: None,
                        date_time: Some("2026-09-18T09:30:00-07:00".into()),
                    },
                    end: GoogleEventTime {
                        date: None,
                        date_time: Some("2026-09-18T10:00:00-07:00".into()),
                    },
                },
                GoogleEvent {
                    id: "all-day".into(),
                    summary: None,
                    status: None,
                    start: GoogleEventTime {
                        date: Some("2026-09-18".into()),
                        date_time: None,
                    },
                    end: GoogleEventTime {
                        date: Some("2026-09-19".into()),
                        date_time: None,
                    },
                },
            ],
            "work@example.com",
            "team@example.com",
        );

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "team@example.com:timed");
        assert_eq!(events[0].title, "Planning");
        assert!(!events[0].all_day);
        assert_eq!(events[1].title, "Untitled event");
        assert!(events[1].all_day);
    }

    #[test]
    fn maps_calendar_list_entries_for_selection_ui() {
        let options = calendar_options(
            vec![
            GoogleCalendarListEntry {
                id: "primary@example.com".into(),
                summary: "My calendar".into(),
                primary: true,
            },
            GoogleCalendarListEntry {
                id: "team@example.com".into(),
                summary: "Team".into(),
                primary: false,
            },
            ],
            "work@example.com",
        );

        assert_eq!(options.len(), 2);
        assert_eq!(options[0].name, "My calendar");
        assert!(options[0].primary);
        assert_eq!(options[1].account_id, "work@example.com");
    }
}
