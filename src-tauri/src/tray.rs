//! The menu bar (tray) icon for meeting recording, and what closing the
//! window or quitting does while recording is on.
//!
//! Decisions are pure functions so they are tested without a window:
//! [`tray_menu`] (the menu for a status and a transcription after Stop),
//! [`tray_visible`], [`icon_kind`], [`close_action`],
//! and [`exit_action`]. The Tauri side ([`TrayWorker`]) only applies them.
//!
//! **Threading.** Tauri runs tray and menu calls on the main thread and
//! blocks the caller until they finish, so every tray update goes through
//! one worker thread that owns the icon, and menu clicks hand their work to
//! a new thread. Nothing on the main thread waits for the tray or for the
//! recording service.

use crate::db::note_entities::TrayMeeting;
use crate::note_entities::NoteReference;
use crate::recording::capture::{SystemAudioSupport, Track};
use crate::recording::offers::TrayOffer;
use crate::recording::policy;
use crate::recording::runtime::{RecordingNotifier, TrayReview};
use crate::recording::settings::{RecordingSettings, TrayNextMeetingTitle};
use crate::recording::transcribe::models::TranscriptionModelStatus;
use crate::recording::transcribe::scheduler::TrayTranscription;
use crate::recording::{RecordingPhase, RecordingStatus, TrackRunState};
use serde::Serialize;
use std::borrow::Cow;
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// The menu model

/// What a tray menu item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    JoinMeeting(usize),
    OpenMeeting(usize),
    RecordOffer,
    Start,
    Pause,
    Resume,
    Stop,
    Review,
    Open,
    Settings,
    Quit,
}

impl TrayCommand {
    #[cfg(test)]
    const STATIC: [Self; 9] = [
        Self::RecordOffer,
        Self::Start,
        Self::Pause,
        Self::Resume,
        Self::Stop,
        Self::Review,
        Self::Open,
        Self::Settings,
        Self::Quit,
    ];

    /// The menu item's ID. Namespaced, because tray menu events share one
    /// listener with the app menu.
    pub fn id(self) -> Cow<'static, str> {
        match self {
            Self::JoinMeeting(index) => Cow::Owned(format!("recording-tray-join-{index}")),
            Self::OpenMeeting(index) => Cow::Owned(format!("recording-tray-open-meeting-{index}")),
            Self::RecordOffer => Cow::Borrowed("recording-tray-record-offer"),
            Self::Start => Cow::Borrowed("recording-tray-start"),
            Self::Pause => Cow::Borrowed("recording-tray-pause"),
            Self::Resume => Cow::Borrowed("recording-tray-resume"),
            Self::Stop => Cow::Borrowed("recording-tray-stop"),
            Self::Review => Cow::Borrowed("recording-tray-review"),
            Self::Open => Cow::Borrowed("recording-tray-open"),
            Self::Settings => Cow::Borrowed("recording-tray-settings"),
            Self::Quit => Cow::Borrowed("recording-tray-quit"),
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        if let Some(rest) = id.strip_prefix("recording-tray-join-") {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                let index: usize = rest.parse().ok()?;
                if index < policy::MAX_TRAY_MEETINGS {
                    return Some(Self::JoinMeeting(index));
                }
            }
            return None;
        }
        if let Some(rest) = id.strip_prefix("recording-tray-open-meeting-") {
            if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
                let index: usize = rest.parse().ok()?;
                if index < policy::MAX_TRAY_MEETINGS {
                    return Some(Self::OpenMeeting(index));
                }
            }
            return None;
        }
        match id {
            "recording-tray-record-offer" => Some(Self::RecordOffer),
            "recording-tray-start" => Some(Self::Start),
            "recording-tray-pause" => Some(Self::Pause),
            "recording-tray-resume" => Some(Self::Resume),
            "recording-tray-stop" => Some(Self::Stop),
            "recording-tray-review" => Some(Self::Review),
            "recording-tray-open" => Some(Self::Open),
            "recording-tray-settings" => Some(Self::Settings),
            "recording-tray-quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItemModel {
    Action {
        command: TrayCommand,
        label: String,
    },
    /// A disabled line that describes the state.
    Status(String),
    Separator,
}

fn action(command: TrayCommand, label: &str) -> MenuItemModel {
    MenuItemModel::Action {
        command,
        label: label.to_string(),
    }
}

/// Longest title shown in the status line, in characters.
const STATUS_TITLE_CHARS: usize = 40;

fn short_title_max(title: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if title.chars().count() <= max_chars {
        return title.to_string();
    }
    if max_chars == 1 {
        return "…".into();
    }
    let mut kept: String = title.chars().take(max_chars - 1).collect();
    // Cut at a word boundary when one is reasonably close.
    let next_is_space = title
        .chars()
        .nth(max_chars - 1)
        .is_some_and(char::is_whitespace);
    if !next_is_space {
        if let Some(space) = kept.rfind(char::is_whitespace) {
            if space >= max_chars / 2 {
                kept.truncate(space);
            }
        }
    }
    format!("{}…", kept.trim_end())
}

fn short_title(title: &str) -> String {
    short_title_max(title, STATUS_TITLE_CHARS)
}

fn format_meeting_time<Tz: chrono::TimeZone>(start: SystemTime, tz: &Tz) -> String {
    let dt = chrono::DateTime::<chrono::Utc>::from(start).with_timezone(tz);
    dt.time()
        .format("%I:%M %p")
        .to_string()
        .trim_start_matches('0')
        .to_string()
}

fn format_until(minutes: u64) -> String {
    if minutes == 0 {
        "now".into()
    } else if minutes < 60 {
        format!("in {minutes}m")
    } else {
        let (hours, mins) = (minutes / 60, minutes % 60);
        if mins == 0 {
            format!("in {hours}h")
        } else {
            format!("in {hours}h {mins}m")
        }
    }
}

fn format_remaining(minutes: u64) -> String {
    let mins = minutes.max(1);
    if mins < 60 {
        format!("{mins}m left")
    } else {
        let (hours, rem) = (mins / 60, mins % 60);
        if rem == 0 {
            format!("{hours}h left")
        } else {
            format!("{hours}h {rem}m left")
        }
    }
}

/// Optional status text shown next to the macOS menu bar icon for the next
/// active or imminent meeting.
pub fn tray_title<Tz: chrono::TimeZone>(
    status: &RecordingStatus,
    settings: &RecordingSettings,
    upcoming: &[TrayMeeting],
    now: chrono::DateTime<Tz>,
) -> Option<String> {
    if !settings.show_upcoming_meetings_in_tray
        || settings.tray_next_meeting_title == TrayNextMeetingTitle::Off
        || status.state != RecordingPhase::Idle
    {
        return None;
    }
    let next = upcoming.first()?;
    let now_sys = SystemTime::from(now.with_timezone(&chrono::Utc));
    let timing = if next.start <= now_sys && next.end > now_sys {
        let mins_left = next
            .end
            .duration_since(now_sys)
            .unwrap_or_default()
            .as_secs()
            .div_ceil(60);
        format_remaining(mins_left)
    } else if next.start > now_sys {
        let until = next.start.duration_since(now_sys).unwrap_or_default();
        if until > policy::TRAY_TITLE_WINDOW {
            return None;
        }
        format_until(until.as_secs().div_ceil(60))
    } else {
        return None;
    };
    match settings.tray_next_meeting_title {
        TrayNextMeetingTitle::Off => None,
        TrayNextMeetingTitle::TimeOnly => Some(timing),
        TrayNextMeetingTitle::TitleAndTime => Some(format!(
            "{} · {timing}",
            short_title_max(&next.title, policy::TRAY_TITLE_MAX_CHARS)
        )),
    }
}

/// Elapsed time as `m:ss`, or `h:mm:ss` from an hour.
pub fn format_elapsed(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Whether the recording has no system audio: this Mac can't capture it,
/// or its track failed.
fn microphone_only(status: &RecordingStatus) -> bool {
    status.system_audio != SystemAudioSupport::Available
        || status.tracks.iter().any(|track| {
            track.track == Track::System
                && matches!(
                    track.state,
                    TrackRunState::Unavailable | TrackRunState::Failed
                )
        })
}

/// The tray menu for upcoming meetings, recording state, and transcription.
pub fn tray_menu<Tz: chrono::TimeZone>(
    status: &RecordingStatus,
    settings: &RecordingSettings,
    transcribing: Option<&TrayTranscription>,
    review: Option<&TrayReview>,
    offer: Option<&TrayOffer>,
    upcoming: &[TrayMeeting],
    now: chrono::DateTime<Tz>,
) -> Vec<MenuItemModel> {
    let mut items = Vec::new();
    let now_sys = SystemTime::from(now.with_timezone(&chrono::Utc));
    if settings.show_upcoming_meetings_in_tray && !upcoming.is_empty() {
        let heading = if upcoming[0].is_tomorrow {
            "Tomorrow"
        } else {
            "Upcoming Today"
        };
        items.push(MenuItemModel::Status(heading.into()));
        for (index, meeting) in upcoming.iter().take(policy::MAX_TRAY_MEETINGS).enumerate() {
            let prefix = if meeting.start <= now_sys {
                "Now".to_string()
            } else {
                format_meeting_time(meeting.start, &now.timezone())
            };
            let suffix = if meeting.join_url.is_some() {
                match meeting.join_provider.as_deref() {
                    Some(provider) => format!(" · Join {provider}"),
                    None => " · Join".into(),
                }
            } else {
                String::new()
            };
            let label = format!("{prefix} · {}{suffix}", short_title(&meeting.title));
            let command = if meeting.join_url.is_some() {
                TrayCommand::JoinMeeting(index)
            } else {
                TrayCommand::OpenMeeting(index)
            };
            items.push(action(command, &label));
        }
    }
    if settings.record_meetings || status.state != RecordingPhase::Idle {
        if !items.is_empty() {
            items.push(MenuItemModel::Separator);
        }
        match status.state {
            RecordingPhase::Idle => {
                if settings.offer_to_record {
                    if let Some(target) = offer {
                        items.push(action(
                            TrayCommand::RecordOffer,
                            &format!("Record “{}”", short_title(&target.title)),
                        ));
                    }
                }
                items.push(action(TrayCommand::Start, "Start Recording"));
            }
            RecordingPhase::Recording | RecordingPhase::Paused => {
                let paused = status.state == RecordingPhase::Paused;
                let mut line = format!(
                    "{} “{}” · {}",
                    if paused { "Paused" } else { "Recording" },
                    short_title(status.title.as_deref().unwrap_or_default()),
                    format_elapsed(status.elapsed_ms)
                );
                if microphone_only(status) {
                    line.push_str(" · Microphone only");
                }
                items.push(MenuItemModel::Status(line));
                if status.still_recording_nudge && !paused {
                    items.push(MenuItemModel::Status("Still recording — Stop?".into()));
                }
                items.push(if paused {
                    action(TrayCommand::Resume, "Resume Recording")
                } else {
                    action(TrayCommand::Pause, "Pause Recording")
                });
                items.push(action(TrayCommand::Stop, "Stop Recording"));
            }
        }
    }
    if transcribing.is_some() || review.is_some() {
        if !items.is_empty() {
            items.push(MenuItemModel::Separator);
        }
    }
    if let Some(job) = transcribing {
        items.push(MenuItemModel::Status(format!(
            "Transcribing “{}”… {}%",
            short_title(&job.title),
            job.percent
        )));
    }
    if let Some(target) = review {
        items.push(action(
            TrayCommand::Review,
            &format!("Review “{}”", short_title(&target.title)),
        ));
    }
    if !items.is_empty() {
        items.push(MenuItemModel::Separator);
    }
    items.push(action(TrayCommand::Open, "Open ThreeStrands"));
    items.push(action(TrayCommand::Settings, "Settings…"));
    items.push(action(TrayCommand::Quit, "Quit ThreeStrands"));
    items
}

/// Whether two menus differ only in their status lines' text, so the menu
/// can be updated in place.
fn same_shape(a: &[MenuItemModel], b: &[MenuItemModel]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|pair| match pair {
            (MenuItemModel::Status(_), MenuItemModel::Status(_)) => true,
            (left, right) => left == right,
        })
}

fn status_lines(items: &[MenuItemModel]) -> Vec<&str> {
    items
        .iter()
        .filter_map(|item| match item {
            MenuItemModel::Status(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
fn status_line(items: &[MenuItemModel]) -> Option<&str> {
    status_lines(items).first().copied()
}

/// The icon is shown while **Record Meetings** is on, while upcoming meetings
/// are shown in the menu bar with at least one upcoming meeting, and while a
/// recording or a transcription after Stop is in progress. Platforms without
/// tray/recording support never show it.
pub fn tray_visible(
    settings: &RecordingSettings,
    status: &RecordingStatus,
    transcribing: bool,
    has_upcoming_meetings: bool,
    supported: bool,
) -> bool {
    supported
        && (settings.record_meetings
            || (settings.show_upcoming_meetings_in_tray && has_upcoming_meetings)
            || status.state != RecordingPhase::Idle
            || transcribing)
}

/// Whether **Start Recording** in the tray must bring the window forward:
/// the first start shows the consent dialog, and **Remind Me to Tell
/// Participants** shows its notice in the window. The webview can't show a
/// hidden window itself.
pub fn start_shows_window(settings: &RecordingSettings) -> bool {
    !settings.consent_acknowledged || settings.remind_to_tell
}

// ---------------------------------------------------------------------------
// Icons

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIconKind {
    Idle,
    IdleBadged,
    Recording,
    Transcribing,
}

/// Recording wins over transcribing, which wins over an offer badge.
pub fn icon_kind(status: &RecordingStatus, transcribing: bool, offer_badged: bool) -> TrayIconKind {
    match status.state {
        RecordingPhase::Recording | RecordingPhase::Paused => TrayIconKind::Recording,
        RecordingPhase::Idle if transcribing => TrayIconKind::Transcribing,
        RecordingPhase::Idle if offer_badged => TrayIconKind::IdleBadged,
        RecordingPhase::Idle => TrayIconKind::Idle,
    }
}

/// Icon size in pixels: 18 points at 2×, which macOS scales for 1×.
pub const ICON_PIXELS: u32 = 36;

/// A template (monochrome) icon as RGBA: black, with the shape in alpha, so
/// macOS tints it for light and dark menu bars. Every variant draws the three
/// wavy strands from the ThreeStrands mark: Idle shows the three strands;
/// IdleBadged adds a top-right offer dot with a clean cutout; Recording pairs
/// bolder strands with a bottom-right recording dot; Transcribing raises the
/// strands slightly above three progress dots.
pub fn icon_rgba(kind: TrayIconKind) -> Vec<u8> {
    const SUB: u32 = 4; // 4×4 samples per pixel for smooth edges
    const SEGMENTS: usize = 28;
    let (y0s, amp, half_w): ([f64; 3], f64, f64) = match kind {
        TrayIconKind::Idle | TrayIconKind::IdleBadged => ([9.5, 18.0, 26.5], 3.2, 1.65),
        TrayIconKind::Recording => ([9.5, 18.0, 26.5], 3.2, 1.85),
        TrayIconKind::Transcribing => ([7.5, 15.0, 22.5], 2.6, 1.5),
    };
    let x_start = 4.5_f64;
    let x_end = 31.5_f64;
    let width = x_end - x_start;
    let strands: [Vec<(f64, f64)>; 3] = y0s.map(|y0| {
        (0..=SEGMENTS)
            .map(|i| {
                let t = i as f64 / SEGMENTS as f64;
                let x = x_start + t * width;
                let y = y0 - amp * (std::f64::consts::TAU * t).sin();
                (x, y)
            })
            .collect()
    });
    let dist_to_segment = |px: f64, py: f64, x0: f64, y0: f64, x1: f64, y1: f64| -> f64 {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len_sq = dx * dx + dy * dy;
        if len_sq == 0.0 {
            return ((px - x0).powi(2) + (py - y0).powi(2)).sqrt();
        }
        let t = (((px - x0) * dx + (py - y0) * dy) / len_sq).clamp(0.0, 1.0);
        let cx = x0 + t * dx;
        let cy = y0 + t * dy;
        ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
    };
    let on_strand = |x: f64, y: f64| -> bool {
        strands.iter().any(|pts| {
            pts.windows(2)
                .any(|w| dist_to_segment(x, y, w[0].0, w[0].1, w[1].0, w[1].1) <= half_w)
        })
    };
    let inside = |x: f64, y: f64| -> bool {
        match kind {
            TrayIconKind::Idle => on_strand(x, y),
            TrayIconKind::IdleBadged => {
                let d_badge = ((x - 29.5).powi(2) + (y - 7.0).powi(2)).sqrt();
                if d_badge <= 3.8 {
                    true
                } else if d_badge <= 6.0 {
                    false
                } else {
                    on_strand(x, y)
                }
            }
            TrayIconKind::Recording => {
                let d_rec = ((x - 28.5).powi(2) + (y - 28.5).powi(2)).sqrt();
                if d_rec <= 5.0 {
                    true
                } else if d_rec <= 7.2 {
                    false
                } else {
                    on_strand(x, y)
                }
            }
            TrayIconKind::Transcribing => {
                let on_dot = [11.0_f64, 18.0, 25.0]
                    .iter()
                    .any(|dx| ((x - dx).powi(2) + (y - 30.5).powi(2)).sqrt() <= 2.0);
                on_dot || on_strand(x, y)
            }
        }
    };
    let mut rgba = Vec::with_capacity((ICON_PIXELS * ICON_PIXELS * 4) as usize);
    for py in 0..ICON_PIXELS {
        for px in 0..ICON_PIXELS {
            let mut covered = 0;
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let x = px as f64 + (sx as f64 + 0.5) / SUB as f64;
                    let y = py as f64 + (sy as f64 + 0.5) / SUB as f64;
                    covered += inside(x, y) as u32;
                }
            }
            let alpha = (covered * 255 / (SUB * SUB)) as u8;
            rgba.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    rgba
}

// ---------------------------------------------------------------------------
// Closing the window and quitting

/// What closing the main window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// The app is already exiting: let the window close.
    Close,
    /// Hide the window; the tray icon brings it back.
    Hide,
    /// Today's behavior: hold the close and let the webview save drafts
    /// first (`compose-before-exit`), which then quits.
    ExitHandshake,
}

/// `hide_on_close` is **Record Meetings** on a platform with the tray. A
/// recording in progress also hides rather than quits, so a recording never
/// stops because a window closed.
pub fn close_action(exiting: bool, hide_on_close: bool, recording_active: bool) -> CloseAction {
    if exiting {
        CloseAction::Close
    } else if hide_on_close || recording_active {
        CloseAction::Hide
    } else {
        CloseAction::ExitHandshake
    }
}

/// What a request to quit (⌘Q, **Quit ThreeStrands**) does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitAction {
    /// The exit handshake already finished: quit.
    Exit,
    /// Show the window and ask the user to confirm stopping the recording
    /// (`recording-quit-requested`); `recording_confirm_quit` continues.
    ConfirmRecordingQuit,
    /// Today's behavior: `compose-before-exit`, then `finish_exit`.
    ExitHandshake,
}

pub fn exit_action(exiting: bool, recording_active: bool) -> ExitAction {
    if exiting {
        ExitAction::Exit
    } else if recording_active {
        ExitAction::ConfirmRecordingQuit
    } else {
        ExitAction::ExitHandshake
    }
}

// ---------------------------------------------------------------------------
// Events

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingsChanged {
    pub recording_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrayRequest {
    Start,
    StartOffer,
    OpenSettings,
    Review,
    OpenMeeting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayStartRequest {
    pub event_ref: Option<NoteReference>,
    pub title: String,
    pub attendees: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayCommandEvent {
    pub command: TrayRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_request: Option<TrayStartRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting_event: Option<crate::models::ScheduleEvent>,
}

// ---------------------------------------------------------------------------
// Tauri glue

struct TrayUpdate {
    status: RecordingStatus,
    settings: RecordingSettings,
    transcribing: Option<TrayTranscription>,
    review: Option<TrayReview>,
    offer: Option<TrayOffer>,
    upcoming: Vec<TrayMeeting>,
    now: SystemTime,
    supported: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionProgress {
    pub recording_id: String,
    pub percent: u8,
}

/// Sends recording changes to the webview and the tray.
pub struct AppNotifier {
    app: tauri::AppHandle,
    tray: std::sync::mpsc::Sender<TrayUpdate>,
}

impl AppNotifier {
    pub fn new(app: tauri::AppHandle) -> Self {
        let tray = TrayWorker::spawn(app.clone());
        Self { app, tray }
    }
}

impl RecordingNotifier for AppNotifier {
    fn state_changed(&self, status: &RecordingStatus) {
        use tauri::Emitter;
        let _ = self.app.emit("recording-state", status);
    }

    fn recordings_changed(&self, recording_id: &str) {
        use tauri::Emitter;
        let _ = self.app.emit(
            "recordings-changed",
            RecordingsChanged {
                recording_id: recording_id.to_string(),
            },
        );
    }

    fn refresh_tray(
        &self,
        status: &RecordingStatus,
        settings: &RecordingSettings,
        transcribing: Option<&TrayTranscription>,
        review: Option<&TrayReview>,
        offer: Option<&TrayOffer>,
        upcoming: &[TrayMeeting],
        now: SystemTime,
        supported: bool,
    ) {
        let _ = self.tray.send(TrayUpdate {
            status: status.clone(),
            settings: settings.clone(),
            transcribing: transcribing.cloned(),
            review: review.cloned(),
            offer: offer.cloned(),
            upcoming: upcoming.to_vec(),
            now,
            supported,
        });
    }

    fn transcription_progress(&self, recording_id: &str, percent: u8) {
        use tauri::Emitter;
        let _ = self.app.emit(
            "transcription-progress",
            TranscriptionProgress {
                recording_id: recording_id.to_string(),
                percent,
            },
        );
    }

    fn models_changed(&self, statuses: &[TranscriptionModelStatus]) {
        use tauri::Emitter;
        let _ = self.app.emit("transcription-models-changed", statuses);
    }
}

/// Owns the tray icon on its own thread and applies the latest update.
struct TrayWorker {
    app: tauri::AppHandle,
    icon: Option<tauri::tray::TrayIcon>,
    model: Vec<MenuItemModel>,
    status_items: Vec<tauri::menu::MenuItem<tauri::Wry>>,
    kind: Option<TrayIconKind>,
    title: Option<String>,
    shown: bool,
}

impl TrayWorker {
    fn spawn(app: tauri::AppHandle) -> std::sync::mpsc::Sender<TrayUpdate> {
        let (sender, receiver) = std::sync::mpsc::channel::<TrayUpdate>();
        let mut worker = TrayWorker {
            app,
            icon: None,
            model: Vec::new(),
            status_items: Vec::new(),
            kind: None,
            title: None,
            shown: false,
        };
        let spawned = std::thread::Builder::new()
            .name("recording-tray".into())
            .spawn(move || {
                while let Ok(mut update) = receiver.recv() {
                    // Only the newest state matters.
                    while let Ok(newer) = receiver.try_recv() {
                        update = newer;
                    }
                    if let Err(error) = worker.apply(&update) {
                        log::warn!(target: "recording", "updating the tray: {error}");
                    }
                }
            });
        if let Err(error) = spawned {
            log::warn!(target: "recording", "starting the tray thread: {error}");
        }
        sender
    }

    fn image(kind: TrayIconKind) -> tauri::image::Image<'static> {
        tauri::image::Image::new_owned(icon_rgba(kind), ICON_PIXELS, ICON_PIXELS)
    }

    fn build_menu(
        &mut self,
        model: &[MenuItemModel],
    ) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
        use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
        let menu = Menu::new(&self.app)?;
        self.status_items.clear();
        for item in model {
            match item {
                MenuItemModel::Action { command, label } => {
                    let entry = MenuItem::with_id(
                        &self.app,
                        command.id().as_ref(),
                        label,
                        true,
                        None::<&str>,
                    )?;
                    menu.append(&entry)?;
                }
                MenuItemModel::Status(text) => {
                    let entry = MenuItem::with_id(
                        &self.app,
                        format!("recording-tray-status-{}", self.status_items.len()),
                        text,
                        false,
                        None::<&str>,
                    )?;
                    menu.append(&entry)?;
                    self.status_items.push(entry);
                }
                MenuItemModel::Separator => {
                    menu.append(&PredefinedMenuItem::separator(&self.app)?)?;
                }
            }
        }
        Ok(menu)
    }

    fn apply(&mut self, update: &TrayUpdate) -> tauri::Result<()> {
        let transcribing = update.transcribing.as_ref();
        if !tray_visible(
            &update.settings,
            &update.status,
            transcribing.is_some(),
            !update.upcoming.is_empty(),
            update.supported,
        ) {
            if let Some(icon) = &self.icon {
                if self.title.is_some() {
                    let _ = icon.set_title(None::<&str>);
                    self.title = None;
                }
                if self.shown {
                    icon.set_visible(false)?;
                    self.shown = false;
                }
            }
            return Ok(());
        }
        let local_now = chrono::DateTime::<chrono::Local>::from(update.now);
        let model = tray_menu(
            &update.status,
            &update.settings,
            transcribing,
            update.review.as_ref(),
            update.offer.as_ref(),
            &update.upcoming,
            local_now,
        );
        let next_title = tray_title(
            &update.status,
            &update.settings,
            &update.upcoming,
            local_now,
        );
        let offer_badged =
            update.settings.offer_to_record && update.offer.as_ref().is_some_and(|o| o.badged);
        let kind = icon_kind(&update.status, transcribing.is_some(), offer_badged);
        match self.icon.clone() {
            None => {
                let menu = self.build_menu(&model)?;
                let mut builder = tauri::tray::TrayIconBuilder::with_id("meeting-recording")
                    .icon(Self::image(kind))
                    .icon_as_template(true)
                    .tooltip("ThreeStrands")
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .on_menu_event(|app, event| on_menu_event(app, event.id().as_ref()));
                if let Some(title) = next_title.as_deref() {
                    builder = builder.title(title);
                }
                let icon = builder.build(&self.app)?;
                self.icon = Some(icon);
                self.shown = true;
            }
            Some(icon) => {
                if same_shape(&model, &self.model) {
                    let before = status_lines(&self.model);
                    for (index, text) in status_lines(&model).into_iter().enumerate() {
                        if before.get(index) != Some(&text) {
                            if let Some(item) = self.status_items.get(index) {
                                item.set_text(text)?;
                            }
                        }
                    }
                } else {
                    let menu = self.build_menu(&model)?;
                    icon.set_menu(Some(menu))?;
                }
                if self.kind != Some(kind) {
                    icon.set_icon_with_as_template(Some(Self::image(kind)), true)?;
                }
                if self.title != next_title {
                    let _ = icon.set_title(next_title.as_deref());
                }
                if !self.shown {
                    icon.set_visible(true)?;
                    self.shown = true;
                }
            }
        }
        self.model = model;
        self.kind = Some(kind);
        self.title = next_title;
        Ok(())
    }
}

/// Runs on the main thread: hands the work to a new thread.
fn on_menu_event(app: &tauri::AppHandle, id: &str) {
    let Some(command) = TrayCommand::from_id(id) else {
        return;
    };
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("recording-tray-command".into())
        .spawn(move || run_command(&app, command));
}

fn run_command(app: &tauri::AppHandle, command: TrayCommand) {
    use tauri::{Emitter, Manager};
    let Some(state) = app.try_state::<crate::AppState>() else {
        return;
    };
    let runtime = &state.recording;
    let request = |request: TrayRequest,
                   recording_id: Option<String>,
                   start_request: Option<TrayStartRequest>,
                   meeting_event: Option<crate::models::ScheduleEvent>| {
        let _ = app.emit(
            "recording-tray-command",
            TrayCommandEvent {
                command: request,
                recording_id,
                start_request,
                meeting_event,
            },
        );
    };
    let result = match command {
        TrayCommand::JoinMeeting(index) => {
            if let Some(url) = runtime
                .upcoming_meeting(index)
                .and_then(|meeting| meeting.join_url)
                .and_then(|raw| crate::recording::meeting_links::validate_join_url(&raw))
            {
                open::that(&url).map_err(|error| error.to_string())
            } else {
                crate::focus_main_window(app);
                let meeting_event = runtime.upcoming_meeting(index).and_then(|meeting| {
                    state
                        .database
                        .resolve_note_event(&meeting.event_ref)
                        .ok()
                        .flatten()
                });
                request(TrayRequest::OpenMeeting, None, None, meeting_event);
                Ok(())
            }
        }
        TrayCommand::OpenMeeting(index) => {
            crate::focus_main_window(app);
            let meeting_event = runtime.upcoming_meeting(index).and_then(|meeting| {
                state
                    .database
                    .resolve_note_event(&meeting.event_ref)
                    .ok()
                    .flatten()
            });
            request(TrayRequest::OpenMeeting, None, None, meeting_event);
            Ok(())
        }
        TrayCommand::RecordOffer => {
            if start_shows_window(&runtime.settings()) {
                crate::focus_main_window(app);
            }
            if let Some(offer) = runtime.current_offer() {
                request(
                    TrayRequest::StartOffer,
                    None,
                    Some(TrayStartRequest {
                        event_ref: offer.event_ref,
                        title: offer.title,
                        attendees: offer.attendees,
                    }),
                    None,
                );
            } else {
                request(TrayRequest::Start, None, None, None);
            }
            Ok(())
        }
        TrayCommand::Start => {
            if start_shows_window(&runtime.settings()) {
                crate::focus_main_window(app);
            }
            request(TrayRequest::Start, None, None, None);
            Ok(())
        }
        TrayCommand::Review => {
            crate::focus_main_window(app);
            let recording_id = runtime
                .latest_unreviewed_review()
                .map(|review| review.recording_id);
            request(TrayRequest::Review, recording_id, None, None);
            Ok(())
        }
        TrayCommand::Settings => {
            crate::focus_main_window(app);
            request(TrayRequest::OpenSettings, None, None, None);
            Ok(())
        }
        TrayCommand::Open => {
            crate::focus_main_window(app);
            Ok(())
        }
        TrayCommand::Pause => runtime.pause().map(drop),
        TrayCommand::Resume => runtime.resume().map(drop),
        TrayCommand::Stop => runtime.stop().map(drop),
        TrayCommand::Quit => {
            // The quit rules in `exit_action` apply in the ExitRequested
            // handler.
            app.exit(0);
            Ok(())
        }
    };
    if let Err(error) = result {
        log::warn!(target: "recording", "tray {command:?}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::TrackStatus;

    fn idle() -> RecordingStatus {
        RecordingStatus {
            state: RecordingPhase::Idle,
            recording_id: None,
            title: None,
            event_ref: None,
            elapsed_ms: 0,
            tracks: Vec::new(),
            system_audio: SystemAudioSupport::Available,
            system_audio_silent_warning: false,
            last_stop_reason: None,
            still_recording_nudge: false,
        }
    }

    fn recording(state: RecordingPhase, system: TrackRunState) -> RecordingStatus {
        RecordingStatus {
            state,
            recording_id: Some("r1".into()),
            title: Some("Weekly Sync".into()),
            elapsed_ms: 754_000,
            tracks: vec![
                TrackStatus {
                    track: Track::Mic,
                    state: TrackRunState::Active,
                    signal_active: false,
                },
                TrackStatus {
                    track: Track::System,
                    state: system,
                    signal_active: false,
                },
            ],
            ..idle()
        }
    }

    fn labels(items: &[MenuItemModel]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                MenuItemModel::Action { label, .. } => label.clone(),
                MenuItemModel::Status(text) => format!("[{text}]"),
                MenuItemModel::Separator => "---".into(),
            })
            .collect()
    }

    const FOOTER: [&str; 4] = ["---", "Open ThreeStrands", "Settings…", "Quit ThreeStrands"];

    fn fixed_now() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339("2026-11-27T10:00:00-05:00").unwrap()
    }

    fn rec_on() -> RecordingSettings {
        RecordingSettings {
            record_meetings: true,
            ..Default::default()
        }
    }

    fn meeting_at(
        id: &str,
        title: &str,
        start: &str,
        end: &str,
        join_url: Option<&str>,
        join_provider: Option<&str>,
        is_tomorrow: bool,
    ) -> TrayMeeting {
        TrayMeeting {
            event_ref: NoteReference {
                version: 1,
                kind: "event".into(),
                id: id.into(),
                account: Some("you@example.com".into()),
                calendar: Some("work".into()),
                note: None,
                contact: None,
                series: None,
            },
            title: title.into(),
            start: SystemTime::from(chrono::DateTime::parse_from_rfc3339(start).unwrap()),
            end: SystemTime::from(chrono::DateTime::parse_from_rfc3339(end).unwrap()),
            attendees: vec!["ann@example.org".into()],
            join_url: join_url.map(str::to_string),
            join_provider: join_provider.map(str::to_string),
            is_tomorrow,
        }
    }

    #[test]
    fn idle_offers_start_and_the_app_items() {
        let menu = tray_menu(&idle(), &rec_on(), None, None, None, &[], fixed_now());
        let mut expected = vec!["Start Recording"];
        expected.extend(FOOTER);
        assert_eq!(labels(&menu), expected);
        assert_eq!(
            menu[0],
            MenuItemModel::Action {
                command: TrayCommand::Start,
                label: "Start Recording".into()
            }
        );
    }

    #[test]
    fn recording_shows_a_status_line_pause_and_stop() {
        let status = recording(RecordingPhase::Recording, TrackRunState::Active);
        let menu = tray_menu(&status, &rec_on(), None, None, None, &[], fixed_now());
        let mut expected = vec![
            "[Recording “Weekly Sync” · 12:34]",
            "Pause Recording",
            "Stop Recording",
        ];
        expected.extend(FOOTER);
        assert_eq!(labels(&menu), expected);
        assert!(menu.contains(&MenuItemModel::Action {
            command: TrayCommand::Pause,
            label: "Pause Recording".into()
        }));
    }

    #[test]
    fn paused_offers_resume() {
        let status = recording(RecordingPhase::Paused, TrackRunState::Active);
        let menu = tray_menu(&status, &rec_on(), None, None, None, &[], fixed_now());
        let mut expected = vec![
            "[Paused “Weekly Sync” · 12:34]",
            "Resume Recording",
            "Stop Recording",
        ];
        expected.extend(FOOTER);
        assert_eq!(labels(&menu), expected);
    }

    #[test]
    fn the_status_line_says_microphone_only_without_system_audio() {
        for system in [TrackRunState::Unavailable, TrackRunState::Failed] {
            let status = recording(RecordingPhase::Recording, system);
            assert_eq!(
                status_line(&tray_menu(
                    &status,
                    &rec_on(),
                    None,
                    None,
                    None,
                    &[],
                    fixed_now()
                )),
                Some("Recording “Weekly Sync” · 12:34 · Microphone only"),
                "{system:?}"
            );
        }
        // Restarting is not (yet) microphone-only.
        let status = recording(RecordingPhase::Recording, TrackRunState::Restarting);
        assert_eq!(
            status_line(&tray_menu(
                &status,
                &rec_on(),
                None,
                None,
                None,
                &[],
                fixed_now()
            )),
            Some("Recording “Weekly Sync” · 12:34")
        );
        let mut old_macos = recording(RecordingPhase::Recording, TrackRunState::Unavailable);
        old_macos.system_audio = SystemAudioSupport::NeedsMacos {
            major: 14,
            minor: 2,
        };
        old_macos.tracks.retain(|t| t.track == Track::Mic);
        assert!(status_line(&tray_menu(
            &old_macos,
            &rec_on(),
            None,
            None,
            None,
            &[],
            fixed_now()
        ))
        .unwrap()
        .ends_with(" · Microphone only"));
    }

    #[test]
    fn long_titles_are_shortened_in_the_status_line() {
        let mut status = recording(RecordingPhase::Recording, TrackRunState::Active);
        status.title = Some("Quarterly planning review with the whole extended team".into());
        assert_eq!(
            status_line(&tray_menu(
                &status,
                &rec_on(),
                None,
                None,
                None,
                &[],
                fixed_now()
            )),
            Some("Recording “Quarterly planning review with the…” · 12:34")
        );
    }

    #[test]
    fn elapsed_time_formats_minutes_then_hours() {
        assert_eq!(format_elapsed(0), "0:00");
        assert_eq!(format_elapsed(59_999), "0:59");
        assert_eq!(format_elapsed(754_000), "12:34");
        assert_eq!(format_elapsed(3_599_000), "59:59");
        assert_eq!(format_elapsed(3_600_000), "1:00:00");
        assert_eq!(format_elapsed(4 * 3_600_000 + 61_000), "4:01:01");
    }

    #[test]
    fn only_the_status_line_changing_keeps_the_menu_shape() {
        let settings = rec_on();
        let mut status = recording(RecordingPhase::Recording, TrackRunState::Active);
        let before = tray_menu(&status, &settings, None, None, None, &[], fixed_now());
        status.elapsed_ms += 1000;
        let after = tray_menu(&status, &settings, None, None, None, &[], fixed_now());
        assert!(same_shape(&before, &after));
        assert_ne!(status_line(&before), status_line(&after));
        status.state = RecordingPhase::Paused;
        assert!(!same_shape(
            &before,
            &tray_menu(&status, &settings, None, None, None, &[], fixed_now())
        ));
        assert!(!same_shape(
            &before,
            &tray_menu(&idle(), &settings, None, None, None, &[], fixed_now())
        ));
    }

    #[test]
    fn menu_ids_round_trip_and_ignore_other_menus() {
        for command in TrayCommand::STATIC {
            assert_eq!(TrayCommand::from_id(command.id().as_ref()), Some(command));
        }
        for i in 0..policy::MAX_TRAY_MEETINGS {
            assert_eq!(
                TrayCommand::from_id(TrayCommand::JoinMeeting(i).id().as_ref()),
                Some(TrayCommand::JoinMeeting(i))
            );
            assert_eq!(
                TrayCommand::from_id(TrayCommand::OpenMeeting(i).id().as_ref()),
                Some(TrayCommand::OpenMeeting(i))
            );
        }
        assert_eq!(
            TrayCommand::from_id(&format!(
                "recording-tray-join-{}",
                policy::MAX_TRAY_MEETINGS
            )),
            None
        );
        assert_eq!(TrayCommand::from_id("recording-tray-join-+0"), None);
        assert_eq!(TrayCommand::from_id("recording-tray-open-meeting-99"), None);
        assert_eq!(TrayCommand::from_id("quit"), None);
        assert_eq!(TrayCommand::from_id("recording-tray-status"), None);
    }

    #[test]
    fn the_icon_shows_while_record_meetings_is_on_upcoming_meetings_show_or_a_recording_runs() {
        let on = rec_on();
        let off = RecordingSettings::default();
        let meetings_hidden = RecordingSettings {
            show_upcoming_meetings_in_tray: false,
            ..Default::default()
        };
        let live = recording(RecordingPhase::Recording, TrackRunState::Active);
        let paused = recording(RecordingPhase::Paused, TrackRunState::Active);
        assert!(tray_visible(&on, &idle(), false, false, true));
        assert!(!tray_visible(&off, &idle(), false, false, true));
        assert!(tray_visible(&off, &idle(), false, true, true));
        assert!(!tray_visible(&meetings_hidden, &idle(), false, true, true));
        assert!(
            tray_visible(&off, &live, false, false, true),
            "turned off mid-recording"
        );
        assert!(tray_visible(&off, &paused, false, false, true));
        assert!(
            !tray_visible(&on, &live, false, true, false),
            "never where recording is unsupported"
        );
    }

    #[test]
    fn tray_start_brings_the_window_forward_when_the_webview_must_show_something() {
        let first = RecordingSettings {
            consent_acknowledged: false,
            remind_to_tell: false,
            ..Default::default()
        };
        assert!(start_shows_window(&first), "the consent dialog");
        let reminded = RecordingSettings {
            consent_acknowledged: true,
            remind_to_tell: true,
            ..Default::default()
        };
        assert!(start_shows_window(&reminded), "the reminder notice");
        let quiet = RecordingSettings {
            consent_acknowledged: true,
            remind_to_tell: false,
            ..Default::default()
        };
        assert!(!start_shows_window(&quiet));
    }

    #[test]
    fn icons_are_template_images_with_distinct_idle_and_recording_shapes() {
        let idle_icon = icon_rgba(TrayIconKind::Idle);
        let badged_icon = icon_rgba(TrayIconKind::IdleBadged);
        let live_icon = icon_rgba(TrayIconKind::Recording);
        for icon in [&idle_icon, &badged_icon, &live_icon] {
            assert_eq!(icon.len(), (ICON_PIXELS * ICON_PIXELS * 4) as usize);
            assert!(
                icon.chunks_exact(4)
                    .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0),
                "template images are black; macOS tints them"
            );
            assert!(icon.chunks_exact(4).any(|p| p[3] == 255));
            assert_eq!(icon[3], 0, "the corner is transparent");
        }
        assert_ne!(idle_icon, live_icon);
        assert_ne!(idle_icon, badged_icon);
        let alpha_at = |icon: &[u8], x: u32, y: u32| icon[((y * ICON_PIXELS + x) * 4 + 3) as usize];
        // Idle has the three strands without badges; Recording adds the bottom-right dot; IdleBadged adds the top-right badge.
        assert_eq!(alpha_at(&idle_icon, 18, 18), 255);
        assert_eq!(alpha_at(&idle_icon, 30, 32), 0);
        assert_eq!(alpha_at(&live_icon, 30, 32), 255);
        assert_eq!(alpha_at(&idle_icon, 29, 7), 0);
        assert_eq!(alpha_at(&badged_icon, 29, 7), 255, "top-right offer badge");
        assert_eq!(icon_kind(&idle(), false, false), TrayIconKind::Idle);
        assert_eq!(icon_kind(&idle(), false, true), TrayIconKind::IdleBadged);
        assert_eq!(
            icon_kind(
                &recording(RecordingPhase::Paused, TrackRunState::Active),
                false,
                true
            ),
            TrayIconKind::Recording
        );
    }

    fn job(percent: u8) -> TrayTranscription {
        TrayTranscription {
            title: "Design Review".into(),
            percent,
        }
    }

    #[test]
    fn a_transcription_after_stop_gets_its_own_status_line() {
        let settings = rec_on();
        let menu = tray_menu(
            &idle(),
            &settings,
            Some(&job(40)),
            None,
            None,
            &[],
            fixed_now(),
        );
        let mut expected = vec![
            "Start Recording",
            "---",
            "[Transcribing “Design Review”… 40%]",
        ];
        expected.extend(FOOTER);
        assert_eq!(labels(&menu), expected);
        // While another recording runs, both lines show.
        let live = recording(RecordingPhase::Recording, TrackRunState::Active);
        let both = tray_menu(
            &live,
            &settings,
            Some(&job(40)),
            None,
            None,
            &[],
            fixed_now(),
        );
        assert_eq!(
            status_lines(&both),
            [
                "Recording “Weekly Sync” · 12:34",
                "Transcribing “Design Review”… 40%"
            ]
        );
        // Progress only changes text, so the menu is updated in place.
        assert!(same_shape(
            &menu,
            &tray_menu(
                &idle(),
                &settings,
                Some(&job(41)),
                None,
                None,
                &[],
                fixed_now()
            )
        ));
        assert!(!same_shape(
            &menu,
            &tray_menu(&idle(), &settings, None, None, None, &[], fixed_now())
        ));
        let mut long = job(5);
        long.title = "Quarterly planning review with the whole extended team".into();
        assert_eq!(
            status_lines(&tray_menu(
                &idle(),
                &settings,
                Some(&long),
                None,
                None,
                &[],
                fixed_now()
            )),
            ["Transcribing “Quarterly planning review with the…”… 5%"]
        );
    }

    #[test]
    fn an_unreviewed_recording_offers_review_in_the_tray() {
        let settings = rec_on();
        let review = TrayReview {
            recording_id: "rec-9".into(),
            title: "Design Review".into(),
        };
        let menu = tray_menu(
            &idle(),
            &settings,
            None,
            Some(&review),
            None,
            &[],
            fixed_now(),
        );
        let mut expected = vec!["Start Recording", "---", "Review “Design Review”"];
        expected.extend(FOOTER);
        assert_eq!(labels(&menu), expected);
        assert!(menu.contains(&MenuItemModel::Action {
            command: TrayCommand::Review,
            label: "Review “Design Review”".into(),
        }));
    }

    #[test]
    fn an_offer_and_silence_nudge_render_when_enabled() {
        let settings = RecordingSettings {
            record_meetings: true,
            offer_to_record: true,
            ..Default::default()
        };
        let offer = TrayOffer {
            event_ref: None,
            title: "Zoom Call".into(),
            attendees: Vec::new(),
            badged: true,
        };
        let offered = tray_menu(
            &idle(),
            &settings,
            None,
            None,
            Some(&offer),
            &[],
            fixed_now(),
        );
        assert_eq!(
            offered[0],
            MenuItemModel::Action {
                command: TrayCommand::RecordOffer,
                label: "Record “Zoom Call”".into(),
            }
        );
        assert_eq!(
            offered[1],
            MenuItemModel::Action {
                command: TrayCommand::Start,
                label: "Start Recording".into(),
            }
        );

        let disabled = RecordingSettings {
            record_meetings: true,
            offer_to_record: false,
            ..Default::default()
        };
        let suppressed = tray_menu(
            &idle(),
            &disabled,
            None,
            None,
            Some(&offer),
            &[],
            fixed_now(),
        );
        assert_eq!(labels(&suppressed)[0], "Start Recording");

        let mut live = recording(RecordingPhase::Recording, TrackRunState::Active);
        live.still_recording_nudge = true;
        let nudged = tray_menu(&live, &settings, None, None, None, &[], fixed_now());
        assert_eq!(
            status_lines(&nudged),
            ["Recording “Weekly Sync” · 12:34", "Still recording — Stop?"]
        );
    }

    #[test]
    fn a_transcription_keeps_the_icon_shown_and_has_its_own_icon() {
        let off = RecordingSettings::default();
        assert!(
            tray_visible(&off, &idle(), true, false, true),
            "turned off mid-transcription"
        );
        assert!(!tray_visible(&off, &idle(), true, false, false));
        assert_eq!(icon_kind(&idle(), true, true), TrayIconKind::Transcribing);
        let live = recording(RecordingPhase::Recording, TrackRunState::Active);
        assert_eq!(
            icon_kind(&live, true, true),
            TrayIconKind::Recording,
            "recording wins"
        );
        let idle_icon = icon_rgba(TrayIconKind::Idle);
        let badged_icon = icon_rgba(TrayIconKind::IdleBadged);
        let rec_icon = icon_rgba(TrayIconKind::Recording);
        let transcribing_icon = icon_rgba(TrayIconKind::Transcribing);
        for variant in [&idle_icon, &badged_icon, &rec_icon, &transcribing_icon] {
            assert_eq!(variant.len(), (ICON_PIXELS * ICON_PIXELS * 4) as usize);
            assert!(variant
                .chunks_exact(4)
                .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
        }
        assert_ne!(idle_icon, badged_icon);
        assert_ne!(idle_icon, rec_icon);
        assert_ne!(idle_icon, transcribing_icon);
        assert_ne!(badged_icon, rec_icon);
        assert_ne!(badged_icon, transcribing_icon);
        assert_ne!(rec_icon, transcribing_icon);

        let alpha_at =
            |bytes: &[u8], x: u32, y: u32| bytes[((y * ICON_PIXELS + x) * 4 + 3) as usize];
        // Transparent corners and no outer circle ring at top center.
        assert_eq!(alpha_at(&idle_icon, 1, 1), 0);
        assert_eq!(alpha_at(&idle_icon, 18, 5), 0);
        // Three stacked wavy strands: top crest, middle crossing, bottom trough.
        assert_eq!(alpha_at(&idle_icon, 11, 6), 255);
        assert_eq!(alpha_at(&idle_icon, 18, 18), 255);
        assert_eq!(alpha_at(&idle_icon, 25, 29), 255);
        // Offer badge dot + cutout halo.
        assert_eq!(alpha_at(&badged_icon, 29, 7), 255);
        assert_eq!(alpha_at(&badged_icon, 24, 7), 0);
        // Recording dot + cutout halo.
        assert_eq!(alpha_at(&rec_icon, 28, 28), 255);
        assert_eq!(alpha_at(&rec_icon, 22, 28), 0);
        // Transcribing three bottom progress dots.
        assert_eq!(alpha_at(&transcribing_icon, 11, 30), 255);
        assert_eq!(alpha_at(&transcribing_icon, 18, 30), 255);
        assert_eq!(alpha_at(&transcribing_icon, 25, 30), 255);
    }

    #[test]
    fn closing_the_window_hides_it_while_recording_is_on_or_running() {
        assert_eq!(
            close_action(false, false, false),
            CloseAction::ExitHandshake
        );
        assert_eq!(close_action(false, true, false), CloseAction::Hide);
        assert_eq!(close_action(false, false, true), CloseAction::Hide);
        assert_eq!(close_action(false, true, true), CloseAction::Hide);
        for hide in [false, true] {
            for active in [false, true] {
                assert_eq!(close_action(true, hide, active), CloseAction::Close);
            }
        }
    }

    #[test]
    fn quitting_while_recording_asks_first() {
        assert_eq!(exit_action(false, false), ExitAction::ExitHandshake);
        assert_eq!(exit_action(false, true), ExitAction::ConfirmRecordingQuit);
        assert_eq!(exit_action(true, true), ExitAction::Exit);
        assert_eq!(exit_action(true, false), ExitAction::Exit);
    }

    #[test]
    fn upcoming_meetings_render_with_join_or_open_commands_and_tomorrow_heading() {
        let now = fixed_now();
        let meetings = vec![
            meeting_at(
                "m1",
                "Weekly Sync",
                "2026-11-27T09:45:00-05:00",
                "2026-11-27T10:15:00-05:00",
                Some("https://meet.google.com/aaa-bbbb-ccc"),
                Some("Google Meet"),
                false,
            ),
            meeting_at(
                "m2",
                "Design Review",
                "2026-11-27T14:00:00-05:00",
                "2026-11-27T14:30:00-05:00",
                Some("https://call.example.com/123"),
                None,
                false,
            ),
            meeting_at(
                "m3",
                "Coffee Chat",
                "2026-11-27T15:30:00-05:00",
                "2026-11-27T16:00:00-05:00",
                None,
                None,
                false,
            ),
        ];

        // Recording off: only upcoming meetings + footer appear.
        let rec_off = RecordingSettings::default();
        let menu_off = tray_menu(&idle(), &rec_off, None, None, None, &meetings, now);
        assert_eq!(
            labels(&menu_off),
            [
                "[Upcoming Today]",
                "Now · Weekly Sync · Join Google Meet",
                "2:00 PM · Design Review · Join",
                "3:30 PM · Coffee Chat",
                "---",
                "Open ThreeStrands",
                "Settings…",
                "Quit ThreeStrands",
            ]
        );
        assert_eq!(
            menu_off[1],
            MenuItemModel::Action {
                command: TrayCommand::JoinMeeting(0),
                label: "Now · Weekly Sync · Join Google Meet".into(),
            }
        );
        assert_eq!(
            menu_off[3],
            MenuItemModel::Action {
                command: TrayCommand::OpenMeeting(2),
                label: "3:30 PM · Coffee Chat".into(),
            }
        );

        // Recording on + offer: upcoming section followed by recording section.
        let offer = TrayOffer {
            event_ref: None,
            title: "Weekly Sync".into(),
            attendees: Vec::new(),
            badged: false,
        };
        let menu_on = tray_menu(&idle(), &rec_on(), None, None, Some(&offer), &meetings, now);
        assert_eq!(
            labels(&menu_on),
            [
                "[Upcoming Today]",
                "Now · Weekly Sync · Join Google Meet",
                "2:00 PM · Design Review · Join",
                "3:30 PM · Coffee Chat",
                "---",
                "Record “Weekly Sync”",
                "Start Recording",
                "---",
                "Open ThreeStrands",
                "Settings…",
                "Quit ThreeStrands",
            ]
        );

        // Tomorrow fallback uses Tomorrow heading.
        let tomorrow_only = vec![meeting_at(
            "t1",
            "Tomorrow Kickoff",
            "2026-11-28T09:00:00-05:00",
            "2026-11-28T09:30:00-05:00",
            Some("https://zoom.us/j/123"),
            Some("Zoom"),
            true,
        )];
        let menu_tmr = tray_menu(&idle(), &rec_off, None, None, None, &tomorrow_only, now);
        assert_eq!(
            labels(&menu_tmr),
            [
                "[Tomorrow]",
                "9:00 AM · Tomorrow Kickoff · Join Zoom",
                "---",
                "Open ThreeStrands",
                "Settings…",
                "Quit ThreeStrands",
            ]
        );
    }

    #[test]
    fn tray_title_formats_countdown_and_respects_window_and_mode() {
        let now = fixed_now();
        let active = vec![meeting_at(
            "m1",
            "Weekly Sync",
            "2026-11-27T09:42:00-05:00",
            "2026-11-27T10:18:00-05:00",
            None,
            None,
            false,
        )];
        let soon = vec![meeting_at(
            "m2",
            "Quarterly Planning Discussion With Team",
            "2026-11-27T10:12:00-05:00",
            "2026-11-27T11:00:00-05:00",
            None,
            None,
            false,
        )];
        let in_90 = vec![meeting_at(
            "m3",
            "Design Review",
            "2026-11-27T11:30:00-05:00",
            "2026-11-27T12:00:00-05:00",
            None,
            None,
            false,
        )];
        let far = vec![meeting_at(
            "m4",
            "Afternoon Sync",
            "2026-11-27T12:01:00-05:00",
            "2026-11-27T12:30:00-05:00",
            None,
            None,
            false,
        )];

        let defaults = RecordingSettings::default();
        assert_eq!(
            tray_title(&idle(), &defaults, &active, now).as_deref(),
            Some("Weekly Sync · 18m left")
        );
        assert_eq!(
            tray_title(&idle(), &defaults, &soon, now).as_deref(),
            Some("Quarterly Planning… · in 12m")
        );
        assert_eq!(
            tray_title(&idle(), &defaults, &in_90, now).as_deref(),
            Some("Design Review · in 1h 30m")
        );
        assert_eq!(tray_title(&idle(), &defaults, &far, now), None);

        let time_only = RecordingSettings {
            tray_next_meeting_title: TrayNextMeetingTitle::TimeOnly,
            ..Default::default()
        };
        assert_eq!(
            tray_title(&idle(), &time_only, &soon, now).as_deref(),
            Some("in 12m")
        );
        assert_eq!(
            tray_title(&idle(), &time_only, &active, now).as_deref(),
            Some("18m left")
        );

        let off = RecordingSettings {
            tray_next_meeting_title: TrayNextMeetingTitle::Off,
            ..Default::default()
        };
        assert_eq!(tray_title(&idle(), &off, &active, now), None);

        let live = recording(RecordingPhase::Recording, TrackRunState::Active);
        assert_eq!(tray_title(&live, &defaults, &active, now), None);
    }

    #[test]
    fn event_payloads_use_the_contract_shape() {
        assert_eq!(
            serde_json::to_value(TrayCommandEvent {
                command: TrayRequest::OpenSettings,
                recording_id: None,
                start_request: None,
                meeting_event: None,
            })
            .unwrap(),
            serde_json::json!({ "command": "openSettings" })
        );
        assert_eq!(
            serde_json::to_value(TrayCommandEvent {
                command: TrayRequest::Start,
                recording_id: None,
                start_request: None,
                meeting_event: None,
            })
            .unwrap(),
            serde_json::json!({ "command": "start" })
        );
        assert_eq!(
            serde_json::to_value(TrayCommandEvent {
                command: TrayRequest::StartOffer,
                recording_id: None,
                start_request: Some(TrayStartRequest {
                    event_ref: None,
                    title: "Zoom Call".into(),
                    attendees: Vec::new(),
                }),
                meeting_event: None,
            })
            .unwrap(),
            serde_json::json!({
                "command": "startOffer",
                "startRequest": { "eventRef": null, "title": "Zoom Call", "attendees": [] }
            })
        );
        assert_eq!(
            serde_json::to_value(TrayCommandEvent {
                command: TrayRequest::Review,
                recording_id: Some("r1".into()),
                start_request: None,
                meeting_event: None,
            })
            .unwrap(),
            serde_json::json!({ "command": "review", "recordingId": "r1" })
        );
        assert_eq!(
            serde_json::to_value(RecordingsChanged {
                recording_id: "r1".into()
            })
            .unwrap(),
            serde_json::json!({ "recordingId": "r1" })
        );
        assert_eq!(
            serde_json::to_value(TranscriptionProgress {
                recording_id: "r1".into(),
                percent: 40
            })
            .unwrap(),
            serde_json::json!({ "recordingId": "r1", "percent": 40 })
        );
    }
}
