//! A scriptable capture backend for the state machine's tests. Clone it
//! before handing it to the service: both clones share one script.

use super::{
    CaptureBackend, CaptureEvent, CaptureSession, EventSink, SystemAudioSupport, Track, TrackTarget,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct TrackScript {
    /// Error every start of this track returns while set.
    fail_all: Option<String>,
    /// Errors for the next starts, consumed one per start.
    fail_next: Vec<String>,
    sink: Option<EventSink>,
    /// Where the last start was told to write.
    target: Option<TrackTarget>,
    running: bool,
    paused: bool,
    starts: usize,
    stops: usize,
}

struct Script {
    system: SystemAudioSupport,
    host_time_ns: u64,
    tracks: HashMap<Track, TrackScript>,
}

#[derive(Clone)]
pub(crate) struct FakeBackend {
    script: Arc<Mutex<Script>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self {
            script: Arc::new(Mutex::new(Script {
                system: SystemAudioSupport::Available,
                host_time_ns: 0,
                tracks: HashMap::new(),
            })),
        }
    }

    fn with<R>(&self, track: Track, work: impl FnOnce(&mut TrackScript) -> R) -> R {
        let mut script = self.script.lock().unwrap();
        work(script.tracks.entry(track).or_default())
    }

    pub fn set_system_audio(&self, support: SystemAudioSupport) {
        self.script.lock().unwrap().system = support;
    }

    pub fn set_host_time_ns(&self, ns: u64) {
        self.script.lock().unwrap().host_time_ns = ns;
    }

    /// Every start of `track` fails until [`Self::allow_starts`].
    pub fn fail_starts(&self, track: Track, error: &str) {
        self.with(track, |t| t.fail_all = Some(error.to_string()));
    }

    pub fn allow_starts(&self, track: Track) {
        self.with(track, |t| t.fail_all = None);
    }

    /// The next `count` starts of `track` fail, then starts succeed.
    pub fn fail_next_starts(&self, track: Track, count: usize, error: &str) {
        self.with(track, |t| t.fail_next = vec![error.to_string(); count]);
    }

    fn emit(&self, track: Track, event: CaptureEvent) {
        let sink = self.with(track, |t| t.sink.clone());
        if let Some(sink) = sink {
            let _ = sink.send(event);
        }
    }

    pub fn emit_first_frame(&self, track: Track, host_time_ns: u64) {
        self.emit(
            track,
            CaptureEvent::FirstFrame {
                track,
                host_time_ns,
            },
        );
    }

    pub fn emit_frames(&self, track: Track, count: u64) {
        self.emit(track, CaptureEvent::Frames { track, count });
    }

    pub fn emit_failed(&self, track: Track, error: &str) {
        self.emit(
            track,
            CaptureEvent::Failed {
                track,
                error: error.to_string(),
            },
        );
    }

    pub fn emit_device_changed(&self, track: Track, device_name: &str) {
        self.emit(
            track,
            CaptureEvent::DeviceChanged {
                track,
                device_name: device_name.to_string(),
            },
        );
    }

    pub fn start_count(&self, track: Track) -> usize {
        self.with(track, |t| t.starts)
    }

    pub fn stop_count(&self, track: Track) -> usize {
        self.with(track, |t| t.stops)
    }

    pub fn is_running(&self, track: Track) -> bool {
        self.with(track, |t| t.running)
    }

    pub fn target(&self, track: Track) -> Option<TrackTarget> {
        self.with(track, |t| t.target.clone())
    }

    pub fn is_paused(&self, track: Track) -> bool {
        self.with(track, |t| t.paused)
    }
}

impl CaptureBackend for FakeBackend {
    fn system_audio(&self) -> SystemAudioSupport {
        self.script.lock().unwrap().system
    }

    fn host_time_ns(&self) -> u64 {
        self.script.lock().unwrap().host_time_ns
    }

    fn start(
        &self,
        track: Track,
        target: &TrackTarget,
        events: EventSink,
    ) -> Result<Box<dyn CaptureSession>, String> {
        self.with(track, |t| {
            t.target = Some(target.clone());
            if !t.fail_next.is_empty() {
                return Err(t.fail_next.remove(0));
            }
            if let Some(error) = &t.fail_all {
                return Err(error.clone());
            }
            t.starts += 1;
            t.running = true;
            t.paused = false;
            t.sink = Some(events);
            Ok(())
        })?;
        Ok(Box::new(FakeSession {
            backend: self.clone(),
            track,
        }))
    }
}

struct FakeSession {
    backend: FakeBackend,
    track: Track,
}

impl CaptureSession for FakeSession {
    fn pause(&mut self) {
        self.backend.with(self.track, |t| t.paused = true);
    }

    fn resume(&mut self) {
        self.backend.with(self.track, |t| t.paused = false);
    }

    fn stop(&mut self) {
        self.backend.with(self.track, |t| {
            if t.running {
                t.running = false;
                t.stops += 1;
            }
        });
    }
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.stop();
    }
}
