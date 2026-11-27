//! Audio on disk: 16 kHz mono 16-bit WAV segments, their manifest, and
//! crash recovery.
//!
//! **Layout.** A recording's `audio_dir` holds, per track, 60-second files
//! `mic-0001.wav`, `mic-0002.wav`, … (`system-…` for system audio) and a
//! manifest `mic.jsonl` (`system.jsonl`). Each manifest line is one JSON
//! object, appended and written through before the audio it describes:
//!
//! - `{"segment":1,"file":"mic-0001.wav","startMs":1234}` when a segment
//!   opens. `startMs` is where its first sample sits on the recording's
//!   clock: milliseconds since the recording started, pauses and sleep
//!   included, so it stays true to the wall clock. The segment's length
//!   comes from its WAV data size.
//! - `{"gap":"pause","afterSegment":3,"atMs":185000}` when a run of
//!   contiguous audio ends early: a pause, a device change, a dropout
//!   (sleep, or audio lost to an overrun), a restart, or the recording
//!   stopping. `atMs` is where the audio ended. The next segment's
//!   `startMs` says where audio resumed.
//!
//! A manifest beside the files, rather than a table, keeps the timing with
//! the audio it describes: deleting the audio deletes both, and recovery
//! needs no database write per segment.
//!
//! **Durability.** Data is flushed to the file about once a second. A WAV
//! header holds placeholder sizes until its segment closes, when the sizes
//! are written and the file is synced. After a crash, the open segment is
//! repaired at the next launch from its length ([`repair_dir`]), so at most
//! the last second of audio is lost.

use crate::recording::capture::Track;
use crate::recording::files;
use crate::recording::policy;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const HEADER_BYTES: u64 = 44;
const BYTES_PER_SAMPLE: u64 = 2;

/// Samples in one full segment.
pub fn segment_samples() -> u64 {
    policy::SEGMENT_LENGTH.as_secs() * policy::OUTPUT_SAMPLE_RATE as u64
}

fn samples_to_ms(samples: u64) -> u64 {
    samples * 1000 / policy::OUTPUT_SAMPLE_RATE as u64
}

/// The 44-byte header of a 16 kHz mono 16-bit PCM WAV file holding
/// `data_bytes` of samples.
pub fn wav_header(data_bytes: u32) -> [u8; 44] {
    let rate = policy::OUTPUT_SAMPLE_RATE;
    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&data_bytes.saturating_add(36).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    header[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
    header[24..28].copy_from_slice(&rate.to_le_bytes());
    header[28..32].copy_from_slice(&(rate * BYTES_PER_SAMPLE as u32).to_le_bytes());
    header[32..34].copy_from_slice(&(BYTES_PER_SAMPLE as u16).to_le_bytes());
    header[34..36].copy_from_slice(&16u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    header
}

/// One WAV file being written.
pub struct WavWriter {
    file: BufWriter<File>,
    samples: u64,
}

impl WavWriter {
    /// Creates the file with placeholder sizes (zero), which a crash leaves
    /// for [`repair_wav`] to fix.
    pub fn create(path: &Path) -> io::Result<Self> {
        let mut file = BufWriter::new(files::create_private_file(path)?);
        file.write_all(&wav_header(0))?;
        Ok(Self { file, samples: 0 })
    }

    pub fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        for sample in samples {
            self.file.write_all(&sample.to_le_bytes())?;
        }
        self.samples += samples.len() as u64;
        Ok(())
    }

    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Hands buffered samples to the operating system.
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    /// Writes the real sizes into the header and syncs the file.
    pub fn finish(mut self) -> io::Result<()> {
        self.file.flush()?;
        let data_bytes = u32::try_from(self.samples * BYTES_PER_SAMPLE).unwrap_or(u32::MAX);
        let header = wav_header(data_bytes);
        let file = self.file.get_mut();
        file.seek(SeekFrom::Start(4))?;
        file.write_all(&header[4..8])?;
        file.seek(SeekFrom::Start(40))?;
        file.write_all(&header[40..44])?;
        file.sync_data()
    }
}

/// What [`repair_wav`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repair {
    /// The header already matched the file.
    Intact,
    /// The header's sizes were rewritten from the file length.
    Repaired,
    /// Too short to hold a header; the file was removed.
    Removed,
    /// Not a WAV file this app wrote; left alone.
    Foreign,
}

/// Fixes the header of a WAV segment whose writer never finished (the app
/// crashed or lost power), using the file's length. A trailing odd byte, half
/// a sample, is cut off.
pub fn repair_wav(path: &Path) -> io::Result<Repair> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let length = file.metadata()?.len();
    if length < HEADER_BYTES {
        drop(file);
        std::fs::remove_file(path)?;
        return Ok(Repair::Removed);
    }
    let mut header = [0u8; 44];
    file.read_exact(&mut header)?;
    let expected = wav_header(0);
    let ours = header[0..4] == expected[0..4] && header[8..40] == expected[8..40];
    if !ours {
        return Ok(Repair::Foreign);
    }
    let data_bytes = (length - HEADER_BYTES) & !1;
    let data_bytes = u32::try_from(data_bytes).unwrap_or(u32::MAX & !1);
    let stored_data = u32::from_le_bytes(header[40..44].try_into().unwrap());
    let stored_riff = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let truncate = length != HEADER_BYTES + data_bytes as u64;
    if stored_data == data_bytes && stored_riff == data_bytes + 36 && !truncate {
        return Ok(Repair::Intact);
    }
    let fixed = wav_header(data_bytes);
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&fixed[4..8])?;
    file.seek(SeekFrom::Start(40))?;
    file.write_all(&fixed[40..44])?;
    if truncate {
        file.set_len(HEADER_BYTES + data_bytes as u64)?;
    }
    file.sync_data()?;
    Ok(Repair::Repaired)
}

/// Why a run of contiguous audio ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GapReason {
    Pause,
    DeviceChange,
    /// The device stopped delivering for a while (sleep, or an overrun).
    Dropout,
    /// The track was restarted after a failure or stall.
    Restart,
    Stop,
}

/// One manifest line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ManifestEntry {
    #[serde(rename_all = "camelCase")]
    Segment {
        segment: u32,
        file: String,
        start_ms: u64,
        /// Duration in milliseconds recorded when transcoded to M4A/AAC.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio_ms: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Gap {
        gap: GapReason,
        after_segment: u32,
        at_ms: u64,
    },
}

pub fn manifest_path(dir: &Path, track: Track) -> PathBuf {
    dir.join(format!("{}.jsonl", track.as_str()))
}

pub fn segment_file_name(track: Track, segment: u32) -> String {
    format!("{}-{segment:04}.wav", track.as_str())
}

pub fn transcoded_file_name(track: Track, segment: u32) -> String {
    format!("{}-{segment:04}.m4a", track.as_str())
}

/// Reads a manifest, skipping a torn last line or any line it can't parse.
pub fn read_manifest(dir: &Path, track: Track) -> Vec<ManifestEntry> {
    let Ok(text) = std::fs::read_to_string(manifest_path(dir, track)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

struct OpenSegment {
    number: u32,
    start_ms: u64,
    wav: WavWriter,
}

/// Writes one track's 16 kHz samples as 60-second segments.
///
/// The capture thread calls [`Self::write`] with each block of resampled
/// samples and the recording-clock time of its first sample. That time
/// places a segment when it opens; inside a run, samples are contiguous by
/// construction, so it is otherwise unused. [`Self::end_run`] closes the
/// current segment and records a gap.
pub struct SegmentWriter {
    dir: PathBuf,
    track: Track,
    manifest: File,
    last_number: u32,
    open: Option<OpenSegment>,
}

impl SegmentWriter {
    /// Opens the writer for `track` in `dir`, continuing the numbering of
    /// segments already there (a restarted track keeps its earlier files).
    pub fn open(dir: &Path, track: Track) -> io::Result<Self> {
        files::ensure_private_dir(dir)?;
        let last_number = read_manifest(dir, track)
            .iter()
            .filter_map(|entry| match entry {
                ManifestEntry::Segment { segment, .. } => Some(*segment),
                ManifestEntry::Gap { .. } => None,
            })
            .max()
            .unwrap_or(0);
        let manifest = files::open_append_private_file(&manifest_path(dir, track))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            track,
            manifest,
            last_number,
            open: None,
        })
    }

    fn append(&mut self, entry: &ManifestEntry) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
        line.push(b'\n');
        self.manifest.write_all(&line)?;
        self.manifest.flush()
    }

    fn open_segment(&mut self, start_ms: u64) -> io::Result<()> {
        let number = self.last_number + 1;
        let file = segment_file_name(self.track, number);
        self.append(&ManifestEntry::Segment {
            segment: number,
            file: file.clone(),
            start_ms,
            audio_ms: None,
        })?;
        let wav = WavWriter::create(&self.dir.join(&file))?;
        self.last_number = number;
        self.open = Some(OpenSegment {
            number,
            start_ms,
            wav,
        });
        Ok(())
    }

    fn close_segment(&mut self) -> io::Result<Option<(u32, u64)>> {
        let Some(segment) = self.open.take() else {
            return Ok(None);
        };
        let end_ms = segment.start_ms + samples_to_ms(segment.wav.samples());
        segment.wav.finish()?;
        Ok(Some((segment.number, end_ms)))
    }

    /// Writes samples whose first one sits at `first_at_ms` on the
    /// recording's clock. A full segment closes at exactly
    /// [`segment_samples`] and the next opens where it ended.
    pub fn write(&mut self, mut samples: &[i16], first_at_ms: u64) -> io::Result<()> {
        let mut at_ms = first_at_ms;
        while !samples.is_empty() {
            if self.open.is_none() {
                self.open_segment(at_ms)?;
            }
            let segment = self.open.as_mut().expect("a segment is open");
            let room = (segment_samples() - segment.wav.samples()) as usize;
            let take = room.min(samples.len());
            segment.wav.write(&samples[..take])?;
            samples = &samples[take..];
            if segment.wav.samples() == segment_samples() {
                let (_, end_ms) = self.close_segment()?.expect("a segment was open");
                at_ms = end_ms;
            } else {
                at_ms += samples_to_ms(take as u64);
            }
        }
        Ok(())
    }

    /// Ends the current run of contiguous audio: closes the open segment
    /// and records the gap. Does nothing when no audio was written since
    /// the last gap.
    pub fn end_run(&mut self, reason: GapReason) -> io::Result<()> {
        if let Some((number, end_ms)) = self.close_segment()? {
            self.append(&ManifestEntry::Gap {
                gap: reason,
                after_segment: number,
                at_ms: end_ms,
            })?;
        }
        Ok(())
    }

    /// Hands buffered samples to the operating system.
    pub fn flush(&mut self) -> io::Result<()> {
        match self.open.as_mut() {
            Some(segment) => segment.wav.flush(),
            None => Ok(()),
        }
    }

    /// Whether a segment is open (audio has been written since the last gap).
    #[cfg(test)]
    pub fn in_run(&self) -> bool {
        self.open.is_some()
    }
}

impl Drop for SegmentWriter {
    fn drop(&mut self) {
        let _ = self.end_run(GapReason::Stop);
    }
}

/// The audio a track kept, from its manifest and files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SavedTrack {
    /// Total length of its segments.
    pub audio_ms: u64,
    /// Where its last segment ends on the recording's clock.
    pub end_ms: u64,
}

/// What a track's files hold, after any repair.
pub fn saved_track(dir: &Path, track: Track) -> SavedTrack {
    let mut saved = SavedTrack::default();
    for entry in read_manifest(dir, track) {
        let ManifestEntry::Segment {
            file,
            start_ms,
            audio_ms,
            ..
        } = entry
        else {
            continue;
        };
        let path = dir.join(&file);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let length = if file.ends_with(".m4a") {
            audio_ms.unwrap_or_else(|| {
                crate::recording::transcribe::transcode::m4a_duration_ms(&path).unwrap_or(0)
            })
        } else {
            let samples = metadata.len().saturating_sub(HEADER_BYTES) / BYTES_PER_SAMPLE;
            samples_to_ms(samples)
        };
        saved.audio_ms += length;
        saved.end_ms = saved.end_ms.max(start_ms + length);
    }
    saved
}

/// Startup repair of a recording's directory: fixes every segment header
/// the crash left unfinished. Returns how many files were repaired or
/// removed.
pub fn repair_dir(dir: &Path) -> io::Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut changed = 0;
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wav") {
            continue;
        }
        match repair_wav(&path)? {
            Repair::Repaired | Repair::Removed => changed += 1,
            Repair::Intact | Repair::Foreign => {}
        }
    }
    Ok(changed)
}

/// Bytes of audio and manifest files under `dir`, recursively.
pub fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_bytes(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::test_support::TempDir;

    fn temp() -> TempDir {
        TempDir::new()
    }

    fn wav_samples(path: &Path) -> (u32, Vec<i16>) {
        let bytes = std::fs::read(path).unwrap();
        let data_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        let samples = bytes[44..]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        (data_size, samples)
    }

    #[test]
    fn the_header_describes_16_khz_mono_16_bit_pcm() {
        let header = wav_header(320);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(header[4..8].try_into().unwrap()), 356);
        assert_eq!(&header[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([header[20], header[21]]), 1, "PCM");
        assert_eq!(u16::from_le_bytes([header[22], header[23]]), 1, "mono");
        assert_eq!(
            u32::from_le_bytes(header[24..28].try_into().unwrap()),
            16_000
        );
        assert_eq!(
            u32::from_le_bytes(header[28..32].try_into().unwrap()),
            32_000
        );
        assert_eq!(u16::from_le_bytes([header[32], header[33]]), 2);
        assert_eq!(u16::from_le_bytes([header[34], header[35]]), 16);
        assert_eq!(&header[36..40], b"data");
        assert_eq!(u32::from_le_bytes(header[40..44].try_into().unwrap()), 320);
    }

    #[test]
    fn a_writer_leaves_placeholder_sizes_until_finished() {
        let dir = temp();
        let path = dir.path().join("a.wav");
        let mut writer = WavWriter::create(&path).unwrap();
        writer.write(&[1, -2, 3]).unwrap();
        writer.flush().unwrap();
        assert_eq!(
            wav_samples(&path),
            (0, vec![1, -2, 3]),
            "flushed, not finalized"
        );
        writer.finish().unwrap();
        assert_eq!(wav_samples(&path), (6, vec![1, -2, 3]));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 42);
        assert!(
            WavWriter::create(&path).is_err(),
            "never overwrites a segment"
        );
    }

    #[test]
    fn a_truncated_header_is_repaired_from_the_file_length() {
        let dir = temp();
        let path = dir.path().join("mic-0003.wav");
        let mut writer = WavWriter::create(&path).unwrap();
        writer.write(&[5; 1000]).unwrap();
        writer.flush().unwrap();
        std::mem::forget(writer); // the crash: never finished
                                  // Half a sample made it to disk too.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[7]).unwrap();
        drop(file);
        assert_eq!(repair_wav(&path).unwrap(), Repair::Repaired);
        let (size, samples) = wav_samples(&path);
        assert_eq!(size, 2000);
        assert_eq!(samples.len(), 1000);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 2044);
        assert_eq!(repair_wav(&path).unwrap(), Repair::Intact);
    }

    #[test]
    fn repair_removes_a_file_without_a_whole_header_and_ignores_foreign_files() {
        let dir = temp();
        let short = dir.path().join("mic-0001.wav");
        std::fs::write(&short, b"RIFF").unwrap();
        assert_eq!(repair_wav(&short).unwrap(), Repair::Removed);
        assert!(!short.exists());
        let foreign = dir.path().join("other.wav");
        let mut bytes = wav_header(0).to_vec();
        bytes[22] = 2; // stereo: not ours
        bytes.extend_from_slice(&[0; 8]);
        std::fs::write(&foreign, &bytes).unwrap();
        assert_eq!(repair_wav(&foreign).unwrap(), Repair::Foreign);
        assert_eq!(std::fs::read(&foreign).unwrap(), bytes);
    }

    #[test]
    fn segments_roll_over_at_exactly_60_seconds() {
        let dir = temp();
        let mut writer = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        let full = segment_samples() as usize;
        assert_eq!(full, 960_000);
        // 59.999 s, then a block that crosses the boundary by 1.5 s.
        let first: Vec<i16> = vec![1; full - 16];
        writer.write(&first, 250).unwrap();
        let second: Vec<i16> = vec![2; 16 + 24_000];
        writer.write(&second, 250 + 59_999).unwrap();
        drop(writer);
        let (size, samples) = wav_samples(&dir.path().join("mic-0001.wav"));
        assert_eq!(size as usize, full * 2);
        assert_eq!(samples.len(), full);
        assert_eq!(
            &samples[full - 17..],
            &[1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2]
        );
        let (_, next) = wav_samples(&dir.path().join("mic-0002.wav"));
        assert_eq!(next.len(), 24_000);
        assert_eq!(
            read_manifest(dir.path(), Track::Mic),
            vec![
                ManifestEntry::Segment {
                    segment: 1,
                    file: "mic-0001.wav".into(),
                    start_ms: 250,
                    audio_ms: None,
                },
                ManifestEntry::Segment {
                    segment: 2,
                    file: "mic-0002.wav".into(),
                    start_ms: 60_250,
                    audio_ms: None,
                },
                ManifestEntry::Gap {
                    gap: GapReason::Stop,
                    after_segment: 2,
                    at_ms: 61_750
                },
            ]
        );
    }

    #[test]
    fn exactly_one_segment_of_audio_does_not_open_an_empty_second_file() {
        let dir = temp();
        let mut writer = SegmentWriter::open(dir.path(), Track::System).unwrap();
        writer
            .write(&vec![3; segment_samples() as usize], 0)
            .unwrap();
        assert!(!writer.in_run());
        drop(writer);
        assert!(dir.path().join("system-0001.wav").exists());
        assert!(!dir.path().join("system-0002.wav").exists());
    }

    #[test]
    fn a_pause_and_a_device_change_end_the_segment_and_record_gaps() {
        let dir = temp();
        let mut writer = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        writer.write(&[1; 16_000], 0).unwrap();
        writer.end_run(GapReason::Pause).unwrap();
        writer.end_run(GapReason::Pause).unwrap(); // nothing written since: no line
        writer.write(&[2; 8_000], 30_000).unwrap();
        writer.end_run(GapReason::DeviceChange).unwrap();
        writer.write(&[3; 1_600], 30_700).unwrap();
        drop(writer);
        assert_eq!(
            read_manifest(dir.path(), Track::Mic),
            vec![
                ManifestEntry::Segment {
                    segment: 1,
                    file: "mic-0001.wav".into(),
                    start_ms: 0,
                    audio_ms: None,
                },
                ManifestEntry::Gap {
                    gap: GapReason::Pause,
                    after_segment: 1,
                    at_ms: 1_000
                },
                ManifestEntry::Segment {
                    segment: 2,
                    file: "mic-0002.wav".into(),
                    start_ms: 30_000,
                    audio_ms: None,
                },
                ManifestEntry::Gap {
                    gap: GapReason::DeviceChange,
                    after_segment: 2,
                    at_ms: 30_500
                },
                ManifestEntry::Segment {
                    segment: 3,
                    file: "mic-0003.wav".into(),
                    start_ms: 30_700,
                    audio_ms: None,
                },
                ManifestEntry::Gap {
                    gap: GapReason::Stop,
                    after_segment: 3,
                    at_ms: 30_800
                },
            ]
        );
        assert_eq!(
            saved_track(dir.path(), Track::Mic),
            SavedTrack {
                audio_ms: 1_600,
                end_ms: 30_800
            }
        );
    }

    #[test]
    fn manifest_lines_use_camel_case() {
        let line = serde_json::to_string(&ManifestEntry::Gap {
            gap: GapReason::DeviceChange,
            after_segment: 2,
            at_ms: 5,
        })
        .unwrap();
        assert_eq!(line, r#"{"gap":"deviceChange","afterSegment":2,"atMs":5}"#);
        let line = serde_json::to_string(&ManifestEntry::Segment {
            segment: 1,
            file: "mic-0001.wav".into(),
            start_ms: 7,
            audio_ms: None,
        })
        .unwrap();
        assert_eq!(line, r#"{"segment":1,"file":"mic-0001.wav","startMs":7}"#);
    }

    #[test]
    fn a_restarted_track_continues_its_numbering() {
        let dir = temp();
        let mut writer = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        writer.write(&[1; 100], 0).unwrap();
        drop(writer);
        let mut again = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        again.write(&[1; 100], 9_000).unwrap();
        drop(again);
        assert!(dir.path().join("mic-0002.wav").exists());
    }

    #[test]
    fn after_a_crash_the_directory_is_repaired_and_the_saved_audio_measured() {
        let dir = temp();
        let mut writer = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        writer
            .write(&vec![1; segment_samples() as usize + 32_000], 1_000)
            .unwrap();
        writer.flush().unwrap();
        std::mem::forget(writer); // the crash
                                  // A torn manifest line from the moment of the crash.
        let mut manifest = OpenOptions::new()
            .append(true)
            .open(manifest_path(dir.path(), Track::Mic))
            .unwrap();
        manifest.write_all(br#"{"segment":3,"fi"#).unwrap();
        drop(manifest);
        assert_eq!(repair_dir(dir.path()).unwrap(), 1, "only the open segment");
        let (size, _) = wav_samples(&dir.path().join("mic-0002.wav"));
        assert_eq!(size, 64_000);
        assert_eq!(
            saved_track(dir.path(), Track::Mic),
            SavedTrack {
                audio_ms: 62_000,
                end_ms: 63_000
            }
        );
        assert_eq!(
            saved_track(dir.path(), Track::System),
            SavedTrack::default()
        );
        assert_eq!(repair_dir(&dir.path().join("missing")).unwrap(), 0);
        assert!(dir_bytes(dir.path()) > 64_000 + 1_920_000);
    }
}
