//! Keep Audio: which recordings' audio files to delete, and deleting them.
//!
//! Transcripts and the recording rows stay until the user deletes them;
//! only audio expires. Deleting audio never touches notes saved from a
//! recording.

use super::policy;
use super::settings::KeepAudio;
use crate::db::recordings::{RecordingAudio, RecordingState};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Whether a recording's audio has outlived **Keep Audio**. Age counts from
/// when the recording ended (its start when it has no end), and audio at
/// exactly the limit is deleted. A recording still in progress never
/// expires.
pub fn audio_expired(keep: KeepAudio, audio: &RecordingAudio, now: SystemTime) -> bool {
    if audio.audio_deleted_at.is_some() || audio.state == RecordingState::Recording {
        return false;
    }
    let limit = match keep {
        KeepAudio::UntilDeleted => return false,
        KeepAudio::UntilTranscribed => return audio.state == RecordingState::Transcribed,
        KeepAudio::Days7 => policy::KEEP_AUDIO_SHORT,
        KeepAudio::Days30 => policy::KEEP_AUDIO_LONG,
    };
    let ended = audio.ended_at.as_deref().unwrap_or(&audio.started_at);
    let Ok(ended) = chrono::DateTime::parse_from_rfc3339(ended) else {
        return false;
    };
    let ended = SystemTime::from(ended);
    now.duration_since(ended).unwrap_or(Duration::ZERO) >= limit
}

/// The recording's audio directory, but only when it is exactly where the
/// app puts recordings (`<audio root>/<id>`). A stored path anywhere else is
/// never deleted.
pub fn owned_audio_dir(audio_root: &Path, audio: &RecordingAudio) -> Option<PathBuf> {
    let expected = audio_root.join(&audio.id);
    let valid_id = !audio.id.is_empty()
        && audio
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    (valid_id && Path::new(&audio.audio_dir) == expected).then_some(expected)
}

/// Deletes a recording's audio directory if it is one the app owns.
pub fn delete_audio_dir(audio_root: &Path, audio: &RecordingAudio) -> std::io::Result<()> {
    let Some(dir) = owned_audio_dir(audio_root, audio) else {
        log::warn!(target: "recording", "not deleting audio outside the recordings folder");
        return Ok(());
    };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> SystemTime {
        SystemTime::from(chrono::DateTime::parse_from_rfc3339("2026-12-31T12:00:00Z").unwrap())
    }

    fn ended(ago: Duration, state: RecordingState) -> RecordingAudio {
        let ended = chrono::DateTime::<chrono::Utc>::from(now() - ago).to_rfc3339();
        RecordingAudio {
            id: "r1".into(),
            state,
            audio_dir: "/root/r1".into(),
            started_at: "2020-01-01T00:00:00Z".into(),
            ended_at: Some(ended),
            audio_deleted_at: None,
        }
    }

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn seven_days_deletes_audio_at_and_past_seven_days() {
        let limit = policy::KEEP_AUDIO_SHORT;
        let keep = KeepAudio::Days7;
        let state = RecordingState::Recorded;
        assert!(!audio_expired(keep, &ended(limit - SECOND, state), now()));
        assert!(audio_expired(keep, &ended(limit, state), now()));
        assert!(audio_expired(keep, &ended(limit + SECOND, state), now()));
    }

    #[test]
    fn thirty_days_deletes_audio_at_and_past_thirty_days() {
        let limit = policy::KEEP_AUDIO_LONG;
        let keep = KeepAudio::Days30;
        let state = RecordingState::Interrupted;
        assert!(!audio_expired(
            keep,
            &ended(policy::KEEP_AUDIO_SHORT, state),
            now()
        ));
        assert!(!audio_expired(keep, &ended(limit - SECOND, state), now()));
        assert!(audio_expired(keep, &ended(limit, state), now()));
        assert!(audio_expired(keep, &ended(limit + SECOND, state), now()));
    }

    #[test]
    fn until_deleted_never_expires() {
        let ancient = Duration::from_secs(10 * 365 * 24 * 60 * 60);
        for state in [RecordingState::Recorded, RecordingState::Transcribed] {
            assert!(!audio_expired(
                KeepAudio::UntilDeleted,
                &ended(ancient, state),
                now()
            ));
        }
    }

    #[test]
    fn until_transcribed_deletes_only_transcribed_audio_at_any_age() {
        let keep = KeepAudio::UntilTranscribed;
        assert!(audio_expired(
            keep,
            &ended(Duration::ZERO, RecordingState::Transcribed),
            now()
        ));
        for state in [
            RecordingState::Recorded,
            RecordingState::Interrupted,
            RecordingState::Transcribing,
            RecordingState::Failed,
        ] {
            let old = ended(policy::KEEP_AUDIO_LONG * 2, state);
            assert!(!audio_expired(keep, &old, now()), "{state:?}");
        }
    }

    #[test]
    fn a_recording_in_progress_or_already_deleted_is_left_alone() {
        let old = policy::KEEP_AUDIO_LONG * 2;
        let mut live = ended(old, RecordingState::Recording);
        live.ended_at = None;
        assert!(!audio_expired(KeepAudio::Days7, &live, now()));
        let mut gone = ended(old, RecordingState::Recorded);
        gone.audio_deleted_at = Some("2026-01-01T00:00:00Z".into());
        assert!(!audio_expired(KeepAudio::Days7, &gone, now()));
    }

    #[test]
    fn without_an_end_the_start_counts() {
        let mut audio = ended(Duration::ZERO, RecordingState::Interrupted);
        audio.ended_at = None;
        assert!(audio_expired(KeepAudio::Days7, &audio, now()));
    }

    #[test]
    fn only_the_apps_own_audio_folder_is_ever_deleted() {
        let root = Path::new("/data/recordings");
        let mut audio = ended(Duration::ZERO, RecordingState::Recorded);
        audio.id = "0b6f-11".into();
        audio.audio_dir = "/data/recordings/0b6f-11".into();
        assert_eq!(
            owned_audio_dir(root, &audio),
            Some(PathBuf::from("/data/recordings/0b6f-11"))
        );
        for bad in ["/data", "/data/recordings", "/data/recordings/other", "/"] {
            audio.audio_dir = bad.into();
            assert_eq!(owned_audio_dir(root, &audio), None, "{bad}");
        }
        audio.id = "..".into();
        audio.audio_dir = "/data/recordings/..".into();
        assert_eq!(owned_audio_dir(root, &audio), None);
    }
}
