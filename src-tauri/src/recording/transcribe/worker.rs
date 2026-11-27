//! The transcription worker: whisper.cpp in a killable child process.
//!
//! whisper.cpp is C++ reading large model files, with a GPU backend; a crash
//! or runaway allocation there must not take the mail client down. So, as
//! for PDFs (`pdf_worker.rs`), the app re-executes its own binary with
//! [`WORKER_ARG`] and [`WORKER_ENV`] (checked first thing in `main`, before
//! Tauri, logging, or the database start).
//!
//! Unlike the PDF worker, one worker handles many segment files: a batch
//! during the recording, or everything left after Stop. The model loads once
//! per worker, and each track's held-back audio ([`super::stream`]) survives
//! from one segment file to the next; when a worker stops between files, it
//! has reported what it holds, and the next worker takes it over. The
//! protocol is in [`super::protocol`].
//!
//! The worker gets an empty environment, `/` as its directory, no core
//! dumps, and a low scheduling priority. The parent enforces a wall-clock
//! timeout for loading and for each request (scaled to its audio), the
//! model's memory budget by polling the worker's physical footprint (macOS
//! does not enforce `RLIMIT_AS`), and a [`StopCheck`] (cancel, memory
//! pressure); it kills and reaps the worker on any breach, on a malformed
//! frame, and on drop.

use super::protocol::{
    self, FrameError, Request, Response, SegmentRequest, TranscriptLine, WorkerEngine,
};
use super::stream::{self, SpeechEngine, Streams};
use super::{
    speech_analyzer, JobConfig, JobError, StopCheck, Transcribed, Transcriber, TranscriptionSession,
};
use crate::recording::policy;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The hidden argument that makes the binary act as a transcription worker.
pub(crate) const WORKER_ARG: &str = "--threestrands-transcription-worker";
/// Marker the parent sets so a stray launch with the argument stays a
/// normal start.
pub(crate) const WORKER_ENV: &str = "THREESTRANDS_TRANSCRIPTION_WORKER";

pub(crate) const EXIT_PROTOCOL: i32 = 20;
pub(crate) const EXIT_MODEL: i32 = 21;
pub(crate) const EXIT_AUDIO: i32 = 22;
pub(crate) const EXIT_ENGINE: i32 = 23;
pub(crate) const EXIT_LIMITS: i32 = 24;

/// How often the parent checks on a running worker.
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long the parent waits for a worker whose output ended to exit, to
/// read its exit status.
const EXIT_GRACE: Duration = Duration::from_secs(2);

pub const TIMEOUT_MESSAGE: &str = "Transcription took too long and was stopped.";
pub const MEMORY_MESSAGE: &str = "Transcription needed too much memory and was stopped.";
pub const CRASH_MESSAGE: &str = "The transcription engine stopped unexpectedly.";
pub const MODEL_MESSAGE: &str = "The transcription model couldn't be loaded.";
pub const AUDIO_MESSAGE: &str = "The recording's audio couldn't be read.";

// ---- Worker side ----

/// Runs the worker and exits when this process was started as one; returns
/// at once otherwise. Call before anything else in `main`.
pub(crate) fn run_worker_if_requested() {
    let mut args = std::env::args_os();
    args.next();
    let is_worker = args.next().is_some_and(|arg| arg == WORKER_ARG)
        && args.next().is_none()
        && std::env::var_os(WORKER_ENV).is_some_and(|value| value == "1");
    if is_worker {
        std::process::exit(worker_main());
    }
}

fn worker_main() -> i32 {
    if !apply_limits() {
        return EXIT_LIMITS;
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_worker_io(
        &mut stdin.lock(),
        &mut std::io::BufWriter::new(stdout.lock()),
        open_engine,
    )
}

#[cfg(target_os = "macos")]
fn open_engine(config: &WorkerConfig) -> Result<Box<dyn SpeechEngine>, String> {
    super::engine::WhisperEngine::open(config)
        .map(|engine| Box::new(engine) as Box<dyn SpeechEngine>)
}

#[cfg(not(target_os = "macos"))]
fn open_engine(_config: &WorkerConfig) -> Result<Box<dyn SpeechEngine>, String> {
    Err("Transcription isn't available on this platform.".into())
}

/// The worker's settings, from the parent's first frame.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerConfig {
    pub engine: WorkerEngine,
    pub model_path: PathBuf,
    pub vad_model_path: PathBuf,
    pub language: Option<String>,
    pub initial_prompt: Option<String>,
    pub threads: u32,
}

/// The worker's loop over any streams and engine: config, then segments
/// until the input ends. Returns the exit code.
pub(crate) fn run_worker_io<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    open: impl FnOnce(&WorkerConfig) -> Result<Box<dyn SpeechEngine>, String>,
) -> i32 {
    let max = policy::WORKER_MAX_FRAME_BYTES;
    let config = match protocol::read_frame::<_, Request>(input, max) {
        Ok(Some(Request::Config {
            engine,
            model_path,
            vad_model_path,
            language,
            initial_prompt,
            threads,
        })) => WorkerConfig {
            engine,
            model_path,
            vad_model_path,
            language,
            initial_prompt,
            threads,
        },
        _ => return EXIT_PROTOCOL,
    };
    let mut engine = if config.engine == WorkerEngine::SpeechAnalyzer {
        if !speech_analyzer::is_available() {
            return EXIT_MODEL;
        }
        None
    } else {
        match open(&config) {
            Ok(engine) => Some(engine),
            Err(_) => return EXIT_MODEL,
        }
    };
    if protocol::write_frame(output, &Response::Ready).is_err() {
        return EXIT_PROTOCOL;
    }
    let mut streams = Streams::default();
    loop {
        let request = match protocol::read_frame::<_, Request>(input, max) {
            Ok(None) => return 0,
            Ok(Some(Request::Segment(request))) => request,
            Ok(Some(Request::Config { .. })) | Err(_) => return EXIT_PROTOCOL,
        };
        if let Some(held) = &request.carry_in {
            if streams
                .restore(request.track, held, stream::read_segment)
                .is_err()
            {
                return EXIT_AUDIO;
            }
        }
        let samples = match stream::read_segment(&request.wav_path) {
            Ok(samples) => samples,
            Err(_) => return EXIT_AUDIO,
        };
        let (lines, held, detected_language) = match engine.as_deref_mut() {
            Some(whisper) => {
                let lines = match streams.feed(whisper, &request, samples) {
                    Ok(lines) => lines,
                    Err(_) => return EXIT_ENGINE,
                };
                let held = streams.held(request.track);
                let detected_language = whisper.detected_language().map(str::to_string);
                (lines, held, detected_language)
            }
            None => {
                let raw = match speech_analyzer::transcribe_file(
                    &request.wav_path,
                    config.language.as_deref(),
                    config.initial_prompt.as_deref(),
                ) {
                    Ok(raw) => raw,
                    Err(_) => return EXIT_ENGINE,
                };
                let lines = raw
                    .into_iter()
                    .map(|(from_ms, to_ms, text)| {
                        let s0 = (from_ms as usize * policy::OUTPUT_SAMPLE_RATE as usize / 1000)
                            .min(samples.len());
                        let s1 = (to_ms as usize * policy::OUTPUT_SAMPLE_RATE as usize / 1000)
                            .clamp(s0, samples.len());
                        let rms = if s1 > s0 {
                            stream::slice_rms(&samples[s0..s1])
                        } else {
                            stream::slice_rms(&samples)
                        };
                        TranscriptLine {
                            track: request.track,
                            start_ms: request.start_ms + from_ms,
                            end_ms: request.start_ms + to_ms,
                            text,
                            rms: Some(rms),
                            speaker_label: None,
                        }
                    })
                    .collect();
                (lines, None, None)
            }
        };
        if protocol::write_frame(
            output,
            &Response::Lines {
                lines,
                held,
                detected_language,
            },
        )
        .is_err()
        {
            return EXIT_PROTOCOL;
        }
    }
}

#[cfg(unix)]
fn apply_limits() -> bool {
    let no_core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `no_core` is a valid rlimit that outlives the call.
    let core = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &no_core) == 0 };
    // SAFETY: plain call on this process; a failure leaves the priority.
    let nice = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, policy::WORKER_NICE) == 0 };
    core && nice
}

#[cfg(not(unix))]
fn apply_limits() -> bool {
    true
}

// ---- Parent side ----

/// What to spawn as the worker; tests substitute a stand-in process.
#[derive(Debug, Clone)]
pub(crate) struct WorkerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

impl WorkerCommand {
    /// This app's own binary in worker mode.
    pub(crate) fn current_exe() -> std::io::Result<Self> {
        Ok(Self {
            program: std::env::current_exe()?,
            args: vec![WORKER_ARG.into()],
            env: vec![(WORKER_ENV.into(), "1".into())],
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WorkerLimits {
    pub load_timeout: Duration,
    /// Overrides [`policy::worker_request_timeout`] (tests).
    pub request_timeout: Option<Duration>,
    /// [`WorkerTranscriber`] replaces it with the job's model budget.
    pub memory_bytes: u64,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            load_timeout: policy::WORKER_LOAD_TIMEOUT,
            request_timeout: None,
            memory_bytes: policy::WORKER_MEMORY_ACCURATE_BYTES,
        }
    }
}

/// Kills and reaps the child however the parent leaves.
struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn footprint_bytes(pid: u32) -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: the buffer is a zeroed `rusage_info_v2`, the layout flavor 2
    // fills in, and `usage` outlives the call.
    let status = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            usage.as_mut_ptr().cast(),
        )
    };
    // SAFETY: initialized by the successful call above.
    (status == 0).then(|| unsafe { usage.assume_init() }.ri_phys_footprint)
}

#[cfg(not(target_os = "macos"))]
fn footprint_bytes(_pid: u32) -> Option<u64> {
    None
}

/// A running worker, configured and ready for segment requests.
pub(crate) struct WorkerSession {
    guard: ChildGuard,
    stdin: std::process::ChildStdin,
    responses: Receiver<Result<Response, String>>,
    limits: WorkerLimits,
    stop: Arc<dyn StopCheck>,
}

impl WorkerSession {
    /// Starts the worker, sends its configuration, and waits until it has
    /// loaded the models.
    pub(crate) fn spawn(
        command: &WorkerCommand,
        config: &WorkerConfig,
        limits: WorkerLimits,
        stop: Arc<dyn StopCheck>,
    ) -> Result<Self, JobError> {
        let crash = || JobError::Failed(CRASH_MESSAGE.into());
        let mut process = std::process::Command::new(&command.program);
        process
            .args(&command.args)
            .env_clear()
            .envs(command.env.iter().map(|(key, value)| (key, value)))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        process.current_dir("/");
        let mut guard = ChildGuard(process.spawn().map_err(|_| crash())?);
        let stdin = guard.0.stdin.take().ok_or_else(crash)?;
        let mut stdout = guard.0.stdout.take().ok_or_else(crash)?;
        let (sender, responses) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("transcription-worker-reader".into())
            .spawn(move || loop {
                let frame = protocol::read_frame::<_, Response>(
                    &mut stdout,
                    policy::WORKER_MAX_FRAME_BYTES,
                );
                let stop = !matches!(frame, Ok(Some(_)));
                let message = match frame {
                    Ok(Some(response)) => Ok(response),
                    Ok(None) => Err("the worker's output ended".to_string()),
                    Err(FrameError::Io(error)) => Err(error.to_string()),
                    Err(error) => Err(error.to_string()),
                };
                if sender.send(message).is_err() || stop {
                    break;
                }
            })
            .map_err(|_| crash())?;
        let mut session = Self {
            guard,
            stdin,
            responses,
            limits,
            stop,
        };
        session.send(&Request::Config {
            engine: config.engine,
            model_path: config.model_path.clone(),
            vad_model_path: config.vad_model_path.clone(),
            language: config.language.clone(),
            initial_prompt: config.initial_prompt.clone(),
            threads: config.threads,
        })?;
        match session.wait(limits.load_timeout)? {
            Response::Ready => Ok(session),
            Response::Lines { .. } => Err(crash()),
        }
    }

    fn send(&mut self, request: &Request) -> Result<(), JobError> {
        if protocol::write_frame(&mut self.stdin, request).is_err() {
            // The worker is gone; its exit status says why.
            return Err(self.failure());
        }
        Ok(())
    }

    /// The reason a worker that stopped talking failed, from its exit code.
    fn failure(&mut self) -> JobError {
        let deadline = Instant::now() + EXIT_GRACE;
        let status = loop {
            match self.guard.0.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
                _ => break None,
            }
        };
        let message = match status.and_then(|status| status.code()) {
            Some(EXIT_MODEL) => MODEL_MESSAGE,
            Some(EXIT_AUDIO) => AUDIO_MESSAGE,
            _ => CRASH_MESSAGE,
        };
        JobError::Failed(message.into())
    }

    fn wait(&mut self, timeout: Duration) -> Result<Response, JobError> {
        let deadline = Instant::now() + timeout;
        let pid = self.guard.0.id();
        loop {
            match self.responses.recv_timeout(POLL_INTERVAL) {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return Err(self.failure()),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if let Some(reason) = self.stop.stop_reason() {
                return Err(reason);
            }
            if Instant::now() >= deadline {
                return Err(JobError::Failed(TIMEOUT_MESSAGE.into()));
            }
            if footprint_bytes(pid).is_some_and(|bytes| bytes > self.limits.memory_bytes) {
                return Err(JobError::Failed(MEMORY_MESSAGE.into()));
            }
        }
    }

    /// Transcribes one segment file holding `audio_ms` of audio.
    pub(crate) fn transcribe(
        &mut self,
        request: &SegmentRequest,
        audio_ms: u64,
    ) -> Result<Transcribed, JobError> {
        self.send(&Request::Segment(request.clone()))?;
        let timeout = self
            .limits
            .request_timeout
            .unwrap_or_else(|| policy::worker_request_timeout(audio_ms));
        match self.wait(timeout)? {
            Response::Lines {
                lines,
                held,
                detected_language,
            } => {
                if lines.iter().any(|line| line.track != request.track) {
                    return Err(JobError::Failed(CRASH_MESSAGE.into()));
                }
                Ok(Transcribed {
                    lines,
                    held,
                    detected_language,
                })
            }
            Response::Ready => Err(JobError::Failed(CRASH_MESSAGE.into())),
        }
    }

    #[cfg(test)]
    fn pid(&self) -> u32 {
        self.guard.0.id()
    }
}

impl TranscriptionSession for WorkerSession {
    fn transcribe(
        &mut self,
        request: &SegmentRequest,
        audio_ms: u64,
    ) -> Result<Transcribed, JobError> {
        WorkerSession::transcribe(self, request, audio_ms)
    }
}

/// The real [`Transcriber`]: one worker process per batch.
pub(crate) struct WorkerTranscriber {
    pub command: WorkerCommand,
    pub limits: WorkerLimits,
}

impl Transcriber for WorkerTranscriber {
    fn open(
        &self,
        config: &JobConfig,
        stop: Arc<dyn StopCheck>,
    ) -> Result<Box<dyn TranscriptionSession>, JobError> {
        let limits = WorkerLimits {
            memory_bytes: config.memory_bytes,
            ..self.limits
        };
        let config = WorkerConfig {
            engine: config.engine,
            model_path: config.model_path.clone(),
            vad_model_path: config.vad_model_path.clone(),
            language: config.language.clone(),
            initial_prompt: config.initial_prompt.clone(),
            threads: config.threads,
        };
        let session = WorkerSession::spawn(&self.command, &config, limits, stop)?;
        Ok(Box::new(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::capture::Track;
    use crate::recording::transcribe::protocol::TranscriptLine;
    use crate::recording::transcribe::stream::tests::FakeEngine;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn config() -> WorkerConfig {
        WorkerConfig {
            engine: WorkerEngine::Whisper,
            model_path: "/m/model.bin".into(),
            vad_model_path: "/m/vad.bin".into(),
            language: Some("en".into()),
            initial_prompt: None,
            threads: 2,
        }
    }

    fn config_frame() -> Request {
        let config = config();
        Request::Config {
            engine: config.engine,
            model_path: config.model_path,
            vad_model_path: config.vad_model_path,
            language: config.language,
            initial_prompt: config.initial_prompt,
            threads: config.threads,
        }
    }

    fn frames(requests: &[Request]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for request in requests {
            protocol::write_frame(&mut bytes, request).unwrap();
        }
        bytes
    }

    fn responses(mut bytes: &[u8]) -> Vec<Response> {
        let mut out = Vec::new();
        while let Some(response) = protocol::read_frame(&mut bytes, 1 << 20).unwrap() {
            out.push(response);
        }
        out
    }

    fn fake_engine(_: &WorkerConfig) -> Result<Box<dyn SpeechEngine>, String> {
        Ok(Box::new(FakeEngine::new()))
    }

    /// A one-second segment file with speech (loud samples) in its middle.
    fn speech_wav(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let mut wav = crate::recording::segments::WavWriter::create(&path).unwrap();
        let mut samples = vec![0i16; 16_000];
        for sample in &mut samples[4_000..8_000] {
            *sample = 20_000;
        }
        wav.write(&samples).unwrap();
        wav.finish().unwrap();
        path
    }

    #[test]
    fn the_worker_loads_once_then_answers_each_segment() {
        let dir = crate::recording::test_support::TempDir::new();
        let wav = speech_wav(dir.path(), "mic-0001.wav");
        let segment = |start_ms| {
            Request::Segment(SegmentRequest {
                track: Track::Mic,
                wav_path: wav.clone(),
                start_ms,
                continues: false,
                ends_run: true,
                carry_in: None,
            })
        };
        let input = frames(&[config_frame(), segment(0), segment(60_000)]);
        let mut out = Vec::new();
        let mut opened = Vec::new();
        let code = run_worker_io(&mut input.as_slice(), &mut out, |config| {
            opened.push(config.clone());
            fake_engine(config)
        });
        assert_eq!(code, 0);
        assert_eq!(opened, vec![config()], "the model loads once");
        let replies = responses(&out);
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0], Response::Ready);
        let Response::Lines { lines, held, .. } = &replies[2] else {
            panic!("{replies:?}");
        };
        assert_eq!(lines.len(), 1);
        assert_eq!((lines[0].start_ms, lines[0].end_ms), (60_250, 60_500));
        assert_eq!(held, &None, "the run ended");
    }

    /// One second of a run, `index` seconds in: speech over its last half
    /// when `ends_in_speech`, and over its first half when `starts_in_speech`.
    fn run_wav(dir: &std::path::Path, index: u32, starts: bool, ends: bool) -> PathBuf {
        let path = dir.join(format!("mic-{index:04}.wav"));
        let mut wav = crate::recording::segments::WavWriter::create(&path).unwrap();
        let mut samples = vec![0i16; 16_000];
        if starts {
            samples[..8_000].fill(20_000);
        }
        if ends {
            samples[8_000..].fill(20_000);
        }
        wav.write(&samples).unwrap();
        wav.finish().unwrap();
        path
    }

    #[test]
    fn a_second_worker_takes_over_the_audio_the_first_held_back() {
        let dir = crate::recording::test_support::TempDir::new();
        let first = run_wav(dir.path(), 1, false, true);
        let second = run_wav(dir.path(), 2, true, false);
        let request = |path: &PathBuf, start_ms, continues, ends_run| SegmentRequest {
            track: Track::Mic,
            wav_path: path.clone(),
            start_ms,
            continues,
            ends_run,
            carry_in: None,
        };
        // The first worker transcribes the first file and is stopped.
        let mut out = Vec::new();
        let input = frames(&[
            config_frame(),
            Request::Segment(request(&first, 0, false, false)),
        ]);
        assert_eq!(
            run_worker_io(&mut input.as_slice(), &mut out, fake_engine),
            0
        );
        let replies = responses(&out);
        let Response::Lines { lines, held, .. } = &replies[1] else {
            panic!("{replies:?}");
        };
        assert!(lines.is_empty(), "the speech at the end is held: {lines:?}");
        let held = held.clone().expect("held audio is reported");
        assert_eq!(held.pieces[0].wav_path, first);

        // The next worker gets it back with the next file.
        let mut carried = request(&second, 1_000, true, true);
        carried.carry_in = Some(held);
        let mut out = Vec::new();
        let input = frames(&[config_frame(), Request::Segment(carried.clone())]);
        assert_eq!(
            run_worker_io(&mut input.as_slice(), &mut out, fake_engine),
            0
        );
        let replies = responses(&out);
        let Response::Lines { lines, held, .. } = &replies[1] else {
            panic!("{replies:?}");
        };
        let placed: Vec<(u64, u64, &str)> = lines
            .iter()
            .map(|l| (l.start_ms, l.end_ms, l.text.as_str()))
            .collect();
        assert_eq!(
            placed,
            [(500, 1_500, "speech 0.61035156 for 1000 ms")],
            "one sentence across both workers"
        );
        assert_eq!(held, &None);

        // Held audio that isn't in its file stops the worker.
        let mut outside = carried;
        outside.carry_in.as_mut().unwrap().pieces[0].from_sample = 16_000;
        let input = frames(&[config_frame(), Request::Segment(outside)]);
        assert_eq!(
            run_worker_io(&mut input.as_slice(), &mut Vec::new(), fake_engine),
            EXIT_AUDIO
        );
    }

    #[test]
    fn the_worker_exits_with_a_code_for_each_failure() {
        let dir = crate::recording::test_support::TempDir::new();
        let missing = Request::Segment(SegmentRequest {
            track: Track::Mic,
            wav_path: dir.path().join("mic-0009.wav"),
            start_ms: 0,
            continues: false,
            ends_run: true,
            carry_in: None,
        });
        let run =
            |input: Vec<u8>| run_worker_io(&mut input.as_slice(), &mut Vec::new(), fake_engine);
        assert_eq!(run(Vec::new()), EXIT_PROTOCOL, "no config");
        assert_eq!(
            run(frames(&[missing.clone()])),
            EXIT_PROTOCOL,
            "a segment before config"
        );
        assert_eq!(
            run(frames(&[config_frame(), config_frame()])),
            EXIT_PROTOCOL
        );
        assert_eq!(run(frames(&[config_frame(), missing])), EXIT_AUDIO);
        let mut truncated = frames(&[config_frame()]);
        truncated.extend_from_slice(&[0, 0, 1]);
        assert_eq!(run(truncated), EXIT_PROTOCOL);
        let mut out = Vec::new();
        assert_eq!(
            run_worker_io(
                &mut frames(&[config_frame()]).as_slice(),
                &mut out,
                |_| Err("no model".into())
            ),
            EXIT_MODEL
        );
        assert!(out.is_empty(), "never ready");
        assert_eq!(
            run(frames(&[config_frame()])),
            0,
            "ready, then the input ended"
        );
    }

    #[cfg(unix)]
    mod process {
        use super::*;

        /// A stand-in worker: a shell script that may first copy prepared
        /// frames to stdout.
        struct StandIn {
            dir: crate::recording::test_support::TempDir,
        }

        impl StandIn {
            fn new() -> Self {
                Self {
                    dir: crate::recording::test_support::TempDir::new(),
                }
            }

            fn frames_file(&self, name: &str, replies: &[Response]) -> PathBuf {
                let mut bytes = Vec::new();
                for reply in replies {
                    protocol::write_frame(&mut bytes, reply).unwrap();
                }
                let path = self.dir.path().join(name);
                std::fs::write(&path, bytes).unwrap();
                path
            }

            fn raw_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
                let path = self.dir.path().join(name);
                std::fs::write(&path, bytes).unwrap();
                path
            }

            fn pid_file(&self) -> PathBuf {
                self.dir.path().join("pid")
            }

            fn pid(&self) -> i32 {
                std::fs::read_to_string(self.pid_file())
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap()
            }
        }

        fn shell(script: &str) -> WorkerCommand {
            WorkerCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
                env: Vec::new(),
            }
        }

        fn limits() -> WorkerLimits {
            WorkerLimits {
                load_timeout: Duration::from_secs(10),
                request_timeout: Some(Duration::from_secs(10)),
                memory_bytes: u64::MAX,
            }
        }

        fn request() -> SegmentRequest {
            SegmentRequest {
                track: Track::Mic,
                wav_path: "/x/mic-0001.wav".into(),
                start_ms: 0,
                continues: false,
                ends_run: true,
                carry_in: None,
            }
        }

        fn line(track: Track) -> TranscriptLine {
            TranscriptLine {
                track,
                start_ms: 1,
                end_ms: 2,
                text: "Hello.".into(),
                rms: None,
                speaker_label: None,
            }
        }

        fn process_exists(pid: i32) -> bool {
            // SAFETY: signal 0 only checks that the process exists.
            unsafe { libc::kill(pid, 0) == 0 }
        }

        fn spawn(command: &WorkerCommand, limits: WorkerLimits) -> Result<WorkerSession, JobError> {
            WorkerSession::spawn(command, &config(), limits, Arc::new(AtomicBool::new(false)))
        }

        fn failed(message: &str) -> JobError {
            JobError::Failed(message.into())
        }

        #[test]
        fn a_well_behaved_worker_is_configured_then_answers() {
            let s = StandIn::new();
            let replies = s.frames_file(
                "replies",
                &[
                    Response::Ready,
                    Response::Lines {
                        lines: vec![line(Track::Mic)],
                        held: None,
                        detected_language: None,
                    },
                ],
            );
            // Reads the config frame and the request before answering each.
            let script = format!(
                "head -c 4 >/dev/null; cat '{}'; sleep 30",
                replies.display()
            );
            let mut session = spawn(&shell(&script), limits()).unwrap();
            assert_eq!(
                session.transcribe(&request(), 60_000),
                Ok(Transcribed {
                    lines: vec![line(Track::Mic)],
                    held: None,
                    detected_language: None,
                })
            );
        }

        #[test]
        fn malformed_truncated_and_unexpected_output_fails_the_job() {
            let s = StandIn::new();
            let mut prefix = (100u32).to_be_bytes().to_vec();
            prefix.extend_from_slice(b"{\"type\":");
            let truncated = s.raw_file("truncated", &prefix);
            let mut junk = (5u32).to_be_bytes().to_vec();
            junk.extend_from_slice(b"junk!");
            let junk = s.raw_file("junk", &junk);
            let oversized = s.raw_file("oversized", &u32::MAX.to_be_bytes());
            let lines_first = s.frames_file(
                "lines-first",
                &[Response::Lines {
                    lines: vec![],
                    held: None,
                    detected_language: None,
                }],
            );
            for (name, file) in [
                ("truncated", truncated),
                ("junk", junk),
                ("oversized", oversized),
                ("lines before ready", lines_first),
            ] {
                let script = format!("cat '{}'", file.display());
                assert_eq!(
                    spawn(&shell(&script), limits()).err(),
                    Some(failed(CRASH_MESSAGE)),
                    "{name}"
                );
            }
            // Lines for the other track than asked for.
            let wrong = s.frames_file(
                "wrong-track",
                &[
                    Response::Ready,
                    Response::Lines {
                        lines: vec![line(Track::System)],
                        held: None,
                        detected_language: None,
                    },
                ],
            );
            let script = format!("cat '{}'; sleep 30", wrong.display());
            let mut session = spawn(&shell(&script), limits()).unwrap();
            assert_eq!(
                session.transcribe(&request(), 1_000),
                Err(failed(CRASH_MESSAGE))
            );
        }

        #[test]
        fn exit_codes_name_the_failure() {
            for (code, message) in [
                (EXIT_MODEL, MODEL_MESSAGE),
                (EXIT_AUDIO, AUDIO_MESSAGE),
                (EXIT_ENGINE, CRASH_MESSAGE),
                (0, CRASH_MESSAGE),
            ] {
                assert_eq!(
                    spawn(&shell(&format!("exit {code}")), limits()).err(),
                    Some(failed(message)),
                    "{code}"
                );
            }
            let s = StandIn::new();
            let ready = s.frames_file("ready", &[Response::Ready]);
            let script = format!(
                "cat '{}'; head -c 4 >/dev/null; exit {EXIT_AUDIO}",
                ready.display()
            );
            let mut session = spawn(&shell(&script), limits()).unwrap();
            assert_eq!(
                session.transcribe(&request(), 1_000),
                Err(failed(AUDIO_MESSAGE))
            );
            for script in ["kill -SEGV $$", "kill -KILL $$"] {
                assert_eq!(
                    spawn(&shell(script), limits()).err(),
                    Some(failed(CRASH_MESSAGE))
                );
            }
            let missing = WorkerCommand {
                program: "/nonexistent/worker".into(),
                args: Vec::new(),
                env: Vec::new(),
            };
            assert_eq!(spawn(&missing, limits()).err(), Some(failed(CRASH_MESSAGE)));
        }

        #[test]
        fn a_hanging_worker_is_killed_and_reaped_at_the_timeouts() {
            let s = StandIn::new();
            let script = format!("echo $$ > '{}'; exec sleep 30", s.pid_file().display());
            let started = Instant::now();
            let short = WorkerLimits {
                load_timeout: Duration::from_millis(300),
                ..limits()
            };
            assert_eq!(
                spawn(&shell(&script), short).err(),
                Some(failed(TIMEOUT_MESSAGE))
            );
            assert!(started.elapsed() < Duration::from_secs(10));
            assert!(!process_exists(s.pid()), "the worker was left behind");

            // Loaded, then stuck on a segment.
            let ready = s.frames_file("ready", &[Response::Ready]);
            let script = format!(
                "echo $$ > '{}'; cat '{}'; exec sleep 30",
                s.pid_file().display(),
                ready.display()
            );
            let request_limit = WorkerLimits {
                request_timeout: Some(Duration::from_millis(300)),
                ..limits()
            };
            let mut session = spawn(&shell(&script), request_limit).unwrap();
            let pid = s.pid();
            assert_eq!(
                session.transcribe(&request(), 1_000),
                Err(failed(TIMEOUT_MESSAGE))
            );
            drop(session);
            assert!(!process_exists(pid));
        }

        #[test]
        fn dropping_a_session_kills_and_reaps_the_worker() {
            let s = StandIn::new();
            let ready = s.frames_file("ready", &[Response::Ready]);
            let script = format!(
                "echo $$ > '{}'; cat '{}'; exec sleep 30",
                s.pid_file().display(),
                ready.display()
            );
            let session = spawn(&shell(&script), limits()).unwrap();
            let pid = session.pid() as i32;
            assert_eq!(pid, s.pid());
            assert!(process_exists(pid));
            drop(session);
            assert!(!process_exists(pid));
        }

        #[test]
        fn cancelling_stops_a_request_and_the_worker() {
            let s = StandIn::new();
            let ready = s.frames_file("ready", &[Response::Ready]);
            let script = format!("cat '{}'; exec sleep 30", ready.display());
            let cancel = Arc::new(AtomicBool::new(false));
            let mut session =
                WorkerSession::spawn(&shell(&script), &config(), limits(), cancel.clone()).unwrap();
            let flag = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                flag.store(true, Ordering::SeqCst);
            });
            let started = Instant::now();
            assert_eq!(
                session.transcribe(&request(), 1_000),
                Err(JobError::Cancelled)
            );
            assert!(started.elapsed() < Duration::from_secs(5));
        }

        /// Stops with [`JobError::Interrupted`] once `pressed` is set, as
        /// the scheduler's check does under memory pressure.
        struct Interrupt(AtomicBool);
        impl StopCheck for Interrupt {
            fn stop_reason(&self) -> Option<JobError> {
                self.0
                    .load(Ordering::SeqCst)
                    .then_some(JobError::Interrupted)
            }
        }

        #[test]
        fn an_interruption_stops_a_request_or_a_load_and_kills_the_worker() {
            let s = StandIn::new();
            let ready = s.frames_file("ready", &[Response::Ready]);
            let script = format!(
                "echo $$ > '{}'; cat '{}'; exec sleep 30",
                s.pid_file().display(),
                ready.display()
            );
            let stop = Arc::new(Interrupt(AtomicBool::new(false)));
            let mut session =
                WorkerSession::spawn(&shell(&script), &config(), limits(), stop.clone()).unwrap();
            let pid = s.pid();
            let flag = stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                flag.0.store(true, Ordering::SeqCst);
            });
            let started = Instant::now();
            assert_eq!(
                session.transcribe(&request(), 1_000),
                Err(JobError::Interrupted)
            );
            assert!(started.elapsed() < Duration::from_secs(5));
            drop(session);
            assert!(!process_exists(pid));

            // While the model loads.
            let script = format!("echo $$ > '{}'; exec sleep 30", s.pid_file().display());
            let stopped = Arc::new(Interrupt(AtomicBool::new(true)));
            assert_eq!(
                WorkerSession::spawn(&shell(&script), &config(), limits(), stopped).err(),
                Some(JobError::Interrupted)
            );
            assert!(!process_exists(s.pid()));
        }

        #[test]
        fn the_worker_gets_a_clean_environment() {
            std::env::set_var("TRANSCRIPTION_WORKER_TEST_SECRET", "leak");
            let s = StandIn::new();
            let ready = s.frames_file("ready", &[Response::Ready]);
            let script = format!(
                "[ -z \"$TRANSCRIPTION_WORKER_TEST_SECRET\" ] && [ -z \"$HOME\" ] && [ \"$MARKER\" = 1 ] && [ \"$(pwd)\" = / ] && cat '{}'; sleep 30",
                ready.display()
            );
            let mut command = shell(&script);
            command.env.push(("MARKER".into(), "1".into()));
            assert!(spawn(&command, limits()).is_ok());
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn macos_kills_a_worker_over_the_memory_cap() {
            let s = StandIn::new();
            let script = format!("echo $$ > '{}'; exec sleep 30", s.pid_file().display());
            let over = WorkerLimits {
                // Any live process exceeds one byte of footprint.
                memory_bytes: 1,
                ..limits()
            };
            let started = Instant::now();
            assert_eq!(
                spawn(&shell(&script), over).err(),
                Some(failed(MEMORY_MESSAGE))
            );
            assert!(started.elapsed() < Duration::from_secs(5));
            assert!(!process_exists(s.pid()));
            assert!(footprint_bytes(std::process::id()).unwrap() > 0);
        }
    }
}
