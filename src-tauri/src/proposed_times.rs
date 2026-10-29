//! Sender-offered alternatives. AI extracts wall clocks; native code resolves
//! IANA zones and checks exact intervals without applying working-hour filters.
use crate::{
    availability,
    models::{AvailabilityPreferences, ProposedTimeCheck},
};
use chrono::{DateTime, Duration, LocalResult, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const MAX_PROPOSED_TIMES: usize = 32;
pub const MAX_PROPOSED_TEXT_CHARS: usize = 500;
pub const MAX_PROPOSED_SPAN_DAYS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposedTime {
    pub source_text: String,
    pub local_start: Option<String>,
    pub duration_minutes: Option<u32>,
    pub time_zone: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfferedTimes {
    pub source_message_id: String,
    pub times: Vec<ProposedTime>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckProposedTimesRequest {
    pub times: Vec<ProposedTime>,
    pub assumed_time_zone: String,
    pub preferences: AvailabilityPreferences,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedProposedTime {
    pub start: Option<String>,
    pub end: Option<String>,
    pub issue: Option<String>,
    pub outside_working_hours: bool,
    pub check: Option<ProposedTimeCheck>,
}

pub fn times_schema() -> serde_json::Value {
    json!({"type":"array", "items": {"type":"object", "additionalProperties":false,
        "required":["sourceText","localStart","durationMinutes","timeZone"],
        "properties": {"sourceText":{"type":"string"}, "localStart":{"type":["string","null"]},
            "durationMinutes":{"type":["integer","null"]}, "timeZone":{"type":["string","null"]}}}})
}

pub fn validate_times(times: &[ProposedTime]) -> Result<(), String> {
    if times.len() > MAX_PROPOSED_TIMES {
        return Err(format!(
            "At most {MAX_PROPOSED_TIMES} proposed times can be checked together"
        ));
    }
    for time in times {
        if time.source_text.trim().is_empty()
            || time.source_text.chars().count() > MAX_PROPOSED_TEXT_CHARS
        {
            return Err("Invalid proposed-time source text".into());
        }
        if time
            .local_start
            .as_ref()
            .is_some_and(|start| NaiveDateTime::parse_from_str(start, "%Y-%m-%dT%H:%M").is_err())
        {
            return Err("Invalid proposed local time".into());
        }
        if time.duration_minutes.is_some_and(|minutes| {
            !(crate::ai::find_times::MIN_FIND_TIMES_DURATION_MINUTES
                ..=crate::ai::find_times::MAX_FIND_TIMES_DURATION_MINUTES)
                .contains(&minutes)
        }) {
            return Err("Proposed duration must be between 5 and 720 minutes".into());
        }
        if time
            .time_zone
            .as_ref()
            .is_some_and(|zone| zone.parse::<chrono_tz::Tz>().is_err())
        {
            return Err("Unknown proposed timezone".into());
        }
    }
    Ok(())
}

pub fn resolve(
    time: &ProposedTime,
    assumed_zone: &str,
    now: DateTime<Utc>,
) -> Result<(String, String), String> {
    let start = time
        .local_start
        .as_deref()
        .ok_or("Date or time needs clarification")?;
    let duration = time
        .duration_minutes
        .ok_or("Meeting length needs clarification")?;
    let zone = time
        .time_zone
        .as_deref()
        .unwrap_or(assumed_zone)
        .parse::<chrono_tz::Tz>()
        .map_err(|_| "Unknown timezone")?;
    let local =
        NaiveDateTime::parse_from_str(start, "%Y-%m-%dT%H:%M").map_err(|_| "Invalid local time")?;
    let start = match zone.from_local_datetime(&local) {
        LocalResult::Single(start) => start.with_timezone(&Utc),
        LocalResult::Ambiguous(_, _) => {
            return Err("This clock time occurs twice; clarify the timezone offset".into())
        }
        LocalResult::None => {
            return Err("This clock time does not exist because the clocks change".into())
        }
    };
    if start <= now {
        return Err("This proposed time has passed".into());
    }
    let end = start
        .checked_add_signed(Duration::minutes(i64::from(duration)))
        .ok_or("Time is out of bounds")?;
    Ok((start.to_rfc3339(), end.to_rfc3339()))
}

pub fn outside_hours(start: &str, end: &str, preferences: &AvailabilityPreferences) -> bool {
    use chrono::{Datelike, Timelike};
    let Ok(zone) = preferences.time_zone.parse::<chrono_tz::Tz>() else {
        return true;
    };
    let Ok(start) = DateTime::parse_from_rfc3339(start) else {
        return true;
    };
    let Ok(end) = DateTime::parse_from_rfc3339(end) else {
        return true;
    };
    // Check the complete interval, including adjacent working windows and midnight.
    let mut instant = start.with_timezone(&Utc);
    while instant < end.with_timezone(&Utc) {
        let local = instant.with_timezone(&zone);
        let minute = format!("{:02}:{:02}", local.hour(), local.minute());
        if !preferences.working_windows.iter().any(|window| {
            window.weekday == local.weekday().num_days_from_sunday() as u8
                && window.start <= minute
                && minute < window.end
        }) {
            return true;
        }
        instant += Duration::minutes(1);
    }
    false
}

/// Bound the one calendar read independently of the ordering of sender options.
pub fn coverage_range(rows: &[CheckedProposedTime]) -> Result<Option<(String, String)>, String> {
    let from = rows
        .iter()
        .filter_map(|row| row.start.as_ref())
        .map(|value| DateTime::parse_from_rfc3339(value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid proposed start")?;
    let to = rows
        .iter()
        .filter_map(|row| row.end.as_ref())
        .map(|value| DateTime::parse_from_rfc3339(value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid proposed end")?;
    let (Some(start), Some(end)) = (from.iter().min(), to.iter().max()) else {
        return Ok(None);
    };
    if *end - *start > Duration::days(MAX_PROPOSED_SPAN_DAYS) {
        return Err(format!(
            "Proposed times span more than {MAX_PROPOSED_SPAN_DAYS} days; check a smaller group"
        ));
    }
    Ok(Some((start.to_rfc3339(), end.to_rfc3339())))
}

pub fn check_resolved(
    rows: &mut [CheckedProposedTime],
    busy: &[crate::models::BusyInterval],
    checked: usize,
    total: usize,
    unchecked: &[String],
    errors: &[String],
) -> Result<(), String> {
    for row in rows {
        if let (Some(start), Some(end)) = (&row.start, &row.end) {
            let from = DateTime::parse_from_rfc3339(start).map_err(|_| "Invalid proposed start")?;
            let to = DateTime::parse_from_rfc3339(end).map_err(|_| "Invalid proposed end")?;
            row.check = Some(ProposedTimeCheck {
                status: availability::check_time(start, end, busy, checked, total)?,
                conflicts: busy
                    .iter()
                    .filter(|interval| {
                        DateTime::parse_from_rfc3339(&interval.start)
                            .ok()
                            .zip(DateTime::parse_from_rfc3339(&interval.end).ok())
                            .is_some_and(|(a, b)| a < to && b > from)
                    })
                    .cloned()
                    .collect(),
                checked_calendar_count: checked,
                total_calendar_count: total,
                unchecked_calendars: unchecked.to_vec(),
                errors: errors.to_vec(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AvailabilityWindow, BusyInterval};
    fn preferences() -> AvailabilityPreferences {
        AvailabilityPreferences {
            time_zone: "America/New_York".into(),
            working_windows: (1..=5)
                .map(|weekday| AvailabilityWindow {
                    weekday,
                    start: "09:00".into(),
                    end: "17:00".into(),
                })
                .collect(),
            default_duration_minutes: 30,
            slot_increment_minutes: 30,
            drive_time_buffer_minutes: 0,
        }
    }
    fn option(local: &str) -> ProposedTime {
        ProposedTime {
            source_text: local.into(),
            local_start: Some(local.into()),
            duration_minutes: Some(60),
            time_zone: None,
        }
    }
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-28T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
    #[test]
    fn coverage_is_bounded_without_silently_clamping_the_offer() {
        let start = now();
        for (minutes, valid) in [
            (MAX_PROPOSED_SPAN_DAYS * 1440 - 1, true),
            (MAX_PROPOSED_SPAN_DAYS * 1440, true),
            (MAX_PROPOSED_SPAN_DAYS * 1440 + 1, false),
        ] {
            let end = start + Duration::minutes(minutes);
            let rows = vec![CheckedProposedTime {
                start: Some(start.to_rfc3339()),
                end: Some(end.to_rfc3339()),
                issue: None,
                outside_working_hours: false,
                check: None,
            }];
            assert_eq!(coverage_range(&rows).is_ok(), valid);
        }
        assert!(coverage_range(&[]).unwrap().is_none());
    }

    #[test]
    fn crt_alternatives_keep_exact_hours_across_dst_and_ignore_working_hour_filters() {
        let options = [
            "2026-10-31T14:00",
            "2026-10-31T19:00",
            "2026-11-01T16:00",
            "2026-11-01T19:00",
            "2026-11-02T19:00",
            "2026-11-03T19:00",
            "2026-11-04T19:00",
        ];
        let mut rows: Vec<_> = options
            .iter()
            .map(|local| {
                let (start, end) = resolve(&option(local), "America/New_York", now()).unwrap();
                assert_eq!(
                    DateTime::parse_from_rfc3339(&end).unwrap()
                        - DateTime::parse_from_rfc3339(&start).unwrap(),
                    Duration::hours(1)
                );
                assert!(outside_hours(&start, &end, &preferences()));
                CheckedProposedTime {
                    start: Some(start),
                    end: Some(end),
                    issue: None,
                    outside_working_hours: true,
                    check: None,
                }
            })
            .collect();
        assert_eq!(rows[0].start.as_deref(), Some("2026-10-31T18:00:00+00:00"));
        assert_eq!(rows[2].start.as_deref(), Some("2026-11-01T21:00:00+00:00"));
        // A conflict only in the second half of the hour must still block it.
        let busy = vec![BusyInterval {
            start: "2026-10-31T14:45:00-04:00".into(),
            end: "2026-10-31T15:00:00-04:00".into(),
        }];
        check_resolved(&mut rows, &busy, 2, 2, &[], &[]).unwrap();
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0].check.as_ref().unwrap().status, "conflicting");
        assert_eq!(rows[0].check.as_ref().unwrap().conflicts.len(), 1);
        assert!(rows[1..]
            .iter()
            .all(|row| row.check.as_ref().unwrap().status == "free"));
        check_resolved(
            &mut rows,
            &[],
            1,
            2,
            &["Personal".into()],
            &["offline".into()],
        )
        .unwrap();
        assert!(rows
            .iter()
            .all(|row| row.check.as_ref().unwrap().status == "partiallyChecked"));
        check_resolved(&mut rows, &[], 0, 0, &[], &[]).unwrap();
        assert!(rows
            .iter()
            .all(|row| row.check.as_ref().unwrap().status == "unverified"));
    }
    #[test]
    fn keeps_uncertainty_and_resolves_explicit_zones_without_guessing_clock_transitions() {
        assert!(
            resolve(&option("2026-11-01T01:30"), "America/New_York", now())
                .unwrap_err()
                .contains("twice")
        );
        let spring_now = DateTime::parse_from_rfc3339("2026-03-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            resolve(&option("2026-03-08T02:30"), "America/New_York", spring_now)
                .unwrap_err()
                .contains("does not exist")
        );
        let mut missing = option("2026-11-04T19:00");
        missing.duration_minutes = None;
        assert!(resolve(&missing, "America/New_York", now())
            .unwrap_err()
            .contains("length"));
        missing.local_start = None;
        assert!(resolve(&missing, "America/New_York", now())
            .unwrap_err()
            .contains("clarification"));
        let mut explicit = option("2026-11-04T19:00");
        explicit.time_zone = Some("Europe/London".into());
        assert_eq!(
            resolve(&explicit, "America/New_York", now()).unwrap().0,
            "2026-11-04T19:00:00+00:00"
        );
        assert!(
            resolve(&option("2026-10-27T19:00"), "America/New_York", now())
                .unwrap_err()
                .contains("passed")
        );
    }
    #[test]
    fn validates_limits_and_never_drops_or_clamps_options() {
        for (count, valid) in [
            (MAX_PROPOSED_TIMES - 1, true),
            (MAX_PROPOSED_TIMES, true),
            (MAX_PROPOSED_TIMES + 1, false),
        ] {
            assert_eq!(
                validate_times(&vec![option("2026-11-04T19:00"); count]).is_ok(),
                valid
            );
        }
        for (count, valid) in [
            (MAX_PROPOSED_TEXT_CHARS - 1, true),
            (MAX_PROPOSED_TEXT_CHARS, true),
            (MAX_PROPOSED_TEXT_CHARS + 1, false),
        ] {
            let mut time = option("2026-11-04T19:00");
            time.source_text = "x".repeat(count);
            assert_eq!(validate_times(&[time]).is_ok(), valid);
        }
        for (duration, valid) in [(4, false), (5, true), (720, true), (721, false)] {
            let mut time = option("2026-11-04T19:00");
            time.duration_minutes = Some(duration);
            assert_eq!(validate_times(&[time]).is_ok(), valid);
        }
        let mut time = option("2026-02-30T19:00");
        assert!(validate_times(&[time.clone()]).is_err());
        time.local_start = None;
        time.time_zone = Some("EST<script>".into());
        assert!(validate_times(&[time]).is_err());
    }
    #[test]
    fn working_hours_cover_the_whole_interval_and_busy_endpoints_do_not_overlap() {
        assert!(!outside_hours(
            "2026-11-02T14:00:00Z",
            "2026-11-02T15:00:00Z",
            &preferences()
        ));
        assert!(outside_hours(
            "2026-11-02T21:30:00Z",
            "2026-11-02T22:30:00Z",
            &preferences()
        ));
        assert_eq!(
            availability::check_time(
                "2026-11-02T14:00:00Z",
                "2026-11-02T15:00:00Z",
                &[BusyInterval {
                    start: "2026-11-02T15:00:00Z".into(),
                    end: "2026-11-02T16:00:00Z".into()
                }],
                1,
                1
            )
            .unwrap(),
            "free"
        );
    }
}
