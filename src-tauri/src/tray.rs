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

use crate::recording::capture::{SystemAudioSupport, Track};
use crate::recording::runtime::RecordingNotifier;
use crate::recording::settings::RecordingSettings;
use crate::recording::transcribe::models::TranscriptionModelStatus;
use crate::recording::transcribe::scheduler::TrayTranscription;
use crate::recording::{RecordingPhase, RecordingStatus, TrackRunState};
use serde::Serialize;

// ---------------------------------------------------------------------------
// The menu model

/// What a tray menu item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Start,
    Pause,
    Resume,
    Stop,
    Open,
    Settings,
    Quit,
}

impl TrayCommand {
    const ALL: [Self; 7] = [
        Self::Start,
        Self::Pause,
        Self::Resume,
        Self::Stop,
        Self::Open,
        Self::Settings,
        Self::Quit,
    ];

    /// The menu item's ID. Namespaced, because tray menu events share one
    /// listener with the app menu.
    pub fn id(self) -> &'static str {
        match self {
            Self::Start => "recording-tray-start",
            Self::Pause => "recording-tray-pause",
            Self::Resume => "recording-tray-resume",
            Self::Stop => "recording-tray-stop",
            Self::Open => "recording-tray-open",
            Self::Settings => "recording-tray-settings",
            Self::Quit => "recording-tray-quit",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.id() == id)
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

fn short_title(title: &str) -> String {
    if title.chars().count() <= STATUS_TITLE_CHARS {
        return title.to_string();
    }
    let mut kept: String = title.chars().take(STATUS_TITLE_CHARS - 1).collect();
    // Cut at a word boundary when one is reasonably close.
    let next_is_space = title
        .chars()
        .nth(STATUS_TITLE_CHARS - 1)
        .is_some_and(char::is_whitespace);
    if !next_is_space {
        if let Some(space) = kept.rfind(char::is_whitespace) {
            if space >= STATUS_TITLE_CHARS / 2 {
                kept.truncate(space);
            }
        }
    }
    format!("{}…", kept.trim_end())
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

/// The tray menu for a recording status and the transcription running after
/// Stop, if any. `settings` will add the upcoming meeting ("Record “…”") in
/// a later stage.
pub fn tray_menu(
    status: &RecordingStatus,
    _settings: &RecordingSettings,
    transcribing: Option<&TrayTranscription>,
) -> Vec<MenuItemModel> {
    let mut items = Vec::new();
    match status.state {
        RecordingPhase::Idle => items.push(action(TrayCommand::Start, "Start Recording")),
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
            items.push(if paused {
                action(TrayCommand::Resume, "Resume Recording")
            } else {
                action(TrayCommand::Pause, "Pause Recording")
            });
            items.push(action(TrayCommand::Stop, "Stop Recording"));
        }
    }
    if let Some(job) = transcribing {
        items.push(MenuItemModel::Separator);
        items.push(MenuItemModel::Status(format!(
            "Transcribing “{}”… {}%",
            short_title(&job.title),
            job.percent
        )));
    }
    items.push(MenuItemModel::Separator);
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

/// The icon is shown while **Record Meetings** is on, and while a recording
/// or a transcription after Stop is in progress even if it was turned off
/// meanwhile. Platforms that can't record never show it.
pub fn tray_visible(
    settings: &RecordingSettings,
    status: &RecordingStatus,
    transcribing: bool,
    supported: bool,
) -> bool {
    supported && (settings.record_meetings || status.state != RecordingPhase::Idle || transcribing)
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
    Recording,
    Transcribing,
}

/// Recording wins over transcribing.
pub fn icon_kind(status: &RecordingStatus, transcribing: bool) -> TrayIconKind {
    match status.state {
        RecordingPhase::Recording | RecordingPhase::Paused => TrayIconKind::Recording,
        RecordingPhase::Idle if transcribing => TrayIconKind::Transcribing,
        RecordingPhase::Idle => TrayIconKind::Idle,
    }
}

/// Icon size in pixels: 18 points at 2×, which macOS scales for 1×.
pub const ICON_PIXELS: u32 = 36;

/// A template (monochrome) icon as RGBA: black, with the shape in alpha, so
/// macOS tints it for light and dark menu bars. Idle is a ring around a dot;
/// recording is a filled disc; transcribing is the ring around three bars,
/// like a sound wave.
pub fn icon_rgba(kind: TrayIconKind) -> Vec<u8> {
    const SUB: u32 = 4; // 4×4 samples per pixel for smooth edges
    let center = ICON_PIXELS as f64 / 2.0;
    let inside = |x: f64, y: f64| -> bool {
        let r = ((x - center).powi(2) + (y - center).powi(2)).sqrt();
        match kind {
            TrayIconKind::Idle => (10.0..=13.0).contains(&r) || r <= 5.0,
            TrayIconKind::Recording => r <= 13.0,
            TrayIconKind::Transcribing => {
                let bar = |offset: f64, half_height: f64| {
                    (x - center - offset).abs() <= 1.25 && (y - center).abs() <= half_height
                };
                (10.0..=13.0).contains(&r) || bar(-5.0, 3.5) || bar(0.0, 6.0) || bar(5.0, 3.5)
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
    OpenSettings,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct TrayCommandEvent {
    pub command: TrayRequest,
}

// ---------------------------------------------------------------------------
// Tauri glue

struct TrayUpdate {
    status: RecordingStatus,
    settings: RecordingSettings,
    transcribing: Option<TrayTranscription>,
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
        supported: bool,
    ) {
        let _ = self.tray.send(TrayUpdate {
            status: status.clone(),
            settings: settings.clone(),
            transcribing: transcribing.cloned(),
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
                    let entry =
                        MenuItem::with_id(&self.app, command.id(), label, true, None::<&str>)?;
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
            update.supported,
        ) {
            if let Some(icon) = &self.icon {
                if self.shown {
                    icon.set_visible(false)?;
                    self.shown = false;
                }
            }
            return Ok(());
        }
        let model = tray_menu(&update.status, &update.settings, transcribing);
        let kind = icon_kind(&update.status, transcribing.is_some());
        match self.icon.clone() {
            None => {
                let menu = self.build_menu(&model)?;
                let icon = tauri::tray::TrayIconBuilder::with_id("meeting-recording")
                    .icon(Self::image(kind))
                    .icon_as_template(true)
                    .tooltip("ThreeStrands")
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .on_menu_event(|app, event| on_menu_event(app, event.id().as_ref()))
                    .build(&self.app)?;
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
                if !self.shown {
                    icon.set_visible(true)?;
                    self.shown = true;
                }
            }
        }
        self.model = model;
        self.kind = Some(kind);
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
    let request = |request: TrayRequest| {
        let _ = app.emit(
            "recording-tray-command",
            TrayCommandEvent { command: request },
        );
    };
    let result = match command {
        TrayCommand::Start => {
            if start_shows_window(&runtime.settings()) {
                crate::focus_main_window(app);
            }
            request(TrayRequest::Start);
            Ok(())
        }
        TrayCommand::Settings => {
            crate::focus_main_window(app);
            request(TrayRequest::OpenSettings);
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

    #[test]
    fn idle_offers_start_and_the_app_items() {
        let menu = tray_menu(&idle(), &RecordingSettings::default(), None);
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
        let menu = tray_menu(&status, &RecordingSettings::default(), None);
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
        let menu = tray_menu(&status, &RecordingSettings::default(), None);
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
                status_line(&tray_menu(&status, &RecordingSettings::default(), None)),
                Some("Recording “Weekly Sync” · 12:34 · Microphone only"),
                "{system:?}"
            );
        }
        // Restarting is not (yet) microphone-only.
        let status = recording(RecordingPhase::Recording, TrackRunState::Restarting);
        assert_eq!(
            status_line(&tray_menu(&status, &RecordingSettings::default(), None)),
            Some("Recording “Weekly Sync” · 12:34")
        );
        let mut old_macos = recording(RecordingPhase::Recording, TrackRunState::Unavailable);
        old_macos.system_audio = SystemAudioSupport::NeedsMacos {
            major: 14,
            minor: 2,
        };
        old_macos.tracks.retain(|t| t.track == Track::Mic);
        assert!(
            status_line(&tray_menu(&old_macos, &RecordingSettings::default(), None))
                .unwrap()
                .ends_with(" · Microphone only")
        );
    }

    #[test]
    fn long_titles_are_shortened_in_the_status_line() {
        let mut status = recording(RecordingPhase::Recording, TrackRunState::Active);
        status.title = Some("Quarterly planning review with the whole extended team".into());
        assert_eq!(
            status_line(&tray_menu(&status, &RecordingSettings::default(), None)),
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
        let settings = RecordingSettings::default();
        let mut status = recording(RecordingPhase::Recording, TrackRunState::Active);
        let before = tray_menu(&status, &settings, None);
        status.elapsed_ms += 1000;
        let after = tray_menu(&status, &settings, None);
        assert!(same_shape(&before, &after));
        assert_ne!(status_line(&before), status_line(&after));
        status.state = RecordingPhase::Paused;
        assert!(!same_shape(&before, &tray_menu(&status, &settings, None)));
        assert!(!same_shape(&before, &tray_menu(&idle(), &settings, None)));
    }

    #[test]
    fn menu_ids_round_trip_and_ignore_other_menus() {
        for command in TrayCommand::ALL {
            assert_eq!(TrayCommand::from_id(command.id()), Some(command));
        }
        assert_eq!(TrayCommand::from_id("quit"), None);
        assert_eq!(TrayCommand::from_id("recording-tray-status"), None);
    }

    #[test]
    fn the_icon_shows_while_record_meetings_is_on_or_a_recording_runs() {
        let on = RecordingSettings {
            record_meetings: true,
            ..Default::default()
        };
        let off = RecordingSettings::default();
        let live = recording(RecordingPhase::Recording, TrackRunState::Active);
        let paused = recording(RecordingPhase::Paused, TrackRunState::Active);
        assert!(tray_visible(&on, &idle(), false, true));
        assert!(!tray_visible(&off, &idle(), false, true));
        assert!(
            tray_visible(&off, &live, false, true),
            "turned off mid-recording"
        );
        assert!(tray_visible(&off, &paused, false, true));
        assert!(
            !tray_visible(&on, &live, false, false),
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
        let live_icon = icon_rgba(TrayIconKind::Recording);
        for icon in [&idle_icon, &live_icon] {
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
        let alpha_at = |icon: &[u8], x: u32, y: u32| icon[((y * ICON_PIXELS + x) * 4 + 3) as usize];
        // Between the dot and the ring, idle is empty and recording filled.
        assert_eq!(alpha_at(&idle_icon, 18 + 7, 18), 0);
        assert_eq!(alpha_at(&live_icon, 18 + 7, 18), 255);
        assert_eq!(icon_kind(&idle(), false), TrayIconKind::Idle);
        assert_eq!(
            icon_kind(
                &recording(RecordingPhase::Paused, TrackRunState::Active),
                false
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
        let settings = RecordingSettings::default();
        let menu = tray_menu(&idle(), &settings, Some(&job(40)));
        let mut expected = vec![
            "Start Recording",
            "---",
            "[Transcribing “Design Review”… 40%]",
        ];
        expected.extend(FOOTER);
        assert_eq!(labels(&menu), expected);
        // While another recording runs, both lines show.
        let live = recording(RecordingPhase::Recording, TrackRunState::Active);
        let both = tray_menu(&live, &settings, Some(&job(40)));
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
            &tray_menu(&idle(), &settings, Some(&job(41)))
        ));
        assert!(!same_shape(&menu, &tray_menu(&idle(), &settings, None)));
        let mut long = job(5);
        long.title = "Quarterly planning review with the whole extended team".into();
        assert_eq!(
            status_lines(&tray_menu(&idle(), &settings, Some(&long))),
            ["Transcribing “Quarterly planning review with the…”… 5%"]
        );
    }

    #[test]
    fn a_transcription_keeps_the_icon_shown_and_has_its_own_icon() {
        let off = RecordingSettings::default();
        assert!(
            tray_visible(&off, &idle(), true, true),
            "turned off mid-transcription"
        );
        assert!(!tray_visible(&off, &idle(), true, false));
        assert_eq!(icon_kind(&idle(), true), TrayIconKind::Transcribing);
        let live = recording(RecordingPhase::Recording, TrackRunState::Active);
        assert_eq!(
            icon_kind(&live, true),
            TrayIconKind::Recording,
            "recording wins"
        );
        let icon = icon_rgba(TrayIconKind::Transcribing);
        assert_eq!(icon.len(), (ICON_PIXELS * ICON_PIXELS * 4) as usize);
        assert!(icon
            .chunks_exact(4)
            .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
        assert_ne!(icon, icon_rgba(TrayIconKind::Idle));
        assert_ne!(icon, icon_rgba(TrayIconKind::Recording));
        let alpha_at = |icon: &[u8], x: u32, y: u32| icon[((y * ICON_PIXELS + x) * 4 + 3) as usize];
        // Between the middle bar and the next, idle has its dot.
        assert_eq!(alpha_at(&icon, 18 + 2, 18), 0);
        assert_eq!(alpha_at(&icon_rgba(TrayIconKind::Idle), 18 + 2, 18), 255);
        assert_eq!(alpha_at(&icon, 18, 18 - 5), 255, "the tall middle bar");
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
    fn event_payloads_use_the_contract_shape() {
        assert_eq!(
            serde_json::to_value(TrayCommandEvent {
                command: TrayRequest::OpenSettings
            })
            .unwrap(),
            serde_json::json!({ "command": "openSettings" })
        );
        assert_eq!(
            serde_json::to_value(TrayCommandEvent {
                command: TrayRequest::Start
            })
            .unwrap(),
            serde_json::json!({ "command": "start" })
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
