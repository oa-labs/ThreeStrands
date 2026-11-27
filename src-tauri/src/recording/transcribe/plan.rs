//! Which segment files a recording has to transcribe, read from its track
//! manifests (see `segments.rs`).
//!
//! A file is **closed** once a later manifest line exists (the next segment
//! opened, or a gap was recorded) or the recording has finished; only closed
//! files, whose WAV headers are final, are transcribed. A file
//! **continues** the run when the line before it is the previous segment
//! (no gap between them), and **ends the run** when a gap follows it or it
//! is the last file of a finished recording.
//!
//! Every path handed to the worker is checked: its name must be exactly the
//! one the segment writer gives that segment, and it must resolve (symbolic
//! links followed) to a file directly inside the recording's audio folder. A
//! file that fails, or is missing or empty, is skipped, and the runs either
//! side of it are treated as separate.

use super::protocol::SegmentRequest;
use crate::recording::capture::Track;
use crate::recording::segments::{self, ManifestEntry};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedSegment {
    pub request: SegmentRequest,
    pub number: u32,
    pub audio_ms: u64,
    pub closed: bool,
}

/// The path of `file` for segment `number` of `track`, if it is the name the
/// segment writer gives it and it resolves to a non-empty file directly
/// inside `audio_dir`. Returns the file's audio length too.
pub fn checked_segment(
    audio_dir: &Path,
    track: Track,
    number: u32,
    file: &str,
) -> Option<(PathBuf, u64)> {
    if file != segments::segment_file_name(track, number) {
        return None;
    }
    let dir = std::fs::canonicalize(audio_dir).ok()?;
    let path = std::fs::canonicalize(dir.join(file)).ok()?;
    if path.parent() != Some(dir.as_path()) {
        return None;
    }
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let samples = metadata.len().saturating_sub(44) / 2;
    let audio_ms = samples * 1000 / crate::recording::policy::OUTPUT_SAMPLE_RATE as u64;
    (audio_ms > 0).then_some((path, audio_ms))
}

/// Each manifest line: a usable segment, a skipped one, or a gap.
enum Line {
    Usable(u32, u64, PathBuf, u64),
    Skipped,
    Gap,
}

/// One track's manifest, read line by line so that scanning a long
/// recording holds one line at a time, skipping a torn last line or any
/// line that doesn't parse (as [`segments::read_manifest`] does).
fn manifest_lines(audio_dir: &Path, track: Track) -> impl Iterator<Item = Line> + '_ {
    use std::io::BufRead;
    let file = std::fs::File::open(segments::manifest_path(audio_dir, track)).ok();
    file.into_iter()
        .flat_map(|file| std::io::BufReader::new(file).lines())
        .map_while(Result::ok)
        .filter_map(|text| serde_json::from_str::<ManifestEntry>(&text).ok())
        .map(move |entry| match entry {
            ManifestEntry::Segment {
                segment,
                file,
                start_ms,
            } => match checked_segment(audio_dir, track, segment, &file) {
                Some((path, audio_ms)) => Line::Usable(segment, start_ms, path, audio_ms),
                None => Line::Skipped,
            },
            ManifestEntry::Gap { .. } => Line::Gap,
        })
}

/// One track's segment files in order, each handed to `visit` as soon as
/// the line after it is read.
fn scan_track(
    audio_dir: &Path,
    track: Track,
    finished: bool,
    mut visit: impl FnMut(PlannedSegment),
) {
    let segment =
        |number, start_ms, path, audio_ms, continues, next: Option<&Line>| PlannedSegment {
            request: SegmentRequest {
                track,
                wav_path: path,
                start_ms,
                continues,
                ends_run: match next {
                    Some(Line::Usable(..)) => false,
                    Some(Line::Gap | Line::Skipped) => true,
                    None => finished,
                },
                carry_in: None,
            },
            number,
            audio_ms,
            closed: next.is_some() || finished,
        };
    // The usable line waiting for the line after it, and whether the line
    // before it was usable too.
    let mut pending: Option<(Line, bool)> = None;
    let mut previous_usable = false;
    for line in manifest_lines(audio_dir, track) {
        let usable = matches!(line, Line::Usable(..));
        if let Some((Line::Usable(number, start_ms, path, audio_ms), continues)) = pending.take() {
            visit(segment(
                number,
                start_ms,
                path,
                audio_ms,
                continues,
                Some(&line),
            ));
        }
        if usable {
            pending = Some((line, previous_usable));
        }
        previous_usable = usable;
    }
    if let Some((Line::Usable(number, start_ms, path, audio_ms), continues)) = pending {
        visit(segment(number, start_ms, path, audio_ms, continues, None));
    }
}

/// One track's segment files in order.
fn plan_track(audio_dir: &Path, track: Track, finished: bool) -> Vec<PlannedSegment> {
    let mut planned = Vec::new();
    scan_track(audio_dir, track, finished, |segment| planned.push(segment));
    planned
}

/// Where a job stands, found without listing every file, so a step of a
/// long recording's job takes the same memory as a short one's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Standing {
    /// The first closed file not done yet, in [`plan`]'s order.
    pub next: Option<PlannedSegment>,
    /// Closed audio not done yet.
    pub waiting_ms: u64,
    /// All the audio.
    pub total_ms: u64,
    /// Where the first system audio file starts.
    pub system_start_ms: Option<u64>,
}

/// [`Standing`] for a job that has done the files `done` says it has.
/// `finished` is false while the recording is still going.
pub fn standing(
    audio_dir: &Path,
    finished: bool,
    done: impl Fn(&PlannedSegment) -> bool,
) -> Standing {
    let mut standing = Standing::default();
    for track in Track::ALL {
        let mut first = true;
        scan_track(audio_dir, track, finished, |segment| {
            standing.total_ms += segment.audio_ms;
            if track == Track::System && first {
                standing.system_start_ms = Some(segment.request.start_ms);
            }
            first = false;
            if !segment.closed || done(&segment) {
                return;
            }
            standing.waiting_ms += segment.audio_ms;
            let key = |s: &PlannedSegment| (s.request.start_ms, s.request.track, s.number);
            if standing
                .next
                .as_ref()
                .is_none_or(|next| key(&segment) < key(next))
            {
                standing.next = Some(segment);
            }
        });
    }
    standing
}

/// Whether `path` is one of `track`'s segment files in `audio_dir`, as the
/// writer names them and [`checked_segment`] accepts them.
pub fn is_segment_file(audio_dir: &Path, track: Track, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let number = name
        .strip_prefix(track.as_str())
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.strip_suffix(".wav"))
        .and_then(|digits| digits.parse::<u32>().ok());
    number
        .and_then(|number| checked_segment(audio_dir, track, number, name))
        .is_some_and(|(checked, _)| checked == path)
}

/// Both tracks' segment files, in order of where they start (the microphone
/// first on a tie). `finished` is false while the recording is still going.
pub fn plan(audio_dir: &Path, finished: bool) -> Vec<PlannedSegment> {
    let mut planned: Vec<PlannedSegment> = Track::ALL
        .into_iter()
        .flat_map(|track| plan_track(audio_dir, track, finished))
        .collect();
    planned.sort_by_key(|segment| {
        (
            segment.request.start_ms,
            segment.request.track,
            segment.number,
        )
    });
    planned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::segments::{GapReason, SegmentWriter};
    use crate::recording::test_support::TempDir;

    fn seconds(count: usize) -> Vec<i16> {
        vec![1; count * 16_000]
    }

    fn summary(planned: &[PlannedSegment]) -> Vec<(Track, u32, u64, u64, bool, bool, bool)> {
        planned
            .iter()
            .map(|p| {
                (
                    p.request.track,
                    p.number,
                    p.request.start_ms,
                    p.audio_ms,
                    p.request.continues,
                    p.request.ends_run,
                    p.closed,
                )
            })
            .collect()
    }

    #[test]
    fn runs_and_closed_files_follow_the_manifest() {
        let dir = TempDir::new();
        let mut mic = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        // 90 s, a pause, then 10 s still being written.
        mic.write(&seconds(90), 0).unwrap();
        mic.end_run(GapReason::Pause).unwrap();
        mic.write(&seconds(10), 120_000).unwrap();
        mic.flush().unwrap();
        let mut system = SegmentWriter::open(dir.path(), Track::System).unwrap();
        system.write(&seconds(61), 5_000).unwrap();
        system.flush().unwrap();

        let live = plan(dir.path(), false);
        assert_eq!(
            summary(&live),
            vec![
                (Track::Mic, 1, 0, 60_000, false, false, true),
                (Track::System, 1, 5_000, 60_000, false, false, true),
                (Track::Mic, 2, 60_000, 30_000, true, true, true),
                (Track::System, 2, 65_000, 1_000, true, false, false),
                (Track::Mic, 3, 120_000, 10_000, false, false, false),
            ]
        );
        drop(mic);
        drop(system);
        let done = plan(dir.path(), true);
        let ends: Vec<(Track, u32, bool, bool)> = done
            .iter()
            .map(|p| (p.request.track, p.number, p.request.ends_run, p.closed))
            .collect();
        assert_eq!(
            ends,
            vec![
                (Track::Mic, 1, false, true),
                (Track::System, 1, false, true),
                (Track::Mic, 2, true, true),
                (Track::System, 2, true, true),
                (Track::Mic, 3, true, true),
            ]
        );
    }

    #[test]
    fn an_interrupted_recording_ends_its_last_run_at_its_last_file() {
        let dir = TempDir::new();
        let mut mic = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        mic.write(&seconds(70), 0).unwrap();
        mic.flush().unwrap();
        std::mem::forget(mic); // the crash
        segments::repair_dir(dir.path()).unwrap();
        let planned = plan(dir.path(), true);
        assert_eq!(
            summary(&planned),
            vec![
                (Track::Mic, 1, 0, 60_000, false, false, true),
                (Track::Mic, 2, 60_000, 10_000, true, true, true),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn files_outside_the_folder_or_misnamed_are_skipped_and_split_the_run() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        let mut mic = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        mic.write(&seconds(180), 0).unwrap();
        drop(mic);
        // Segment 2 replaced by a link to a file elsewhere.
        let elsewhere = outside.path().join("mic-0002.wav");
        std::fs::rename(dir.path().join("mic-0002.wav"), &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.path().join("mic-0002.wav")).unwrap();
        let planned = plan(dir.path(), true);
        assert_eq!(
            summary(&planned),
            vec![
                (Track::Mic, 1, 0, 60_000, false, true, true),
                (Track::Mic, 3, 120_000, 60_000, false, true, true),
            ]
        );
        assert!(checked_segment(dir.path(), Track::Mic, 1, "../mic-0001.wav").is_none());
        assert!(checked_segment(dir.path(), Track::Mic, 1, "mic-0003.wav").is_none());
        assert!(checked_segment(dir.path(), Track::System, 1, "mic-0001.wav").is_none());
        assert!(checked_segment(dir.path(), Track::Mic, 1, "mic-0001.wav").is_some());
        assert!(
            checked_segment(dir.path(), Track::Mic, 9, "mic-0009.wav").is_none(),
            "missing"
        );
        // A manifest line pointing outside the folder is never planned.
        std::fs::write(
            segments::manifest_path(dir.path(), Track::System),
            "{\"segment\":1,\"file\":\"../../etc/passwd\",\"startMs\":0}\n",
        )
        .unwrap();
        assert!(plan(dir.path(), true)
            .iter()
            .all(|p| p.request.track == Track::Mic));
    }

    #[test]
    fn standing_matches_the_plan_without_listing_it() {
        let dir = TempDir::new();
        let mut mic = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        mic.write(&seconds(90), 0).unwrap();
        mic.end_run(GapReason::Pause).unwrap();
        mic.write(&seconds(70), 120_000).unwrap();
        mic.flush().unwrap();
        let mut system = SegmentWriter::open(dir.path(), Track::System).unwrap();
        system.write(&seconds(121), 5_000).unwrap();
        system.flush().unwrap();
        for finished in [false, true] {
            let planned = plan(dir.path(), finished);
            for done_through in 0..4 {
                let done = |s: &PlannedSegment| s.number <= done_through;
                let standing = standing(dir.path(), finished, done);
                let undone: Vec<&PlannedSegment> =
                    planned.iter().filter(|s| s.closed && !done(s)).collect();
                assert_eq!(standing.next.as_ref(), undone.first().copied());
                assert_eq!(
                    standing.waiting_ms,
                    undone.iter().map(|s| s.audio_ms).sum::<u64>()
                );
                assert_eq!(
                    standing.total_ms,
                    planned.iter().map(|s| s.audio_ms).sum::<u64>()
                );
                assert_eq!(standing.system_start_ms, Some(5_000));
            }
        }
        assert_eq!(standing(dir.path(), true, |_| true).next, None);
        assert_eq!(
            standing(&dir.path().join("missing"), true, |_| false),
            Standing::default()
        );
    }

    #[test]
    fn only_the_tracks_own_segment_files_are_recognized() {
        let dir = TempDir::new();
        let mut mic = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        mic.write(&seconds(61), 0).unwrap();
        drop(mic);
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let first = canonical.join("mic-0001.wav");
        assert!(is_segment_file(dir.path(), Track::Mic, &first));
        assert!(!is_segment_file(dir.path(), Track::System, &first));
        assert!(!is_segment_file(
            dir.path(),
            Track::Mic,
            &canonical.join("mic-0009.wav")
        ));
        assert!(!is_segment_file(
            dir.path(),
            Track::Mic,
            &canonical.join("x").join("..").join("mic-0001.wav")
        ));
        assert!(!is_segment_file(
            dir.path(),
            Track::Mic,
            Path::new("/etc/mic-0001.wav")
        ));
        assert!(!is_segment_file(
            dir.path(),
            Track::Mic,
            Path::new("mic-0001.wav")
        ));
    }

    #[test]
    fn a_recording_without_audio_plans_nothing() {
        let dir = TempDir::new();
        assert!(plan(dir.path(), true).is_empty());
        assert!(plan(&dir.path().join("missing"), true).is_empty());
    }
}
