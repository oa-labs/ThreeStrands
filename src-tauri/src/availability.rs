use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::models::{AvailabilityCandidate, AvailabilityPreferences, BusyInterval};

fn parse_range(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| "Availability times must be RFC3339 timestamps".to_string())
}

fn parse_clock(value: &str) -> Result<NaiveTime, String> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .map_err(|_| format!("Invalid working-hours time: {value}"))
}

pub(crate) fn validate_preferences(preferences: &AvailabilityPreferences) -> Result<Tz, String> {
    let zone = preferences
        .time_zone
        .parse::<Tz>()
        .map_err(|_| format!("Unknown IANA timezone: {}", preferences.time_zone))?;
    if !(5..=12 * 60).contains(&preferences.default_duration_minutes) {
        return Err("Duration must be between 5 and 720 minutes".to_string());
    }
    if !(5..=120).contains(&preferences.slot_increment_minutes) {
        return Err("Slot increment must be between 5 and 120 minutes".to_string());
    }
    if preferences.working_windows.len() > 14 {
        return Err("Too many working-hour windows".to_string());
    }
    for window in &preferences.working_windows {
        if window.weekday > 6 {
            return Err("Working-hour weekday must be between 0 and 6".to_string());
        }
        let start = parse_clock(&window.start)?;
        let end = parse_clock(&window.end)?;
        if start >= end {
            return Err("Working-hour windows must end after they start".to_string());
        }
    }
    Ok(zone)
}

fn busy_overlaps(start: DateTime<Utc>, end: DateTime<Utc>, busy: &[BusyInterval]) -> bool {
    busy.iter().any(|interval| {
        let Ok(interval_start) = parse_range(&interval.start) else { return false; };
        let Ok(interval_end) = parse_range(&interval.end) else { return false; };
        interval_start < end && interval_end > start
    })
}

fn local_datetime(zone: Tz, date: NaiveDate, time: NaiveTime) -> Option<DateTime<Utc>> {
    let local = NaiveDateTime::new(date, time);
    match zone.from_local_datetime(&local) {
        LocalResult::Single(value) => Some(value.with_timezone(&Utc)),
        // During a fall-back transition, use the earlier occurrence. During
        // spring-forward, nonexistent local times are skipped.
        LocalResult::Ambiguous(earlier, _) => Some(earlier.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

/// Most candidates returned by one search.
pub(crate) const MAX_CANDIDATES: usize = 20;

/// Free working-hour slots in the range, earliest first. `max_per_day`
/// spreads the result across days instead of filling it from the first one.
pub fn find_candidates(
    range_start: &str,
    range_end: &str,
    preferences: &AvailabilityPreferences,
    busy: &[BusyInterval],
    checked_calendar_count: usize,
    total_calendar_count: usize,
    max_per_day: Option<usize>,
) -> Result<Vec<AvailabilityCandidate>, String> {
    if max_per_day.is_some_and(|limit| !(1..=MAX_CANDIDATES).contains(&limit)) {
        return Err(format!("Slots per day must be between 1 and {MAX_CANDIDATES}"));
    }
    let per_day = max_per_day.unwrap_or(MAX_CANDIDATES);
    let range_start = parse_range(range_start)?;
    let range_end = parse_range(range_end)?;
    if range_end <= range_start {
        return Err("Availability range must end after it starts".to_string());
    }
    let zone = validate_preferences(preferences)?;
    let first_date = range_start.with_timezone(&zone).date_naive();
    let last_date = range_end.with_timezone(&zone).date_naive();
    let mut date = first_date;
    let mut candidates = Vec::new();
    let duration = Duration::minutes(preferences.default_duration_minutes as i64);
    let increment = Duration::minutes(preferences.slot_increment_minutes as i64);
    let now = Utc::now();

    while date <= last_date && candidates.len() < MAX_CANDIDATES {
        let weekday = date.weekday().num_days_from_sunday() as u8;
        let day_start = candidates.len();
        for window in preferences.working_windows.iter().filter(|window| window.weekday == weekday) {
            let start = parse_clock(&window.start)?;
            let end = parse_clock(&window.end)?;
            let mut cursor = start;
            while cursor + duration <= end && candidates.len() < MAX_CANDIDATES && candidates.len() - day_start < per_day {
                let Some(slot_start) = local_datetime(zone, date, cursor) else {
                    cursor += increment;
                    continue;
                };
                let Some(slot_end) = local_datetime(zone, date, cursor + duration) else {
                    cursor += increment;
                    continue;
                };
                if slot_start >= range_start && slot_end <= range_end && slot_start > now && !busy_overlaps(slot_start, slot_end, busy) {
                    let status = if total_calendar_count == 0 {
                        "unverified"
                    } else if checked_calendar_count == total_calendar_count {
                        "verified"
                    } else {
                        "partiallyChecked"
                    };
                    candidates.push(AvailabilityCandidate {
                        start: slot_start.to_rfc3339(),
                        end: slot_end.to_rfc3339(),
                        status: status.to_string(),
                    });
                }
                cursor += increment;
            }
        }
        date += Duration::days(1);
    }
    Ok(candidates)
}

pub fn check_time(
    start: &str,
    end: &str,
    busy: &[BusyInterval],
    checked_calendar_count: usize,
    total_calendar_count: usize,
) -> Result<String, String> {
    let start = parse_range(start)?;
    let end = parse_range(end)?;
    if end <= start {
        return Err("Proposed time must end after it starts".to_string());
    }
    if busy_overlaps(start, end, busy) {
        return Ok("conflicting".to_string());
    }
    if total_calendar_count == 0 {
        Ok("unverified".to_string())
    } else if checked_calendar_count < total_calendar_count {
        Ok("partiallyChecked".to_string())
    } else {
        Ok("free".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AvailabilityWindow, BusyInterval};

    fn preferences() -> AvailabilityPreferences {
        AvailabilityPreferences {
            time_zone: "America/New_York".to_string(),
            working_windows: vec![AvailabilityWindow { weekday: 1, start: "09:00".to_string(), end: "12:00".to_string() }],
            default_duration_minutes: 30,
            slot_increment_minutes: 30,
        }
    }

    #[test]
    fn skips_busy_slots_and_marks_coverage() {
        let candidates = find_candidates(
            "2099-09-21T00:00:00Z",
            "2099-09-22T00:00:00Z",
            &preferences(),
            &[BusyInterval { start: "2099-09-21T13:30:00Z".to_string(), end: "2099-09-21T14:30:00Z".to_string() }],
            1,
            1,
            None,
        ).unwrap();
        assert_eq!(candidates[0].start, "2099-09-21T13:00:00+00:00");
        assert!(candidates.iter().all(|candidate| candidate.status == "verified"));
        assert!(!candidates.iter().any(|candidate| candidate.start == "2099-09-21T13:30:00+00:00"));
    }

    #[test]
    fn handles_dst_transition_without_creating_invalid_local_times() {
        let mut preferences = preferences();
        preferences.working_windows = vec![AvailabilityWindow { weekday: 0, start: "01:00".to_string(), end: "04:00".to_string() }];
        let candidates = find_candidates(
            "2026-03-08T00:00:00Z",
            "2026-03-09T00:00:00Z",
            &preferences,
            &[],
            0,
            0,
            None,
        ).unwrap();
        assert!(candidates.iter().all(|candidate| candidate.status == "unverified"));
        assert!(candidates.iter().all(|candidate| candidate.start != "2026-03-08T02:00:00-05:00"));
    }

    #[test]
    fn a_per_day_limit_spreads_candidates_across_days() {
        let mut preferences = preferences();
        preferences.working_windows = (1..=5)
            .map(|weekday| AvailabilityWindow { weekday, start: "09:00".to_string(), end: "17:00".to_string() })
            .collect();
        // Monday 2099-09-21 through Friday 2099-09-25, New York time.
        let range = ("2099-09-21T04:00:00Z", "2099-09-26T04:00:00Z");
        let busy = [BusyInterval { start: "2099-09-22T13:00:00Z".to_string(), end: "2099-09-22T15:00:00Z".to_string() }];
        let unlimited = find_candidates(range.0, range.1, &preferences, &busy, 1, 1, None).unwrap();
        assert_eq!(unlimited.len(), MAX_CANDIDATES);
        // Without a limit, Monday's sixteen half-hour slots come first.
        assert_eq!(unlimited.iter().filter(|candidate| candidate.start.starts_with("2099-09-21")).count(), 16);
        assert_eq!(unlimited[1].start, "2099-09-21T13:30:00+00:00");

        let spread = find_candidates(range.0, range.1, &preferences, &busy, 1, 1, Some(1)).unwrap();
        let starts = spread.iter().map(|candidate| candidate.start.as_str()).collect::<Vec<_>>();
        assert_eq!(starts, vec![
            "2099-09-21T13:00:00+00:00",
            "2099-09-22T15:00:00+00:00",
            "2099-09-23T13:00:00+00:00",
            "2099-09-24T13:00:00+00:00",
            "2099-09-25T13:00:00+00:00",
        ]);
        assert_eq!(find_candidates(range.0, range.1, &preferences, &busy, 1, 1, Some(2)).unwrap().len(), 10);
        assert_eq!(find_candidates(range.0, range.1, &preferences, &busy, 1, 1, Some(MAX_CANDIDATES)).unwrap().len(), MAX_CANDIDATES);
        assert!(find_candidates(range.0, range.1, &preferences, &busy, 1, 1, Some(0)).is_err());
        assert!(find_candidates(range.0, range.1, &preferences, &busy, 1, 1, Some(MAX_CANDIDATES + 1)).is_err());
    }

    #[test]
    fn proposed_time_reports_coverage_state() {
        assert_eq!(check_time("2026-09-21T13:00:00Z", "2026-09-21T13:30:00Z", &[], 0, 0).unwrap(), "unverified");
        assert_eq!(check_time("2026-09-21T13:00:00Z", "2026-09-21T13:30:00Z", &[], 1, 2).unwrap(), "partiallyChecked");
    }
}
