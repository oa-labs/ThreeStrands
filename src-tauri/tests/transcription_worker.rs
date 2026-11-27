//! Runs the real binary in transcription worker mode, the way the app
//! spawns it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn worker() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_threestrands"));
    command
        .arg("--threestrands-transcription-worker")
        .env_clear()
        .env("THREESTRANDS_TRANSCRIPTION_WORKER", "1")
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn frame(json: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(json).unwrap();
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(&body);
    bytes
}

fn read_frame(out: &mut impl Read) -> Option<serde_json::Value> {
    let mut prefix = [0u8; 4];
    out.read_exact(&mut prefix).ok()?;
    let mut body = vec![0u8; u32::from_be_bytes(prefix) as usize];
    out.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn config(model: &str, vad: &str, language: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "type": "config",
        "modelPath": model,
        "vadModelPath": vad,
        "language": language,
        "threads": 4,
    })
}

#[test]
fn the_binary_refuses_a_model_it_cannot_load() {
    let mut child = worker().spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&frame(&config(
            "/nonexistent/model.bin",
            "/nonexistent/vad.bin",
            Some("en"),
        )))
        .unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    assert_eq!(
        child.wait().unwrap().code(),
        Some(21),
        "the model exit code"
    );
    assert!(out.is_empty(), "never ready");
}

#[test]
fn the_binary_rejects_a_stream_that_does_not_start_with_config() {
    let mut child = worker().spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"\0\0\0\x02{}")
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(20));
}

/// The real model, end to end. Needs the models downloaded first (run the
/// ignored `smoke_download_fast_model` unit test with the same directory)
/// and a 16 kHz mono 16-bit WAV of speech:
///
/// ```text
/// THREESTRANDS_SMOKE_MODELS=/path/to/models THREESTRANDS_SMOKE_WAV=/path/to/mic-0001.wav \
///   cargo test --test transcription_worker -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn smoke_transcribes_speech_with_the_fast_model() {
    let models = std::path::PathBuf::from(std::env::var("THREESTRANDS_SMOKE_MODELS").unwrap());
    let wav = std::env::var("THREESTRANDS_SMOKE_WAV").unwrap();
    let audio_bytes = std::fs::metadata(&wav).unwrap().len() - 44;
    let audio_seconds = audio_bytes as f64 / 32_000.0;
    let started = std::time::Instant::now();
    let mut child = worker().spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    stdin
        .write_all(&frame(&config(
            models.join("ggml-small.en-q5_1.bin").to_str().unwrap(),
            models.join("ggml-silero-v5.1.2.bin").to_str().unwrap(),
            Some("en"),
        )))
        .unwrap();
    assert_eq!(read_frame(&mut stdout).unwrap()["type"], "ready");
    let loaded = started.elapsed();
    stdin
        .write_all(&frame(&serde_json::json!({
            "type": "segment",
            "track": "mic",
            "wavPath": wav,
            "startMs": 0,
            "continues": false,
            "endsRun": true,
        })))
        .unwrap();
    let reply = read_frame(&mut stdout).unwrap();
    let elapsed = started.elapsed();
    drop(stdin);
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert_eq!(reply["type"], "lines");
    let lines = reply["lines"].as_array().unwrap();
    for line in lines {
        println!(
            "[{:>6} → {:>6}] {}",
            line["startMs"], line["endMs"], line["text"]
        );
    }
    let transcribing = (elapsed - loaded).as_secs_f64();
    println!(
        "audio {audio_seconds:.1} s, load {:.2} s, transcribe {transcribing:.2} s, total {:.2} s, RTF {:.3} (with load {:.3})",
        loaded.as_secs_f64(),
        elapsed.as_secs_f64(),
        transcribing / audio_seconds,
        elapsed.as_secs_f64() / audio_seconds,
    );
    assert!(!lines.is_empty());
}
