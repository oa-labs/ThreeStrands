//! Meeting Recording settings. Device-local by design: stored in
//! `meeting_recording_settings`, never replicated, and never part of the
//! encrypted settings transfer, because they decide what this computer
//! records of other people.
//!
//! Every field has a default and unknown fields are ignored, so JSON stored
//! by an older or newer build still loads.

use super::policy::TranscriptionModel;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptionModelChoice {
    #[default]
    Fast,
    Accurate,
}

impl TranscriptionModelChoice {
    #[allow(dead_code)] // The transcription stage reads it.
    pub fn model(self) -> TranscriptionModel {
        match self {
            Self::Fast => TranscriptionModel::Fast,
            Self::Accurate => TranscriptionModel::Accurate,
        }
    }
}

/// Which transcription engine the user chose in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptionEngineChoice {
    #[default]
    Auto,
    SpeechAnalyzer,
    Whisper,
}

/// The resolved engine a job actually runs on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectiveEngine {
    SpeechAnalyzer,
    Whisper,
}

/// Picks the engine to run for a user choice and platform capability.
pub fn effective_engine(
    choice: TranscriptionEngineChoice,
    speech_analyzer_available: bool,
) -> EffectiveEngine {
    match choice {
        TranscriptionEngineChoice::Auto | TranscriptionEngineChoice::SpeechAnalyzer
            if speech_analyzer_available =>
        {
            EffectiveEngine::SpeechAnalyzer
        }
        _ => EffectiveEngine::Whisper,
    }
}

/// How long a recording's audio files are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepAudio {
    #[default]
    Days7,
    Days30,
    UntilDeleted,
    UntilTranscribed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordingSettings {
    pub record_meetings: bool,
    /// Optional Core Audio / OS device UID override; `None` follows the system default.
    pub microphone_device_uid: Option<String>,
    pub transcription_engine: TranscriptionEngineChoice,
    pub transcription_model: TranscriptionModelChoice,
    /// Optional language code (e.g. `"en"`, `"es"`); `None` uses Auto detect-once.
    pub transcription_language: Option<String>,
    /// Transcribe finished minutes in batches while the meeting is still
    /// being recorded. Off, nothing is transcribed until Stop.
    pub transcribe_during_meetings: bool,
    pub keep_audio: KeepAudio,
    pub offer_to_record: bool,
    pub remind_to_tell: bool,
    pub summarize_with_ai: bool,
    pub include_my_notes: bool,
    pub attach_transcript: bool,
    pub consent_acknowledged: bool,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            record_meetings: false,
            microphone_device_uid: None,
            transcription_engine: TranscriptionEngineChoice::Auto,
            transcription_model: TranscriptionModelChoice::Fast,
            transcription_language: None,
            transcribe_during_meetings: true,
            keep_audio: KeepAudio::Days7,
            offer_to_record: true,
            remind_to_tell: true,
            summarize_with_ai: false,
            include_my_notes: false,
            attach_transcript: false,
            consent_acknowledged: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_conservative() {
        let settings = RecordingSettings::default();
        assert!(!settings.record_meetings);
        assert_eq!(settings.microphone_device_uid, None);
        assert_eq!(
            settings.transcription_engine,
            TranscriptionEngineChoice::Auto
        );
        assert_eq!(settings.transcription_language, None);
        assert!(!settings.summarize_with_ai);
        assert!(!settings.consent_acknowledged);
        assert!(settings.offer_to_record && settings.remind_to_tell);
        assert_eq!(settings.keep_audio, KeepAudio::Days7);
        assert_eq!(settings.transcription_model, TranscriptionModelChoice::Fast);
        assert!(settings.transcribe_during_meetings);
    }

    #[test]
    fn serializes_camel_case_and_older_json_fills_defaults() {
        let json = serde_json::to_value(RecordingSettings::default()).unwrap();
        assert_eq!(json["keepAudio"], "days7");
        assert_eq!(json["transcriptionEngine"], "auto");
        assert_eq!(json["transcriptionModel"], "fast");
        assert_eq!(json["microphoneDeviceUid"], serde_json::Value::Null);
        assert_eq!(json["transcriptionLanguage"], serde_json::Value::Null);
        assert_eq!(json["recordMeetings"], false);
        let older: RecordingSettings = serde_json::from_str(
            r#"{"recordMeetings":true,"keepAudio":"untilTranscribed","futureField":1}"#,
        )
        .unwrap();
        assert!(older.record_meetings);
        assert_eq!(older.keep_audio, KeepAudio::UntilTranscribed);
        assert_eq!(older.transcription_engine, TranscriptionEngineChoice::Auto);
        assert_eq!(older.microphone_device_uid, None);
        assert_eq!(older.transcription_language, None);
        assert!(older.offer_to_record, "a missing field takes its default");
        assert!(
            older.transcribe_during_meetings,
            "settings saved before the switch existed read as on"
        );
        assert_eq!(json["transcribeDuringMeetings"], true);
        let off: RecordingSettings =
            serde_json::from_str(r#"{"transcribeDuringMeetings":false}"#).unwrap();
        assert!(!off.transcribe_during_meetings);
        let empty: RecordingSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, RecordingSettings::default());
    }

    #[test]
    fn every_choice_round_trips_and_engine_resolves() {
        for keep in ["days7", "days30", "untilDeleted", "untilTranscribed"] {
            let parsed: KeepAudio = serde_json::from_value(keep.into()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), keep);
        }
        for model in ["fast", "accurate"] {
            let parsed: TranscriptionModelChoice = serde_json::from_value(model.into()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), model);
        }
        for engine in ["auto", "speechAnalyzer", "whisper"] {
            let parsed: TranscriptionEngineChoice = serde_json::from_value(engine.into()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), engine);
        }
        assert_eq!(
            effective_engine(TranscriptionEngineChoice::Auto, true),
            EffectiveEngine::SpeechAnalyzer
        );
        assert_eq!(
            effective_engine(TranscriptionEngineChoice::Auto, false),
            EffectiveEngine::Whisper
        );
        assert_eq!(
            effective_engine(TranscriptionEngineChoice::Whisper, true),
            EffectiveEngine::Whisper
        );
        assert_eq!(
            effective_engine(TranscriptionEngineChoice::SpeechAnalyzer, false),
            EffectiveEngine::Whisper
        );
    }
}
