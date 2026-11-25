//! PDF parsing in a killable child process.
//!
//! `pdf-extract` and `lopdf` cannot cap decompressed stream size or parse
//! depth, and a stack overflow aborts the whole process, so a hostile PDF is
//! never parsed in the app. The app re-executes its own binary with
//! `WORKER_ARG` (checked first thing in `main`, before Tauri, logging, or the
//! database start), pipes the PDF to the worker's stdin, and reads framed page
//! text from its stdout.
//!
//! Protocol: stdin is the raw PDF until EOF. On success the worker exits 0
//! after writing each page as a 4-byte big-endian length and that many bytes
//! of UTF-8. `EXIT_PASSWORD` means the PDF is encrypted; any other status,
//! signal death, or malformed output means it is not a readable PDF.
//!
//! The worker runs with an empty environment, no arguments derived from the
//! file, and (on unix) resource limits set before parsing. The parent enforces
//! the wall-clock timeout, an output byte cap, and, on macOS where
//! `RLIMIT_AS` is not enforced, a memory cap by polling the worker's physical
//! footprint. It kills and reaps the worker on any breach and on drop.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::attachment_text::{
    self, PdfFailure, MAX_CONCURRENT_PDF_WORKERS, MAX_PDF_WORKER_OUTPUT_BYTES,
    PDF_WORKER_MEMORY_BYTES,
};
use crate::limits::MAX_ATTACHMENT_BYTES;

/// The hidden argument that makes the binary act as a PDF worker.
pub(crate) const WORKER_ARG: &str = "--threestrands-pdf-worker";
/// Marker the parent sets so a stray launch with the argument stays a normal start.
const WORKER_ENV: &str = "THREESTRANDS_PDF_WORKER";
pub(crate) const EXIT_PASSWORD: i32 = 10;
pub(crate) const EXIT_UNREADABLE: i32 = 11;
/// How often the parent checks on a running worker.
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const TIMEOUT_REASON: &str = "it took too long to read";

// ---- Framing ----

pub(crate) fn encode_pages<W: Write>(pages: &[String], out: &mut W) -> std::io::Result<()> {
    for page in pages {
        let length =
            u32::try_from(page.len()).map_err(|_| std::io::Error::other("page text too large"))?;
        out.write_all(&length.to_be_bytes())?;
        out.write_all(page.as_bytes())?;
    }
    Ok(())
}

/// Parses worker output; any truncation or invalid UTF-8 rejects the lot.
pub(crate) fn decode_pages(mut bytes: &[u8]) -> Option<Vec<String>> {
    let mut pages = Vec::new();
    while !bytes.is_empty() {
        let (length, rest) = bytes.split_first_chunk::<4>()?;
        let length = u32::from_be_bytes(*length) as usize;
        if rest.len() < length {
            return None;
        }
        let (page, rest) = rest.split_at(length);
        pages.push(String::from_utf8(page.to_vec()).ok()?);
        bytes = rest;
    }
    Some(pages)
}

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
        return EXIT_UNREADABLE;
    }
    let mut input = Vec::new();
    let read = std::io::stdin()
        .lock()
        .take(MAX_ATTACHMENT_BYTES as u64 + 1)
        .read_to_end(&mut input);
    if read.is_err() || input.len() > MAX_ATTACHMENT_BYTES {
        return EXIT_UNREADABLE;
    }
    let stdout = std::io::stdout();
    run_worker_io(input, &mut std::io::BufWriter::new(stdout.lock()))
}

/// Parses `input` on a large fixed stack and writes the framed pages.
pub(crate) fn run_worker_io<W: Write>(input: Vec<u8>, out: &mut W) -> i32 {
    let parsed = std::thread::Builder::new()
        .stack_size(attachment_text::PDF_WORKER_STACK_BYTES)
        .spawn(move || attachment_text::pdf_pages(&input))
        .map(|handle| handle.join());
    match parsed {
        Ok(Ok(Ok(pages))) => {
            if encode_pages(&pages, out).and_then(|()| out.flush()).is_ok() {
                0
            } else {
                EXIT_UNREADABLE
            }
        }
        Ok(Ok(Err(PdfFailure::Password))) => EXIT_PASSWORD,
        _ => EXIT_UNREADABLE,
    }
}

#[cfg(unix)]
fn apply_limits() -> bool {
    use crate::attachment_text::{PDF_WORKER_CPU_SECONDS, PDF_WORKER_MAX_OPEN_FILES};
    macro_rules! limit {
        ($resource:expr, $value:expr) => {{
            let value = $value as libc::rlim_t;
            let limit = libc::rlimit {
                rlim_cur: value,
                rlim_max: value,
            };
            // SAFETY: `limit` is a valid rlimit that outlives the call.
            unsafe { libc::setrlimit($resource, &limit) == 0 }
        }};
    }
    let ok = limit!(libc::RLIMIT_CORE, 0)
        && limit!(libc::RLIMIT_CPU, PDF_WORKER_CPU_SECONDS)
        && limit!(libc::RLIMIT_NOFILE, PDF_WORKER_MAX_OPEN_FILES);
    // macOS does not enforce RLIMIT_AS or RLIMIT_DATA; the parent polls there.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let ok = ok && limit!(libc::RLIMIT_AS, PDF_WORKER_MEMORY_BYTES);
    ok
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
    pub timeout: Duration,
    pub max_output: usize,
    pub memory_bytes: u64,
}

impl WorkerLimits {
    pub(crate) fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            max_output: MAX_PDF_WORKER_OUTPUT_BYTES,
            memory_bytes: PDF_WORKER_MEMORY_BYTES,
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
    // Linux enforces the cap inside the worker with RLIMIT_AS.
    None
}

/// Runs one worker to completion under `limits`, blocking the calling thread.
/// Returns the user-facing reason on failure.
pub(crate) fn run_worker(
    command: &WorkerCommand,
    input: Vec<u8>,
    limits: WorkerLimits,
) -> Result<Vec<String>, String> {
    let unreadable = || PdfFailure::Unreadable.message();
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
    let mut guard = ChildGuard(process.spawn().map_err(|_| unreadable())?);
    let pid = guard.0.id();
    let mut stdin = guard.0.stdin.take().ok_or_else(unreadable)?;
    let mut stdout = guard.0.stdout.take().ok_or_else(unreadable)?;

    // The child may die before reading its input; a broken pipe is fine.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let overflow = Arc::new(AtomicBool::new(false));
    let reader = {
        let overflow = Arc::clone(&overflow);
        let cap = limits.max_output;
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut chunk = [0u8; 16 * 1024];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if out.len() + count > cap {
                            overflow.store(true, Ordering::SeqCst);
                            break;
                        }
                        out.extend_from_slice(&chunk[..count]);
                    }
                }
            }
            out
        })
    };

    let deadline = std::time::Instant::now() + limits.timeout;
    let mut breach: Option<String> = None;
    let status = loop {
        match guard.0.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if std::time::Instant::now() >= deadline {
            breach = Some(TIMEOUT_REASON.to_string());
        } else if overflow.load(Ordering::SeqCst)
            || footprint_bytes(pid).is_some_and(|bytes| bytes > limits.memory_bytes)
        {
            breach = Some(unreadable());
        }
        if breach.is_some() {
            break None;
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    // Kills a worker that breached a limit and reaps it either way.
    drop(guard);
    let _ = writer.join();
    let output = reader.join().unwrap_or_default();
    if let Some(reason) = breach {
        return Err(reason);
    }
    let Some(status) = status else {
        return Err(unreadable());
    };
    if overflow.load(Ordering::SeqCst) {
        return Err(unreadable());
    }
    match status.code() {
        Some(0) => decode_pages(&output).ok_or_else(unreadable),
        Some(EXIT_PASSWORD) => Err(PdfFailure::Password.message()),
        _ => Err(unreadable()),
    }
}

static SLOTS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_PDF_WORKERS)));

/// Waits for a free slot in `slots`, then runs `job` on the blocking pool
/// holding it until the job itself returns, so a runaway job still counts
/// against the limit.
pub(crate) async fn spawn_in_slot<T: Send + 'static>(
    slots: &Arc<Semaphore>,
    job: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    let permit = Arc::clone(slots)
        .acquire_owned()
        .await
        .expect("the PDF slot semaphore is never closed");
    tokio::task::spawn_blocking(move || {
        let result = job();
        drop(permit);
        result
    })
}

/// Parses a PDF into page text, isolated from the app where the platform
/// allows it, one PDF at a time.
pub(crate) async fn pdf_pages(
    command: &WorkerCommand,
    bytes: Vec<u8>,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    pdf_pages_in(&SLOTS, command, bytes, timeout).await
}

#[cfg(desktop)]
async fn pdf_pages_in(
    slots: &Arc<Semaphore>,
    command: &WorkerCommand,
    bytes: Vec<u8>,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let command = command.clone();
    let task = spawn_in_slot(slots, move || {
        run_worker(&command, bytes, WorkerLimits::with_timeout(timeout))
    })
    .await;
    task.await.map_err(|_| PdfFailure::Unreadable.message())?
}

// Android cannot re-execute the app binary, so PDFs are parsed in-process
// WITHOUT isolation: a stack overflow still aborts the app and a runaway
// parse cannot be killed, only abandoned at the timeout. The one-at-a-time
// slot stays held until that thread finishes, which bounds the damage to one
// runaway thread, and failures are cached by the caller.
#[cfg(not(desktop))]
async fn pdf_pages_in(
    slots: &Arc<Semaphore>,
    _command: &WorkerCommand,
    bytes: Vec<u8>,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let task = spawn_in_slot(slots, move || {
        attachment_text::pdf_pages(&bytes).map_err(PdfFailure::message)
    })
    .await;
    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(PdfFailure::Unreadable.message()),
        Err(_) => Err(TIMEOUT_REASON.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn frames_round_trip_and_reject_damage() {
        let original = pages(&["one", "", "héllo \u{1F600}"]);
        let mut framed = Vec::new();
        encode_pages(&original, &mut framed).unwrap();
        assert_eq!(decode_pages(&framed), Some(original));
        assert_eq!(decode_pages(b""), Some(Vec::new()));
        // Truncated length, truncated body, and invalid UTF-8.
        assert_eq!(decode_pages(&[0, 0, 0]), None);
        assert_eq!(decode_pages(&[0, 0, 0, 5, b'a']), None);
        assert_eq!(decode_pages(&[0, 0, 0, 1, 0xFF]), None);
        // A length that claims more than is there, however large.
        assert_eq!(decode_pages(&[0xFF, 0xFF, 0xFF, 0xFF, b'a']), None);
    }

    #[test]
    fn the_worker_maps_pdfs_to_frames_and_exit_codes() {
        let mut out = Vec::new();
        let pdf = crate::attachment_text::tests::pdf(&[
            "BT /F1 12 Tf 72 700 Td (First page) Tj ET",
            "BT /F1 12 Tf 72 700 Td (Second page) Tj ET",
        ]);
        assert_eq!(run_worker_io(pdf, &mut out), 0);
        let decoded = decode_pages(&out).unwrap();
        assert_eq!(decoded.len(), 2);
        assert!(decoded[0].contains("First page") && decoded[1].contains("Second page"));

        let mut out = Vec::new();
        assert_eq!(
            run_worker_io(b"%PDF-1.4\nnot really".to_vec(), &mut out),
            EXIT_UNREADABLE
        );
        assert!(out.is_empty());
        assert_eq!(run_worker_io(Vec::new(), &mut Vec::new()), EXIT_UNREADABLE);
        assert_eq!(
            run_worker_io(
                crate::attachment_text::tests::encrypted_pdf(),
                &mut Vec::new()
            ),
            EXIT_PASSWORD
        );
    }

    #[cfg(unix)]
    mod process {
        use super::*;
        use std::time::Instant;

        fn shell(script: &str) -> WorkerCommand {
            WorkerCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
                env: Vec::new(),
            }
        }

        fn limits(timeout: Duration, max_output: usize) -> WorkerLimits {
            WorkerLimits {
                timeout,
                max_output,
                memory_bytes: u64::MAX,
            }
        }

        const UNREADABLE: &str = "it isn't a readable PDF";

        fn process_exists(pid: i32) -> bool {
            // SAFETY: signal 0 only checks that the process exists.
            unsafe { libc::kill(pid, 0) == 0 }
        }

        #[test]
        fn reads_framed_pages_and_maps_exit_codes() {
            let ok = run_worker(
                &shell("cat >/dev/null; printf '\\0\\0\\0\\2hi\\0\\0\\0\\1!'"),
                b"%PDF".to_vec(),
                limits(Duration::from_secs(5), 1024),
            );
            assert_eq!(ok, Ok(pages(&["hi", "!"])));
            assert_eq!(
                run_worker(
                    &shell("exit 10"),
                    Vec::new(),
                    limits(Duration::from_secs(5), 1024)
                ),
                Err("it is password protected".to_string())
            );
            for script in [
                "exit 11",
                "exit 3",
                "printf 'junk'",
                "printf '\\0\\0\\0\\9ab'",
            ] {
                assert_eq!(
                    run_worker(
                        &shell(script),
                        Vec::new(),
                        limits(Duration::from_secs(5), 1024)
                    ),
                    Err(UNREADABLE.to_string()),
                    "{script}"
                );
            }
            let missing = WorkerCommand {
                program: "/nonexistent/worker".into(),
                args: Vec::new(),
                env: Vec::new(),
            };
            assert_eq!(
                run_worker(&missing, Vec::new(), limits(Duration::from_secs(5), 1024)),
                Err(UNREADABLE.to_string())
            );
        }

        #[test]
        fn a_large_input_does_not_deadlock_a_worker_that_ignores_it() {
            let input = vec![b'x'; 8 * 1024 * 1024];
            assert_eq!(
                run_worker(
                    &shell("exit 11"),
                    input,
                    limits(Duration::from_secs(5), 1024)
                ),
                Err(UNREADABLE.to_string())
            );
        }

        #[test]
        fn a_hanging_worker_is_killed_and_reaped_at_the_timeout() {
            let pid_file = std::env::temp_dir().join(format!(
                "threestrands-pdf-worker-test-{}",
                std::process::id()
            ));
            let script = format!("echo $$ > '{}'; exec sleep 30", pid_file.display());
            let started = Instant::now();
            let result = run_worker(
                &shell(&script),
                Vec::new(),
                limits(Duration::from_millis(300), 1024),
            );
            assert_eq!(result, Err("it took too long to read".to_string()));
            assert!(started.elapsed() < Duration::from_secs(10));
            let pid: i32 = std::fs::read_to_string(&pid_file)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let _ = std::fs::remove_file(&pid_file);
            assert!(!process_exists(pid), "worker {pid} was left behind");
        }

        #[test]
        fn a_crashing_worker_fails_cleanly() {
            for script in ["kill -SEGV $$", "kill -ABRT $$", "kill -KILL $$"] {
                assert_eq!(
                    run_worker(
                        &shell(script),
                        Vec::new(),
                        limits(Duration::from_secs(5), 1024)
                    ),
                    Err(UNREADABLE.to_string()),
                    "{script}"
                );
            }
        }

        #[test]
        fn output_is_capped_below_at_and_above_the_limit() {
            let emit = |bytes: usize| format!("head -c {bytes} /dev/zero");
            let run = |bytes: usize| {
                run_worker(
                    &shell(&emit(bytes)),
                    Vec::new(),
                    limits(Duration::from_secs(5), 64),
                )
            };
            // Zeros decode as empty pages; the cap decides before decoding.
            assert_eq!(run(60).map(|pages| pages.len()), Ok(15));
            assert_eq!(run(64).map(|pages| pages.len()), Ok(16));
            assert_eq!(run(68), Err(UNREADABLE.to_string()));
            // An endless writer is killed rather than waited out.
            let started = Instant::now();
            assert_eq!(
                run_worker(
                    &shell("exec yes"),
                    Vec::new(),
                    limits(Duration::from_secs(20), 1024)
                ),
                Err(UNREADABLE.to_string())
            );
            assert!(started.elapsed() < Duration::from_secs(10));
        }

        #[test]
        fn the_worker_gets_a_clean_environment() {
            std::env::set_var("PDF_WORKER_TEST_SECRET", "leak");
            let script = "[ -z \"$PDF_WORKER_TEST_SECRET\" ] && [ -z \"$HOME\" ] && [ \"$MARKER\" = 1 ] && [ \"$(pwd)\" = / ]";
            let mut command = shell(script);
            command.env.push(("MARKER".into(), "1".into()));
            assert_eq!(
                run_worker(&command, Vec::new(), limits(Duration::from_secs(5), 1024)),
                Ok(Vec::new())
            );
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn macos_kills_a_worker_over_the_memory_cap() {
            let over = WorkerLimits {
                timeout: Duration::from_secs(20),
                max_output: 1024,
                // Any live process exceeds one byte of footprint.
                memory_bytes: 1,
            };
            let started = Instant::now();
            assert_eq!(
                run_worker(&shell("exec sleep 30"), Vec::new(), over),
                Err(UNREADABLE.to_string())
            );
            assert!(started.elapsed() < Duration::from_secs(10));
            assert!(footprint_bytes(std::process::id()).unwrap() > 0);
            let at = WorkerLimits {
                memory_bytes: u64::MAX,
                ..over
            };
            assert_eq!(run_worker(&shell("exit 0"), Vec::new(), at), Ok(Vec::new()));
        }

        #[tokio::test]
        async fn workers_run_no_more_than_the_slot_count_at_once() {
            for slots in [1usize, 2] {
                let semaphore = Arc::new(Semaphore::new(slots));
                let running = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let mut tasks = Vec::new();
                for _ in 0..4 {
                    let (running, peak) = (Arc::clone(&running), Arc::clone(&peak));
                    tasks.push(
                        spawn_in_slot(&semaphore, move || {
                            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                            peak.fetch_max(now, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(60));
                            running.fetch_sub(1, Ordering::SeqCst);
                        })
                        .await,
                    );
                }
                for task in tasks {
                    task.await.unwrap();
                }
                assert_eq!(peak.load(Ordering::SeqCst), slots);
            }
            assert_eq!(MAX_CONCURRENT_PDF_WORKERS, 1);
        }

        #[tokio::test]
        async fn a_second_pdf_waits_for_the_first_worker() {
            let semaphore = Arc::new(Semaphore::new(1));
            let command = shell("sleep 0.3; exit 11");
            let started = Instant::now();
            let (a, b) = tokio::join!(
                pdf_pages_in(&semaphore, &command, Vec::new(), Duration::from_secs(5)),
                pdf_pages_in(&semaphore, &command, Vec::new(), Duration::from_secs(5)),
            );
            assert!(a.is_err() && b.is_err());
            assert!(started.elapsed() >= Duration::from_millis(600));
        }
    }
}
