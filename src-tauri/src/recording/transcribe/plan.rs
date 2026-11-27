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

/// One track's segment files in order.
fn plan_track(audio_dir: &Path, track: Track, finished: bool) -> Vec<PlannedSegment> {
    let entries = segments::read_manifest(audio_dir, track);
    // Each manifest line: a usable segment, a skipped one, or a gap.
    enum Line {
        Usable(u32, u64, PathBuf, u64),
        Skipped,
        Gap,
    }
    let lines: Vec<Line> = entries
        .iter()
        .map(|entry| match entry {
            ManifestEntry::Segment {
                segment,
                file,
                start_ms,
            } => match checked_segment(audio_dir, track, *segment, file) {
                Some((path, audio_ms)) => Line::Usable(*segment, *start_ms, path, audio_ms),
                None => Line::Skipped,
            },
            ManifestEntry::Gap { .. } => Line::Gap,
        })
        .collect();
    let mut planned = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Line::Usable(number, start_ms, path, audio_ms) = line else {
            continue;
        };
        let continues = index > 0 && matches!(lines[index - 1], Line::Usable(..));
        let next = lines.get(index + 1);
        let closed = next.is_some() || finished;
        let ends_run = match next {
            Some(Line::Usable(..)) => false,
            Some(Line::Gap | Line::Skipped) => true,
            None => finished,
        };
        planned.push(PlannedSegment {
            request: SegmentRequest {
                track,
                wav_path: path.clone(),
                start_ms: *start_ms,
                continues,
                ends_run,
            },
            number: *number,
            audio_ms: *audio_ms,
            closed,
        });
    }
    planned
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
    fn a_recording_without_audio_plans_nothing() {
        let dir = TempDir::new();
        assert!(plan(dir.path(), true).is_empty());
        assert!(plan(&dir.path().join("missing"), true).is_empty());
    }
}
