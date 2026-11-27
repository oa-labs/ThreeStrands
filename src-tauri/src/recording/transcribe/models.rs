//! Transcription models on disk, and downloading them.
//!
//! Files live in `recording-models/` under the app data directory, named as
//! pinned in [`policy`]. A download writes `<name>.part`, resumes it with
//! `Range: bytes=n-` (starting over if the server answers 200), aborts past
//! the pinned size, then checks the size and SHA-256 and renames the file
//! into place. A mismatch deletes the partial file. The small voice
//! activity model downloads first as part of either model's download and is
//! not reported separately.
//!
//! **Where from.** Only https, starting from the pinned URL, following at
//! most [`policy::MODEL_DOWNLOAD_MAX_REDIRECTS`] redirects, each to a host
//! [`policy::model_host_allowed`] accepts. Nothing downloaded is executed;
//! the worker only reads the files as model weights.
//!
//! **Ready** means the model and voice activity files exist at their pinned
//! sizes. The full SHA-256 is checked again before every worker load
//! ([`ModelStore::verified_paths`]); a damaged file is deleted and the model
//! marked failed.

use super::super::policy::{self, ModelFile, TranscriptionModel};
use super::super::settings::TranscriptionModelChoice;
use super::super::DiskProbe;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const NO_DISK_MESSAGE: &str = "Not enough free disk space to download the model.";
pub const DAMAGED_MESSAGE: &str = "The model file was damaged. Download it again.";
pub const MISMATCH_MESSAGE: &str =
    "The downloaded model didn't match the expected file. Download it again.";
pub const TOO_LARGE_MESSAGE: &str =
    "The download was larger than the expected model and was stopped.";
pub const FAILED_MESSAGE: &str =
    "The model couldn't be downloaded. Check your connection and try again.";
pub const HOST_MESSAGE: &str =
    "The model download was redirected somewhere unexpected and was stopped.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelState {
    Absent,
    Downloading,
    Verifying,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionModelStatus {
    pub model: TranscriptionModelChoice,
    pub bytes: u64,
    pub state: ModelState,
    pub received_bytes: u64,
    pub error: Option<String>,
}

/// A pinned file, owned so tests can pin small files of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub file_name: String,
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
}

impl From<ModelFile> for Pin {
    fn from(file: ModelFile) -> Self {
        Self {
            file_name: file.file_name.into(),
            bytes: file.bytes,
            sha256: file.sha256.into(),
            url: file.url.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pins {
    pub fast: Pin,
    pub accurate: Pin,
    pub vad: Pin,
}

impl Pins {
    pub fn pinned() -> Self {
        Self {
            fast: TranscriptionModel::Fast.file().into(),
            accurate: TranscriptionModel::Accurate.file().into(),
            vad: policy::VAD_MODEL.into(),
        }
    }

    pub fn model(&self, model: TranscriptionModelChoice) -> &Pin {
        match model {
            TranscriptionModelChoice::Fast => &self.fast,
            TranscriptionModelChoice::Accurate => &self.accurate,
        }
    }
}

/// Which URLs a download may fetch.
#[derive(Clone)]
pub struct UrlPolicy {
    pub https_only: bool,
    pub allowed: Arc<dyn Fn(&reqwest::Url) -> bool + Send + Sync>,
}

impl UrlPolicy {
    /// https to Hugging Face hosts only, on the default port, without
    /// credentials in the URL.
    pub fn hugging_face() -> Self {
        Self {
            https_only: true,
            allowed: Arc::new(|url: &reqwest::Url| {
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.port().is_none()
                    && url.host_str().is_some_and(policy::model_host_allowed)
            }),
        }
    }
}

/// What the store reports: every status change, and a model becoming ready.
pub type ModelListener =
    Arc<dyn Fn(&[TranscriptionModelStatus], Option<TranscriptionModelChoice>) + Send + Sync>;

struct Download {
    state: ModelState,
    received: u64,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Inner {
    downloads: HashMap<TranscriptionModelChoice, Download>,
    errors: HashMap<TranscriptionModelChoice, String>,
}

pub struct ModelStore {
    dir: PathBuf,
    pins: Pins,
    disk: Arc<dyn DiskProbe>,
    urls: UrlPolicy,
    inner: Mutex<Inner>,
    /// Signalled when a download ends.
    ended: Condvar,
    /// Held while the voice activity model downloads, so two downloads
    /// don't write it at once.
    vad_lock: Mutex<()>,
    listener: Mutex<Option<ModelListener>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const MODELS: [TranscriptionModelChoice; 2] = [
    TranscriptionModelChoice::Fast,
    TranscriptionModelChoice::Accurate,
];

enum FetchError {
    Cancelled,
    Failed(String),
}

impl ModelStore {
    pub fn new(dir: PathBuf, pins: Pins, disk: Arc<dyn DiskProbe>, urls: UrlPolicy) -> Self {
        Self {
            dir,
            pins,
            disk,
            urls,
            inner: Mutex::new(Inner::default()),
            ended: Condvar::new(),
            vad_lock: Mutex::new(()),
            listener: Mutex::new(None),
        }
    }

    pub fn set_listener(&self, listener: ModelListener) {
        *lock(&self.listener) = Some(listener);
    }

    fn path(&self, pin: &Pin) -> PathBuf {
        self.dir.join(&pin.file_name)
    }

    fn part_path(&self, pin: &Pin) -> PathBuf {
        self.dir.join(format!("{}.part", pin.file_name))
    }

    fn size_of(path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }

    fn file_ready(&self, pin: &Pin) -> bool {
        std::fs::metadata(self.path(pin)).is_ok_and(|m| m.is_file() && m.len() == pin.bytes)
    }

    /// The model and the voice activity model are on disk at their pinned
    /// sizes.
    pub fn is_ready(&self, model: TranscriptionModelChoice) -> bool {
        self.file_ready(self.pins.model(model)) && self.file_ready(&self.pins.vad)
    }

    pub fn statuses(&self) -> Vec<TranscriptionModelStatus> {
        let inner = lock(&self.inner);
        MODELS
            .iter()
            .map(|&model| self.status_of(&inner, model))
            .collect()
    }

    fn status_of(
        &self,
        inner: &Inner,
        model: TranscriptionModelChoice,
    ) -> TranscriptionModelStatus {
        let pin = self.pins.model(model);
        let partial = || Self::size_of(&self.part_path(pin)).min(pin.bytes);
        let (state, received_bytes, error) = match inner.downloads.get(&model) {
            Some(download) if !download.cancel.load(Ordering::SeqCst) => {
                (download.state, download.received, None)
            }
            _ if self.is_ready(model) => (ModelState::Ready, pin.bytes, None),
            _ => match inner.errors.get(&model) {
                Some(error) => (ModelState::Failed, partial(), Some(error.clone())),
                None => (ModelState::Absent, partial(), None),
            },
        };
        TranscriptionModelStatus {
            model,
            bytes: pin.bytes,
            state,
            received_bytes,
            error,
        }
    }

    fn notify(&self, ready: Option<TranscriptionModelChoice>) {
        let statuses = self.statuses();
        let listener = lock(&self.listener).clone();
        if let Some(listener) = listener {
            listener(&statuses, ready);
        }
    }

    /// Waits for a cancelled download of `model` to finish winding down.
    fn wait_gone<'a>(
        &'a self,
        mut inner: MutexGuard<'a, Inner>,
        model: TranscriptionModelChoice,
    ) -> MutexGuard<'a, Inner> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while inner
            .downloads
            .get(&model)
            .is_some_and(|d| d.cancel.load(Ordering::SeqCst))
        {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            inner = self
                .ended
                .wait_timeout(inner, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        inner
    }

    /// Starts downloading `model` (the voice activity model first, when it
    /// is missing) on a thread of its own.
    pub fn start_download(
        self: &Arc<Self>,
        model: TranscriptionModelChoice,
    ) -> Result<std::thread::JoinHandle<()>, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let inner = self.wait_gone(lock(&self.inner), model);
            let mut inner = inner;
            if inner.downloads.contains_key(&model) {
                return Err("This model is already downloading.".into());
            }
            if self.is_ready(model) {
                return Err("This model is already downloaded.".into());
            }
            let remaining = [self.pins.model(model), &self.pins.vad]
                .into_iter()
                .filter(|pin| !self.file_ready(pin))
                .map(|pin| {
                    pin.bytes
                        .saturating_sub(Self::size_of(&self.part_path(pin)).min(pin.bytes))
                })
                .sum::<u64>();
            let free = self
                .disk
                .free_bytes()
                .map_err(|_| "Couldn't check free disk space.".to_string())?;
            if !policy::download_fits(free, remaining) {
                return Err(NO_DISK_MESSAGE.into());
            }
            std::fs::create_dir_all(&self.dir)
                .map_err(|_| "Couldn't create the folder for transcription models.".to_string())?;
            inner.errors.remove(&model);
            let received = Self::size_of(&self.part_path(self.pins.model(model)));
            inner.downloads.insert(
                model,
                Download {
                    state: ModelState::Downloading,
                    received: received.min(self.pins.model(model).bytes),
                    cancel: cancel.clone(),
                },
            );
        }
        self.notify(None);
        let store = Arc::clone(self);
        std::thread::Builder::new()
            .name("transcription-model-download".into())
            .spawn(move || store.run_download(model, cancel))
            .map_err(|_| {
                let error = "Couldn't start the download.".to_string();
                lock(&self.inner).downloads.remove(&model);
                error
            })
    }

    /// Stops a download, keeping its partial file. Nothing happens when the
    /// model isn't downloading.
    pub fn cancel_download(&self, model: TranscriptionModelChoice) {
        let cancelled = match lock(&self.inner).downloads.get(&model) {
            Some(download) => !download.cancel.swap(true, Ordering::SeqCst),
            None => false,
        };
        if cancelled {
            self.notify(None);
        }
    }

    /// Deletes the model file and any partial download of it. The voice
    /// activity model stays, shared by both.
    pub fn delete(&self, model: TranscriptionModelChoice) -> Result<(), String> {
        self.cancel_download(model);
        {
            let mut inner = self.wait_gone(lock(&self.inner), model);
            if inner.downloads.contains_key(&model) {
                return Err("The download is still stopping. Try again in a moment.".into());
            }
            let pin = self.pins.model(model);
            for path in [self.path(pin), self.part_path(pin)] {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err("Couldn't delete the model file.".into()),
                }
            }
            inner.errors.remove(&model);
        }
        self.notify(None);
        Ok(())
    }

    /// The model's and the voice activity model's paths, after checking
    /// their full SHA-256. A damaged file is deleted and the model marked
    /// failed.
    pub fn verified_paths(
        &self,
        model: TranscriptionModelChoice,
    ) -> Result<(PathBuf, PathBuf), String> {
        let pin = self.pins.model(model);
        if !self.is_ready(model) {
            return Err("The transcription model isn't downloaded.".into());
        }
        let mut damaged = false;
        for file in [pin, &self.pins.vad] {
            let path = self.path(file);
            if sha256_file(&path).ok().as_deref() != Some(file.sha256.as_str()) {
                let _ = std::fs::remove_file(&path);
                damaged = true;
            }
        }
        if damaged {
            lock(&self.inner)
                .errors
                .insert(model, DAMAGED_MESSAGE.into());
            self.notify(None);
            return Err(DAMAGED_MESSAGE.into());
        }
        Ok((self.path(pin), self.path(&self.pins.vad)))
    }

    fn set_download(&self, model: TranscriptionModelChoice, state: ModelState, received: u64) {
        if let Some(download) = lock(&self.inner).downloads.get_mut(&model) {
            download.state = state;
            download.received = received;
        }
    }

    fn run_download(&self, model: TranscriptionModelChoice, cancel: Arc<AtomicBool>) {
        let result = self.download_both(model, &cancel);
        {
            let mut inner = lock(&self.inner);
            inner.downloads.remove(&model);
            if let Err(FetchError::Failed(error)) = &result {
                inner.errors.insert(model, error.clone());
            }
        }
        self.ended.notify_all();
        let ready = matches!(result, Ok(())).then_some(model);
        self.notify(ready);
    }

    fn download_both(
        &self,
        model: TranscriptionModelChoice,
        cancel: &AtomicBool,
    ) -> Result<(), FetchError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| FetchError::Failed(FAILED_MESSAGE.into()))?;
        let client = crate::http_client::builder()
            .https_only(self.urls.https_only)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(policy::MODEL_DOWNLOAD_CONNECT_TIMEOUT)
            .read_timeout(policy::MODEL_DOWNLOAD_READ_TIMEOUT)
            .build()
            .map_err(|_| FetchError::Failed(FAILED_MESSAGE.into()))?;
        {
            let _vad = lock(&self.vad_lock);
            if !self.file_ready(&self.pins.vad) {
                runtime.block_on(self.fetch(&client, &self.pins.vad, cancel, &mut |_| {}))?;
                self.install(&self.pins.vad, cancel)?;
            }
        }
        let pin = self.pins.model(model);
        if !self.file_ready(pin) {
            let mut last = Instant::now();
            let mut progress = |received: u64| {
                self.set_download(model, ModelState::Downloading, received);
                if policy::progress_due(last.elapsed()) {
                    last = Instant::now();
                    self.notify(None);
                }
            };
            runtime.block_on(self.fetch(&client, pin, cancel, &mut progress))?;
            self.set_download(model, ModelState::Verifying, pin.bytes);
            self.notify(None);
            self.install(pin, cancel)?;
        }
        Ok(())
    }

    /// Checks a complete `.part` file's SHA-256 and moves it into place.
    fn install(&self, pin: &Pin, cancel: &AtomicBool) -> Result<(), FetchError> {
        let part = self.part_path(pin);
        if cancel.load(Ordering::SeqCst) {
            return Err(FetchError::Cancelled);
        }
        if Self::size_of(&part) != pin.bytes {
            return Err(FetchError::Failed(FAILED_MESSAGE.into()));
        }
        if sha256_file(&part).ok().as_deref() != Some(pin.sha256.as_str()) {
            let _ = std::fs::remove_file(&part);
            return Err(FetchError::Failed(MISMATCH_MESSAGE.into()));
        }
        std::fs::rename(&part, self.path(pin))
            .map_err(|_| FetchError::Failed(FAILED_MESSAGE.into()))
    }

    /// Downloads `pin` into its `.part` file, resuming what is there.
    async fn fetch(
        &self,
        client: &reqwest::Client,
        pin: &Pin,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64),
    ) -> Result<(), FetchError> {
        let failed = || FetchError::Failed(FAILED_MESSAGE.into());
        let part = self.part_path(pin);
        let mut have = Self::size_of(&part);
        if have > pin.bytes {
            let _ = std::fs::remove_file(&part);
            have = 0;
        }
        if have == pin.bytes {
            return Ok(());
        }
        let mut url = reqwest::Url::parse(&pin.url).map_err(|_| failed())?;
        let mut redirects = 0;
        let mut response = loop {
            if !(self.urls.allowed)(&url) {
                return Err(FetchError::Failed(HOST_MESSAGE.into()));
            }
            let mut request = client.get(url.clone());
            if have > 0 {
                request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
            }
            let response = tokio::select! {
                response = request.send() => response.map_err(|_| failed())?,
                () = cancelled(cancel) => return Err(FetchError::Cancelled),
            };
            if response.status().is_redirection() {
                redirects += 1;
                if !policy::redirect_allowed(redirects) {
                    return Err(FetchError::Failed(HOST_MESSAGE.into()));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(failed)?;
                url = url.join(location).map_err(|_| failed())?;
                continue;
            }
            break response;
        };
        let status = response.status();
        let append = if status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0 {
            let start = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(content_range_start);
            if start != Some(have) {
                return Err(failed());
            }
            true
        } else if status == reqwest::StatusCode::OK {
            false
        } else {
            if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                // The partial file is no use; the next try starts over.
                let _ = std::fs::remove_file(&part);
            }
            return Err(failed());
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&part)
            .map_err(|_| failed())?;
        let mut received = if append { have } else { 0 };
        progress(received);
        loop {
            let chunk = tokio::select! {
                chunk = response.chunk() => chunk.map_err(|_| failed())?,
                () = cancelled(cancel) => return Err(FetchError::Cancelled),
            };
            let Some(chunk) = chunk else {
                break;
            };
            received += chunk.len() as u64;
            if received > pin.bytes {
                drop(file);
                let _ = std::fs::remove_file(&part);
                return Err(FetchError::Failed(TOO_LARGE_MESSAGE.into()));
            }
            file.write_all(&chunk).map_err(|_| failed())?;
            progress(received);
        }
        file.sync_all().map_err(|_| failed())?;
        if received != pin.bytes {
            // Ended early; the partial file stays for a resume.
            return Err(failed());
        }
        Ok(())
    }
}

/// Resolves once `cancel` is set.
async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The first byte of a `Content-Range: bytes a-b/n` header.
fn content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes ")?;
    range.split('-').next()?.trim().parse().ok()
}

/// Lowercase hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
pub(crate) mod tests;
