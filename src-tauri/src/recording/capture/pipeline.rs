//! The capture thread's work for one track, independent of the platform:
//! take device audio as the callback stamped it, downmix it to mono,
//! resample it to 16 kHz, write it as segments, and report health.
//!
//! Device callbacks only copy audio into a ring buffer and stamp it
//! ([`Stamp`]); everything here runs on the capture thread, where
//! allocating, locking, and file writes are fine.
//!
//! **Placing audio on the recording's clock.** Each stamp carries the host
//! time of its first frame. While stamps follow each other within
//! [`policy::CAPTURE_GAP_TOLERANCE`], the audio is one run and is written
//! contiguously. A larger jump (sleep, a dropout, an overrun that lost
//! audio) ends the run and records a gap. A new segment is placed from the
//! stamp nearest its first sample, so the device clock's drift against the
//! host clock never accumulates past one segment.

use super::resample::{to_i16, Resampler};
use super::{CaptureEvent, EventSink, Track, TrackTarget};
use crate::recording::policy;
use crate::recording::segments::{GapReason, SegmentWriter};
use std::io;
use std::time::Instant;

/// One device callback's audio, as the callback stamped it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stamp {
    /// Host time of the callback's first frame, in nanoseconds on the
    /// backend's host clock.
    pub host_ns: u64,
    /// Frames the callback put in the ring buffer.
    pub frames: u32,
}

/// The stamp the current block of input started at, and how many input
/// frames of the run came before it.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    host_ns: u64,
    run_frame: u64,
}

pub struct TrackPipeline {
    track: Track,
    origin_host_ns: u64,
    writer: SegmentWriter,
    events: EventSink,
    rate: u32,
    channels: usize,
    resampler: Resampler,
    mono: Vec<f32>,
    resampled: Vec<f32>,
    pcm: Vec<i16>,
    /// Input frames in the current run so far.
    run_frames: u64,
    anchor: Option<Anchor>,
    /// Host time at which the next callback's audio should begin.
    expected_next_ns: Option<u64>,
    first_frame_sent: bool,
    paused: bool,
    written_since_report: u64,
    peak_since_report: f32,
    last_report: Instant,
}

impl TrackPipeline {
    pub fn new(
        track: Track,
        target: &TrackTarget,
        rate: u32,
        channels: usize,
        events: EventSink,
    ) -> io::Result<Self> {
        Ok(Self {
            track,
            origin_host_ns: target.origin_host_ns,
            writer: SegmentWriter::open(&target.audio_dir, track)?,
            events,
            rate,
            channels: channels.max(1),
            resampler: Resampler::new(rate, policy::OUTPUT_SAMPLE_RATE),
            mono: Vec::new(),
            resampled: Vec::new(),
            pcm: Vec::new(),
            run_frames: 0,
            anchor: None,
            expected_next_ns: None,
            first_frame_sent: false,
            paused: false,
            written_since_report: 0,
            peak_since_report: 0.0,
            last_report: Instant::now(),
        })
    }

    #[cfg(test)]
    pub fn format(&self) -> (u32, usize) {
        (self.rate, self.channels)
    }

    fn send(&self, event: CaptureEvent) {
        // The service may already have dropped its receiver; that's fine.
        let _ = self.events.send(event);
    }

    /// Reports a device change and switches to the new device's format.
    /// The current run ends first.
    pub fn change_device(&mut self, rate: u32, channels: usize, name: &str) -> io::Result<()> {
        self.change_device_route(rate, channels, name, None)
    }

    pub fn change_device_route(
        &mut self,
        rate: u32,
        channels: usize,
        name: &str,
        is_headphones: Option<bool>,
    ) -> io::Result<()> {
        self.end_run(GapReason::DeviceChange)?;
        if rate != self.rate {
            self.rate = rate;
            self.resampler = Resampler::new(rate, policy::OUTPUT_SAMPLE_RATE);
        }
        self.channels = channels.max(1);
        self.report_device(name, is_headphones);
        Ok(())
    }

    pub fn report_device(&self, name: &str, is_headphones: Option<bool>) {
        self.send(CaptureEvent::DeviceChanged {
            track: self.track,
            device_name: name.to_string(),
            is_headphones,
        });
    }

    /// Pausing ends the run; audio that arrives while paused is dropped.
    pub fn set_paused(&mut self, paused: bool) -> io::Result<()> {
        if paused && !self.paused {
            self.end_run(GapReason::Pause)?;
        }
        self.paused = paused;
        Ok(())
    }

    /// The recording-clock time, in milliseconds, of the next resampled
    /// sample.
    fn next_output_ms(&self) -> u64 {
        let Some(anchor) = self.anchor else {
            return 0;
        };
        let position = self.resampler.input_position(self.resampler.produced());
        let offset_ns = (position - anchor.run_frame as f64) * 1e9 / self.rate as f64;
        let host_ns = anchor.host_ns as f64 + offset_ns;
        ((host_ns - self.origin_host_ns as f64).max(0.0) / 1e6).round() as u64
    }

    fn write_resampled(&mut self, at_ms: u64) -> io::Result<()> {
        if self.resampled.is_empty() {
            return Ok(());
        }
        self.pcm.clear();
        self.pcm.extend(self.resampled.iter().map(|&s| to_i16(s)));
        self.resampled.clear();
        self.writer.write(&self.pcm, at_ms)?;
        self.written_since_report += self.pcm.len() as u64;
        Ok(())
    }

    /// Takes one callback's interleaved audio.
    pub fn push(&mut self, stamp: Stamp, interleaved: &[f32]) -> io::Result<()> {
        if !self.first_frame_sent && stamp.frames > 0 {
            self.first_frame_sent = true;
            self.send(CaptureEvent::FirstFrame {
                track: self.track,
                host_time_ns: stamp.host_ns,
            });
        }
        if self.paused || stamp.frames == 0 {
            return Ok(());
        }
        if let Some(expected) = self.expected_next_ns {
            if stamp.host_ns.abs_diff(expected) > policy::CAPTURE_GAP_TOLERANCE.as_nanos() as u64 {
                self.end_run(GapReason::Dropout)?;
            }
        }
        self.anchor = Some(Anchor {
            host_ns: stamp.host_ns,
            run_frame: self.run_frames,
        });
        self.mono.clear();
        let channels = self.channels;
        self.mono.extend(
            interleaved
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
        for &sample in &self.mono {
            self.peak_since_report = self.peak_since_report.max(sample.abs());
        }
        let at_ms = self.next_output_ms();
        let mut resampled = std::mem::take(&mut self.resampled);
        self.resampler.process(&self.mono, &mut resampled);
        self.resampled = resampled;
        self.write_resampled(at_ms)?;
        let frames = self.mono.len() as u64;
        self.run_frames += frames;
        self.expected_next_ns = Some(stamp.host_ns + frames * 1_000_000_000 / self.rate as u64);
        Ok(())
    }

    /// Ends the current run: writes the resampler's tail, closes the
    /// segment, and records why.
    pub fn end_run(&mut self, reason: GapReason) -> io::Result<()> {
        if self.anchor.is_some() {
            let at_ms = self.next_output_ms();
            let mut resampled = std::mem::take(&mut self.resampled);
            self.resampler.finish(&mut resampled);
            self.resampled = resampled;
            self.write_resampled(at_ms)?;
        }
        self.writer.end_run(reason)?;
        self.run_frames = 0;
        self.anchor = None;
        self.expected_next_ns = None;
        Ok(())
    }

    /// Flushes written audio and reports progress once per
    /// [`policy::FLUSH_INTERVAL`]. Call it often; it does nothing between
    /// intervals.
    pub fn maybe_flush(&mut self, now: Instant) -> io::Result<()> {
        if now.duration_since(self.last_report) < policy::FLUSH_INTERVAL {
            return Ok(());
        }
        self.flush(now)
    }

    pub fn flush(&mut self, now: Instant) -> io::Result<()> {
        self.last_report = now;
        self.writer.flush()?;
        if self.written_since_report > 0 {
            self.send(CaptureEvent::Frames {
                track: self.track,
                count: self.written_since_report,
                peak: self.peak_since_report,
            });
            self.written_since_report = 0;
            self.peak_since_report = 0.0;
        }
        Ok(())
    }

    /// Reports a failure the service should act on (restart the track).
    pub fn fail(&mut self, error: &str) {
        let _ = self.end_run(GapReason::Restart);
        self.send(CaptureEvent::Failed {
            track: self.track,
            error: error.to_string(),
        });
    }

    /// Ends the track: the last audio is written and the segment finalized.
    pub fn finish(mut self) -> io::Result<()> {
        self.end_run(GapReason::Stop)?;
        self.flush(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::segments::{read_manifest, saved_track, ManifestEntry};
    use crate::recording::test_support::TempDir;
    use std::sync::mpsc::{channel, Receiver};

    const MS: u64 = 1_000_000;
    const ORIGIN: u64 = 10_000 * MS;

    fn pipeline(
        dir: &TempDir,
        rate: u32,
        channels: usize,
    ) -> (TrackPipeline, Receiver<CaptureEvent>) {
        let (sender, receiver) = channel();
        let target = TrackTarget {
            audio_dir: dir.path().to_path_buf(),
            origin_host_ns: ORIGIN,
            microphone_uid: None,
        };
        (
            TrackPipeline::new(Track::Mic, &target, rate, channels, sender).unwrap(),
            receiver,
        )
    }

    /// Feeds `seconds` of stereo audio at 48 kHz in 10 ms callbacks from
    /// `start_ns`, returning the host time after the last one.
    fn feed(pipeline: &mut TrackPipeline, start_ns: u64, seconds: f64) -> u64 {
        let frames = 480;
        let callbacks = (seconds * 100.0).round() as usize;
        let block = vec![0.25f32; frames * 2];
        let mut at = start_ns;
        for _ in 0..callbacks {
            pipeline
                .push(
                    Stamp {
                        host_ns: at,
                        frames: frames as u32,
                    },
                    &block,
                )
                .unwrap();
            at += 10 * MS;
        }
        at
    }

    fn segment_starts(dir: &TempDir) -> Vec<u64> {
        read_manifest(dir.path(), Track::Mic)
            .into_iter()
            .filter_map(|entry| match entry {
                ManifestEntry::Segment { start_ms, .. } => Some(start_ms),
                ManifestEntry::Gap { .. } => None,
            })
            .collect()
    }

    fn gaps(dir: &TempDir) -> Vec<(GapReason, u64)> {
        read_manifest(dir.path(), Track::Mic)
            .into_iter()
            .filter_map(|entry| match entry {
                ManifestEntry::Gap { gap, at_ms, .. } => Some((gap, at_ms)),
                ManifestEntry::Segment { .. } => None,
            })
            .collect()
    }

    #[test]
    fn audio_is_placed_by_its_host_time_and_reported() {
        let dir = TempDir::new();
        let (mut pipeline, events) = pipeline(&dir, 48_000, 2);
        // The first callback arrives 250 ms after the recording started.
        feed(&mut pipeline, ORIGIN + 250 * MS, 2.0);
        pipeline.flush(Instant::now()).unwrap();
        pipeline.finish().unwrap();
        assert_eq!(segment_starts(&dir), vec![250]);
        let saved = saved_track(dir.path(), Track::Mic);
        assert_eq!(saved.audio_ms, 2_000);
        assert_eq!(saved.end_ms, 2_250);
        let events: Vec<CaptureEvent> = events.try_iter().collect();
        assert_eq!(
            events[0],
            CaptureEvent::FirstFrame {
                track: Track::Mic,
                host_time_ns: ORIGIN + 250 * MS
            }
        );
        let written: u64 = events
            .iter()
            .filter_map(|e| match e {
                CaptureEvent::Frames { count, .. } => Some(*count),
                _ => None,
            })
            .sum();
        assert_eq!(written, 32_000, "every 16 kHz sample is reported");
    }

    #[test]
    fn a_jump_in_host_time_ends_the_segment_and_records_a_dropout() {
        let dir = TempDir::new();
        let (mut pipeline, _events) = pipeline(&dir, 48_000, 2);
        let end = feed(&mut pipeline, ORIGIN, 1.0);
        // Within tolerance: still one run.
        let end = feed(&mut pipeline, end + 50 * MS, 1.0);
        // The computer slept for 30 seconds.
        feed(&mut pipeline, end + 30_000 * MS, 1.0);
        pipeline.finish().unwrap();
        assert_eq!(segment_starts(&dir), vec![0, 32_050]);
        assert_eq!(
            gaps(&dir),
            vec![(GapReason::Dropout, 2_000), (GapReason::Stop, 33_050)]
        );
    }

    #[test]
    fn pausing_ends_the_segment_and_drops_audio_until_resumed() {
        let dir = TempDir::new();
        let (mut pipeline, _events) = pipeline(&dir, 48_000, 2);
        let end = feed(&mut pipeline, ORIGIN, 1.0);
        pipeline.set_paused(true).unwrap();
        let end = feed(&mut pipeline, end, 5.0);
        pipeline.set_paused(false).unwrap();
        feed(&mut pipeline, end, 1.0);
        pipeline.finish().unwrap();
        assert_eq!(segment_starts(&dir), vec![0, 6_000]);
        assert_eq!(gaps(&dir)[0], (GapReason::Pause, 1_000));
        assert_eq!(saved_track(dir.path(), Track::Mic).audio_ms, 2_000);
    }

    #[test]
    fn a_device_change_ends_the_run_and_switches_format() {
        let dir = TempDir::new();
        let (mut pipeline, events) = pipeline(&dir, 48_000, 2);
        let end = feed(&mut pipeline, ORIGIN, 1.0);
        pipeline.change_device(44_100, 1, "AirPods").unwrap();
        assert_eq!(pipeline.format(), (44_100, 1));
        // One second of mono 44.1 kHz in 441-frame callbacks.
        let block = vec![0.1f32; 441];
        for i in 0..100u64 {
            pipeline
                .push(
                    Stamp {
                        host_ns: end + 400 * MS + i * 10 * MS,
                        frames: 441,
                    },
                    &block,
                )
                .unwrap();
        }
        pipeline.finish().unwrap();
        assert_eq!(segment_starts(&dir), vec![0, 1_400]);
        assert_eq!(gaps(&dir)[0], (GapReason::DeviceChange, 1_000));
        assert_eq!(saved_track(dir.path(), Track::Mic).audio_ms, 2_000);
        assert!(events.try_iter().any(|e| e
            == CaptureEvent::DeviceChanged {
                track: Track::Mic,
                device_name: "AirPods".into(),
                is_headphones: None,
            }));
    }

    #[test]
    fn downmixing_averages_the_channels() {
        let dir = TempDir::new();
        let (mut pipeline, _events) = pipeline(&dir, 16_000, 2);
        let block: Vec<f32> = (0..160).flat_map(|_| [0.5f32, 0.0]).collect();
        pipeline
            .push(
                Stamp {
                    host_ns: ORIGIN,
                    frames: 160,
                },
                &block,
            )
            .unwrap();
        pipeline.finish().unwrap();
        let bytes = std::fs::read(dir.path().join("mic-0001.wav")).unwrap();
        let first = i16::from_le_bytes([bytes[44], bytes[45]]);
        assert_eq!(first, to_i16(0.25));
    }

    #[test]
    fn a_failure_ends_the_run_and_is_reported() {
        let dir = TempDir::new();
        let (mut pipeline, events) = pipeline(&dir, 48_000, 2);
        feed(&mut pipeline, ORIGIN, 0.5);
        pipeline.fail("The microphone was disconnected.");
        assert_eq!(gaps(&dir), vec![(GapReason::Restart, 500)]);
        assert!(events.try_iter().any(|e| matches!(
            e,
            CaptureEvent::Failed { track: Track::Mic, ref error } if error == "The microphone was disconnected."
        )));
    }
}
