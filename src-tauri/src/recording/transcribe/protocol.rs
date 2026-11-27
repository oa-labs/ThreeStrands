//! The transcription worker's protocol: length-prefixed JSON frames both
//! ways over the worker's stdin and stdout.
//!
//! A frame is a 4-byte big-endian length and that many bytes of UTF-8 JSON.
//! Neither side accepts a frame longer than
//! [`policy::WORKER_MAX_FRAME_BYTES`](crate::recording::policy::WORKER_MAX_FRAME_BYTES).
//!
//! The parent sends [`Request::Config`] first (model paths, language,
//! threads) and waits for [`Response::Ready`]. Then, one at a time, it sends
//! a [`Request::Segment`] per segment file and reads its
//! [`Response::Lines`]. Closing stdin ends the worker, which exits 0. Any
//! other exit, a malformed or oversized frame, or an unexpected response
//! fails the job.
//!
//! **Batches.** A worker may stop before its job ends: during a recording
//! it transcribes the audio waiting and exits, and memory pressure can stop
//! it. Each [`Response::Lines`] says where the audio the worker is still
//! holding back for that track starts ([`HeldAudio`]: files and sample
//! offsets), and the next worker's first request for the track hands it
//! back in [`SegmentRequest::carry_in`], so speech held at the end of one
//! batch is transcribed whole at the start of the next.

use crate::recording::capture::Track;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    Config {
        model_path: PathBuf,
        vad_model_path: PathBuf,
        /// A whisper language code, or `None` to detect it.
        language: Option<String>,
        threads: u32,
    },
    Segment(SegmentRequest),
}

/// One segment file to transcribe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentRequest {
    pub track: Track,
    pub wav_path: PathBuf,
    /// Where the file's first sample sits on the recording's clock.
    pub start_ms: u64,
    /// The file directly continues the previous file of this track (no gap
    /// between them), so speech held back at that file's end goes on here.
    pub continues: bool,
    /// The run of contiguous audio ends with this file, so nothing is held
    /// back: speech still going at its end is transcribed now.
    pub ends_run: bool,
    /// Audio an earlier worker was holding back for this track when it
    /// stopped. The worker reads it first and treats it as its own held
    /// audio, so this file continues it as if one worker had done both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_in: Option<HeldAudio>,
}

/// Audio held back at the end of a track's last file: where it starts on
/// the recording's clock, and the files it comes from, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldAudio {
    pub start_ms: u64,
    pub pieces: Vec<HeldPiece>,
}

/// `samples` samples of a segment file, from sample `from_sample` on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldPiece {
    pub wav_path: PathBuf,
    pub from_sample: u64,
    pub samples: u64,
}

/// A transcribed sentence, on the recording's clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptLine {
    pub track: Track,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Response {
    Ready,
    Lines {
        lines: Vec<TranscriptLine>,
        /// What the worker now holds back for the request's track, if
        /// anything.
        #[serde(default)]
        held: Option<HeldAudio>,
    },
}

/// Why a frame couldn't be read.
#[derive(Debug)]
pub enum FrameError {
    /// The stream ended inside a frame.
    Truncated,
    /// The length prefix is over the cap.
    TooLarge(usize),
    /// The body isn't the expected JSON.
    Malformed(String),
    Io(io::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "truncated frame"),
            Self::TooLarge(length) => write!(f, "frame of {length} bytes is too large"),
            Self::Malformed(error) => write!(f, "malformed frame: {error}"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

pub fn write_frame<W: Write, T: Serialize>(out: &mut W, message: &T) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    let length = u32::try_from(body.len()).map_err(|_| io::Error::other("frame too large"))?;
    out.write_all(&length.to_be_bytes())?;
    out.write_all(&body)?;
    out.flush()
}

/// Reads one frame. `Ok(None)` means the stream ended cleanly between
/// frames.
pub fn read_frame<R: Read, T: DeserializeOwned>(
    input: &mut R,
    max_bytes: usize,
) -> Result<Option<T>, FrameError> {
    let mut prefix = [0u8; 4];
    let mut filled = 0;
    while filled < prefix.len() {
        match input.read(&mut prefix[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Truncated),
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameError::Io(error)),
        }
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if length > max_bytes {
        return Err(FrameError::TooLarge(length));
    }
    let mut body = vec![0u8; length];
    input
        .read_exact(&mut body)
        .map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof => FrameError::Truncated,
            _ => FrameError::Io(error),
        })?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| FrameError::Malformed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment() -> Request {
        Request::Segment(SegmentRequest {
            track: Track::System,
            wav_path: "/a/system-0002.wav".into(),
            start_ms: 60_250,
            continues: true,
            ends_run: false,
            carry_in: None,
        })
    }

    fn frames(messages: &[Request]) -> Vec<u8> {
        let mut out = Vec::new();
        for message in messages {
            write_frame(&mut out, message).unwrap();
        }
        out
    }

    fn read_all(bytes: &[u8]) -> Result<Vec<Request>, FrameError> {
        let mut input = bytes;
        let mut out = Vec::new();
        while let Some(message) = read_frame(&mut input, 1024)? {
            out.push(message);
        }
        Ok(out)
    }

    #[test]
    fn frames_round_trip_in_camel_case() {
        let config = Request::Config {
            model_path: "/m/ggml-small.en-q5_1.bin".into(),
            vad_model_path: "/m/ggml-silero-v5.1.2.bin".into(),
            language: Some("en".into()),
            threads: 4,
        };
        let bytes = frames(&[config.clone(), segment()]);
        assert_eq!(read_all(&bytes).unwrap(), vec![config, segment()]);
        let body = &frames(&[segment()])[4..];
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(body).unwrap(),
            serde_json::json!({
                "type": "segment",
                "track": "system",
                "wavPath": "/a/system-0002.wav",
                "startMs": 60250,
                "continues": true,
                "endsRun": false
            })
        );
        let held = HeldAudio {
            start_ms: 59_000,
            pieces: vec![HeldPiece {
                wav_path: "/a/mic-0001.wav".into(),
                from_sample: 944_000,
                samples: 16_000,
            }],
        };
        let lines = Response::Lines {
            lines: vec![TranscriptLine {
                track: Track::Mic,
                start_ms: 1,
                end_ms: 2,
                text: "Hi.".into(),
            }],
            held: Some(held.clone()),
        };
        let mut out = Vec::new();
        write_frame(&mut out, &lines).unwrap();
        assert_eq!(
            read_frame::<_, Response>(&mut out.as_slice(), 1024).unwrap(),
            Some(lines)
        );
        assert_eq!(
            serde_json::to_value(&held).unwrap(),
            serde_json::json!({
                "startMs": 59000,
                "pieces": [{ "wavPath": "/a/mic-0001.wav", "fromSample": 944000, "samples": 16000 }]
            })
        );
        // A request carrying held audio in, and lines without a held field.
        let Request::Segment(mut carried) = segment() else {
            unreachable!()
        };
        carried.carry_in = Some(held);
        let carried = Request::Segment(carried);
        assert_eq!(
            read_all(&frames(&[carried.clone()])).unwrap(),
            vec![carried]
        );
        let bare: Response = serde_json::from_str(r#"{"type":"lines","lines":[]}"#).unwrap();
        assert_eq!(
            bare,
            Response::Lines {
                lines: vec![],
                held: None
            }
        );
    }

    #[test]
    fn an_empty_stream_ends_cleanly() {
        assert!(read_all(b"").unwrap().is_empty());
    }

    #[test]
    fn truncated_frames_are_rejected() {
        let bytes = frames(&[segment()]);
        for cut in [1, 3, 4, 5, bytes.len() - 1] {
            assert!(
                matches!(read_all(&bytes[..cut]), Err(FrameError::Truncated)),
                "cut at {cut}"
            );
        }
        // A whole frame followed by half a prefix.
        let mut two = bytes.clone();
        two.extend_from_slice(&[0, 0]);
        assert!(matches!(read_all(&two), Err(FrameError::Truncated)));
    }

    #[test]
    fn malformed_frames_are_rejected() {
        for body in [&b"not json"[..], b"{\"type\":\"nope\"}", b"\xff\xfe", b"{}"] {
            let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
            bytes.extend_from_slice(body);
            assert!(
                matches!(read_all(&bytes), Err(FrameError::Malformed(_))),
                "{body:?}"
            );
        }
    }

    #[test]
    fn frames_are_capped_below_at_and_above_the_limit() {
        let read = |length: usize| {
            let mut bytes = (length as u32).to_be_bytes().to_vec();
            bytes.extend(std::iter::repeat_n(b' ', length));
            let mut input = bytes.as_slice();
            read_frame::<_, Request>(&mut input, 64)
        };
        // Spaces are malformed JSON, but within the cap they are read.
        assert!(matches!(read(63), Err(FrameError::Malformed(_))));
        assert!(matches!(read(64), Err(FrameError::Malformed(_))));
        assert!(matches!(read(65), Err(FrameError::TooLarge(65))));
        // A huge claimed length is refused before anything is allocated.
        let mut input: &[u8] = &[0xFF, 0xFF, 0xFF, 0xFF];
        assert!(matches!(
            read_frame::<_, Request>(&mut input, 64),
            Err(FrameError::TooLarge(_))
        ));
    }
}
