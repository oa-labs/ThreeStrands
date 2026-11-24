use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
};
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

/// Most working-hour windows across the week: four per day.
pub(crate) const MAX_WORKING_WINDOWS: usize = 28;
pub(crate) const MAX_DRIVE_TIME_BUFFER_MINUTES: u32 = 120;
/// Matches `MAX_AVAILABILITY_PROFILES` in src/settings.ts; counts the default profile.
pub(crate) const MAX_AVAILABILITY_PROFILES: usize = 8;
/// Matches `MAX_AVAILABILITY_PROFILE_NAME_CHARS` in src/settings.ts.
pub(crate) const MAX_AVAILABILITY_PROFILE_NAME_CHARS: usize = 40;
/// Matches `MAX_AVAILABILITY_PROFILE_ID_CHARS` in src/settings.ts.
pub(crate) const MAX_AVAILABILITY_PROFILE_ID_CHARS: usize = 64;
/// Matches `MAX_AVAILABILITY_ACCOUNT_DEFAULTS` in src/settings.ts.
pub(crate) const MAX_AVAILABILITY_ACCOUNT_DEFAULTS: usize = 200;

pub(crate) fn validate_drive_time_buffer(minutes: u32) -> Result<(), String> {
    if minutes > MAX_DRIVE_TIME_BUFFER_MINUTES {
        return Err(format!(
            "Drive time buffer must be between 0 and {MAX_DRIVE_TIME_BUFFER_MINUTES} minutes"
        ));
    }
    Ok(())
}

/// Include events outside the requested range whose drive time reaches it.
pub(crate) fn buffered_range(
    start: &str,
    end: &str,
    minutes: u32,
) -> Result<(String, String), String> {
    validate_drive_time_buffer(minutes)?;
    let start = parse_range(start)?;
    let end = parse_range(end)?;
    if end <= start {
        return Err("Availability range must end after it starts".into());
    }
    let buffer = Duration::minutes(i64::from(minutes));
    let start = start
        .checked_sub_signed(buffer)
        .ok_or("Availability range is out of bounds")?;
    let end = end
        .checked_add_signed(buffer)
        .ok_or("Availability range is out of bounds")?;
    Ok((start.to_rfc3339(), end.to_rfc3339()))
}

pub(crate) fn validate_preferences(preferences: &AvailabilityPreferences) -> Result<Tz, String> {
    validate_drive_time_buffer(preferences.drive_time_buffer_minutes)?;
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
    if preferences.working_windows.len() > MAX_WORKING_WINDOWS {
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

/// One weekday's windows sorted by start, with overlapping or touching
/// windows merged so a slot is never offered twice.
fn merged_windows(
    preferences: &AvailabilityPreferences,
    weekday: u8,
) -> Result<Vec<(NaiveTime, NaiveTime)>, String> {
    let mut windows = preferences
        .working_windows
        .iter()
        .filter(|window| window.weekday == weekday)
        .map(|window| Ok((parse_clock(&window.start)?, parse_clock(&window.end)?)))
        .collect::<Result<Vec<_>, String>>()?;
    windows.sort();
    let mut merged: Vec<(NaiveTime, NaiveTime)> = Vec::with_capacity(windows.len());
    for (start, end) in windows {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    Ok(merged)
}

fn busy_overlaps(start: DateTime<Utc>, end: DateTime<Utc>, busy: &[BusyInterval]) -> bool {
    busy.iter().any(|interval| {
        let Ok(interval_start) = parse_range(&interval.start) else {
            return false;
        };
        let Ok(interval_end) = parse_range(&interval.end) else {
            return false;
        };
        interval_start < end && interval_end > start
    })
}

/// The buffer an in-person meeting keeps free on each side; zero otherwise.
pub(crate) fn meeting_padding(in_person: bool, drive_time_buffer_minutes: u32) -> Duration {
    if in_person {
        Duration::minutes(i64::from(drive_time_buffer_minutes))
    } else {
        Duration::zero()
    }
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
/// When the days hold more slots than `MAX_CANDIDATES`, the later-ranked slots
/// are dropped (every day's first slot before any day's second), never the
/// later days.
pub fn find_candidates(
    range_start: &str,
    range_end: &str,
    preferences: &AvailabilityPreferences,
    busy: &[BusyInterval],
    checked_calendar_count: usize,
    total_calendar_count: usize,
    max_per_day: Option<usize>,
) -> Result<Vec<AvailabilityCandidate>, String> {
    find_candidates_for(
        range_start,
        range_end,
        preferences,
        busy,
        checked_calendar_count,
        total_calendar_count,
        max_per_day,
        false,
    )
}

/// `find_candidates`, where an in-person meeting also keeps the drive time
/// buffer free on both sides of the slot against every busy interval.
#[allow(clippy::too_many_arguments)]
pub fn find_candidates_for(
    range_start: &str,
    range_end: &str,
    preferences: &AvailabilityPreferences,
    busy: &[BusyInterval],
    checked_calendar_count: usize,
    total_calendar_count: usize,
    max_per_day: Option<usize>,
    in_person: bool,
) -> Result<Vec<AvailabilityCandidate>, String> {
    let padding = meeting_padding(in_person, preferences.drive_time_buffer_minutes);
    if max_per_day.is_some_and(|limit| !(1..=MAX_CANDIDATES).contains(&limit)) {
        return Err(format!(
            "Slots per day must be between 1 and {MAX_CANDIDATES}"
        ));
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
    let mut by_day: Vec<Vec<AvailabilityCandidate>> = Vec::new();
    let mut total = 0;
    let duration = Duration::minutes(preferences.default_duration_minutes as i64);
    let increment = Duration::minutes(preferences.slot_increment_minutes as i64);
    let now = Utc::now();

    // Without a per-day limit the earliest slots fill the cap, so stop early.
    while date <= last_date && (max_per_day.is_some() || total < MAX_CANDIDATES) {
        let weekday = date.weekday().num_days_from_sunday() as u8;
        let mut candidates = Vec::new();
        for (start, end) in merged_windows(preferences, weekday)? {
            let mut cursor = start;
            while cursor + duration <= end && candidates.len() < per_day {
                let Some(slot_start) = local_datetime(zone, date, cursor) else {
                    cursor += increment;
                    continue;
                };
                let Some(slot_end) = local_datetime(zone, date, cursor + duration) else {
                    cursor += increment;
                    continue;
                };
                if slot_start >= range_start
                    && slot_end <= range_end
                    && slot_start > now
                    && !busy_overlaps(slot_start - padding, slot_end + padding, busy)
                {
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
        total += candidates.len();
        by_day.push(candidates);
        date += Duration::days(1);
    }
    let mut kept = Vec::new();
    let deepest = by_day.iter().map(Vec::len).max().unwrap_or(0);
    for rank in 0..deepest {
        kept.extend(by_day.iter().filter_map(|day| day.get(rank)).cloned());
    }
    if max_per_day.is_none() {
        // One rank per day would otherwise reorder a plain earliest-first search.
        kept = by_day.into_iter().flatten().collect();
    }
    kept.truncate(MAX_CANDIDATES);
    kept.sort_by_key(|candidate| {
        chrono::DateTime::parse_from_rfc3339(&candidate.start)
            .map(|time| time.timestamp())
            .unwrap_or_default()
    });
    Ok(kept)
}

pub fn check_time(
    start: &str,
    end: &str,
    busy: &[BusyInterval],
    checked_calendar_count: usize,
    total_calendar_count: usize,
) -> Result<String, String> {
    check_time_padded(
        start,
        end,
        busy,
        checked_calendar_count,
        total_calendar_count,
        Duration::zero(),
    )
}

/// `check_time`, with `padding` kept free on each side of the time.
pub fn check_time_padded(
    start: &str,
    end: &str,
    busy: &[BusyInterval],
    checked_calendar_count: usize,
    total_calendar_count: usize,
    padding: Duration,
) -> Result<String, String> {
    let start = parse_range(start)?;
    let end = parse_range(end)?;
    if end <= start {
        return Err("Proposed time must end after it starts".to_string());
    }
    if busy_overlaps(start - padding, end + padding, busy) {
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
            working_windows: vec![AvailabilityWindow {
                weekday: 1,
                start: "09:00".to_string(),
                end: "12:00".to_string(),
            }],
            default_duration_minutes: 30,
            slot_increment_minutes: 30,
            drive_time_buffer_minutes: 0,
        }
    }

    #[test]
    fn drive_time_limits_and_cross_day_search_ranges() {
        for minutes in [
            0,
            MAX_DRIVE_TIME_BUFFER_MINUTES - 1,
            MAX_DRIVE_TIME_BUFFER_MINUTES,
        ] {
            let mut prefs = preferences();
            prefs.drive_time_buffer_minutes = minutes;
            assert!(validate_preferences(&prefs).is_ok());
        }
        let mut prefs = preferences();
        prefs.drive_time_buffer_minutes = MAX_DRIVE_TIME_BUFFER_MINUTES + 1;
        assert!(validate_preferences(&prefs).is_err());
        assert!(buffered_range("invalid", "2099-09-21T00:00:00Z", 30).is_err());
        assert!(buffered_range("2099-09-21T01:00:00Z", "2099-09-21T00:00:00Z", 30).is_err());
        assert_eq!(
            buffered_range("2099-09-21T00:00:00Z", "2099-09-21T23:45:00Z", 30).unwrap(),
            (
                "2099-09-20T23:30:00+00:00".into(),
                "2099-09-22T00:15:00+00:00".into()
            )
        );
    }

    #[test]
    fn skips_busy_slots_and_marks_coverage() {
        let candidates = find_candidates(
            "2099-09-21T00:00:00Z",
            "2099-09-22T00:00:00Z",
            &preferences(),
            &[BusyInterval {
                start: "2099-09-21T13:30:00Z".to_string(),
                end: "2099-09-21T14:30:00Z".to_string(),
            }],
            1,
            1,
            None,
        )
        .unwrap();
        assert_eq!(candidates[0].start, "2099-09-21T13:00:00+00:00");
        assert!(candidates
            .iter()
            .all(|candidate| candidate.status == "verified"));
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.start == "2099-09-21T13:30:00+00:00"));
    }

    #[test]
    fn handles_dst_transition_without_creating_invalid_local_times() {
        let mut preferences = preferences();
        preferences.working_windows = vec![AvailabilityWindow {
            weekday: 0,
            start: "01:00".to_string(),
            end: "04:00".to_string(),
        }];
        // Sunday 2099-03-08 is the US spring-forward day: New York local
        // time jumps from 02:00 EST straight to 03:00 EDT.
        let candidates = find_candidates(
            "2099-03-08T05:00:00Z",
            "2099-03-09T04:00:00Z",
            &preferences,
            &[],
            0,
            0,
            None,
        )
        .unwrap();
        assert!(!candidates.is_empty());
        assert!(candidates
            .iter()
            .all(|candidate| candidate.status == "unverified"));
        let slots = candidates
            .iter()
            .map(|candidate| (candidate.start.as_str(), candidate.end.as_str()))
            .collect::<Vec<_>>();
        // 01:00 EST is 06:00Z and 03:00 EDT is 07:00Z. Slots that start or
        // end inside the nonexistent 02:00 local hour are skipped.
        assert_eq!(
            slots,
            vec![
                ("2099-03-08T06:00:00+00:00", "2099-03-08T06:30:00+00:00"),
                ("2099-03-08T07:00:00+00:00", "2099-03-08T07:30:00+00:00"),
                ("2099-03-08T07:30:00+00:00", "2099-03-08T08:00:00+00:00"),
            ]
        );
    }

    #[test]
    fn an_in_person_slot_keeps_the_drive_time_buffer_beside_any_busy_interval() {
        let mut preferences = preferences();
        preferences.drive_time_buffer_minutes = 30;
        // Monday 2099-09-21, 9:00 to 9:30 in New York: a meeting with no location.
        let busy = [BusyInterval {
            start: "2099-09-21T13:00:00Z".to_string(),
            end: "2099-09-21T13:30:00Z".to_string(),
        }];
        let range = ("2099-09-21T04:00:00Z", "2099-09-22T04:00:00Z");
        let starts = |preferences: &AvailabilityPreferences, in_person| {
            find_candidates_for(range.0, range.1, preferences, &busy, 1, 1, None, in_person)
                .unwrap()
                .into_iter()
                .map(|candidate| candidate.start)
                .collect::<Vec<_>>()
        };
        // Not in person, 9:30 is offered right after it.
        assert_eq!(starts(&preferences, false)[0], "2099-09-21T13:30:00+00:00");
        // In person, the first time a full buffer later is 10:00.
        assert_eq!(
            starts(&preferences, true),
            vec![
                "2099-09-21T14:00:00+00:00",
                "2099-09-21T14:30:00+00:00",
                "2099-09-21T15:00:00+00:00",
                "2099-09-21T15:30:00+00:00"
            ]
        );
        // The buffer also applies before the busy interval.
        let early = [BusyInterval {
            start: "2099-09-21T14:00:00Z".to_string(),
            end: "2099-09-21T14:30:00Z".to_string(),
        }];
        let before =
            find_candidates_for(range.0, range.1, &preferences, &early, 1, 1, None, true).unwrap();
        assert!(before
            .iter()
            .all(|c| c.start != "2099-09-21T13:30:00+00:00"));
        assert!(before
            .iter()
            .any(|c| c.start == "2099-09-21T13:00:00+00:00"));
        // With no buffer, in person changes nothing.
        preferences.drive_time_buffer_minutes = 0;
        assert_eq!(starts(&preferences, true), starts(&preferences, false));
    }

    #[test]
    fn checking_an_in_person_time_counts_the_buffer_as_a_conflict() {
        let busy = [BusyInterval {
            start: "2099-09-21T13:00:00Z".to_string(),
            end: "2099-09-21T13:30:00Z".to_string(),
        }];
        let (start, end) = ("2099-09-21T13:40:00Z", "2099-09-21T14:10:00Z");
        assert_eq!(check_time(start, end, &busy, 1, 1).unwrap(), "free");
        let padding = meeting_padding(true, 30);
        assert_eq!(
            check_time_padded(start, end, &busy, 1, 1, padding).unwrap(),
            "conflicting"
        );
        // Not in person, the buffer is not applied.
        assert_eq!(
            check_time_padded(start, end, &busy, 1, 1, meeting_padding(false, 30)).unwrap(),
            "free"
        );
        // Exactly the buffer away is free.
        assert_eq!(
            check_time_padded("2099-09-21T14:00:00Z", end, &busy, 1, 1, padding).unwrap(),
            "free"
        );
    }

    #[test]
    fn a_full_cap_drops_later_slots_not_later_days() {
        let mut preferences = preferences();
        preferences.working_windows = (0..=6)
            .map(|weekday| AvailabilityWindow {
                weekday,
                start: "09:00".to_string(),
                end: "17:00".to_string(),
            })
            .collect();
        // Fourteen days of three slots each is far over the cap.
        let found = find_candidates(
            "2099-09-21T04:00:00Z",
            "2099-10-05T04:00:00Z",
            &preferences,
            &[],
            1,
            1,
            Some(3),
        )
        .unwrap();
        assert_eq!(found.len(), MAX_CANDIDATES);
        for day in 21..=30 {
            let prefix = format!("2099-09-{day}");
            assert!(
                found.iter().any(|c| c.start.starts_with(&prefix)),
                "{prefix}"
            );
        }
        for day in 1..=4 {
            let prefix = format!("2099-10-0{day}");
            assert!(
                found.iter().any(|c| c.start.starts_with(&prefix)),
                "{prefix}"
            );
        }
        // Fourteen first slots, then six second slots on the earliest days.
        assert_eq!(
            found
                .iter()
                .filter(|c| c.start.starts_with("2099-09-21"))
                .count(),
            2
        );
        assert_eq!(
            found
                .iter()
                .filter(|c| c.start.starts_with("2099-10-04"))
                .count(),
            1
        );
        let mut sorted = found.iter().map(|c| c.start.clone()).collect::<Vec<_>>();
        sorted.sort();
        assert_eq!(
            sorted,
            found.iter().map(|c| c.start.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_per_day_limit_spreads_candidates_across_days() {
        let mut preferences = preferences();
        preferences.working_windows = (1..=5)
            .map(|weekday| AvailabilityWindow {
                weekday,
                start: "09:00".to_string(),
                end: "17:00".to_string(),
            })
            .collect();
        // Monday 2099-09-21 through Friday 2099-09-25, New York time.
        let range = ("2099-09-21T04:00:00Z", "2099-09-26T04:00:00Z");
        let busy = [BusyInterval {
            start: "2099-09-22T13:00:00Z".to_string(),
            end: "2099-09-22T15:00:00Z".to_string(),
        }];
        let unlimited = find_candidates(range.0, range.1, &preferences, &busy, 1, 1, None).unwrap();
        assert_eq!(unlimited.len(), MAX_CANDIDATES);
        // Without a limit, Monday's sixteen half-hour slots come first.
        assert_eq!(
            unlimited
                .iter()
                .filter(|candidate| candidate.start.starts_with("2099-09-21"))
                .count(),
            16
        );
        assert_eq!(unlimited[1].start, "2099-09-21T13:30:00+00:00");

        let spread = find_candidates(range.0, range.1, &preferences, &busy, 1, 1, Some(1)).unwrap();
        let starts = spread
            .iter()
            .map(|candidate| candidate.start.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            starts,
            vec![
                "2099-09-21T13:00:00+00:00",
                "2099-09-22T15:00:00+00:00",
                "2099-09-23T13:00:00+00:00",
                "2099-09-24T13:00:00+00:00",
                "2099-09-25T13:00:00+00:00",
            ]
        );
        assert_eq!(
            find_candidates(range.0, range.1, &preferences, &busy, 1, 1, Some(2))
                .unwrap()
                .len(),
            10
        );
        assert_eq!(
            find_candidates(
                range.0,
                range.1,
                &preferences,
                &busy,
                1,
                1,
                Some(MAX_CANDIDATES)
            )
            .unwrap()
            .len(),
            MAX_CANDIDATES
        );
        assert!(find_candidates(range.0, range.1, &preferences, &busy, 1, 1, Some(0)).is_err());
        assert!(find_candidates(
            range.0,
            range.1,
            &preferences,
            &busy,
            1,
            1,
            Some(MAX_CANDIDATES + 1)
        )
        .is_err());
    }

    fn window(weekday: u8, start: &str, end: &str) -> AvailabilityWindow {
        AvailabilityWindow {
            weekday,
            start: start.to_string(),
            end: end.to_string(),
        }
    }

    fn slot_starts(preferences: &AvailabilityPreferences) -> Vec<String> {
        // Monday 2099-09-21, New York time (UTC-4).
        find_candidates(
            "2099-09-21T04:00:00Z",
            "2099-09-22T04:00:00Z",
            preferences,
            &[],
            1,
            1,
            None,
        )
        .unwrap()
        .into_iter()
        .map(|candidate| candidate.start)
        .collect()
    }

    #[test]
    fn several_windows_on_one_day_leave_the_gaps_unavailable() {
        let mut preferences = preferences();
        preferences.slot_increment_minutes = 60;
        preferences.working_windows =
            vec![window(1, "13:00", "15:00"), window(1, "09:00", "12:00")];
        assert_eq!(
            slot_starts(&preferences),
            vec![
                "2099-09-21T13:00:00+00:00",
                "2099-09-21T14:00:00+00:00",
                "2099-09-21T15:00:00+00:00",
                "2099-09-21T17:00:00+00:00",
                "2099-09-21T18:00:00+00:00",
            ]
        );
    }

    #[test]
    fn overlapping_or_touching_windows_merge_without_duplicate_slots() {
        let mut preferences = preferences();
        preferences.slot_increment_minutes = 60;
        preferences.working_windows = vec![
            window(1, "09:00", "11:00"),
            window(1, "10:00", "12:00"),
            window(1, "12:00", "13:00"),
        ];
        let starts = slot_starts(&preferences);
        assert_eq!(starts.len(), 4);
        assert_eq!(starts.first().unwrap(), "2099-09-21T13:00:00+00:00");
        assert_eq!(starts.last().unwrap(), "2099-09-21T16:00:00+00:00");
    }

    #[test]
    fn working_window_count_is_limited() {
        let mut preferences = preferences();
        preferences.working_windows = (0..MAX_WORKING_WINDOWS)
            .map(|index| {
                window(
                    (index % 7) as u8,
                    &format!("{:02}:00", index / 7 * 2 + 8),
                    &format!("{:02}:00", index / 7 * 2 + 9),
                )
            })
            .collect();
        assert!(validate_preferences(&preferences).is_ok());
        preferences
            .working_windows
            .push(window(0, "20:00", "21:00"));
        assert!(validate_preferences(&preferences).is_err());
    }

    #[test]
    fn proposed_time_reports_coverage_state() {
        assert_eq!(
            check_time("2026-09-21T13:00:00Z", "2026-09-21T13:30:00Z", &[], 0, 0).unwrap(),
            "unverified"
        );
        assert_eq!(
            check_time("2026-09-21T13:00:00Z", "2026-09-21T13:30:00Z", &[], 1, 2).unwrap(),
            "partiallyChecked"
        );
    }
}
