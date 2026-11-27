//! The transcription worker: whisper.cpp in a killable child process.
//!
//! whisper.cpp is C++ reading large model files, with a GPU backend; a crash
//! or runaway allocation there must not take the mail client down. So, as
//! for PDFs (`pdf_worker.rs`), the app re-executes its own binary with
//! [`WORKER_ARG`] and [`WORKER_ENV`] (checked first thing in `main`, before
//! Tauri, logging, or the database start).
//!
//! Unlike the PDF worker, one worker lives for a whole transcription job
//! (one recording), so the model loads once and each track's held-back
//! audio ([`super::stream`]) survives from one segment file to the next.
//! The protocol is in [`super::protocol`].
//!
//! The worker gets an empty environment, `/` as its directory, no core
//! dumps, and a low scheduling priority. The parent enforces a wall-clock
//! timeout for loading and for each request (scaled to its audio), a memory
//! cap by polling the worker's physical footprint (macOS does not enforce
//! `RLIMIT_AS`), and a cancel flag; it kills and reaps the worker on any
//! breach, on a malformed frame, and on drop.

use super::protocol::{self, FrameError, Request, Response, SegmentRequest, TranscriptLine};
use super::stream::{self, SpeechEngine, Streams};
use super::{JobConfig, JobError, Transcriber, TranscriptionSession};
use crate::recording::policy;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
    pub model_path: PathBuf,
    pub vad_model_path: PathBuf,
    pub language: Option<String>,
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
            model_path,
            vad_model_path,
            language,
            threads,
        })) => WorkerConfig {
            model_path,
            vad_model_path,
            language,
            threads,
        },
        _ => return EXIT_PROTOCOL,
    };
    let mut engine = match open(&config) {
        Ok(engine) => engine,
        Err(_) => return EXIT_MODEL,
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
        let samples = match stream::read_segment(&request.wav_path) {
            Ok(samples) => samples,
            Err(_) => return EXIT_AUDIO,
        };
        let lines = match streams.feed(engine.as_mut(), &request, samples) {
            Ok(lines) => lines,
            Err(_) => return EXIT_ENGINE,
        };
        if protocol::write_frame(output, &Response::Lines { lines }).is_err() {
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
    pub memory_bytes: u64,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            load_timeout: policy::WORKER_LOAD_TIMEOUT,
            request_timeout: None,
            memory_bytes: policy::WORKER_MEMORY_BYTES,
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
fn footprint_bytes(pid: u32) -> Option<u64> {
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
    cancel: Arc<AtomicBool>,
}

impl WorkerSession {
    /// Starts the worker, sends its configuration, and waits until it has
    /// loaded the models.
    pub(crate) fn spawn(
        command: &WorkerCommand,
        config: &WorkerConfig,
        limits: WorkerLimits,
        cancel: Arc<AtomicBool>,
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
            cancel,
        };
        session.send(&Request::Config {
            model_path: config.model_path.clone(),
            vad_model_path: config.vad_model_path.clone(),
            language: config.language.clone(),
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
            if self.cancel.load(Ordering::SeqCst) {
                return Err(JobError::Cancelled);
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
    ) -> Result<Vec<TranscriptLine>, JobError> {
        self.send(&Request::Segment(request.clone()))?;
        let timeout = self
            .limits
            .request_timeout
            .unwrap_or_else(|| policy::worker_request_timeout(audio_ms));
        match self.wait(timeout)? {
            Response::Lines { lines } => {
                if lines.iter().any(|line| line.track != request.track) {
                    return Err(JobError::Failed(CRASH_MESSAGE.into()));
                }
                Ok(lines)
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
    ) -> Result<Vec<TranscriptLine>, JobError> {
        WorkerSession::transcribe(self, request, audio_ms)
    }
}

/// The real [`Transcriber`]: one worker process per job.
pub(crate) struct WorkerTranscriber {
    pub command: WorkerCommand,
    pub limits: WorkerLimits,
}

impl Transcriber for WorkerTranscriber {
    fn open(
        &self,
        config: &JobConfig,
        cancel: Arc<AtomicBool>,
    ) -> Result<Box<dyn TranscriptionSession>, JobError> {
        let config = WorkerConfig {
            model_path: config.model_path.clone(),
            vad_model_path: config.vad_model_path.clone(),
            language: config.language.clone(),
            threads: config.threads,
        };
        let session = WorkerSession::spawn(&self.command, &config, self.limits, cancel)?;
        Ok(Box::new(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::capture::Track;
    use crate::recording::transcribe::stream::tests::FakeEngine;

    fn config() -> WorkerConfig {
        WorkerConfig {
            model_path: "/m/model.bin".into(),
            vad_model_path: "/m/vad.bin".into(),
            language: Some("en".into()),
            threads: 2,
        }
    }

    fn config_frame() -> Request {
        let config = config();
        Request::Config {
            model_path: config.model_path,
            vad_model_path: config.vad_model_path,
            language: config.language,
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
        let Response::Lines { lines } = &replies[2] else {
            panic!("{replies:?}");
        };
        assert_eq!(lines.len(), 1);
        assert_eq!((lines[0].start_ms, lines[0].end_ms), (60_250, 60_500));
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
            }
        }

        fn line(track: Track) -> TranscriptLine {
            TranscriptLine {
                track,
                start_ms: 1,
                end_ms: 2,
                text: "Hello.".into(),
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
                Ok(vec![line(Track::Mic)])
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
            let lines_first = s.frames_file("lines-first", &[Response::Lines { lines: vec![] }]);
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
