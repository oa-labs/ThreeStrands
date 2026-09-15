use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarParameterName,
    ICalendarParameterValue, ICalendarProperty, ICalendarValue,
};
use serde::Serialize;

const MAX_EVENTS: usize = 20;
const MAX_CALENDAR_BYTES: usize = 2 * 1024 * 1024;

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
}
