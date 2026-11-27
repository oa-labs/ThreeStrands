//! Inside the worker: one track's audio, segment file by segment file, into
//! transcript lines, without cutting speech at file boundaries.
//!
//! Each track keeps a carry: audio at the end of the last file that may hold
//! unfinished speech ([`spans::hold_from`]). The next file of the same run
//! is appended to it before voice activity detection, so a sentence spoken
//! across a minute boundary reaches whisper whole. When the run ends (a
//! gap, or the recording's end) nothing is held. A file that doesn't
//! continue the run first flushes whatever is still carried.

use super::protocol::{SegmentRequest, TranscriptLine};
use super::spans::{self, Span};
use crate::recording::capture::Track;
use crate::recording::policy;
use std::path::Path;

/// What the worker needs from a speech recognizer; whisper.cpp on macOS,
/// a scripted fake in tests.
pub trait SpeechEngine {
    /// Raw speech spans (sample ranges, unpadded) in 16 kHz mono audio.
    fn detect(&mut self, samples: &[f32]) -> Result<Vec<Span>, String>;
    /// The sentences in one span: start and end in milliseconds from the
    /// span's first sample, and text.
    fn transcribe(&mut self, samples: &[f32]) -> Result<Vec<(u64, u64, String)>, String>;
}

/// Reads one of the app's 16 kHz mono 16-bit WAV segments as samples in
/// -1..1. Anything else is refused.
pub fn read_segment(path: &Path) -> Result<Vec<f32>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("opening the segment: {e}"))?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > policy::WORKER_MAX_SEGMENT_FILE_BYTES {
        return Err("the segment is too large".into());
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(policy::WORKER_MAX_SEGMENT_FILE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() < 44 {
        return Err("the segment has no header".into());
    }
    let expected = crate::recording::segments::wav_header(0);
    if bytes[0..4] != expected[0..4] || bytes[8..40] != expected[8..40] {
        return Err("the segment isn't 16 kHz mono 16-bit audio".into());
    }
    Ok(bytes[44..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0)
        .collect())
}

#[derive(Default)]
struct Carry {
    samples: Vec<f32>,
    start_ms: u64,
}

/// Both tracks' carries.
#[derive(Default)]
pub struct Streams {
    mic: Carry,
    system: Carry,
}

impl Streams {
    fn carry(&mut self, track: Track) -> &mut Carry {
        match track {
            Track::Mic => &mut self.mic,
            Track::System => &mut self.system,
        }
    }

    /// Transcribes one segment's `samples`, holding back unfinished speech
    /// at its end unless the run ends here.
    pub fn feed(
        &mut self,
        engine: &mut dyn SpeechEngine,
        request: &SegmentRequest,
        samples: Vec<f32>,
    ) -> Result<Vec<TranscriptLine>, String> {
        let track = request.track;
        let mut lines = Vec::new();
        let carry = std::mem::take(self.carry(track));
        let (buffer, start_ms) = if request.continues && !carry.samples.is_empty() {
            let mut buffer = carry.samples;
            buffer.extend_from_slice(&samples);
            (buffer, carry.start_ms)
        } else {
            if !carry.samples.is_empty() {
                // The run ended without a file saying so; finish it first.
                lines.extend(transcribe_all(
                    engine,
                    track,
                    &carry.samples,
                    carry.start_ms,
                )?);
            }
            (samples, request.start_ms)
        };
        let raw = engine.detect(&buffer)?;
        let keep_from = if request.ends_run {
            buffer.len()
        } else {
            spans::hold_from(&raw, buffer.len())
        };
        let clipped: Vec<Span> = raw
            .iter()
            .filter(|(start, _)| *start < keep_from)
            .map(|&(start, end)| (start, end.min(keep_from)))
            .collect();
        for span in spans::plan_spans(&clipped, keep_from) {
            lines.extend(transcribe_span(engine, track, &buffer, span, start_ms)?);
        }
        *self.carry(track) = Carry {
            start_ms: start_ms + spans::samples_to_ms(keep_from),
            samples: buffer[keep_from..].to_vec(),
        };
        Ok(lines)
    }

    /// Milliseconds of audio a track is holding back.
    #[cfg(test)]
    fn held_ms(&self, track: Track) -> u64 {
        let carry = match track {
            Track::Mic => &self.mic,
            Track::System => &self.system,
        };
        spans::samples_to_ms(carry.samples.len())
    }
}

fn transcribe_all(
    engine: &mut dyn SpeechEngine,
    track: Track,
    samples: &[f32],
    start_ms: u64,
) -> Result<Vec<TranscriptLine>, String> {
    let raw = engine.detect(samples)?;
    let mut lines = Vec::new();
    for span in spans::plan_spans(&raw, samples.len()) {
        lines.extend(transcribe_span(engine, track, samples, span, start_ms)?);
    }
    Ok(lines)
}

fn transcribe_span(
    engine: &mut dyn SpeechEngine,
    track: Track,
    buffer: &[f32],
    (start, end): Span,
    buffer_start_ms: u64,
) -> Result<Vec<TranscriptLine>, String> {
    let span_start_ms = buffer_start_ms + spans::samples_to_ms(start);
    let span_ms = spans::samples_to_ms(end - start);
    Ok(engine
        .transcribe(&buffer[start..end])?
        .into_iter()
        .map(|(from, to, text)| {
            let from = from.min(span_ms);
            TranscriptLine {
                track,
                start_ms: span_start_ms + from,
                end_ms: span_start_ms + to.clamp(from, span_ms),
                text,
            }
        })
        .collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const S: usize = 16_000;

    /// Audio where speech is any sample at or above 0.5. Detection reports
    /// the runs of speech; transcription names each run it sees by the
    /// value of its first speech sample and how long the run is, so a test
    /// can tell whether a sentence arrived whole.
    pub(crate) struct FakeEngine {
        pub transcribed: Vec<usize>,
    }

    impl FakeEngine {
        pub fn new() -> Self {
            Self {
                transcribed: Vec::new(),
            }
        }
    }

    fn runs(samples: &[f32]) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut start = None;
        for (i, &sample) in samples.iter().enumerate() {
            match (sample >= 0.5, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    spans.push((s, i));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            spans.push((s, samples.len()));
        }
        spans
    }

    impl SpeechEngine for FakeEngine {
        fn detect(&mut self, samples: &[f32]) -> Result<Vec<Span>, String> {
            Ok(runs(samples))
        }

        fn transcribe(&mut self, samples: &[f32]) -> Result<Vec<(u64, u64, String)>, String> {
            self.transcribed.push(samples.len());
            Ok(runs(samples)
                .into_iter()
                .map(|(a, b)| {
                    (
                        spans::samples_to_ms(a),
                        spans::samples_to_ms(b),
                        format!(
                            "speech {} for {} ms",
                            samples[a],
                            spans::samples_to_ms(b - a)
                        ),
                    )
                })
                .collect())
        }
    }

    /// `len` seconds of silence with speech of `value` over `from..to`
    /// (seconds, fractional).
    fn audio(len: f64, speech: &[(f64, f64, f32)]) -> Vec<f32> {
        let mut samples = vec![0.0; (len * S as f64) as usize];
        for &(from, to, value) in speech {
            for sample in &mut samples[(from * S as f64) as usize..(to * S as f64) as usize] {
                *sample = value;
            }
        }
        samples
    }

    fn request(track: Track, start_ms: u64, continues: bool, ends_run: bool) -> SegmentRequest {
        SegmentRequest {
            track,
            wav_path: "/unused".into(),
            start_ms,
            continues,
            ends_run,
        }
    }

    #[test]
    fn speech_across_a_file_boundary_reaches_whisper_whole() {
        let mut engine = FakeEngine::new();
        let mut streams = Streams::default();
        // 58 s to 63 s of speech, across the minute boundary.
        let first = streams
            .feed(
                &mut engine,
                &request(Track::Mic, 1_000, false, false),
                audio(60.0, &[(10.0, 12.0, 0.7), (58.0, 60.0, 0.9)]),
            )
            .unwrap();
        assert_eq!(first.len(), 1, "only the finished sentence: {first:?}");
        assert_eq!(first[0].text, "speech 0.7 for 2000 ms");
        assert_eq!((first[0].start_ms, first[0].end_ms), (11_000, 13_000));
        assert!(streams.held_ms(Track::Mic) >= 2_000);

        let second = streams
            .feed(
                &mut engine,
                &request(Track::Mic, 61_000, true, true),
                audio(20.0, &[(0.0, 3.0, 0.9)]),
            )
            .unwrap();
        assert_eq!(second.len(), 1, "{second:?}");
        assert_eq!(
            second[0].text, "speech 0.9 for 5000 ms",
            "one sentence, not two halves"
        );
        assert_eq!((second[0].start_ms, second[0].end_ms), (59_000, 64_000));
        assert_eq!(
            streams.held_ms(Track::Mic),
            0,
            "the run ended: nothing held"
        );
    }

    #[test]
    fn the_end_of_a_run_flushes_what_is_held() {
        let mut engine = FakeEngine::new();
        let mut streams = Streams::default();
        let lines = streams
            .feed(
                &mut engine,
                &request(Track::System, 0, false, true),
                audio(30.0, &[(29.0, 30.0, 0.8)]),
            )
            .unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!((lines[0].start_ms, lines[0].end_ms), (29_000, 30_000));
    }

    #[test]
    fn a_file_that_does_not_continue_flushes_the_carry_at_its_own_time() {
        let mut engine = FakeEngine::new();
        let mut streams = Streams::default();
        streams
            .feed(
                &mut engine,
                &request(Track::Mic, 0, false, false),
                audio(60.0, &[(59.0, 60.0, 0.6)]),
            )
            .unwrap();
        // A pause: the next file starts a new run 10 minutes later.
        let lines = streams
            .feed(
                &mut engine,
                &request(Track::Mic, 600_000, false, true),
                audio(5.0, &[(1.0, 2.0, 0.8)]),
            )
            .unwrap();
        let placed: Vec<(u64, u64, &str)> = lines
            .iter()
            .map(|l| (l.start_ms, l.end_ms, l.text.as_str()))
            .collect();
        assert_eq!(
            placed,
            vec![
                (59_000, 60_000, "speech 0.6 for 1000 ms"),
                (601_000, 602_000, "speech 0.8 for 1000 ms"),
            ]
        );
    }

    #[test]
    fn tracks_hold_their_own_audio() {
        let mut engine = FakeEngine::new();
        let mut streams = Streams::default();
        streams
            .feed(
                &mut engine,
                &request(Track::Mic, 0, false, false),
                audio(60.0, &[(58.0, 60.0, 0.6)]),
            )
            .unwrap();
        let system = streams
            .feed(
                &mut engine,
                &request(Track::System, 0, false, false),
                audio(60.0, &[(5.0, 6.0, 0.7)]),
            )
            .unwrap();
        assert_eq!(system.len(), 1);
        assert_eq!(system[0].track, Track::System);
        assert!(streams.held_ms(Track::Mic) >= 2_000);
        let mic = streams
            .feed(
                &mut engine,
                &request(Track::Mic, 60_000, true, true),
                audio(10.0, &[(0.0, 1.0, 0.6)]),
            )
            .unwrap();
        assert_eq!(mic[0].text, "speech 0.6 for 3000 ms");
        assert_eq!(mic[0].track, Track::Mic);
    }

    #[test]
    fn endless_speech_still_moves_on_in_spans_of_at_most_30_seconds() {
        let mut engine = FakeEngine::new();
        let mut streams = Streams::default();
        let mut start = 0;
        for index in 0..4 {
            let last = index == 3;
            streams
                .feed(
                    &mut engine,
                    &request(Track::System, start, index > 0, last),
                    audio(60.0, &[(0.0, 60.0, 0.9)]),
                )
                .unwrap();
            start += 60_000;
            assert!(streams.held_ms(Track::System) <= policy::VAD_MAX_SPAN_MS as u64);
        }
        let max = spans::ms_to_samples(policy::VAD_MAX_SPAN_MS);
        assert!(
            engine.transcribed.iter().all(|&len| len <= max),
            "{:?}",
            engine.transcribed
        );
        assert_eq!(
            engine.transcribed.iter().sum::<usize>(),
            240 * S,
            "every sample, once"
        );
    }

    #[test]
    fn segments_are_read_only_in_the_apps_format() {
        let dir = crate::recording::test_support::TempDir::new();
        let path = dir.path().join("mic-0001.wav");
        let mut wav = crate::recording::segments::WavWriter::create(&path).unwrap();
        wav.write(&[0, 16_384, -32_768]).unwrap();
        wav.finish().unwrap();
        assert_eq!(read_segment(&path).unwrap(), vec![0.0, 0.5, -1.0]);
        let foreign = dir.path().join("x.wav");
        let mut bytes = crate::recording::segments::wav_header(4).to_vec();
        bytes[24..28].copy_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        std::fs::write(&foreign, bytes).unwrap();
        assert!(read_segment(&foreign).is_err());
        std::fs::write(&foreign, b"RIFF").unwrap();
        assert!(read_segment(&foreign).is_err());
        let huge = dir.path().join("huge.wav");
        let file = std::fs::File::create(&huge).unwrap();
        file.set_len(policy::WORKER_MAX_SEGMENT_FILE_BYTES + 1)
            .unwrap();
        assert!(read_segment(&huge).is_err());
        assert!(read_segment(&dir.path().join("missing.wav")).is_err());
    }
}
