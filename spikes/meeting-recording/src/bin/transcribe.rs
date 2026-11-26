//! Transcribes 16 kHz mono WAV files with whisper.cpp (whisper-rs, Metal) and
//! Silero VAD, reporting timing, real-time factor, and word error rate
//! against an optional reference text.
//!
//!   transcribe --model <ggml.bin> [--vad <silero.bin>] [--ref <text>]
//!              [--no-gpu] [--timestamps] [--max-span <s>] [--tsv <out>] <wav>...
//!
//! Run under `/usr/bin/time -l` for peak memory.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use meeting_recording_spike::{read_wav_16k, TARGET_RATE};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadContext,
    WhisperVadContextParams, WhisperVadParams,
};

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Word error rate: word-level Levenshtein distance over reference length.
fn wer(reference: &[String], hypothesis: &[String]) -> f64 {
    let mut prev: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, r) in reference.iter().enumerate() {
        let mut row = vec![i + 1; hypothesis.len() + 1];
        for (j, h) in hypothesis.iter().enumerate() {
            row[j + 1] = (prev[j] + usize::from(r != h))
                .min(prev[j + 1] + 1)
                .min(row[j] + 1);
        }
        prev = row;
    }
    prev[hypothesis.len()] as f64 / reference.len().max(1) as f64
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut model, mut vad, mut reference, mut gpu, mut timestamps, mut wavs) =
        (None, None, None, true, false, vec![]);
    let mut max_span: f32 = 30.0;
    let mut tsv: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = args.next(),
            "--vad" => vad = args.next(),
            "--ref" => reference = args.next(),
            "--no-gpu" => gpu = false,
            "--timestamps" => timestamps = true,
            "--tsv" => tsv = args.next(),
            "--max-span" => max_span = args.next().and_then(|v| v.parse().ok()).unwrap_or(30.0),
            _ => wavs.push(PathBuf::from(arg)),
        }
    }
    let model = model.context("--model is required")?;

    let load = Instant::now();
    let mut cparams = WhisperContextParameters::default();
    cparams.use_gpu(gpu);
    // anarlog: flash attention crashes on macOS.
    cparams.flash_attn(false);
    let ctx = WhisperContext::new_with_params(&model, cparams)?;
    let mut state = ctx.create_state()?;
    let load_s = load.elapsed().as_secs_f64();

    let mut audio = Vec::new();
    for wav in &wavs {
        audio.extend(read_wav_16k(wav)?);
    }
    let audio_s = audio.len() as f64 / TARGET_RATE as f64;

    let make_params = || {
        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 5,
            patience: -1.0,
        });
        params.set_language(Some("en"));
        params.set_n_threads(
            std::thread::available_parallelism()
                .map(|n| n.get() as i32)
                .unwrap_or(4)
                .min(8),
        );
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_no_speech_thold(0.55);
        params.set_no_timestamps(!timestamps);
        params
    };

    // whisper.cpp applies `params.vad` only in `whisper_full`, and whisper-rs's
    // `WhisperState::full` calls `whisper_full_with_state`, so `enable_vad` is
    // silently ignored. Run Silero separately and transcribe each speech span,
    // taking timestamps from the span boundaries.
    let run = Instant::now();
    let mut vad_s = 0.0;
    let spans: Vec<(usize, usize)> = match vad.as_deref() {
        None => vec![(0, audio.len())],
        Some(vad) => {
            let t = Instant::now();
            let mut vctx_params = WhisperVadContextParams::default();
            vctx_params.set_n_threads(1);
            vctx_params.set_use_gpu(false);
            let mut vctx = WhisperVadContext::new(vad, vctx_params)?;
            let mut v = WhisperVadParams::new();
            v.set_threshold(0.5);
            v.set_min_speech_duration(250);
            v.set_min_silence_duration(500);
            v.set_max_speech_duration(max_span);
            v.set_speech_pad(300);
            let spans = vctx
                .segments_from_samples(v, &audio)?
                .map(|s| {
                    let a = ((s.start / 100.0) * TARGET_RATE as f32) as usize;
                    let b = ((s.end / 100.0) * TARGET_RATE as f32) as usize;
                    (a.min(audio.len()), b.min(audio.len()))
                })
                .filter(|(a, b)| b > a)
                .collect();
            vad_s = t.elapsed().as_secs_f64();
            spans
        }
    };
    let speech_s: f64 = spans.iter().map(|(a, b)| (b - a) as f64).sum::<f64>() / TARGET_RATE as f64;

    let mut text = String::new();
    let mut rows = String::new();
    for (a, b) in &spans {
        state.full(make_params(), &audio[*a..*b])?;
        let offset = *a as f64 / TARGET_RATE as f64;
        for segment in state.as_iter() {
            let line = segment.to_str_lossy()?.trim().to_string();
            let (start, end) = (
                offset + segment.start_timestamp() as f64 / 100.0,
                if timestamps {
                    offset + segment.end_timestamp() as f64 / 100.0
                } else {
                    *b as f64 / TARGET_RATE as f64
                },
            );
            println!("[{start:>7.2} → {end:>7.2}] {line}");
            rows.push_str(&format!(
                "{start:.2}\t{end:.2}\t{}\n",
                line.replace('\t', " ")
            ));
            text.push_str(&line);
            text.push(' ');
        }
    }
    let run_s = run.elapsed().as_secs_f64();
    if let Some(path) = tsv {
        std::fs::write(path, rows)?;
    }
    println!(
        "spans={} speech={:.1}s vad_time={:.2}s",
        spans.len(),
        speech_s,
        vad_s
    );
    println!(
        "model={} gpu={} vad={} audio={:.1}s load={:.2}s transcribe={:.2}s rtf={:.3}",
        PathBuf::from(&model)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        gpu,
        vad.is_some(),
        audio_s,
        load_s,
        run_s,
        run_s / audio_s.max(0.001)
    );
    if let Some(reference) = reference {
        let reference = std::fs::read_to_string(reference)?;
        println!("wer={:.3}", wer(&words(&reference), &words(&text)));
    }
    Ok(())
}
