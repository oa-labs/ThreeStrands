//! Recording holds the same memory at two hours as at ten minutes.
//!
//! Two hours of synthetic audio go through what a long meeting exercises,
//! as fast as the machine allows: the device callback's rings, the capture
//! pipeline (downmix, the windowed-sinc resampler, the segment writer and
//! its manifest), and the service reading capture events each second on a
//! fake clock. Live heap bytes are counted on this thread only
//! ([`heap`]), so other tests running at the same time don't disturb them.

use super::capture::fake::FakeBackend;
use super::capture::pipeline::{Stamp, TrackPipeline};
use super::capture::ring::{ring, Consumer, Producer};
use super::capture::Track;
use super::test_support::{heap, TempDir};
use super::*;
use std::sync::Mutex;
use std::time::Instant;

struct StepClock(Mutex<SystemTime>);
impl Clock for StepClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

struct Plenty;
impl DiskProbe for Plenty {
    fn free_bytes(&self) -> Result<u64, String> {
        Ok(u64::MAX)
    }
}

/// Frames per device callback: 10 ms.
fn callback_frames(rate: u32) -> usize {
    rate as usize / 100
}

/// One track as the macOS backend runs it: the device callback copies
/// stamped audio into preallocated rings, and the capture thread drains
/// them into the pipeline and flushes once a second.
struct SimulatedTrack {
    samples_in: Producer<f32>,
    stamps_in: Producer<Stamp>,
    samples_out: Consumer<f32>,
    stamps_out: Consumer<Stamp>,
    pipeline: TrackPipeline,
    scratch: Vec<f32>,
    channels: usize,
    rate: u32,
    /// One second of interleaved audio, played over and over.
    source: Vec<f32>,
    position: usize,
    host_ns: u64,
}

impl SimulatedTrack {
    fn new(backend: &FakeBackend, track: Track, rate: u32, channels: usize) -> Self {
        let target = backend.target(track).unwrap();
        let sink = backend.sink(track).unwrap();
        let pipeline = TrackPipeline::new(track, &target, rate, channels, sink).unwrap();
        let ring_samples =
            (rate as f64 * channels as f64 * policy::CAPTURE_RING.as_secs_f64()) as usize;
        let (samples_in, samples_out) = ring::<f32>(ring_samples);
        let (stamps_in, stamps_out) = ring::<Stamp>(4096);
        // A tone with a little deterministic noise.
        let mut seed = 0x2545_f491_u32;
        let source = (0..rate as usize * channels)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let frame = (i / channels) as f32;
                let tone = (frame * 440.0 * std::f32::consts::TAU / rate as f32).sin();
                0.3 * tone + 0.02 * (seed as f32 / u32::MAX as f32 - 0.5)
            })
            .collect();
        Self {
            samples_in,
            stamps_in,
            samples_out,
            stamps_out,
            pipeline,
            scratch: Vec::new(),
            channels,
            rate,
            source,
            position: 0,
            host_ns: target.origin_host_ns,
        }
    }

    /// One 10 ms device callback, then the capture thread's drain.
    fn callback(&mut self) {
        let frames = callback_frames(self.rate);
        let count = frames * self.channels;
        let (source, start) = (&self.source, self.position);
        assert!(self
            .samples_in
            .push_with(count, |i| source[(start + i) % source.len()]));
        assert!(self.stamps_in.push_with(1, |_| Stamp {
            host_ns: self.host_ns,
            frames: frames as u32,
        }));
        self.position = (start + count) % self.source.len();
        self.host_ns += 10_000_000;
        while let Some(stamp) = self.stamps_out.pop() {
            let wanted = stamp.frames as usize * self.channels;
            self.scratch.resize(wanted, 0.0);
            let got = self.samples_out.pop_into(&mut self.scratch[..wanted]);
            assert_eq!(got, wanted);
            self.pipeline.push(stamp, &self.scratch[..wanted]).unwrap();
        }
    }
}

/// What a recording held on the heap: live bytes and the high-water mark
/// after the early minutes, then after the rest.
struct Held {
    early: (i64, i64),
    late: (i64, i64),
}

/// Records `minutes` of audio on each `(track, rate, channels)` as fast as
/// possible, counting from the end of the first minute (which sets up what
/// every later minute reuses), and returns what was held after
/// `early_minutes` and at the end.
fn record(tracks: &[(Track, u32, usize)], early_minutes: u64, minutes: u64) -> Held {
    let root = TempDir::new();
    let backend = FakeBackend::new();
    if !tracks.iter().any(|(track, ..)| *track == Track::System) {
        backend.set_system_audio(super::capture::SystemAudioSupport::Unsupported);
    }
    let clock = Arc::new(StepClock(Mutex::new(
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
    )));
    let mut service = RecordingService::new(
        Arc::new(Database::open_memory()),
        Arc::new(backend.clone()),
        clock.clone(),
        Arc::new(Plenty),
        root.path().to_path_buf(),
    );
    service.start(None, "Long Meeting").unwrap();
    let mut simulated: Vec<SimulatedTrack> = tracks
        .iter()
        .map(|&(track, rate, channels)| SimulatedTrack::new(&backend, track, rate, channels))
        .collect();
    let started = Instant::now();
    let mut second = 0u64;
    let mut run_until_minute = |minute: u64, simulated: &mut [SimulatedTrack]| {
        while second < minute * 60 {
            for _ in 0..100 {
                for track in simulated.iter_mut() {
                    track.callback();
                }
            }
            second += 1;
            let now = started + Duration::from_secs(second);
            for track in simulated.iter_mut() {
                track.pipeline.maybe_flush(now).unwrap();
            }
            *clock.0.lock().unwrap() += Duration::from_secs(1);
            service.tick();
        }
    };
    run_until_minute(1, &mut simulated);
    heap::start();
    run_until_minute(early_minutes, &mut simulated);
    let early = (heap::live(), heap::peak());
    run_until_minute(minutes, &mut simulated);
    let late = (heap::live(), heap::peak());
    heap::stop();

    let status = service.status();
    assert_eq!(status.state, RecordingPhase::Recording, "{status:?}");
    for &(track, ..) in tracks {
        let row = status.tracks.iter().find(|t| t.track == track).unwrap();
        assert_eq!(row.state, TrackRunState::Active, "{track:?}");
        let saved = segments::saved_track(&backend.target(track).unwrap().audio_dir, track);
        assert!(saved.audio_ms >= (minutes - 1) * 60_000, "{saved:?}");
    }
    service.stop().unwrap();
    Held { early, late }
}

/// Room for allocator rounding and the odd buffer that grows by a step.
const SLACK: i64 = 16 * 1024;

fn assert_flat(held: &Held, early_minutes: u64, minutes: u64) {
    let (live_early, peak_early) = held.early;
    let (live_late, peak_late) = held.late;
    assert!(
        peak_early < 4 * 1024 * 1024,
        "{peak_early} bytes in the first {early_minutes} minutes"
    );
    assert!(
        peak_late <= peak_early + SLACK,
        "peak {peak_late} bytes over {minutes} minutes, {peak_early} over {early_minutes}"
    );
    assert!(
        live_late <= live_early + SLACK,
        "{live_late} bytes held at {minutes} minutes, {live_early} at {early_minutes}"
    );
}

#[test]
fn two_hours_of_recording_hold_no_more_memory_than_ten_minutes() {
    // Both tracks at 16 kHz, the microphone in stereo: the rings, the
    // downmix, the resampler's pass-through, the segment writer and its
    // manifest, and the service, for the whole two hours.
    let held = record(
        &[(Track::Mic, 16_000, 2), (Track::System, 16_000, 1)],
        10,
        120,
    );
    assert_flat(&held, 10, 120);
}

#[test]
fn resampling_holds_no_more_memory_after_twenty_minutes_than_after_two() {
    // The windowed-sinc filter at the device rate it is cheapest at, since
    // it is the slow part of a debug build: its buffers stay the same size.
    let held = record(&[(Track::Mic, 8_000, 1)], 2, 20);
    assert_flat(&held, 2, 20);
}
