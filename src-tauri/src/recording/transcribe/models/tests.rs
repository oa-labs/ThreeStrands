//! Model downloads against a local HTTP server.

use super::*;
use crate::recording::test_support::TempDir;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::sync::atomic::AtomicU64;

pub(crate) const MODEL_BYTES: usize = 100_000;
const VAD_BYTES: usize = 1_000;

pub(crate) fn content(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32 * 7 + seed as u32) as u8)
        .collect()
}

pub(crate) fn model_content() -> Vec<u8> {
    content(MODEL_BYTES, 3)
}

pub(crate) fn vad_content() -> Vec<u8> {
    content(VAD_BYTES, 9)
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// What the server answers.
pub(crate) struct Reply {
    pub status: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Content-Length to claim, when it differs from the body.
    pub declared: Option<usize>,
    /// Pause after this many body bytes.
    pub pause_after: Option<(usize, Duration)>,
}

impl Reply {
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status: "200 OK",
            headers: Vec::new(),
            body,
            declared: None,
            pause_after: None,
        }
    }

    pub fn redirect(location: &str) -> Self {
        Self {
            status: "302 Found",
            headers: vec![("Location".into(), location.into())],
            ..Self::ok(Vec::new())
        }
    }

    /// The bytes of `full` from `start`, as a range response.
    pub fn range(full: &[u8], start: usize) -> Self {
        Self {
            status: "206 Partial Content",
            headers: vec![(
                "Content-Range".into(),
                format!("bytes {start}-{}/{}", full.len() - 1, full.len()),
            )],
            ..Self::ok(full[start..].to_vec())
        }
    }

    /// `full`, honoring a `Range` request the way a file server does.
    pub fn file(full: &[u8], range: Option<u64>) -> Self {
        match range {
            Some(start) => Self::range(full, start as usize),
            None => Self::ok(full.to_vec()),
        }
    }
}

type Handler = dyn Fn(&str, Option<u64>) -> Reply + Send + Sync;

pub(crate) struct Server {
    pub port: u16,
    pub log: Arc<Mutex<Vec<(String, Option<u64>)>>>,
}

impl Server {
    pub fn start(handler: impl Fn(&str, Option<u64>) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let requests = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let handler = handler.clone();
                let requests = requests.clone();
                std::thread::spawn(move || serve(stream, &*handler, &requests));
            }
        });
        Self { port, log }
    }

    pub fn requests(&self) -> Vec<(String, Option<u64>)> {
        self.log.lock().unwrap().clone()
    }
}

fn serve(
    mut stream: std::net::TcpStream,
    handler: &Handler,
    log: &Mutex<Vec<(String, Option<u64>)>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("range: bytes=") {
            range = value.trim().trim_end_matches('-').parse().ok();
        }
    }
    log.lock().unwrap().push((path.clone(), range));
    let reply = handler(&path, range);
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.declared.unwrap_or(reply.body.len())
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    match reply.pause_after {
        Some((at, pause)) => {
            let at = at.min(reply.body.len());
            let _ = stream.write_all(&reply.body[..at]);
            let _ = stream.flush();
            std::thread::sleep(pause);
            let _ = stream.write_all(&reply.body[at..]);
        }
        None => {
            let _ = stream.write_all(&reply.body);
        }
    }
    let _ = stream.flush();
}

struct Disk(AtomicU64);

impl DiskProbe for Disk {
    fn free_bytes(&self) -> Result<u64, String> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

pub(crate) fn test_pins(server_port: u16) -> Pins {
    let url = |path: &str| format!("http://127.0.0.1:{server_port}{path}");
    Pins {
        fast: Pin {
            file_name: "fast.bin".into(),
            bytes: MODEL_BYTES as u64,
            sha256: hex(&model_content()),
            url: url("/fast"),
        },
        accurate: Pin {
            file_name: "accurate.bin".into(),
            bytes: MODEL_BYTES as u64,
            sha256: hex(&content(MODEL_BYTES, 5)),
            url: url("/accurate"),
        },
        vad: Pin {
            file_name: "vad.bin".into(),
            bytes: VAD_BYTES as u64,
            sha256: hex(&vad_content()),
            url: url("/vad"),
        },
    }
}

/// Plain http to this one local server.
pub(crate) fn local_urls(port: u16) -> UrlPolicy {
    UrlPolicy {
        https_only: false,
        allowed: Arc::new(move |url: &reqwest::Url| {
            url.host_str() == Some("127.0.0.1") && url.port() == Some(port)
        }),
    }
}

type Events = Arc<
    Mutex<
        Vec<(
            Vec<TranscriptionModelStatus>,
            Option<TranscriptionModelChoice>,
        )>,
    >,
>;

struct Harness {
    server: Server,
    dir: TempDir,
    disk: Arc<Disk>,
    store: Arc<ModelStore>,
    events: Events,
}

const FAST: TranscriptionModelChoice = TranscriptionModelChoice::Fast;

impl Harness {
    fn new(handler: impl Fn(&str, Option<u64>) -> Reply + Send + Sync + 'static) -> Self {
        let server = Server::start(handler);
        let dir = TempDir::new();
        let disk = Arc::new(Disk(AtomicU64::new(u64::MAX)));
        let store = Arc::new(ModelStore::new(
            dir.path().join("recording-models"),
            test_pins(server.port),
            disk.clone(),
            local_urls(server.port),
        ));
        let events: Events = Arc::default();
        let sink = events.clone();
        store.set_listener(Arc::new(move |statuses, ready| {
            sink.lock().unwrap().push((statuses.to_vec(), ready));
        }));
        Self {
            server,
            dir,
            disk,
            store,
            events,
        }
    }

    /// A server with the usual files, honoring ranges.
    fn serving() -> Self {
        Self::new(|path, range| match path {
            "/vad" => Reply::file(&vad_content(), range),
            "/fast" => Reply::file(&model_content(), range),
            _ => Reply {
                status: "404 Not Found",
                ..Reply::ok(Vec::new())
            },
        })
    }

    fn models(&self) -> PathBuf {
        self.dir.path().join("recording-models")
    }

    fn download(&self) {
        self.store.start_download(FAST).unwrap().join().unwrap();
    }

    fn fast(&self) -> TranscriptionModelStatus {
        self.store.statuses().remove(0)
    }

    fn part(&self) -> PathBuf {
        self.models().join("fast.bin.part")
    }

    fn install_vad(&self) {
        std::fs::create_dir_all(self.models()).unwrap();
        std::fs::write(self.models().join("vad.bin"), vad_content()).unwrap();
    }
}

#[test]
fn the_voice_model_downloads_first_then_the_model_becomes_ready() {
    let h = Harness::serving();
    assert_eq!(h.fast().state, ModelState::Absent);
    h.download();
    let paths: Vec<String> = h.server.requests().into_iter().map(|(p, _)| p).collect();
    assert_eq!(paths, ["/vad", "/fast"]);
    assert_eq!(
        h.fast(),
        TranscriptionModelStatus {
            model: FAST,
            bytes: MODEL_BYTES as u64,
            state: ModelState::Ready,
            received_bytes: MODEL_BYTES as u64,
            error: None,
        }
    );
    assert_eq!(
        std::fs::read(h.models().join("fast.bin")).unwrap(),
        model_content()
    );
    assert!(!h.part().exists());
    assert!(h.store.is_ready(FAST));
    assert!(!h.store.is_ready(TranscriptionModelChoice::Accurate));
    let events = h.events.lock().unwrap();
    let states: Vec<ModelState> = events.iter().map(|(s, _)| s[0].state).collect();
    assert_eq!(states.first(), Some(&ModelState::Downloading));
    assert!(states.contains(&ModelState::Verifying));
    assert_eq!(states.last(), Some(&ModelState::Ready));
    assert_eq!(events.last().unwrap().1, Some(FAST), "announced as ready");
    assert!(events
        .iter()
        .all(|(s, _)| s.len() == 2 && s[1].model == TranscriptionModelChoice::Accurate));
    // A second model download doesn't fetch the voice model again.
    drop(events);
    assert_eq!(
        h.store.start_download(FAST).unwrap_err(),
        "This model is already downloaded."
    );
}

#[test]
fn statuses_use_the_contract_shape() {
    let h = Harness::serving();
    assert_eq!(
        serde_json::to_value(h.store.statuses()).unwrap(),
        serde_json::json!([
            { "model": "fast", "bytes": 100_000, "state": "absent", "receivedBytes": 0, "error": null },
            { "model": "accurate", "bytes": 100_000, "state": "absent", "receivedBytes": 0, "error": null },
        ])
    );
}

#[test]
fn a_partial_download_resumes_with_a_range_request() {
    let h = Harness::serving();
    h.install_vad();
    std::fs::write(h.part(), &model_content()[..40_000]).unwrap();
    assert_eq!(
        h.fast().received_bytes,
        40_000,
        "absent, with a partial to resume"
    );
    h.download();
    assert_eq!(
        h.server.requests(),
        vec![("/fast".to_string(), Some(40_000))]
    );
    assert_eq!(h.fast().state, ModelState::Ready);
    assert_eq!(
        std::fs::read(h.models().join("fast.bin")).unwrap(),
        model_content()
    );
}

#[test]
fn a_server_that_ignores_the_range_restarts_the_file_from_zero() {
    let h = Harness::new(|path, _range| match path {
        "/fast" => Reply::ok(model_content()),
        _ => Reply::ok(vad_content()),
    });
    h.install_vad();
    std::fs::write(h.part(), vec![0xAA; 40_000]).unwrap();
    h.download();
    assert_eq!(
        h.server.requests(),
        vec![("/fast".to_string(), Some(40_000))]
    );
    assert_eq!(h.fast().state, ModelState::Ready);
    assert_eq!(
        std::fs::read(h.models().join("fast.bin")).unwrap(),
        model_content()
    );
}

#[test]
fn a_download_ending_early_keeps_its_partial_file_for_a_resume() {
    let calls = Arc::new(AtomicU64::new(0));
    let counter = calls.clone();
    let h = Harness::new(move |path, range| match path {
        "/fast" if counter.fetch_add(1, Ordering::SeqCst) == 0 => Reply {
            declared: Some(MODEL_BYTES),
            ..Reply::ok(model_content()[..30_000].to_vec())
        },
        "/fast" => Reply::file(&model_content(), range),
        _ => Reply::ok(vad_content()),
    });
    h.install_vad();
    h.download();
    let failed = h.fast();
    assert_eq!(failed.state, ModelState::Failed);
    assert_eq!(failed.error.as_deref(), Some(FAILED_MESSAGE));
    assert_eq!(failed.received_bytes, 30_000);
    h.download();
    assert_eq!(h.server.requests()[1], ("/fast".to_string(), Some(30_000)));
    assert_eq!(h.fast().state, ModelState::Ready);
}

#[test]
fn a_download_past_the_pinned_size_is_aborted_and_deleted() {
    let h = Harness::new(|path, _| match path {
        "/fast" => {
            let mut body = model_content();
            body.extend_from_slice(&[0; 10]);
            Reply::ok(body)
        }
        _ => Reply::ok(vad_content()),
    });
    h.download();
    let status = h.fast();
    assert_eq!(status.state, ModelState::Failed);
    assert_eq!(status.error.as_deref(), Some(TOO_LARGE_MESSAGE));
    assert!(!h.part().exists());
    assert!(!h.models().join("fast.bin").exists());
}

#[test]
fn a_hash_mismatch_deletes_the_partial_and_fails() {
    let h = Harness::new(|path, _| match path {
        "/fast" => Reply::ok(content(MODEL_BYTES, 4)),
        _ => Reply::ok(vad_content()),
    });
    h.download();
    let status = h.fast();
    assert_eq!(status.state, ModelState::Failed);
    assert_eq!(status.error.as_deref(), Some(MISMATCH_MESSAGE));
    assert_eq!(status.received_bytes, 0);
    assert!(!h.part().exists());
    assert!(!h.models().join("fast.bin").exists());
    assert!(
        h.models().join("vad.bin").exists(),
        "the voice model was fine"
    );
    // A failed voice model fails the download before the model is fetched.
    let h = Harness::new(|_, _| Reply::ok(vec![1; VAD_BYTES]));
    h.download();
    assert_eq!(h.fast().error.as_deref(), Some(MISMATCH_MESSAGE));
    assert_eq!(h.server.requests().len(), 1);
}

#[test]
fn redirects_are_followed_only_to_allowed_hosts_and_only_so_far() {
    let h = Harness::new(|path, range| match path {
        "/vad" => Reply::redirect("/files/vad"),
        "/files/vad" => Reply::ok(vad_content()),
        "/fast" => Reply::redirect("/files/fast"),
        "/files/fast" => Reply::file(&model_content(), range),
        _ => Reply::ok(Vec::new()),
    });
    h.download();
    assert_eq!(
        h.fast().state,
        ModelState::Ready,
        "same-host redirects are fine"
    );

    let port = Arc::new(AtomicU64::new(0));
    let seen = port.clone();
    let h = Harness::new(move |path, _| match path {
        "/vad" => Reply::ok(vad_content()),
        _ => Reply::redirect(&format!(
            "http://localhost:{}/fast",
            seen.load(Ordering::SeqCst)
        )),
    });
    port.store(h.server.port as u64, Ordering::SeqCst);
    h.download();
    assert_eq!(h.fast().error.as_deref(), Some(HOST_MESSAGE));
    assert_eq!(
        h.server.requests().len(),
        2,
        "the disallowed host was never contacted"
    );

    let h = Harness::new(|path, _| match path {
        "/vad" => Reply::ok(vad_content()),
        _ => Reply::redirect("/fast"),
    });
    h.download();
    assert_eq!(h.fast().error.as_deref(), Some(HOST_MESSAGE));
    assert_eq!(
        h.server.requests().len(),
        1 + 1 + policy::MODEL_DOWNLOAD_MAX_REDIRECTS,
        "the cap stops a redirect loop"
    );
}

#[test]
fn a_download_that_would_leave_too_little_disk_is_refused() {
    let h = Harness::serving();
    std::fs::create_dir_all(h.models()).unwrap();
    std::fs::write(h.part(), &model_content()[..40_000]).unwrap();
    // The voice model and the rest of the model.
    let remaining = (VAD_BYTES + MODEL_BYTES - 40_000) as u64;
    h.disk.0.store(
        policy::MIN_FREE_DISK_BYTES + remaining - 1,
        Ordering::SeqCst,
    );
    assert_eq!(h.store.start_download(FAST).unwrap_err(), NO_DISK_MESSAGE);
    assert!(h.server.requests().is_empty());
    assert_eq!(h.fast().state, ModelState::Absent);
    h.disk
        .0
        .store(policy::MIN_FREE_DISK_BYTES + remaining, Ordering::SeqCst);
    h.download();
    assert_eq!(h.fast().state, ModelState::Ready);
}

#[test]
fn cancelling_stops_the_download_and_keeps_the_partial_file() {
    let h = Harness::new(|path, range| match path {
        "/fast" => Reply {
            pause_after: Some((30_000, Duration::from_secs(20))),
            ..Reply::file(&model_content(), range)
        },
        _ => Reply::ok(vad_content()),
    });
    let handle = h.store.start_download(FAST).unwrap();
    assert_eq!(
        h.store.start_download(FAST).unwrap_err(),
        "This model is already downloading."
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while ModelStore::size_of(&h.part()) < 30_000 {
        assert!(Instant::now() < deadline, "the first bytes never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(h.fast().state, ModelState::Downloading);
    let started = Instant::now();
    h.store.cancel_download(FAST);
    assert_eq!(h.fast().state, ModelState::Absent, "at once");
    handle.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    let status = h.fast();
    assert_eq!(
        (status.state, status.received_bytes, status.error),
        (ModelState::Absent, 30_000, None)
    );
    h.store.cancel_download(FAST); // nothing downloading: fine
}

#[test]
fn a_damaged_model_is_deleted_before_it_is_loaded() {
    let h = Harness::serving();
    h.download();
    let (model, vad) = h.store.verified_paths(FAST).unwrap();
    assert_eq!(model, h.models().join("fast.bin"));
    assert_eq!(vad, h.models().join("vad.bin"));
    let mut bytes = model_content();
    bytes[500] ^= 1;
    std::fs::write(&model, bytes).unwrap();
    assert!(h.store.is_ready(FAST), "the size alone still looks right");
    assert_eq!(h.store.verified_paths(FAST).unwrap_err(), DAMAGED_MESSAGE);
    assert!(!model.exists());
    let status = h.fast();
    assert_eq!(status.state, ModelState::Failed);
    assert_eq!(status.error.as_deref(), Some(DAMAGED_MESSAGE));
    assert_eq!(
        h.store.verified_paths(FAST).unwrap_err(),
        "The transcription model isn't downloaded."
    );
    // Downloading again clears the failure.
    h.download();
    assert_eq!(h.fast().state, ModelState::Ready);
}

#[test]
fn deleting_removes_the_model_and_its_partial_but_not_the_voice_model() {
    let h = Harness::serving();
    h.download();
    std::fs::write(h.part(), b"partial").unwrap();
    h.store.delete(FAST).unwrap();
    assert!(!h.models().join("fast.bin").exists());
    assert!(!h.part().exists());
    assert!(h.models().join("vad.bin").exists());
    assert_eq!(h.fast().state, ModelState::Absent);
    h.store.delete(FAST).unwrap(); // nothing there: fine
}

#[test]
fn production_downloads_reach_only_hugging_face_over_https() {
    let allowed = UrlPolicy::hugging_face().allowed;
    let check = |url: &str| allowed(&reqwest::Url::parse(url).unwrap());
    for pin in [
        Pins::pinned().fast,
        Pins::pinned().accurate,
        Pins::pinned().vad,
    ] {
        assert!(check(&pin.url), "{}", pin.url);
    }
    assert!(check("https://cas-bridge.xethub.hf.co/xet/abc?sig=1"));
    for url in [
        "http://huggingface.co/x",
        "https://huggingface.co:8443/x",
        "https://user:pw@huggingface.co/x",
        "https://example.com/x",
        "https://huggingface.co.example.com/x",
        "ftp://huggingface.co/x",
    ] {
        assert!(!check(url), "{url}");
    }
    assert!(UrlPolicy::hugging_face().https_only);
}

#[test]
fn content_ranges_are_parsed() {
    assert_eq!(
        content_range_start("bytes 40000-99999/100000"),
        Some(40_000)
    );
    assert_eq!(content_range_start("bytes */100000"), None);
    assert_eq!(content_range_start("items 1-2/3"), None);
}

/// Downloads the real Fast model (and the voice model) from Hugging Face
/// through this code, for the worker's smoke test:
///
/// ```text
/// THREESTRANDS_SMOKE_MODELS=/path/to/models cargo test --lib smoke_download_fast_model -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn smoke_download_fast_model() {
    let dir = PathBuf::from(std::env::var("THREESTRANDS_SMOKE_MODELS").unwrap());
    let store = Arc::new(ModelStore::new(
        dir.clone(),
        Pins::pinned(),
        Arc::new(crate::recording::VolumeProbe(std::env::temp_dir())),
        UrlPolicy::hugging_face(),
    ));
    let started = Instant::now();
    if !store.is_ready(FAST) {
        store.start_download(FAST).unwrap().join().unwrap();
    }
    let status = store.statuses().remove(0);
    println!("{status:?} after {:.1} s", started.elapsed().as_secs_f64());
    assert_eq!(status.state, ModelState::Ready, "{:?}", status.error);
    let started = Instant::now();
    store.verified_paths(FAST).unwrap();
    println!("verified in {:.2} s", started.elapsed().as_secs_f64());
}
