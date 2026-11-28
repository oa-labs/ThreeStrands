//! Recording offers and post-call "Still recording?" nudges.
//!
//! Pure decisions live in [`OfferEngine`], which watches:
//! - upcoming or active calendar meetings (`OFFER_WINDOW`);
//! - active microphone processes from Core Audio (or a test probe), filtered
//!   through [`policy::classify_bundle_id`] so conferencing apps (always) and
//!   browsers (only during scheduled meetings) raise a delayed tray offer & badge
//!   while dictation tools and ThreeStrands' own PID are ignored;
//! - silence after a call app releases the microphone or past the event's end
//!   time, raising a tray / indicator nudge without ever stopping automatically.

use super::policy::{self, MicProcessKind};
use super::settings::RecordingSettings;
use super::RecordingPhase;
use crate::db::note_entities::OfferedMeeting;
use crate::note_entities::NoteReference;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveMicProcess {
    pub pid: i32,
    pub bundle_id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MicActivitySnapshot {
    pub mic_running: bool,
    pub processes: Vec<ActiveMicProcess>,
}

/// An offer shown in the tray menu (and optionally as a tray icon badge).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayOffer {
    pub event_ref: Option<NoteReference>,
    pub title: String,
    pub attendees: Vec<String>,
    /// True when a conferencing app or in-meeting browser held the mic past `OFFER_MIC_DELAY`.
    pub badged: bool,
}

pub trait MicActivityProbe: Send + Sync {
    fn snapshot(&self) -> MicActivitySnapshot;
}

#[cfg(any(not(target_os = "macos"), test))]
pub struct NullMicProbe;

#[cfg(any(not(target_os = "macos"), test))]
impl MicActivityProbe for NullMicProbe {
    fn snapshot(&self) -> MicActivitySnapshot {
        MicActivitySnapshot::default()
    }
}

pub fn platform_mic_probe() -> Arc<dyn MicActivityProbe> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(mac::MacMicProbe::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(NullMicProbe)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{ActiveMicProcess, MicActivityProbe, MicActivitySnapshot};
    use objc2::{msg_send, rc::Retained, ClassType};
    use objc2_app_kit::NSRunningApplication;
    use objc2_core_audio::{
        kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioHardwarePropertyDefaultInputDevice,
        kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyElementMain,
        kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioProcessPropertyBundleID,
        kAudioProcessPropertyIsRunningInput, kAudioProcessPropertyPID,
        AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
        AudioObjectID, AudioObjectPropertyAddress, AudioObjectRemovePropertyListener,
    };
    use objc2_core_foundation::{CFRetained, CFString};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::Once;

    const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;
    static MIC_ACTIVITY_EPOCH: AtomicU64 = AtomicU64::new(0);
    static WATCHED_INPUT_DEVICE: AtomicU32 = AtomicU32::new(0);

    fn address(selector: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        }
    }

    unsafe extern "C-unwind" fn on_mic_activity_change(
        _object: AudioObjectID,
        _count: u32,
        _addresses: NonNull<AudioObjectPropertyAddress>,
        _client: *mut c_void,
    ) -> i32 {
        MIC_ACTIVITY_EPOCH.fetch_add(1, Ordering::Relaxed);
        0
    }

    fn ensure_default_input_listener() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let mut addr = address(kAudioHardwarePropertyDefaultInputDevice);
            // SAFETY: callback touches only atomics and lives for the process lifetime.
            let _ = unsafe {
                AudioObjectAddPropertyListener(
                    SYSTEM,
                    NonNull::from(&mut addr),
                    Some(on_mic_activity_change),
                    std::ptr::null_mut(),
                )
            };
        });
    }

    fn sync_device_running_listener(device: AudioObjectID) {
        let prev = WATCHED_INPUT_DEVICE.swap(device, Ordering::AcqRel);
        if prev == device {
            return;
        }
        let mut addr = address(kAudioDevicePropertyDeviceIsRunningSomewhere);
        if prev != 0 {
            // SAFETY: symmetric removal for a previously added static callback.
            unsafe {
                AudioObjectRemovePropertyListener(
                    prev,
                    NonNull::from(&mut addr),
                    Some(on_mic_activity_change),
                    std::ptr::null_mut(),
                );
            }
        }
        if device != 0 {
            // SAFETY: static callback touches only atomics.
            unsafe {
                AudioObjectAddPropertyListener(
                    device,
                    NonNull::from(&mut addr),
                    Some(on_mic_activity_change),
                    std::ptr::null_mut(),
                );
            }
        }
    }

    unsafe fn get_u32(object: AudioObjectID, selector: u32) -> Option<u32> {
        let mut value: u32 = 0;
        let mut size = size_of::<u32>() as u32;
        let mut addr = address(selector);
        let status = AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        );
        (status == 0).then_some(value)
    }

    unsafe fn get_i32(object: AudioObjectID, selector: u32) -> Option<i32> {
        let mut value: i32 = 0;
        let mut size = size_of::<i32>() as u32;
        let mut addr = address(selector);
        let status = AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        );
        (status == 0).then_some(value)
    }

    unsafe fn get_ids(object: AudioObjectID, selector: u32) -> Option<Vec<AudioObjectID>> {
        let mut addr = address(selector);
        let mut size = 0u32;
        if AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        ) != 0
        {
            return None;
        }
        let count = size as usize / size_of::<AudioObjectID>();
        if count == 0 {
            return Some(Vec::new());
        }
        let mut ids = vec![0u32; count];
        if AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(ids.as_mut_ptr()).cast(),
        ) != 0
        {
            return None;
        }
        ids.truncate(size as usize / size_of::<AudioObjectID>());
        Some(ids)
    }

    unsafe fn get_cfstring(object: AudioObjectID, selector: u32) -> Option<String> {
        let mut value: *const CFString = std::ptr::null();
        let mut size = size_of::<*const CFString>() as u32;
        let mut addr = address(selector);
        if AudioObjectGetPropertyData(
            object,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        ) != 0
        {
            return None;
        }
        let ptr = NonNull::new(value as *mut CFString)?;
        // Core Audio CFString properties follow the Create rule.
        let owned = CFRetained::from_raw(ptr);
        Some(owned.to_string())
    }

    pub struct MacMicProbe;

    impl MacMicProbe {
        pub fn new() -> Self {
            ensure_default_input_listener();
            Self
        }
    }

    impl MicActivityProbe for MacMicProbe {
        fn snapshot(&self) -> MicActivitySnapshot {
            ensure_default_input_listener();
            let device =
                unsafe { get_u32(SYSTEM, kAudioHardwarePropertyDefaultInputDevice) }.unwrap_or(0);
            sync_device_running_listener(device);
            if device == 0 {
                return MicActivitySnapshot::default();
            }
            let mic_running =
                unsafe { get_u32(device, kAudioDevicePropertyDeviceIsRunningSomewhere) }
                    .is_some_and(|v| v != 0);
            if !mic_running {
                return MicActivitySnapshot::default();
            }
            let own_pid = std::process::id() as i32;
            let process_objects =
                unsafe { get_ids(SYSTEM, kAudioHardwarePropertyProcessObjectList) }
                    .unwrap_or_default();
            let mut processes = Vec::new();
            for obj in process_objects {
                let running_input = unsafe { get_u32(obj, kAudioProcessPropertyIsRunningInput) }
                    .is_some_and(|v| v != 0);
                if !running_input {
                    continue;
                }
                let Some(pid) = (unsafe { get_i32(obj, kAudioProcessPropertyPID) }) else {
                    continue;
                };
                if pid <= 0 || pid == own_pid {
                    continue;
                }
                let mut bundle_id = unsafe { get_cfstring(obj, kAudioProcessPropertyBundleID) }
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let mut name = None;
                // SAFETY: `runningApplicationWithProcessIdentifier:` is standard AppKit Cocoa API
                // taking `pid_t` (`libc::pid_t`) and returning nullable `NSRunningApplication *`.
                let app: Option<Retained<NSRunningApplication>> = unsafe {
                    msg_send![
                        NSRunningApplication::class(),
                        runningApplicationWithProcessIdentifier: pid as libc::pid_t
                    ]
                };
                if let Some(app) = app {
                    if bundle_id.is_none() {
                        bundle_id = app
                            .bundleIdentifier()
                            .map(|id| id.to_string())
                            .filter(|s| !s.is_empty());
                    }
                    name = app.localizedName().map(|n| n.to_string());
                }
                processes.push(ActiveMicProcess {
                    pid,
                    bundle_id,
                    name,
                });
            }
            MicActivitySnapshot {
                mic_running: true,
                processes,
            }
        }
    }
}

pub struct OfferInput<'a> {
    pub now: SystemTime,
    pub settings: &'a RecordingSettings,
    pub phase: RecordingPhase,
    pub recording_id: Option<&'a str>,
    pub scheduled_meeting: Option<&'a OfferedMeeting>,
    pub recording_event_end: Option<SystemTime>,
    pub both_tracks_silent_for: Duration,
    pub mic: &'a MicActivitySnapshot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OfferOutcome {
    pub offer: Option<TrayOffer>,
    pub still_recording_nudge: bool,
}

#[derive(Debug)]
pub struct OfferEngine {
    own_pid: i32,
    mic_active_since: Option<SystemTime>,
    call_app_seen_during_recording: bool,
    call_app_released_at: Option<SystemTime>,
    last_recording_id: Option<String>,
}

impl Default for OfferEngine {
    fn default() -> Self {
        Self::new(std::process::id() as i32)
    }
}

impl OfferEngine {
    pub fn new(own_pid: i32) -> Self {
        Self {
            own_pid,
            mic_active_since: None,
            call_app_seen_during_recording: false,
            call_app_released_at: None,
            last_recording_id: None,
        }
    }

    fn reset(&mut self) {
        self.mic_active_since = None;
        self.call_app_seen_during_recording = false;
        self.call_app_released_at = None;
        self.last_recording_id = None;
    }

    fn classify_active_sources<'a>(
        &self,
        mic: &'a MicActivitySnapshot,
    ) -> (Option<&'static str>, bool) {
        if !mic.mic_running {
            return (None, false);
        }
        let mut conferencing_app: Option<&'static str> = None;
        let mut browser = false;
        for proc_info in &mic.processes {
            if proc_info.pid == self.own_pid {
                continue;
            }
            let Some(bundle_id) = proc_info.bundle_id.as_deref() else {
                continue;
            };
            match policy::classify_bundle_id(bundle_id) {
                MicProcessKind::Denied | MicProcessKind::Other => {}
                MicProcessKind::Conferencing { app_name } => {
                    conferencing_app.get_or_insert(app_name);
                }
                MicProcessKind::Browser => {
                    browser = true;
                }
            }
        }
        (conferencing_app, browser)
    }

    pub fn evaluate(&mut self, input: OfferInput<'_>) -> OfferOutcome {
        if !input.settings.record_meetings || !input.settings.offer_to_record {
            self.reset();
            return OfferOutcome::default();
        }
        let (conferencing_app, browser) = self.classify_active_sources(input.mic);

        if input.phase == RecordingPhase::Idle {
            self.call_app_seen_during_recording = false;
            self.call_app_released_at = None;
            self.last_recording_id = None;

            let qualifying_mic =
                conferencing_app.is_some() || (browser && input.scheduled_meeting.is_some());
            if qualifying_mic {
                self.mic_active_since.get_or_insert(input.now);
            } else {
                self.mic_active_since = None;
            }
            let badged = self.mic_active_since.is_some_and(|since| {
                policy::offer_delay_elapsed(input.now.duration_since(since).unwrap_or_default())
            });
            let offer = if let Some(meeting) = input.scheduled_meeting {
                Some(TrayOffer {
                    event_ref: Some(meeting.event_ref.clone()),
                    title: meeting.title.clone(),
                    attendees: meeting.attendees.clone(),
                    badged,
                })
            } else if badged {
                conferencing_app.map(|app_name| TrayOffer {
                    event_ref: None,
                    title: format!("{app_name} Call"),
                    attendees: Vec::new(),
                    badged: true,
                })
            } else {
                None
            };
            return OfferOutcome {
                offer,
                still_recording_nudge: false,
            };
        }

        // Active recording or paused.
        self.mic_active_since = None;
        let current_id = input.recording_id.map(str::to_string);
        if self.last_recording_id != current_id {
            self.last_recording_id = current_id;
            self.call_app_seen_during_recording = false;
            self.call_app_released_at = None;
        }
        let call_active =
            conferencing_app.is_some() || (browser && input.recording_event_end.is_some());
        if call_active {
            self.call_app_seen_during_recording = true;
            self.call_app_released_at = None;
        } else if self.call_app_seen_during_recording {
            self.call_app_released_at.get_or_insert(input.now);
        }

        if input.phase == RecordingPhase::Paused {
            return OfferOutcome {
                offer: None,
                still_recording_nudge: false,
            };
        }

        let past_end = input
            .recording_event_end
            .is_some_and(|end| input.now >= end);
        let nudge_after_call = self.call_app_released_at.is_some()
            && policy::still_recording_silence_reached(input.both_tracks_silent_for, past_end);
        let nudge_past_end = past_end
            && policy::still_recording_silence_reached(input.both_tracks_silent_for, false);

        OfferOutcome {
            offer: None,
            still_recording_nudge: nudge_after_call || nudge_past_end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(offer_to_record: bool) -> RecordingSettings {
        RecordingSettings {
            record_meetings: true,
            offer_to_record,
            ..Default::default()
        }
    }

    fn proc_entry(pid: i32, bundle: &str) -> ActiveMicProcess {
        ActiveMicProcess {
            pid,
            bundle_id: Some(bundle.into()),
            name: None,
        }
    }

    fn scheduled(t: SystemTime) -> OfferedMeeting {
        OfferedMeeting {
            event_ref: NoteReference {
                version: 1,
                kind: "event".into(),
                id: "evt-1".into(),
                account: Some("me@example.com".into()),
                calendar: Some("work".into()),
                note: None,
                contact: None,
                series: None,
            },
            title: "Weekly Sync".into(),
            attendees: vec!["ann@example.org".into()],
            start: t,
            end: t + Duration::from_secs(1800),
        }
    }

    #[test]
    fn conferencing_app_offers_after_delay_and_ignores_brief_mic_checks() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let mut engine = OfferEngine::new(999);
        let zoom = MicActivitySnapshot {
            mic_running: true,
            processes: vec![proc_entry(101, "us.zoom.xos")],
        };

        // At 0s and 2s: before OFFER_MIC_DELAY, no unscheduled offer or badge.
        assert_eq!(
            engine.evaluate(OfferInput {
                now: t0,
                settings: &settings(true),
                phase: RecordingPhase::Idle,
                recording_id: None,
                scheduled_meeting: None,
                recording_event_end: None,
                both_tracks_silent_for: Duration::ZERO,
                mic: &zoom,
            }),
            OfferOutcome::default()
        );
        assert_eq!(
            engine
                .evaluate(OfferInput {
                    now: t0 + Duration::from_secs(2),
                    settings: &settings(true),
                    phase: RecordingPhase::Idle,
                    recording_id: None,
                    scheduled_meeting: None,
                    recording_event_end: None,
                    both_tracks_silent_for: Duration::ZERO,
                    mic: &zoom,
                })
                .offer,
            None
        );

        // At 3s (OFFER_MIC_DELAY): unscheduled call offer + badge appears.
        let at_delay = engine.evaluate(OfferInput {
            now: t0 + policy::OFFER_MIC_DELAY,
            settings: &settings(true),
            phase: RecordingPhase::Idle,
            recording_id: None,
            scheduled_meeting: None,
            recording_event_end: None,
            both_tracks_silent_for: Duration::ZERO,
            mic: &zoom,
        });
        assert_eq!(
            at_delay.offer,
            Some(TrayOffer {
                event_ref: None,
                title: "Zoom Call".into(),
                attendees: Vec::new(),
                badged: true,
            })
        );
    }

    #[test]
    fn browsers_badge_only_during_scheduled_meetings_and_denylist_or_own_pid_are_ignored() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let mut engine = OfferEngine::new(999);
        let chrome = MicActivitySnapshot {
            mic_running: true,
            processes: vec![proc_entry(200, "com.google.Chrome.helper")],
        };
        let denied_and_own = MicActivitySnapshot {
            mic_running: true,
            processes: vec![
                proc_entry(999, "us.zoom.xos"),
                proc_entry(201, "com.apple.Siri"),
                proc_entry(202, "com.electron.wispr-flow"),
                proc_entry(203, "com.apple.VoiceMemos"),
            ],
        };

        // Chrome with NO scheduled meeting never triggers an offer even after delay.
        assert_eq!(
            engine
                .evaluate(OfferInput {
                    now: t0,
                    settings: &settings(true),
                    phase: RecordingPhase::Idle,
                    recording_id: None,
                    scheduled_meeting: None,
                    recording_event_end: None,
                    both_tracks_silent_for: Duration::ZERO,
                    mic: &chrome,
                })
                .offer,
            None
        );
        assert_eq!(
            engine
                .evaluate(OfferInput {
                    now: t0 + Duration::from_secs(10),
                    settings: &settings(true),
                    phase: RecordingPhase::Idle,
                    recording_id: None,
                    scheduled_meeting: None,
                    recording_event_end: None,
                    both_tracks_silent_for: Duration::ZERO,
                    mic: &chrome,
                })
                .offer,
            None
        );

        // Chrome WITH a scheduled meeting shows the meeting offer immediately and badges after delay.
        let meeting = scheduled(t0);
        let initial = engine.evaluate(OfferInput {
            now: t0,
            settings: &settings(true),
            phase: RecordingPhase::Idle,
            recording_id: None,
            scheduled_meeting: Some(&meeting),
            recording_event_end: None,
            both_tracks_silent_for: Duration::ZERO,
            mic: &chrome,
        });
        assert!(!initial.offer.as_ref().unwrap().badged);
        let badged = engine.evaluate(OfferInput {
            now: t0 + policy::OFFER_MIC_DELAY,
            settings: &settings(true),
            phase: RecordingPhase::Idle,
            recording_id: None,
            scheduled_meeting: Some(&meeting),
            recording_event_end: None,
            both_tracks_silent_for: Duration::ZERO,
            mic: &chrome,
        });
        assert_eq!(badged.offer.as_ref().unwrap().title, "Weekly Sync");
        assert!(badged.offer.as_ref().unwrap().badged);

        // Own PID + denylisted dictation tools do not badge a scheduled meeting or raise call offers.
        let ignored = engine.evaluate(OfferInput {
            now: t0 + Duration::from_secs(20),
            settings: &settings(true),
            phase: RecordingPhase::Idle,
            recording_id: None,
            scheduled_meeting: Some(&meeting),
            recording_event_end: None,
            both_tracks_silent_for: Duration::ZERO,
            mic: &denied_and_own,
        });
        assert!(!ignored.offer.unwrap().badged);
    }

    #[test]
    fn post_call_silence_nudge_fires_only_when_idle_call_app_released_mic_and_clears_on_pause_or_speech(
    ) {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let mut engine = OfferEngine::new(999);
        let zoom = MicActivitySnapshot {
            mic_running: true,
            processes: vec![proc_entry(101, "us.zoom.xos")],
        };
        let silent_mic = MicActivitySnapshot::default();
        let event_end = Some(t0 + Duration::from_secs(1800));

        // Call active while recording -> no nudge.
        assert!(
            !engine
                .evaluate(OfferInput {
                    now: t0,
                    settings: &settings(true),
                    phase: RecordingPhase::Recording,
                    recording_id: Some("rec-1"),
                    scheduled_meeting: None,
                    recording_event_end: event_end,
                    both_tracks_silent_for: Duration::from_secs(600),
                    mic: &zoom,
                })
                .still_recording_nudge
        );

        // Call app releases mic: silent 179s -> no nudge; 180s -> nudge.
        assert!(
            !engine
                .evaluate(OfferInput {
                    now: t0 + Duration::from_secs(180),
                    settings: &settings(true),
                    phase: RecordingPhase::Recording,
                    recording_id: Some("rec-1"),
                    scheduled_meeting: None,
                    recording_event_end: event_end,
                    both_tracks_silent_for: policy::STILL_RECORDING_SILENCE
                        - Duration::from_secs(1),
                    mic: &silent_mic,
                })
                .still_recording_nudge
        );
        assert!(
            engine
                .evaluate(OfferInput {
                    now: t0 + Duration::from_secs(200),
                    settings: &settings(true),
                    phase: RecordingPhase::Recording,
                    recording_id: Some("rec-1"),
                    scheduled_meeting: None,
                    recording_event_end: event_end,
                    both_tracks_silent_for: policy::STILL_RECORDING_SILENCE,
                    mic: &silent_mic,
                })
                .still_recording_nudge
        );

        // Paused clears nudge; resumed speech clears nudge; past event end shortens threshold to 30s.
        assert!(
            !engine
                .evaluate(OfferInput {
                    now: t0 + Duration::from_secs(210),
                    settings: &settings(true),
                    phase: RecordingPhase::Paused,
                    recording_id: Some("rec-1"),
                    scheduled_meeting: None,
                    recording_event_end: event_end,
                    both_tracks_silent_for: policy::STILL_RECORDING_SILENCE,
                    mic: &silent_mic,
                })
                .still_recording_nudge
        );
        assert!(
            !engine
                .evaluate(OfferInput {
                    now: t0 + Duration::from_secs(220),
                    settings: &settings(true),
                    phase: RecordingPhase::Recording,
                    recording_id: Some("rec-1"),
                    scheduled_meeting: None,
                    recording_event_end: event_end,
                    both_tracks_silent_for: Duration::ZERO,
                    mic: &silent_mic,
                })
                .still_recording_nudge
        );
        assert!(
            engine
                .evaluate(OfferInput {
                    now: t0 + Duration::from_secs(1800),
                    settings: &settings(true),
                    phase: RecordingPhase::Recording,
                    recording_id: Some("rec-1"),
                    scheduled_meeting: None,
                    recording_event_end: event_end,
                    both_tracks_silent_for: policy::STILL_RECORDING_PAST_END_SILENCE,
                    mic: &silent_mic,
                })
                .still_recording_nudge
        );

        // offer_to_record = false disables both offers and nudges.
        assert_eq!(
            engine.evaluate(OfferInput {
                now: t0 + Duration::from_secs(1900),
                settings: &settings(false),
                phase: RecordingPhase::Recording,
                recording_id: Some("rec-1"),
                scheduled_meeting: None,
                recording_event_end: event_end,
                both_tracks_silent_for: policy::STILL_RECORDING_SILENCE,
                mic: &silent_mic,
            }),
            OfferOutcome::default()
        );
    }
}
