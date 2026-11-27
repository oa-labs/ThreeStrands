//! Opt-in Stage 8 cloud transcription: OpenRouter (`microsoft/mai-transcribe-2`)
//! and OpenAI-compatible `/audio/transcriptions` endpoints.
//!
//! **Separate tracks invariant:** microphone (`You`) and system audio (`Others`)
//! are NEVER mixed before upload. Each track is chunked independently within the
//! provider upload cap (~25 MB; OpenRouter uses base64 JSON capped at 18 MB raw
//! WAV), uploaded one track at a time, and passed through [`super::merge`] for
//! post-filtering and acoustic echo removal. Optional diarization runs only on
//! the `system` track.

use super::merge::{self, MergeOptions};
use super::plan::{self, PlannedSegment};
use super::protocol::TranscriptLine;
use super::stream::{read_segment, slice_rms};
use super::{JobError, StopCheck};
use crate::ai::manifest::{fingerprint, ContextManifest, Feature, SourceScope, CONTEXT_VERSION};
use crate::ai::requests::{RequestRecord, Runtime as RequestRuntime};
use crate::ai::usage::{record_event, RequestPurpose, UsageEvent};
use crate::ai::AiProvider;
use crate::db::recordings::TranscriptRow;
use crate::recording::capture::Track;
use crate::recording::policy;
use crate::recording::segments;
use base64::Engine as _;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct TrackChunk {
    pub track: Track,
    pub start_ms: u64,
    pub duration_ms: u64,
    pub wav_bytes: Vec<u8>,
    pub rms: f32,
}

#[derive(Debug, Clone)]
pub struct CloudChunkRequest {
    pub provider: AiProvider,
    pub model: String,
    pub endpoint: Option<String>,
    pub api_key: String,
    pub track: Track,
    pub wav_bytes: Vec<u8>,
    pub duration_ms: u64,
    pub language: Option<String>,
    pub custom_vocabulary: Vec<String>,
    pub prompt: Option<String>,
    /// True only on `Track::System` when remote speaker diarization is requested.
    pub diarize: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CloudSegmentLine {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub speaker_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CloudChunkResponse {
    pub lines: Vec<CloudSegmentLine>,
    pub cost_usd: Option<f64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub raw_body: String,
}

/// Transport seam for uploading one mono track chunk to a cloud transcription API.
pub trait CloudTranscriptionBackend: Send + Sync {
    fn transcribe_chunk(&self, request: &CloudChunkRequest) -> Result<CloudChunkResponse, String>;
}

/// Real HTTPS client talking to OpenRouter or OpenAI-compatible `/audio/transcriptions`.
pub struct HttpCloudTranscriber;

impl HttpCloudTranscriber {
    pub fn new() -> Self {
        Self
    }

    pub async fn transcribe_chunk_async(
        &self,
        request: &CloudChunkRequest,
    ) -> Result<CloudChunkResponse, String> {
        let base_url = request
            .provider
            .base_url(request.endpoint.as_deref())
            .map_err(|e| e)?;
        let url = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));
        let client = crate::http_client::builder()
            .timeout(policy::CLOUD_TRANSCRIPTION_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())?;

        let response = match request.provider {
            AiProvider::OpenRouter => {
                let payload = build_openrouter_payload(request);
                client
                    .post(&url)
                    .bearer_auth(&request.api_key)
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| format!("Cloud transcription request failed: {e}"))?
            }
            AiProvider::OpenAi | AiProvider::Fireworks | AiProvider::Custom => {
                let file_part = Part::bytes(request.wav_bytes.clone())
                    .file_name("audio.wav")
                    .mime_str("audio/wav")
                    .map_err(|e| e.to_string())?;
                let mut form = Form::new()
                    .part("file", file_part)
                    .text("model", request.model.clone())
                    .text("response_format", "verbose_json")
                    .text("timestamp_granularities[]", "segment");
                if let Some(language) = request.language.as_deref() {
                    form = form.text("language", language.to_string());
                }
                if let Some(prompt) = request.prompt.as_deref() {
                    form = form.text("prompt", prompt.to_string());
                }
                client
                    .post(&url)
                    .bearer_auth(&request.api_key)
                    .multipart(form)
                    .send()
                    .await
                    .map_err(|e| format!("Cloud transcription request failed: {e}"))?
            }
            _ => return Err(
                "Choose OpenRouter, OpenAI, Fireworks, or Custom Endpoint for cloud transcription."
                    .into(),
            ),
        };

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| format!("Reading cloud transcription response: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "Cloud transcription failed ({}): {}",
                status.as_u16(),
                compact_error_body(&body)
            ));
        }

        parse_cloud_transcription_response(
            &body,
            request.track,
            0,
            request.duration_ms,
            request.diarize && request.track == Track::System,
        )
    }
}

impl Default for HttpCloudTranscriber {
    fn default() -> Self {
        Self::new()
    }
}

impl CloudTranscriptionBackend for HttpCloudTranscriber {
    fn transcribe_chunk(&self, request: &CloudChunkRequest) -> Result<CloudChunkResponse, String> {
        let drive = || -> Result<CloudChunkResponse, String> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("Starting cloud transcription runtime: {e}"))?;
            rt.block_on(self.transcribe_chunk_async(request))
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                scope
                    .spawn(drive)
                    .join()
                    .unwrap_or_else(|_| Err("Cloud transcription worker panicked.".into()))
            })
        } else {
            drive()
        }
    }
}

fn compact_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "Empty error response".into();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(msg) = value
            .get("error")
            .and_then(|e| e.get("message").or(Some(e)))
            .and_then(serde_json::Value::as_str)
        {
            return msg.chars().take(240).collect();
        }
    }
    trimmed.chars().take(240).collect()
}

/// Builds OpenRouter's JSON request payload, including Azure `MAI-Transcribe-2`
/// options (`phraseList`, `transcribeStyle: "clean"`, and `system`-only `diarization`).
pub fn build_openrouter_payload(request: &CloudChunkRequest) -> serde_json::Value {
    let encoded = base64::engine::general_purpose::STANDARD.encode(&request.wav_bytes);
    let diarize_remote = request.diarize && request.track == Track::System;
    let vocab = policy::sanitize_custom_vocabulary(&request.custom_vocabulary);
    let mut payload = json!({
        "model": request.model,
        "input_audio": {
            "data": encoded,
            "format": "wav",
        },
        "response_format": "verbose_json",
        "timestamp_granularities": ["segment"],
    });
    let obj = payload.as_object_mut().expect("JSON object");
    if let Some(lang) = request.language.as_deref() {
        obj.insert("language".into(), json!(lang));
    }
    if !vocab.is_empty() {
        obj.insert("keyterms".into(), json!(vocab));
    }
    if diarize_remote {
        obj.insert("diarize".into(), json!(true));
    }
    if request
        .model
        .to_ascii_lowercase()
        .contains("mai-transcribe-2")
    {
        let mut azure = json!({
            "enhancedMode": {
                "enabled": true,
                "modelOptions": {
                    "transcribeStyle": "clean",
                },
            },
            "diarization": {
                "enabled": diarize_remote,
            },
        });
        if !vocab.is_empty() {
            azure["phraseList"] = json!({ "phrases": vocab });
        }
        obj.insert(
            "provider".into(),
            json!({
                "options": {
                    "azure": azure,
                }
            }),
        );
    }
    payload
}

/// Parses an OpenRouter or OpenAI-compatible `/audio/transcriptions` response.
/// Relative timestamps inside the chunk are preserved; callers shift by chunk
/// `start_ms` when building `TranscriptLine`s.
pub fn parse_cloud_transcription_response(
    body: &str,
    track: Track,
    chunk_start_ms: u64,
    chunk_duration_ms: u64,
    diarize: bool,
) -> Result<CloudChunkResponse, String> {
    #[derive(Deserialize, Default)]
    struct RawResponse {
        #[serde(default)]
        text: String,
        #[serde(default)]
        segments: Vec<RawSegment>,
        #[serde(default)]
        usage: Option<RawUsage>,
    }
    #[derive(Deserialize, Default)]
    struct RawSegment {
        #[serde(default)]
        start: f64,
        #[serde(default)]
        end: f64,
        #[serde(default)]
        text: String,
        #[serde(default)]
        speaker: Option<serde_json::Value>,
        #[serde(default, rename = "speaker_id")]
        speaker_id: Option<serde_json::Value>,
        #[serde(default, rename = "speakerId")]
        speaker_id_camel: Option<serde_json::Value>,
    }
    #[derive(Deserialize, Default)]
    struct RawUsage {
        cost: Option<f64>,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    }

    let parsed: RawResponse = serde_json::from_str(body)
        .map_err(|_| "Cloud transcription returned an invalid response.".to_string())?;
    let allow_speaker = diarize && track == Track::System;
    let mut speaker_map: HashMap<String, String> = HashMap::new();
    let mut lines = Vec::new();

    if parsed.segments.is_empty() {
        let text = parsed.text.trim().to_string();
        if !text.is_empty() {
            lines.push(CloudSegmentLine {
                start_ms: chunk_start_ms,
                end_ms: chunk_start_ms.saturating_add(chunk_duration_ms),
                text,
                speaker_label: None,
            });
        }
    } else {
        for segment in parsed.segments {
            let text = segment.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let rel_start = (segment.start.max(0.0) * 1000.0).round() as u64;
            let rel_end = (segment.end.max(segment.start).max(0.0) * 1000.0).round() as u64;
            let raw_speaker = segment
                .speaker
                .or(segment.speaker_id)
                .or(segment.speaker_id_camel)
                .and_then(|v| match v {
                    serde_json::Value::String(s) if !s.trim().is_empty() => {
                        Some(s.trim().to_string())
                    }
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                });
            let speaker_label = if allow_speaker {
                raw_speaker.map(|raw| {
                    let next_index = speaker_map.len() + 1;
                    speaker_map
                        .entry(raw)
                        .or_insert_with(|| format!("Speaker {next_index}"))
                        .clone()
                })
            } else {
                None
            };
            lines.push(CloudSegmentLine {
                start_ms: chunk_start_ms.saturating_add(rel_start),
                end_ms: chunk_start_ms.saturating_add(rel_end.max(rel_start)),
                text,
                speaker_label,
            });
        }
    }

    let usage = parsed.usage.unwrap_or_default();
    let cost_usd = usage.cost.filter(|c| c.is_finite() && *c >= 0.0);
    let input_tokens = usage.prompt_tokens.or(usage.input_tokens).unwrap_or(0);
    let output_tokens = usage.completion_tokens.or(usage.output_tokens).unwrap_or(0);

    Ok(CloudChunkResponse {
        lines,
        cost_usd,
        input_tokens,
        output_tokens,
        raw_body: body.to_string(),
    })
}

fn samples_to_wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(&segments::wav_header(data_bytes));
    for &sample in samples {
        let clamped = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&clamped.to_le_bytes());
    }
    out
}

fn flush_chunk(track: Track, start_ms: u64, samples: Vec<f32>) -> Option<TrackChunk> {
    if samples.is_empty() {
        return None;
    }
    let duration_ms = (samples.len() as u64 * 1000) / policy::OUTPUT_SAMPLE_RATE as u64;
    let rms = slice_rms(&samples);
    let wav_bytes = samples_to_wav_bytes(&samples);
    Some(TrackChunk {
        track,
        start_ms,
        duration_ms,
        wav_bytes,
        rms,
    })
}

/// Plans independent per-track WAV upload chunks (never mixing `mic` and
/// `system`), decoding `.m4a` archives back to 16 kHz PCM when needed and
/// splitting at gaps, `max_chunk_ms`, and `max_wav_bytes`.
pub fn plan_track_chunks(
    audio_dir: &Path,
    track: Track,
    max_chunk_ms: u64,
    max_wav_bytes: usize,
) -> Result<Vec<TrackChunk>, String> {
    let planned: Vec<PlannedSegment> = plan::plan(audio_dir, true)
        .into_iter()
        .filter(|segment| segment.request.track == track)
        .collect();
    if planned.is_empty() {
        return Ok(Vec::new());
    }
    let max_samples_by_bytes = max_wav_bytes.saturating_sub(44) / 2;
    let max_samples_by_ms = (max_chunk_ms as usize * policy::OUTPUT_SAMPLE_RATE as usize) / 1000;
    let max_samples = max_samples_by_bytes.min(max_samples_by_ms).max(1);

    let mut chunks = Vec::new();
    let mut current_start_ms: Option<u64> = None;
    let mut current_samples: Vec<f32> = Vec::new();

    for segment in planned {
        if !segment.request.continues && !current_samples.is_empty() {
            if let Some(chunk) = flush_chunk(
                track,
                current_start_ms.unwrap_or(segment.request.start_ms),
                std::mem::take(&mut current_samples),
            ) {
                chunks.push(chunk);
            }
            current_start_ms = None;
        }
        let samples = read_segment(&segment.request.wav_path)?;
        if samples.is_empty() {
            continue;
        }
        let mut offset = 0usize;
        while offset < samples.len() {
            if current_start_ms.is_none() {
                let offset_ms = (offset as u64 * 1000) / policy::OUTPUT_SAMPLE_RATE as u64;
                current_start_ms = Some(segment.request.start_ms.saturating_add(offset_ms));
            }
            let room = max_samples.saturating_sub(current_samples.len());
            if room == 0 {
                if let Some(chunk) = flush_chunk(
                    track,
                    current_start_ms.take().unwrap_or(segment.request.start_ms),
                    std::mem::take(&mut current_samples),
                ) {
                    chunks.push(chunk);
                }
                continue;
            }
            let take = room.min(samples.len() - offset);
            current_samples.extend_from_slice(&samples[offset..offset + take]);
            offset += take;
            if current_samples.len() >= max_samples {
                if let Some(chunk) = flush_chunk(
                    track,
                    current_start_ms.take().unwrap_or(segment.request.start_ms),
                    std::mem::take(&mut current_samples),
                ) {
                    chunks.push(chunk);
                }
            }
        }
    }

    if let Some(chunk) = flush_chunk(
        track,
        current_start_ms.unwrap_or(0),
        std::mem::take(&mut current_samples),
    ) {
        chunks.push(chunk);
    }

    Ok(chunks)
}

pub struct CloudJobInput<'a> {
    pub recording_id: &'a str,
    pub title: &'a str,
    pub attendees: &'a [String],
    pub audio_dir: &'a Path,
    pub provider: AiProvider,
    pub model: &'a str,
    pub endpoint: Option<&'a str>,
    pub api_key: &'a str,
    pub language: Option<&'a str>,
    pub custom_vocabulary: &'a [String],
    pub diarize: bool,
    pub headphones: bool,
}

/// Uploads `mic` and `system` tracks separately to the configured cloud provider,
/// enforces the daily AI budget via `requests`, records AI usage events, and
/// merges both tracks with acoustic echo filtering.
pub fn transcribe_recording_cloud(
    backend: &dyn CloudTranscriptionBackend,
    requests: &RequestRuntime,
    input: CloudJobInput<'_>,
    stop: Option<&dyn StopCheck>,
    mut on_progress: impl FnMut(u8),
) -> Result<Vec<TranscriptRow>, JobError> {
    if !policy::cloud_provider_supported(input.provider) {
        return Err(JobError::Failed(
            "Choose OpenRouter, OpenAI, Fireworks, or Custom Endpoint in AI settings before using cloud transcription."
                .into(),
        ));
    }
    let model = input.model.trim();
    if model.is_empty() {
        return Err(JobError::Failed(
            "Choose a cloud transcription model in Settings.".into(),
        ));
    }
    if input.api_key.trim().is_empty() {
        return Err(JobError::Failed(
            "Add an API key in AI settings before using cloud transcription.".into(),
        ));
    }
    let base_url = input
        .provider
        .base_url(input.endpoint)
        .map_err(JobError::Failed)?;

    let max_raw_bytes = match input.provider {
        AiProvider::OpenRouter => policy::CLOUD_OPENROUTER_MAX_RAW_BYTES,
        _ => policy::CLOUD_TRANSCRIPTION_MAX_UPLOAD_BYTES,
    };

    let mut all_chunks = Vec::new();
    for track in Track::ALL {
        let track_chunks = plan_track_chunks(
            input.audio_dir,
            track,
            policy::CLOUD_TRANSCRIPTION_MAX_CHUNK_MS,
            max_raw_bytes,
        )
        .map_err(JobError::Failed)?;
        all_chunks.extend(track_chunks);
    }
    if all_chunks.is_empty() {
        return Err(JobError::Failed(
            "This recording has no audio to transcribe.".into(),
        ));
    }

    let total_ms: u64 = all_chunks.iter().map(|c| c.duration_ms).sum();
    let custom_vocab = policy::sanitize_custom_vocabulary(input.custom_vocabulary);
    let whisper_prompt =
        policy::sanitize_initial_prompt_with_vocab(input.title, input.attendees, &custom_vocab);
    let language = policy::normalize_language_code(input.language);

    let mut lines: Vec<TranscriptLine> = Vec::new();
    let mut done_ms: u64 = 0;

    for (index, chunk) in all_chunks.into_iter().enumerate() {
        if let Some(check) = stop {
            if let Some(reason) = check.stop_reason() {
                return Err(reason);
            }
        }
        let estimated_usd =
            policy::estimate_cloud_transcription_cost_usd(input.provider, model, chunk.duration_ms);
        let record = RequestRecord {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            at: chrono::Utc::now().to_rfc3339(),
            feature: Feature::CloudTranscription.id().into(),
            provider: input.provider.id().into(),
            model: model.to_string(),
            endpoint_revision: fingerprint(base_url.as_bytes()),
            prompt_version: Feature::CloudTranscription.prompt_version(),
            schema_version: Feature::CloudTranscription.schema_version(),
            prompt_revision: fingerprint(custom_vocab.join(",").as_bytes()),
            schema_revision: None,
            context: ContextManifest {
                version: CONTEXT_VERSION,
                payload_revision: fingerprint(&chunk.wav_bytes),
                scope: SourceScope {
                    account_ids: Vec::new(),
                    record_id: Some(input.recording_id.to_string()),
                    revision: Some(format!("{}:{index}", chunk.track.as_str())),
                },
                sources: Vec::new(),
            },
            response_revision: None,
            outcome: "pending".into(),
            request_outcome: "pending".into(),
            charged_usd: 0.0,
            price: None,
        };
        let reservation = requests
            .reserve_fixed_cost(record, estimated_usd)
            .map_err(JobError::Failed)?;

        let req = CloudChunkRequest {
            provider: input.provider,
            model: model.to_string(),
            endpoint: input.endpoint.map(str::to_string),
            api_key: input.api_key.to_string(),
            track: chunk.track,
            wav_bytes: chunk.wav_bytes,
            duration_ms: chunk.duration_ms,
            language: language.clone(),
            custom_vocabulary: custom_vocab.clone(),
            prompt: whisper_prompt.clone(),
            diarize: input.diarize && chunk.track == Track::System,
        };

        let response = match backend.transcribe_chunk(&req) {
            Ok(resp) => resp,
            Err(error) => {
                let _ = reservation.failed("error");
                return Err(JobError::Failed(error));
            }
        };

        let _ = reservation.finish(
            &response.raw_body,
            response.input_tokens,
            response.output_tokens,
            response.cost_usd,
            response.cost_usd.is_some(),
            "received",
        );

        record_event(UsageEvent {
            provider: input.provider,
            model: model.to_string(),
            input_tokens: response.input_tokens,
            output_tokens: response.output_tokens,
            cost_usd: response.cost_usd.or(Some(estimated_usd)),
            purpose: Some(RequestPurpose::about(
                Feature::CloudTranscription.id(),
                input.title,
                None,
            )),
        });

        for segment in response.lines {
            lines.push(TranscriptLine {
                track: chunk.track,
                start_ms: chunk.start_ms.saturating_add(segment.start_ms),
                end_ms: chunk.start_ms.saturating_add(segment.end_ms),
                text: segment.text,
                rms: Some(chunk.rms),
                speaker_label: segment.speaker_label,
            });
        }

        done_ms += chunk.duration_ms;
        if total_ms > 0 {
            on_progress(((done_ms.min(total_ms) * 100) / total_ms) as u8);
        }
    }

    let merged = merge::merge_with_options(
        lines,
        MergeOptions {
            headphones: input.headphones,
        },
    );
    Ok(merged
        .into_iter()
        .map(|entry| TranscriptRow {
            track: entry.line.track,
            start_ms: entry.line.start_ms as i64,
            end_ms: entry.line.end_ms as i64,
            text: entry.line.text,
            rms: entry.line.rms,
            dropped_as_echo: entry.dropped_as_echo,
            speaker_label: entry.line.speaker_label,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::requests::RequestPolicy;
    use crate::recording::segments::SegmentWriter;
    use crate::recording::test_support::TempDir;
    use axum::{extract::Multipart, routing::post, Json, Router};
    use std::sync::Mutex;

    fn samples_for_ms(ms: usize, value: i16) -> Vec<i16> {
        vec![value; ms * policy::OUTPUT_SAMPLE_RATE as usize / 1000]
    }

    #[test]
    fn chunk_planner_keeps_tracks_separate_and_respects_byte_and_duration_bounds() {
        let dir = TempDir::new();
        let mut mic = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        mic.write(&samples_for_ms(2_000, 1_500), 0).unwrap();
        mic.write(&samples_for_ms(2_000, 1_500), 2_000).unwrap();
        drop(mic);

        let mut system = SegmentWriter::open(dir.path(), Track::System).unwrap();
        system.write(&samples_for_ms(3_000, 3_000), 500).unwrap();
        drop(system);

        // Force duration splitting (1.5 s mic, 2.0 s system) with plenty of byte room.
        let mic_chunks = plan_track_chunks(dir.path(), Track::Mic, 1_500, 100_000).unwrap();
        let sys_chunks = plan_track_chunks(dir.path(), Track::System, 2_000, 100_000).unwrap();

        assert!(mic_chunks.iter().all(|c| c.track == Track::Mic));
        assert!(sys_chunks.iter().all(|c| c.track == Track::System));
        assert_eq!(mic_chunks.len(), 3);
        assert_eq!(mic_chunks[0].start_ms, 0);
        assert_eq!(mic_chunks[0].duration_ms, 1_500);
        assert_eq!(mic_chunks[1].start_ms, 1_500);
        assert_eq!(mic_chunks[2].start_ms, 3_000);
        assert_eq!(sys_chunks.len(), 2);
        assert_eq!(sys_chunks[0].start_ms, 500);
        assert_eq!(sys_chunks[1].start_ms, 2_500);
        for chunk in mic_chunks.iter().chain(sys_chunks.iter()) {
            assert!(chunk.wav_bytes.len() <= 100_000);
            assert!(chunk.rms > 0.01);
        }

        // Also force byte-cap splitting below duration limit (32 KB <= 1 s of PCM).
        let byte_split = plan_track_chunks(dir.path(), Track::System, 10_000, 32_044).unwrap();
        assert_eq!(byte_split.len(), 3);
        assert!(byte_split.iter().all(|c| c.wav_bytes.len() <= 32_044));
    }

    #[test]
    fn openrouter_payload_sends_phrase_list_and_system_only_diarization() {
        let wav = samples_to_wav_bytes(&[0.25; 160]);
        let mic_req = CloudChunkRequest {
            provider: AiProvider::OpenRouter,
            model: policy::CLOUD_DEFAULT_MODEL_OPENROUTER.into(),
            endpoint: None,
            api_key: "sk-or-test".into(),
            track: Track::Mic,
            wav_bytes: wav.clone(),
            duration_ms: 10,
            language: Some("en".into()),
            custom_vocabulary: vec!["ThreeStrands".into(), "CoreAudio".into()],
            prompt: Some("Sync".into()),
            diarize: true,
        };
        let mic_payload = build_openrouter_payload(&mic_req);
        assert_eq!(mic_payload["model"], "microsoft/mai-transcribe-2");
        assert_eq!(mic_payload["input_audio"]["format"], "wav");
        assert!(mic_payload.get("diarize").is_none());
        assert_eq!(
            mic_payload["provider"]["options"]["azure"]["diarization"]["enabled"],
            false
        );
        assert_eq!(
            mic_payload["provider"]["options"]["azure"]["enhancedMode"]["modelOptions"]
                ["transcribeStyle"],
            "clean"
        );
        assert_eq!(
            mic_payload["provider"]["options"]["azure"]["phraseList"]["phrases"],
            json!(["ThreeStrands", "CoreAudio"])
        );

        let sys_req = CloudChunkRequest {
            track: Track::System,
            ..mic_req
        };
        let sys_payload = build_openrouter_payload(&sys_req);
        assert_eq!(sys_payload["diarize"], true);
        assert_eq!(
            sys_payload["provider"]["options"]["azure"]["diarization"]["enabled"],
            true
        );
    }

    #[test]
    fn diarization_normalizes_remote_speakers_and_never_labels_mic() {
        let body = r#"{
            "segments": [
                {"start": 0.5, "end": 2.0, "text": "Hello team.", "speaker": "spk_alpha"},
                {"start": 2.5, "end": 4.0, "text": "Ready to start?", "speaker": "spk_beta"},
                {"start": 4.2, "end": 5.5, "text": "Sounds good.", "speaker": "spk_alpha"}
            ],
            "usage": {"cost": 0.0012, "prompt_tokens": 10, "completion_tokens": 12}
        }"#;
        let sys =
            parse_cloud_transcription_response(body, Track::System, 1_000, 6_000, true).unwrap();
        assert_eq!(sys.lines.len(), 3);
        assert_eq!(sys.lines[0].start_ms, 1_500);
        assert_eq!(sys.lines[0].speaker_label.as_deref(), Some("Speaker 1"));
        assert_eq!(sys.lines[1].speaker_label.as_deref(), Some("Speaker 2"));
        assert_eq!(sys.lines[2].speaker_label.as_deref(), Some("Speaker 1"));
        assert_eq!(sys.cost_usd, Some(0.0012));

        let mic = parse_cloud_transcription_response(body, Track::Mic, 0, 6_000, true).unwrap();
        assert!(mic.lines.iter().all(|l| l.speaker_label.is_none()));
    }

    #[test]
    fn cloud_orchestrator_enforces_daily_budget_and_merges_tracks() {
        let dir = TempDir::new();
        let mut mic = SegmentWriter::open(dir.path(), Track::Mic).unwrap();
        mic.write(&samples_for_ms(60_000, 2_000), 0).unwrap();
        drop(mic);
        let mut sys = SegmentWriter::open(dir.path(), Track::System).unwrap();
        sys.write(&samples_for_ms(60_000, 2_000), 0).unwrap();
        drop(sys);

        struct FakeCloud(Mutex<Vec<Track>>);
        impl CloudTranscriptionBackend for FakeCloud {
            fn transcribe_chunk(
                &self,
                req: &CloudChunkRequest,
            ) -> Result<CloudChunkResponse, String> {
                self.0.lock().unwrap().push(req.track);
                Ok(CloudChunkResponse {
                    lines: vec![CloudSegmentLine {
                        start_ms: 0,
                        end_ms: 1_000,
                        text: match req.track {
                            Track::Mic => "I will send the ThreeStrands notes.".into(),
                            Track::System => "Please review QBR metrics.".into(),
                        },
                        speaker_label: (req.track == Track::System).then(|| "Speaker 1".into()),
                    }],
                    cost_usd: Some(0.0015),
                    input_tokens: 0,
                    output_tokens: 0,
                    raw_body: "{}".into(),
                })
            }
        }

        // Tiny budget below 1-minute MAI-Transcribe-2 cost is refused before upload.
        let tight = RequestRuntime::new();
        tight
            .configure(RequestPolicy {
                daily_budget_usd: Some(0.0005),
            })
            .unwrap();
        let fake = FakeCloud(Mutex::new(Vec::new()));
        let err = transcribe_recording_cloud(
            &fake,
            &tight,
            CloudJobInput {
                recording_id: "r1",
                title: "Weekly Sync",
                attendees: &[],
                audio_dir: dir.path(),
                provider: AiProvider::OpenRouter,
                model: policy::CLOUD_DEFAULT_MODEL_OPENROUTER,
                endpoint: None,
                api_key: "key",
                language: None,
                custom_vocabulary: &["ThreeStrands".into()],
                diarize: true,
                headphones: false,
            },
            None,
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, JobError::Failed(ref msg) if msg.contains("budget")));
        assert!(fake.0.lock().unwrap().is_empty());

        // Generous budget succeeds, keeps tracks separate, and reconciles reported cost.
        let open = RequestRuntime::new();
        open.configure(RequestPolicy {
            daily_budget_usd: Some(1.0),
        })
        .unwrap();
        let rows = transcribe_recording_cloud(
            &fake,
            &open,
            CloudJobInput {
                recording_id: "r1",
                title: "Weekly Sync",
                attendees: &[],
                audio_dir: dir.path(),
                provider: AiProvider::OpenRouter,
                model: policy::CLOUD_DEFAULT_MODEL_OPENROUTER,
                endpoint: None,
                api_key: "key",
                language: None,
                custom_vocabulary: &["ThreeStrands".into()],
                diarize: true,
                headphones: false,
            },
            None,
            |_| {},
        )
        .unwrap();
        assert_eq!(*fake.0.lock().unwrap(), vec![Track::Mic, Track::System]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].track, Track::Mic);
        assert_eq!(rows[0].speaker_label, None);
        assert_eq!(rows[1].track, Track::System);
        assert_eq!(rows[1].speaker_label.as_deref(), Some("Speaker 1"));
        assert!((open.snapshot().unwrap().charged_usd - 0.003).abs() < 1e-12);
    }

    #[tokio::test]
    async fn http_client_handles_custom_openai_multipart_over_local_axum() {
        async fn respond(mut multipart: Multipart) -> Json<serde_json::Value> {
            let mut saw_file = false;
            let mut model = String::new();
            while let Some(field) = multipart.next_field().await.unwrap() {
                if field.name() == Some("file") {
                    let bytes = field.bytes().await.unwrap();
                    saw_file = bytes.starts_with(b"RIFF");
                } else if field.name() == Some("model") {
                    model = field.text().await.unwrap();
                }
            }
            assert!(saw_file);
            assert_eq!(model, "gpt-4o-mini-transcribe");
            Json(json!({
                "segments": [{"start": 0.0, "end": 1.2, "text": "Multipart works."}]
            }))
        }

        let app = Router::new().route("/audio/transcriptions", post(respond));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let resp = HttpCloudTranscriber::new()
            .transcribe_chunk_async(&CloudChunkRequest {
                provider: AiProvider::Custom,
                model: "gpt-4o-mini-transcribe".into(),
                endpoint: Some(endpoint),
                api_key: "test-key".into(),
                track: Track::Mic,
                wav_bytes: samples_to_wav_bytes(&[0.1; 160]),
                duration_ms: 10,
                language: Some("en".into()),
                custom_vocabulary: Vec::new(),
                prompt: Some("Sync".into()),
                diarize: false,
            })
            .await
            .unwrap();
        server.abort();
        assert_eq!(resp.lines.len(), 1);
        assert_eq!(resp.lines[0].text, "Multipart works.");
    }
}
