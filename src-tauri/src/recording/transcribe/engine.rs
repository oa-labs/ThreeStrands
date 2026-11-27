//! whisper.cpp behind [`SpeechEngine`], inside the worker process only.
//!
//! Follows what the stage 0 spike measured:
//!
//! - Voice activity detection runs as its own step through
//!   `WhisperVadContext`. whisper-rs 0.16's `FullParams::enable_vad` does
//!   nothing (`WhisperState::full` calls `whisper_full_with_state`, which
//!   skips VAD) and panics without a model path, so it is never used.
//!   Padding is applied by [`super::spans`] rather than whisper.cpp, which
//!   pads both sides equally.
//! - Timestamp tokens stay on: spans are at most 30 s, where they drop
//!   nothing, and they split each span into sentences for the echo merge.
//! - Flash attention stays off; it crashes on macOS (anarlog).

use super::spans::{ms_to_samples, Span};
use super::stream::SpeechEngine;
use super::worker::WorkerConfig;
use crate::recording::policy;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperVadContext, WhisperVadContextParams, WhisperVadParams,
};

pub struct WhisperEngine {
    // Dropped before the context it was made from.
    state: WhisperState,
    _context: WhisperContext,
    vad: WhisperVadContext,
    language: Option<String>,
    threads: i32,
}

impl WhisperEngine {
    pub fn open(config: &WorkerConfig) -> Result<Self, String> {
        let model = config
            .model_path
            .to_str()
            .ok_or("The model path isn't valid UTF-8.")?;
        let vad_model = config
            .vad_model_path
            .to_str()
            .ok_or("The voice activity model path isn't valid UTF-8.")?;
        let threads = (config.threads.max(1) as i32).min(policy::WORKER_MAX_THREADS as i32);
        let mut parameters = WhisperContextParameters::default();
        parameters.use_gpu(true);
        parameters.flash_attn(false);
        let context =
            WhisperContext::new_with_params(model, parameters).map_err(|e| e.to_string())?;
        let state = context.create_state().map_err(|e| e.to_string())?;
        let mut vad_parameters = WhisperVadContextParams::default();
        vad_parameters.set_n_threads(1);
        vad_parameters.set_use_gpu(false);
        let vad = WhisperVadContext::new(vad_model, vad_parameters).map_err(|e| e.to_string())?;
        Ok(Self {
            state,
            _context: context,
            vad,
            language: config.language.clone(),
            threads,
        })
    }
}

/// Voice activity settings. Padding is zero here and added by
/// [`super::spans::plan_spans`]; the longest speech leaves room for it.
fn vad_parameters() -> WhisperVadParams {
    let mut parameters = WhisperVadParams::new();
    parameters.set_threshold(policy::VAD_START_THRESHOLD);
    parameters.set_min_speech_duration(policy::VAD_MIN_SPEECH_MS as i32);
    parameters.set_min_silence_duration(policy::VAD_MIN_SILENCE_MS as i32);
    let longest = policy::VAD_MAX_SPAN_MS - policy::VAD_PAD_BEFORE_MS - policy::VAD_PAD_AFTER_MS;
    parameters.set_max_speech_duration(longest as f32 / 1000.0);
    parameters.set_speech_pad(0);
    parameters.set_samples_overlap(0.0);
    parameters
}

/// Centiseconds (whisper.cpp's unit) to a sample index.
fn centiseconds_to_samples(centiseconds: f32) -> usize {
    (centiseconds.max(0.0) as f64 / 100.0 * policy::OUTPUT_SAMPLE_RATE as f64) as usize
}

impl SpeechEngine for WhisperEngine {
    fn detect(&mut self, samples: &[f32]) -> Result<Vec<Span>, String> {
        // Silero needs some audio to judge; a sliver is never speech.
        if samples.len() < ms_to_samples(policy::VAD_MIN_SPEECH_MS) {
            return Ok(Vec::new());
        }
        let segments = self
            .vad
            .segments_from_samples(vad_parameters(), samples)
            .map_err(|e| e.to_string())?;
        Ok(segments
            .map(|segment| {
                (
                    centiseconds_to_samples(segment.start).min(samples.len()),
                    centiseconds_to_samples(segment.end).min(samples.len()),
                )
            })
            .filter(|(start, end)| end > start)
            .collect())
    }

    fn transcribe(&mut self, samples: &[f32]) -> Result<Vec<(u64, u64, String)>, String> {
        let mut parameters = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: policy::WHISPER_BEAM_SIZE,
            patience: -1.0,
        });
        parameters.set_language(Some(self.language.as_deref().unwrap_or("auto")));
        parameters.set_n_threads(self.threads);
        parameters.set_print_progress(false);
        parameters.set_print_realtime(false);
        parameters.set_print_special(false);
        parameters.set_print_timestamps(false);
        parameters.set_suppress_blank(true);
        parameters.set_no_speech_thold(policy::WHISPER_NO_SPEECH_THRESHOLD);
        parameters.set_no_timestamps(false);
        // Each span stands alone, so a misheard span can't seed the next.
        parameters.set_no_context(true);
        self.state
            .full(parameters, samples)
            .map_err(|e| e.to_string())?;
        let mut sentences = Vec::new();
        for segment in self.state.as_iter() {
            let text = segment.to_str_lossy().map_err(|e| e.to_string())?;
            let start = segment.start_timestamp().max(0) as u64 * 10;
            let end = segment.end_timestamp().max(0) as u64 * 10;
            sentences.push((start, end.max(start), text.into_owned()));
        }
        Ok(sentences)
    }
}
