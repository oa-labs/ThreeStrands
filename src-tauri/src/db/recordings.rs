//! Meeting recordings: one row per recording with its tracks, plus the
//! device-local Meeting Recording settings. Nothing here replicates or enters
//! settings transfer. Transcript lines have their own table,
//! `meeting_recording_segments`; deleting a recording cascades to tracks and
//! transcript lines.

use super::{serialization_error, Database, DatabaseError, DbResult};
use crate::note_entities::NoteReference;
use crate::recording::capture::Track;
use crate::recording::settings::RecordingSettings;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingState {
    Recording,
    Interrupted,
    /// Stopped, waiting for transcription.
    Recorded,
    Transcribing,
    Transcribed,
    Failed,
}

impl RecordingState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Interrupted => "interrupted",
            Self::Recorded => "recorded",
            Self::Transcribing => "transcribing",
            Self::Transcribed => "transcribed",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> DbResult<Self> {
        Ok(match value {
            "recording" => Self::Recording,
            "interrupted" => Self::Interrupted,
            "recorded" => Self::Recorded,
            "transcribing" => Self::Transcribing,
            "transcribed" => Self::Transcribed,
            "failed" => Self::Failed,
            other => {
                return Err(DatabaseError::corrupt(format!(
                    "Unknown recording state {other}"
                )))
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackState {
    Active,
    Failed,
    Unavailable,
}

impl TrackState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }

    fn parse(value: &str) -> DbResult<Self> {
        Ok(match value {
            "active" => Self::Active,
            "failed" => Self::Failed,
            "unavailable" => Self::Unavailable,
            other => {
                return Err(DatabaseError::corrupt(format!(
                    "Unknown track state {other}"
                )))
            }
        })
    }
}

/// A recording to create, with the starting state of each of its tracks.
pub struct NewRecording<'a> {
    pub id: &'a str,
    pub event_ref: Option<&'a NoteReference>,
    pub title: &'a str,
    pub started_at: &'a str,
    pub audio_dir: &'a str,
    pub tracks: &'a [(Track, TrackState)],
}

/// Fields of a track to change; `None` leaves a field as it is.
#[derive(Debug, Default, Clone)]
pub struct TrackPatch {
    pub state: Option<TrackState>,
    pub device_name: Option<String>,
    /// Where the track's first audio sits on the recording's clock.
    pub first_frame_offset_ms: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingRecordingTrack {
    pub track: Track,
    pub state: TrackState,
    pub device_name: Option<String>,
    pub first_frame_offset_ms: Option<i64>,
    pub error: Option<String>,
}

/// A recording as the app lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingRecording {
    pub id: String,
    pub event_ref: Option<NoteReference>,
    pub title: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_ms: i64,
    pub state: RecordingState,
    pub error: Option<String>,
    pub tracks: Vec<MeetingRecordingTrack>,
    /// No audio of this recording exists on disk: deleted (retention or
    /// Delete Audio), or never saved. The database knows only the first;
    /// the recording runtime adds the second.
    pub audio_deleted: bool,
    /// Whole percent while `transcribing`; set by the recording runtime.
    pub transcription_percent: Option<u8>,
    #[serde(skip)]
    pub audio_dir: String,
}

/// A transcript line to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRow {
    pub track: Track,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub dropped_as_echo: bool,
}

/// A recording's audio, as retention and deletion see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingAudio {
    pub id: String,
    pub state: RecordingState,
    pub audio_dir: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub audio_deleted_at: Option<String>,
}

/// How an event occurrence is keyed: its reference without contact or series.
fn event_target(reference: &NoteReference) -> DbResult<String> {
    if reference.kind != "event" {
        return Err(DatabaseError::invalid(
            "A recording can only belong to an event",
        ));
    }
    reference.target().map_err(DatabaseError::invalid)
}

fn tracks_of(connection: &Connection, id: &str) -> DbResult<Vec<MeetingRecordingTrack>> {
    let mut statement = connection.prepare(
        "SELECT track,state,device_name,first_frame_offset_ms,error
         FROM meeting_recording_tracks WHERE recording_id=?1 ORDER BY track DESC",
    )?;
    let rows = statement.query_map([id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })?;
    let mut tracks = Vec::new();
    for row in rows {
        let (track, state, device_name, first_frame_offset_ms, error) = row?;
        tracks.push(MeetingRecordingTrack {
            track: Track::parse(&track)
                .ok_or_else(|| DatabaseError::corrupt(format!("Unknown track {track}")))?,
            state: TrackState::parse(&state)?,
            device_name,
            first_frame_offset_ms,
            error,
        });
    }
    Ok(tracks)
}

const RECORDING_COLUMNS: &str =
    "id,event_ref,title,started_at,ended_at,duration_ms,state,error,audio_deleted_at,audio_dir";

fn read_recording(connection: &Connection, row: &rusqlite::Row<'_>) -> DbResult<MeetingRecording> {
    let id: String = row.get(0)?;
    let event_ref: Option<String> = row.get(1)?;
    let state: String = row.get(6)?;
    Ok(MeetingRecording {
        event_ref: event_ref
            .map(|json| serde_json::from_str(&json).map_err(serialization_error))
            .transpose()?,
        title: row.get(2)?,
        started_at: row.get(3)?,
        ended_at: row.get(4)?,
        duration_ms: row.get(5)?,
        state: RecordingState::parse(&state)?,
        error: row.get(7)?,
        tracks: tracks_of(connection, &id)?,
        audio_deleted: row.get::<_, Option<String>>(8)?.is_some(),
        transcription_percent: None,
        audio_dir: row.get(9)?,
        id,
    })
}

impl Database {
    /// Creates a recording in the `recording` state with its tracks.
    pub fn create_recording(&self, new: &NewRecording<'_>) -> DbResult<()> {
        let (event_json, target) = match new.event_ref {
            Some(reference) => (
                Some(serde_json::to_string(reference).map_err(serialization_error)?),
                Some(event_target(reference)?),
            ),
            None => (None, None),
        };
        self.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO meeting_recordings(id,event_ref,event_target,title,started_at,state,audio_dir)
                 VALUES(?1,?2,?3,?4,?5,'recording',?6)",
                params![new.id, event_json, target, new.title, new.started_at, new.audio_dir],
            )?;
            for (track, state) in new.tracks {
                tx.execute(
                    "INSERT INTO meeting_recording_tracks(recording_id,track,state) VALUES(?1,?2,?3)",
                    params![new.id, track.as_str(), state.as_str()],
                )?;
            }
            Ok(())
        })
    }

    pub fn update_recording_track(
        &self,
        id: &str,
        track: Track,
        patch: &TrackPatch,
    ) -> DbResult<()> {
        self.with_connection(|c| {
            let changed = c.execute(
                "UPDATE meeting_recording_tracks SET
                    state=COALESCE(?3,state),
                    device_name=COALESCE(?4,device_name),
                    first_frame_offset_ms=COALESCE(?5,first_frame_offset_ms),
                    error=COALESCE(?6,error)
                 WHERE recording_id=?1 AND track=?2",
                params![
                    id,
                    track.as_str(),
                    patch.state.map(TrackState::as_str),
                    patch.device_name,
                    patch.first_frame_offset_ms,
                    patch.error
                ],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Recording track"));
            }
            Ok(())
        })
    }

    pub fn set_recording_state(
        &self,
        id: &str,
        state: RecordingState,
        error: Option<&str>,
    ) -> DbResult<()> {
        self.with_connection(|c| {
            let changed = c.execute(
                "UPDATE meeting_recordings SET state=?2,error=?3 WHERE id=?1",
                params![id, state.as_str(), error],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Recording"));
            }
            Ok(())
        })
    }

    /// Ends a recording: when it ended, how long it ran (pauses excluded),
    /// and the state it ends in.
    pub fn finish_recording(
        &self,
        id: &str,
        ended_at: &str,
        duration_ms: i64,
        state: RecordingState,
        error: Option<&str>,
    ) -> DbResult<()> {
        self.with_connection(|c| {
            let changed = c.execute(
                "UPDATE meeting_recordings SET ended_at=?2,duration_ms=?3,state=?4,error=?5 WHERE id=?1",
                params![id, ended_at, duration_ms, state.as_str(), error],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Recording"));
            }
            Ok(())
        })
    }

    pub fn recording(&self, id: &str) -> DbResult<Option<MeetingRecording>> {
        self.with_connection(|c| {
            c.query_row(
                &format!("SELECT {RECORDING_COLUMNS} FROM meeting_recordings WHERE id=?1"),
                [id],
                |row| Ok(read_recording(c, row)),
            )
            .optional()?
            .transpose()
        })
    }

    /// An event occurrence's recordings, newest first.
    pub fn list_recordings(&self, event: &NoteReference) -> DbResult<Vec<MeetingRecording>> {
        let target = event_target(event)?;
        self.with_connection(|c| {
            let mut statement = c.prepare(&format!(
                "SELECT {RECORDING_COLUMNS} FROM meeting_recordings
                 WHERE event_target=?1 ORDER BY started_at DESC, id DESC"
            ))?;
            let rows = statement.query_map([target], |row| Ok(read_recording(c, row)))?;
            rows.map(|row| row?).collect()
        })
    }

    /// Deletes a recording; its tracks and segments go with it.
    pub fn delete_recording(&self, id: &str) -> DbResult<bool> {
        self.with_connection(|c| {
            Ok(c.execute("DELETE FROM meeting_recordings WHERE id=?1", [id])? > 0)
        })
    }

    /// Every recording's audio location and age, oldest first.
    pub fn recording_audio(&self) -> DbResult<Vec<RecordingAudio>> {
        self.with_connection(|c| {
            let mut statement = c.prepare(
                "SELECT id,state,audio_dir,started_at,ended_at,audio_deleted_at
                 FROM meeting_recordings ORDER BY started_at, id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?;
            let mut audio = Vec::new();
            for row in rows {
                let (id, state, audio_dir, started_at, ended_at, audio_deleted_at) = row?;
                audio.push(RecordingAudio {
                    id,
                    state: RecordingState::parse(&state)?,
                    audio_dir,
                    started_at,
                    ended_at,
                    audio_deleted_at,
                });
            }
            Ok(audio)
        })
    }

    /// One recording's audio location, if the recording exists.
    pub fn recording_audio_of(&self, id: &str) -> DbResult<Option<RecordingAudio>> {
        Ok(self
            .recording_audio()?
            .into_iter()
            .find(|audio| audio.id == id))
    }

    /// Notes that a recording's audio files were deleted.
    pub fn mark_recording_audio_deleted(&self, id: &str, at: &str) -> DbResult<()> {
        self.with_connection(|c| {
            let changed = c.execute(
                "UPDATE meeting_recordings SET audio_deleted_at=?2 WHERE id=?1",
                params![id, at],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Recording"));
            }
            Ok(())
        })
    }

    pub fn recording_count(&self) -> DbResult<i64> {
        self.with_connection(|c| {
            Ok(
                c.query_row("SELECT COUNT(*) FROM meeting_recordings", [], |row| {
                    row.get(0)
                })?,
            )
        })
    }

    /// Startup crash recovery: a row still `recording` belongs to a process
    /// that is gone. Returns how many were marked.
    pub fn mark_interrupted_recordings(&self) -> DbResult<usize> {
        self.with_connection(|c| {
            Ok(c.execute(
                "UPDATE meeting_recordings SET state='interrupted' WHERE state='recording'",
                [],
            )?)
        })
    }

    /// Startup recovery: a transcription can't still be running in a new
    /// process, so its recording waits to be transcribed again. Returns how
    /// many were reset.
    pub fn reset_transcribing_recordings(&self) -> DbResult<usize> {
        self.with_connection(|c| {
            Ok(c.execute(
                "UPDATE meeting_recordings SET state='recorded' WHERE state='transcribing'",
                [],
            )?)
        })
    }

    /// Replaces a recording's transcript in one transaction and marks it
    /// transcribed by `engine` with `model`. `system_first_ms` is where the
    /// system track's audio starts, when it saved any; it fills in a first
    /// frame the track didn't record, so speaker labels follow the audio.
    pub fn save_transcript(
        &self,
        id: &str,
        rows: &[TranscriptRow],
        engine: &str,
        model: &str,
        system_first_ms: Option<i64>,
    ) -> DbResult<()> {
        self.with_transaction(|tx| {
            let changed = tx.execute(
                "UPDATE meeting_recordings SET state='transcribed',error=NULL,engine=?2,model=?3 WHERE id=?1",
                params![id, engine, model],
            )?;
            if changed == 0 {
                return Err(DatabaseError::NotFound("Recording"));
            }
            tx.execute(
                "DELETE FROM meeting_recording_segments WHERE recording_id=?1",
                [id],
            )?;
            for (seq, row) in rows.iter().enumerate() {
                tx.execute(
                    "INSERT INTO meeting_recording_segments
                        (recording_id,seq,track,start_ms,end_ms,text,dropped_as_echo)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        id,
                        seq as i64,
                        row.track.as_str(),
                        row.start_ms,
                        row.end_ms,
                        row.text,
                        row.dropped_as_echo
                    ],
                )?;
            }
            if let Some(first) = system_first_ms {
                tx.execute(
                    "UPDATE meeting_recording_tracks SET first_frame_offset_ms=?2
                     WHERE recording_id=?1 AND track='system' AND first_frame_offset_ms IS NULL",
                    params![id, first],
                )?;
            }
            Ok(())
        })
    }

    /// A recording's transcript lines in order, echo included.
    pub fn transcript_rows(&self, id: &str) -> DbResult<Vec<TranscriptRow>> {
        self.with_connection(|c| {
            let mut statement = c.prepare(
                "SELECT track,start_ms,end_ms,text,dropped_as_echo
                 FROM meeting_recording_segments WHERE recording_id=?1 ORDER BY seq",
            )?;
            let rows = statement.query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (track, start_ms, end_ms, text, dropped_as_echo) = row?;
                out.push(TranscriptRow {
                    track: Track::parse(&track)
                        .ok_or_else(|| DatabaseError::corrupt(format!("Unknown track {track}")))?,
                    start_ms,
                    end_ms,
                    text,
                    dropped_as_echo,
                });
            }
            Ok(out)
        })
    }

    /// The engine and model a recording was transcribed with.
    #[cfg(test)]
    pub fn recording_engine(&self, id: &str) -> DbResult<Option<(Option<String>, Option<String>)>> {
        self.with_connection(|c| {
            Ok(c.query_row(
                "SELECT engine,model FROM meeting_recordings WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
        })
    }

    /// The device-local settings; defaults when none were saved.
    pub fn recording_settings(&self) -> DbResult<RecordingSettings> {
        self.with_connection(|c| {
            let json: Option<String> = c
                .query_row(
                    "SELECT settings_json FROM meeting_recording_settings WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            match json {
                Some(json) => {
                    serde_json::from_str(&json).map_err(|e| DatabaseError::corrupt(e.to_string()))
                }
                None => Ok(RecordingSettings::default()),
            }
        })
    }

    pub fn set_recording_settings(&self, settings: &RecordingSettings) -> DbResult<()> {
        let json = serde_json::to_string(settings).map_err(serialization_error)?;
        self.with_connection(|c| {
            c.execute(
                "INSERT INTO meeting_recording_settings(id,settings_json) VALUES(1,?1)
                 ON CONFLICT(id) DO UPDATE SET settings_json=excluded.settings_json",
                [json],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::settings::KeepAudio;

    fn event(id: &str) -> NoteReference {
        NoteReference {
            version: 1,
            kind: "event".into(),
            id: id.into(),
            account: Some("you@example.com".into()),
            calendar: Some("primary".into()),
            note: None,
            contact: None,
            series: None,
        }
    }

    fn create(database: &Database, id: &str, event_ref: Option<&NoteReference>, at: &str) {
        database
            .create_recording(&NewRecording {
                id,
                event_ref,
                title: "Weekly Sync",
                started_at: at,
                audio_dir: "/audio/x",
                tracks: &[
                    (Track::Mic, TrackState::Active),
                    (Track::System, TrackState::Unavailable),
                ],
            })
            .unwrap();
    }

    #[test]
    fn a_new_recording_has_its_tracks_and_updates_apply() {
        let database = Database::open_memory();
        create(&database, "r1", Some(&event("e1")), "2026-11-27T10:00:00Z");
        let recording = database.recording("r1").unwrap().unwrap();
        assert_eq!(recording.state, RecordingState::Recording);
        assert_eq!(recording.event_ref, Some(event("e1")));
        assert_eq!(recording.tracks.len(), 2);
        database
            .update_recording_track(
                "r1",
                Track::Mic,
                &TrackPatch {
                    device_name: Some("MacBook Mic".into()),
                    first_frame_offset_ms: Some(120),
                    ..Default::default()
                },
            )
            .unwrap();
        database
            .update_recording_track(
                "r1",
                Track::Mic,
                &TrackPatch {
                    state: Some(TrackState::Failed),
                    error: Some("Device lost".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let mic = &database.recording("r1").unwrap().unwrap().tracks;
        let mic = mic.iter().find(|t| t.track == Track::Mic).unwrap();
        assert_eq!(mic.state, TrackState::Failed);
        assert_eq!(mic.device_name.as_deref(), Some("MacBook Mic"));
        assert_eq!(mic.first_frame_offset_ms, Some(120));
        assert_eq!(mic.error.as_deref(), Some("Device lost"));
        assert!(matches!(
            database.update_recording_track("nope", Track::Mic, &TrackPatch::default()),
            Err(DatabaseError::NotFound(_))
        ));
    }

    #[test]
    fn state_and_finish_are_written() {
        let database = Database::open_memory();
        create(&database, "r1", None, "2026-11-27T10:00:00Z");
        database
            .set_recording_state("r1", RecordingState::Failed, Some("No audio was recorded."))
            .unwrap();
        let failed = database.recording("r1").unwrap().unwrap();
        assert_eq!(failed.error.as_deref(), Some("No audio was recorded."));
        database
            .finish_recording(
                "r1",
                "2026-11-27T10:30:00Z",
                1_800_000,
                RecordingState::Recorded,
                None,
            )
            .unwrap();
        let done = database.recording("r1").unwrap().unwrap();
        assert_eq!(done.state, RecordingState::Recorded);
        assert_eq!(done.ended_at.as_deref(), Some("2026-11-27T10:30:00Z"));
        assert_eq!(done.duration_ms, 1_800_000);
        assert_eq!(done.error, None);
        assert!(database.recording("missing").unwrap().is_none());
        assert!(matches!(
            database.set_recording_state("missing", RecordingState::Failed, None),
            Err(DatabaseError::NotFound(_))
        ));
    }

    #[test]
    fn listing_finds_one_occurrence_newest_first_and_ignores_series_and_contact_differences() {
        let database = Database::open_memory();
        create(&database, "old", Some(&event("e1")), "2026-11-27T09:00:00Z");
        create(&database, "new", Some(&event("e1")), "2026-11-27T11:00:00Z");
        create(
            &database,
            "other",
            Some(&event("e2")),
            "2026-11-27T12:00:00Z",
        );
        create(&database, "none", None, "2026-11-27T13:00:00Z");
        let mut with_series = event("e1");
        with_series.series = Some("weekly".into());
        let ids: Vec<String> = database
            .list_recordings(&with_series)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, ["new", "old"]);
        let mut not_event = event("e1");
        not_event.kind = "note".into();
        assert!(database.list_recordings(&not_event).is_err());
    }

    #[test]
    fn deleting_cascades_to_tracks_and_segments() {
        let database = Database::open_memory();
        create(&database, "r1", Some(&event("e1")), "2026-11-27T10:00:00Z");
        database
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO meeting_recording_segments(recording_id,seq,track,start_ms,end_ms,text)
                     VALUES('r1',0,'mic',0,1000,'hello')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(database.delete_recording("r1").unwrap());
        assert!(!database.delete_recording("r1").unwrap());
        for table in ["meeting_recording_tracks", "meeting_recording_segments"] {
            let count: i64 = database
                .with_connection(|c| {
                    Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }

    fn row(track: Track, start_ms: i64, text: &str, echo: bool) -> TranscriptRow {
        TranscriptRow {
            track,
            start_ms,
            end_ms: start_ms + 1000,
            text: text.into(),
            dropped_as_echo: echo,
        }
    }

    #[test]
    fn a_saved_transcript_replaces_the_last_one_and_marks_the_recording() {
        let database = Database::open_memory();
        create(&database, "r1", None, "2026-11-27T10:00:00Z");
        database
            .set_recording_state("r1", RecordingState::Transcribing, None)
            .unwrap();
        database
            .save_transcript(
                "r1",
                &[row(Track::Mic, 0, "Old.", false)],
                "whisper.cpp",
                "a.bin",
                None,
            )
            .unwrap();
        let rows = vec![
            row(Track::Mic, 0, "Hello.", false),
            row(Track::System, 500, "Hi there.", false),
            row(Track::Mic, 600, "Hi there.", true),
        ];
        database
            .save_transcript(
                "r1",
                &rows,
                "whisper.cpp",
                "ggml-small.en-q5_1.bin",
                Some(250),
            )
            .unwrap();
        assert_eq!(
            database.transcript_rows("r1").unwrap(),
            rows,
            "in order, echo kept"
        );
        let recording = database.recording("r1").unwrap().unwrap();
        assert_eq!(recording.state, RecordingState::Transcribed);
        assert_eq!(recording.error, None);
        assert_eq!(
            database.recording_engine("r1").unwrap(),
            Some((
                Some("whisper.cpp".into()),
                Some("ggml-small.en-q5_1.bin".into())
            ))
        );
        let system = recording
            .tracks
            .iter()
            .find(|t| t.track == Track::System)
            .unwrap();
        assert_eq!(system.first_frame_offset_ms, Some(250));
        assert!(matches!(
            database.save_transcript("missing", &rows, "e", "m", None),
            Err(DatabaseError::NotFound(_))
        ));
        assert!(database.transcript_rows("missing").unwrap().is_empty());
    }

    #[test]
    fn a_first_frame_already_recorded_is_kept() {
        let database = Database::open_memory();
        create(&database, "r1", None, "2026-11-27T10:00:00Z");
        database
            .update_recording_track(
                "r1",
                Track::System,
                &TrackPatch {
                    first_frame_offset_ms: Some(5),
                    ..Default::default()
                },
            )
            .unwrap();
        database
            .save_transcript("r1", &[], "e", "m", Some(900))
            .unwrap();
        let recording = database.recording("r1").unwrap().unwrap();
        let system = recording
            .tracks
            .iter()
            .find(|t| t.track == Track::System)
            .unwrap();
        assert_eq!(system.first_frame_offset_ms, Some(5));
    }

    #[test]
    fn transcribing_rows_go_back_to_recorded_at_launch() {
        let database = Database::open_memory();
        create(&database, "a", None, "2026-11-27T10:00:00Z");
        create(&database, "b", None, "2026-11-27T11:00:00Z");
        database
            .set_recording_state("a", RecordingState::Transcribing, None)
            .unwrap();
        database
            .set_recording_state("b", RecordingState::Transcribed, None)
            .unwrap();
        assert_eq!(database.reset_transcribing_recordings().unwrap(), 1);
        assert_eq!(
            database.recording("a").unwrap().unwrap().state,
            RecordingState::Recorded
        );
        assert_eq!(
            database.recording("b").unwrap().unwrap().state,
            RecordingState::Transcribed
        );
    }

    #[test]
    fn listed_recordings_say_whether_their_audio_was_deleted() {
        let database = Database::open_memory();
        create(&database, "r1", Some(&event("e1")), "2026-11-27T10:00:00Z");
        let listed = &database.list_recordings(&event("e1")).unwrap()[0];
        assert!(!listed.audio_deleted);
        assert_eq!(listed.transcription_percent, None);
        let json = serde_json::to_value(listed).unwrap();
        assert_eq!(json["audioDeleted"], false);
        assert_eq!(json["transcriptionPercent"], serde_json::Value::Null);
        assert!(json.get("audioDir").is_none(), "the path stays native");
        database
            .mark_recording_audio_deleted("r1", "2026-11-28T10:00:00Z")
            .unwrap();
        assert!(database.list_recordings(&event("e1")).unwrap()[0].audio_deleted);
    }

    #[test]
    fn only_recording_rows_become_interrupted() {
        let database = Database::open_memory();
        create(&database, "live", None, "2026-11-27T10:00:00Z");
        create(&database, "done", None, "2026-11-27T09:00:00Z");
        database
            .finish_recording(
                "done",
                "2026-11-27T09:10:00Z",
                1,
                RecordingState::Recorded,
                None,
            )
            .unwrap();
        assert_eq!(database.mark_interrupted_recordings().unwrap(), 1);
        assert_eq!(
            database.recording("live").unwrap().unwrap().state,
            RecordingState::Interrupted
        );
        assert_eq!(
            database.recording("done").unwrap().unwrap().state,
            RecordingState::Recorded
        );
        assert_eq!(database.mark_interrupted_recordings().unwrap(), 0);
    }

    #[test]
    fn settings_default_when_missing_and_round_trip() {
        let database = Database::open_memory();
        assert_eq!(
            database.recording_settings().unwrap(),
            RecordingSettings::default()
        );
        let settings = RecordingSettings {
            record_meetings: true,
            keep_audio: KeepAudio::UntilDeleted,
            ..Default::default()
        };
        database.set_recording_settings(&settings).unwrap();
        assert_eq!(database.recording_settings().unwrap(), settings);
        database
            .set_recording_settings(&RecordingSettings::default())
            .unwrap();
        assert_eq!(
            database.recording_settings().unwrap(),
            RecordingSettings::default()
        );
    }

    #[test]
    fn stored_settings_from_an_older_build_still_load() {
        let database = Database::open_memory();
        database
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO meeting_recording_settings(id,settings_json) VALUES(1,'{\"recordMeetings\":true,\"removedField\":3}')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let settings = database.recording_settings().unwrap();
        assert!(settings.record_meetings);
        assert!(settings.offer_to_record);
    }
}
