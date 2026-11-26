//! Records the microphone and system audio (process tap) for N seconds into
//! separate 16 kHz mono WAV segment tracks, printing levels each second.
//!
//!   capture <out-dir> [seconds] [--no-mic] [--no-tap]

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use meeting_recording_spike::TrackWriter;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(
        args.first()
            .context("usage: capture <out-dir> [seconds] [--no-mic] [--no-tap]")?,
    );
    let seconds: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let use_mic = !args.iter().any(|a| a == "--no-mic");
    let use_tap = !args.iter().any(|a| a == "--no-tap");

    // Microphone first, so this process has a Core Audio process object to
    // exclude from the tap.
    let (mic_tx, mic_rx) = mpsc::channel::<Vec<f32>>();
    let mut mic = None;
    let mut _mic_stream = None;
    if use_mic {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no default input device")?;
        let supported = device.default_input_config()?;
        let name = device
            .description()
            .map(|d| format!("{d:?}"))
            .unwrap_or_default();
        println!(
            "mic: {name} {:?} {} Hz x{}",
            supported.sample_format(),
            supported.sample_rate(),
            supported.channels()
        );
        let config = supported.config();
        let tx = mic_tx.clone();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
                config.clone(),
                move |data, _| {
                    let _ = tx.send(data.to_vec());
                },
                |e| eprintln!("mic error: {e}"),
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
                config.clone(),
                move |data, _| {
                    let _ = tx.send(data.iter().map(|&s| s as f32 / 32768.0).collect());
                },
                |e| eprintln!("mic error: {e}"),
                None,
            )?,
            other => anyhow::bail!("unsupported mic format {other:?}"),
        };
        stream.play()?;
        mic = Some(TrackWriter::new(
            "mic",
            &out,
            config.sample_rate,
            config.channels as usize,
        )?);
        _mic_stream = Some(stream);
        std::thread::sleep(Duration::from_millis(200));
    }

    let (tap_tx, tap_rx) = mpsc::channel::<Vec<f32>>();
    let mut system = None;
    #[cfg(target_os = "macos")]
    let mut _tap = None;
    #[cfg(target_os = "macos")]
    if use_tap {
        match meeting_recording_spike::tap::ProcessTap::start(tap_tx.clone()) {
            Ok(tap) => {
                println!(
                    "tap: {} Hz x{} (excluded own process: {})",
                    tap.sample_rate, tap.channels, tap.excluded_self
                );
                system = Some(TrackWriter::new(
                    "system",
                    &out,
                    tap.sample_rate as u32,
                    tap.channels as usize,
                )?);
                _tap = Some(tap);
            }
            Err(error) => println!("tap: unavailable: {error:#}"),
        }
    }
    let _ = tap_tx;

    let start = Instant::now();
    let mut next_report = Duration::from_secs(1);
    while start.elapsed() < Duration::from_secs(seconds) {
        while let Ok(chunk) = mic_rx.try_recv() {
            if let Some(w) = mic.as_mut() {
                w.push(&chunk)?;
            }
        }
        while let Ok(chunk) = tap_rx.try_recv() {
            if let Some(w) = system.as_mut() {
                w.push(&chunk)?;
            }
        }
        if start.elapsed() >= next_report {
            let level = |w: &Option<TrackWriter>| {
                w.as_ref()
                    .map(|w| {
                        format!(
                            "{:>6.1} dBFS {:>6.2}s written",
                            w.stats.rms_dbfs(),
                            w.stats.written_frames as f64 / 16_000.0
                        )
                    })
                    .unwrap_or("-".into())
            };
            println!(
                "{:>3}s  mic {}  system {}",
                next_report.as_secs(),
                level(&mic),
                level(&system)
            );
            next_report += Duration::from_secs(1);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    #[cfg(target_os = "macos")]
    drop(_tap);
    drop(_mic_stream);
    for (name, writer) in [("mic", mic), ("system", system)] {
        if let Some(writer) = writer {
            let s = writer.finish()?;
            println!(
                "{name}: device_frames={} written_16k={} ({:.2}s) callbacks={} max_gap_ms={} rms={:.1}dBFS peak={:.3} segments={}",
                s.device_frames,
                s.written_frames,
                s.written_frames as f64 / 16_000.0,
                s.callbacks,
                s.max_gap_ms,
                s.rms_dbfs(),
                s.peak,
                s.segments
            );
        }
    }
    Ok(())
}
