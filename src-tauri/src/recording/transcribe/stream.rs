//! Inside the worker: one track's audio, segment file by segment file, into
//! transcript lines, without cutting speech at file boundaries.
//!
//! Each track keeps a carry: audio at the end of the last file that may hold
//! unfinished speech ([`spans::hold_from`]). The next file of the same run
//! is appended to it before voice activity detection, so a sentence spoken
//! across a minute boundary reaches whisper whole. When the run ends (a
//! gap, or the recording's end) nothing is held. A file that doesn't
//! continue the run first flushes whatever is still carried.
//!
//! The carry remembers which files and samples it came from, so a worker
//! that stops between batches can report it ([`Streams::held`]) and the
//! next worker can pick it up ([`Streams::restore`]) with exactly the same
//! samples and clock: the result is the same as one worker doing both.

use super::protocol::{HeldAudio, HeldPiece, SegmentRequest, TranscriptLine};
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
    /// Language code detected and pinned by the engine, if any.
    fn detected_language(&self) -> Option<&str> {
        None
    }
}

/// Computes the root-mean-square energy of `samples` (`0.0` when empty).
pub fn slice_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// Reads one of the app's 16 kHz mono segment files (`.wav` or transcoded
/// `.m4a`) as samples in `-1..1`. Anything else is refused.
pub fn read_segment(path: &Path) -> Result<Vec<f32>, String> {
    if path.extension().and_then(|e| e.to_str()) == Some("m4a") {
        return super::transcode::read_m4a_samples(path);
    }
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
    /// Where `samples` come from, oldest first; their lengths add up to
    /// `samples.len()`.
    pieces: Vec<HeldPiece>,
}

/// `pieces` without their first `skip` samples.
fn skip_samples(pieces: Vec<HeldPiece>, mut skip: u64) -> Vec<HeldPiece> {
    let mut kept = Vec::new();
    for mut piece in pieces {
        if skip >= piece.samples {
            skip -= piece.samples;
            continue;
        }
        piece.from_sample += skip;
        piece.samples -= skip;
        skip = 0;
        kept.push(piece);
    }
    kept
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
        let piece = HeldPiece {
            wav_path: request.wav_path.clone(),
            from_sample: 0,
            samples: samples.len() as u64,
        };
        let (buffer, start_ms, pieces) = if request.continues && !carry.samples.is_empty() {
            let mut buffer = carry.samples;
            buffer.extend_from_slice(&samples);
            let mut pieces = carry.pieces;
            pieces.push(piece);
            (buffer, carry.start_ms, pieces)
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
            (samples, request.start_ms, vec![piece])
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
            pieces: skip_samples(pieces, keep_from as u64),
        };
        Ok(lines)
    }

    /// What `track` is holding back for its next file, if anything.
    pub fn held(&self, track: Track) -> Option<HeldAudio> {
        let carry = match track {
            Track::Mic => &self.mic,
            Track::System => &self.system,
        };
        (!carry.samples.is_empty()).then(|| HeldAudio {
            start_ms: carry.start_ms,
            pieces: carry.pieces.clone(),
        })
    }

    /// Takes over audio an earlier worker held back for `track`, reading its
    /// pieces with `read`. Refused when the track already holds audio, a
    /// piece lies outside its file, or the whole is longer than anything a
    /// worker holds back (one span).
    pub fn restore(
        &mut self,
        track: Track,
        held: &HeldAudio,
        mut read: impl FnMut(&Path) -> Result<Vec<f32>, String>,
    ) -> Result<(), String> {
        if !self.carry(track).samples.is_empty() {
            return Err("this track already holds audio".into());
        }
        let most = spans::ms_to_samples(policy::VAD_MAX_SPAN_MS);
        let mut samples = Vec::new();
        for piece in &held.pieces {
            let file = read(&piece.wav_path)?;
            let range = usize::try_from(piece.from_sample)
                .ok()
                .zip(usize::try_from(piece.samples).ok())
                .and_then(|(from, count)| Some(from..from.checked_add(count)?))
                .filter(|range| range.end <= file.len())
                .ok_or("the held audio is outside its file")?;
            if samples.len() + range.len() > most {
                return Err("too much held audio".into());
            }
            samples.extend_from_slice(&file[range]);
        }
        *self.carry(track) = Carry {
            samples,
            start_ms: held.start_ms,
            pieces: held.pieces.clone(),
        };
        Ok(())
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
    let span_samples = &buffer[start..end];
    Ok(engine
        .transcribe(span_samples)?
        .into_iter()
        .map(|(from, to, text)| {
            let from = from.min(span_ms);
            let to = to.clamp(from, span_ms);
            let s0 = (from as usize * policy::OUTPUT_SAMPLE_RATE as usize / 1000)
                .min(span_samples.len());
            let s1 = (to as usize * policy::OUTPUT_SAMPLE_RATE as usize / 1000)
                .clamp(s0, span_samples.len());
            let rms = if s1 > s0 {
                slice_rms(&span_samples[s0..s1])
            } else {
                slice_rms(span_samples)
            };
            TranscriptLine {
                track,
                start_ms: span_start_ms + from,
                end_ms: span_start_ms + to,
                text,
                rms: Some(rms),
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
            carry_in: None,
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

    /// A run of `files` contiguous one-minute files (named `/run/N`), with
    /// speech of value `0.6 + i/100` for each `(from, to)` second range on
    /// the run's clock.
    fn run_of(files: usize, speech: &[(f64, f64)]) -> Vec<(SegmentRequest, Vec<f32>)> {
        let whole: Vec<(f64, f64, f32)> = speech
            .iter()
            .enumerate()
            .map(|(i, &(from, to))| (from, to, 0.6 + i as f32 / 100.0))
            .collect();
        let samples = audio(files as f64 * 60.0, &whole);
        (0..files)
            .map(|index| {
                let mut request = request(
                    Track::Mic,
                    index as u64 * 60_000,
                    index > 0,
                    index + 1 == files,
                );
                request.wav_path = format!("/run/{index}").into();
                (
                    request,
                    samples[index * 60 * S..(index + 1) * 60 * S].to_vec(),
                )
            })
            .collect()
    }

    fn placed(lines: &[TranscriptLine]) -> Vec<(u64, u64, String)> {
        lines
            .iter()
            .map(|l| (l.start_ms, l.end_ms, l.text.clone()))
            .collect()
    }

    /// Feeds `run` to fresh workers `batch` files at a time, handing each
    /// one what the last held back, as the scheduler does.
    fn in_batches(
        run: &[(SegmentRequest, Vec<f32>)],
        batch: usize,
    ) -> (Vec<TranscriptLine>, Vec<usize>, Vec<Option<HeldAudio>>) {
        let files: std::collections::HashMap<std::path::PathBuf, Vec<f32>> = run
            .iter()
            .map(|(request, samples)| (request.wav_path.clone(), samples.clone()))
            .collect();
        let mut engine = FakeEngine::new();
        let mut lines = Vec::new();
        let mut held_at_ends = Vec::new();
        let mut held: Option<HeldAudio> = None;
        for chunk in run.chunks(batch) {
            let mut streams = Streams::default();
            if let Some(held) = held.take() {
                streams
                    .restore(Track::Mic, &held, |path| Ok(files[path].clone()))
                    .unwrap();
            }
            for (request, samples) in chunk {
                lines.extend(streams.feed(&mut engine, request, samples.clone()).unwrap());
            }
            held = streams.held(Track::Mic);
            held_at_ends.push(held.clone());
        }
        (lines, engine.transcribed, held_at_ends)
    }

    #[test]
    fn workers_taking_turns_transcribe_exactly_what_one_worker_would() {
        // Speech across every kind of boundary: inside a file, across a
        // minute, across a batch's end (minutes 2|3 and 4|5 with batches of
        // two), and endless speech longer than a span across a batch's end.
        let run = run_of(
            6,
            &[
                (10.0, 12.5),
                (58.5, 61.5),
                (119.0, 123.0),
                (178.25, 185.0),
                (230.0, 275.0),
                (299.5, 300.2),
                (355.0, 359.0),
            ],
        );
        let mut engine = FakeEngine::new();
        let mut streams = Streams::default();
        let mut one = Vec::new();
        for (request, samples) in &run {
            one.extend(streams.feed(&mut engine, request, samples.clone()).unwrap());
        }
        for batch in [1, 2, 4] {
            let (lines, transcribed, held) = in_batches(&run, batch);
            assert_eq!(placed(&lines), placed(&one), "batches of {batch}");
            assert_eq!(transcribed, engine.transcribed, "batches of {batch}");
            assert!(
                held[..held.len() - 1].iter().any(Option::is_some),
                "a batch ended holding speech: {batch}"
            );
            assert_eq!(held.last(), Some(&None), "the run's end holds nothing");
        }
        // The sentence across the end of the first batch of two arrived
        // whole.
        assert!(
            placed(&one)
                .iter()
                .any(|(start, end, text)| (*start, *end) == (119_000, 123_000)
                    && text.ends_with("for 4000 ms")),
            "{:?}",
            placed(&one)
        );
    }

    #[test]
    fn held_audio_names_its_file_and_offset_on_the_recording_clock() {
        let run = run_of(2, &[(58.0, 61.0)]);
        let mut engine = FakeEngine::new();
        let mut streams = Streams::default();
        assert_eq!(streams.held(Track::Mic), None);
        streams
            .feed(&mut engine, &run[0].0, run[0].1.clone())
            .unwrap();
        let from = 58 * S - spans::ms_to_samples(policy::VAD_PAD_BEFORE_MS);
        assert_eq!(
            streams.held(Track::Mic),
            Some(HeldAudio {
                start_ms: spans::samples_to_ms(from),
                pieces: vec![HeldPiece {
                    wav_path: "/run/0".into(),
                    from_sample: from as u64,
                    samples: (60 * S - from) as u64,
                }],
            })
        );
        assert_eq!(streams.held(Track::System), None, "per track");
    }

    #[test]
    fn held_audio_can_span_a_short_file() {
        // A 10-second file that continues the run but doesn't end it: what
        // is held reaches back into the file before it.
        let mut engine = FakeEngine::new();
        let mut streams = Streams::default();
        let mut first = request(Track::Mic, 0, false, false);
        first.wav_path = "/a".into();
        let mut second = request(Track::Mic, 60_000, true, false);
        second.wav_path = "/b".into();
        streams
            .feed(&mut engine, &first, audio(60.0, &[(55.0, 60.0, 0.8)]))
            .unwrap();
        streams
            .feed(&mut engine, &second, audio(10.0, &[(0.0, 10.0, 0.8)]))
            .unwrap();
        let held = streams.held(Track::Mic).unwrap();
        let from = 55 * S - spans::ms_to_samples(policy::VAD_PAD_BEFORE_MS);
        assert_eq!(
            held.pieces,
            vec![
                HeldPiece {
                    wav_path: "/a".into(),
                    from_sample: from as u64,
                    samples: (60 * S - from) as u64
                },
                HeldPiece {
                    wav_path: "/b".into(),
                    from_sample: 0,
                    samples: 10 * S as u64
                },
            ]
        );
        let mut restored = Streams::default();
        let files = [
            ("/a", audio(60.0, &[(55.0, 60.0, 0.8)])),
            ("/b", audio(10.0, &[(0.0, 10.0, 0.8)])),
        ];
        restored
            .restore(Track::Mic, &held, |path| {
                Ok(files
                    .iter()
                    .find(|(name, _)| path == Path::new(name))
                    .unwrap()
                    .1
                    .clone())
            })
            .unwrap();
        assert_eq!(restored.held(Track::Mic), Some(held));
        assert_eq!(restored.held_ms(Track::Mic), streams.held_ms(Track::Mic));
    }

    #[test]
    fn restoring_refuses_audio_outside_its_file_or_longer_than_a_span() {
        let file = vec![0.0f32; 60 * S];
        let read = |_: &Path| Ok(file.clone());
        let held = |from_sample: u64, samples: u64| HeldAudio {
            start_ms: 0,
            pieces: vec![HeldPiece {
                wav_path: "/a".into(),
                from_sample,
                samples,
            }],
        };
        let most = spans::ms_to_samples(policy::VAD_MAX_SPAN_MS) as u64;
        let end = 60 * S as u64;
        assert!(Streams::default()
            .restore(Track::Mic, &held(end - most, most), read)
            .is_ok());
        assert!(Streams::default()
            .restore(Track::Mic, &held(end - most, most + 1), read)
            .is_err());
        assert!(Streams::default()
            .restore(Track::Mic, &held(0, most + 1), read)
            .is_err());
        assert!(Streams::default()
            .restore(Track::Mic, &held(u64::MAX, 2), read)
            .is_err());
        assert!(Streams::default()
            .restore(Track::Mic, &held(0, 1), |_| Err("missing".into()))
            .is_err());
        let mut holding = Streams::default();
        holding.restore(Track::Mic, &held(0, 1), read).unwrap();
        assert!(
            holding.restore(Track::Mic, &held(0, 1), read).is_err(),
            "only into an empty carry"
        );
    }
}
