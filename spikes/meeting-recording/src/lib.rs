//! Shared pieces of the meeting-recording spike: a track writer that turns
//! device audio into 16 kHz mono 16-bit WAV segments, and level statistics.
//! THROWAWAY: see README.md. Production code belongs in `src-tauri`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

#[cfg(target_os = "macos")]
pub mod tap;

pub const TARGET_RATE: u32 = 16_000;
pub const SEGMENT_SECONDS: u32 = 60;
const CHUNK: usize = 1024;

/// Per-track counters printed while recording and in the final report.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub device_frames: u64,
    pub written_frames: u64,
    pub callbacks: u64,
    /// Longest wall-clock gap between two callbacks, in milliseconds.
    pub max_gap_ms: u128,
    pub sum_sq: f64,
    pub peak: f32,
    pub segments: u32,
}

impl Stats {
    pub fn rms_dbfs(&self) -> f64 {
        if self.written_frames == 0 || self.sum_sq == 0.0 {
            return f64::NEG_INFINITY;
        }
        10.0 * (self.sum_sq / self.written_frames as f64).log10()
    }
}

/// Downmixes interleaved device audio to mono, resamples it with ONE
/// persistent `rubato` resampler (meetily found a resampler per chunk raised
/// levels by ~173%), and writes 60-second WAV segments whose headers are
/// finalized when each segment closes.
pub struct TrackWriter {
    name: String,
    dir: PathBuf,
    channels: usize,
    resampler: Option<Fft<f32>>,
    pending: Vec<f32>,
    writer: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    segment_frames: u32,
    last_callback: Option<Instant>,
    pub stats: Stats,
}

impl TrackWriter {
    pub fn new(name: &str, dir: &Path, device_rate: u32, channels: usize) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let resampler = if device_rate == TARGET_RATE {
            None
        } else {
            Some(
                Fft::<f32>::new(
                    device_rate as usize,
                    TARGET_RATE as usize,
                    CHUNK,
                    1,
                    FixedSync::Input,
                )
                .context("resampler")?,
            )
        };
        Ok(Self {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            channels: channels.max(1),
            resampler,
            pending: Vec::new(),
            writer: None,
            segment_frames: 0,
            last_callback: None,
            stats: Stats::default(),
        })
    }

    /// Accepts one device callback's interleaved samples.
    pub fn push(&mut self, interleaved: &[f32]) -> Result<()> {
        let now = Instant::now();
        if let Some(last) = self.last_callback {
            self.stats.max_gap_ms = self
                .stats
                .max_gap_ms
                .max(now.duration_since(last).as_millis());
        }
        self.last_callback = Some(now);
        self.stats.callbacks += 1;
        let frames = interleaved.len() / self.channels;
        self.stats.device_frames += frames as u64;
        for frame in interleaved.chunks_exact(self.channels) {
            self.pending
                .push(frame.iter().sum::<f32>() / self.channels as f32);
        }
        self.drain(false)
    }

    fn drain(&mut self, flush: bool) -> Result<()> {
        let Some(mut resampler) = self.resampler.take() else {
            let out = std::mem::take(&mut self.pending);
            return self.write(&out);
        };
        let result = (|| -> Result<()> {
            loop {
                let need = resampler.input_frames_next();
                if self.pending.len() < need && !(flush && !self.pending.is_empty()) {
                    break;
                }
                let take = need.min(self.pending.len());
                let mut chunk: Vec<f32> = self.pending.drain(..take).collect();
                chunk.resize(need, 0.0);
                let input = InterleavedSlice::new(&chunk[..], 1, need)?;
                let out = resampler.process(&input, None)?.take_data();
                self.write(&out)?;
                if flush && self.pending.is_empty() {
                    break;
                }
            }
            Ok(())
        })();
        self.resampler = Some(resampler);
        result
    }

    fn write(&mut self, mono16k: &[f32]) -> Result<()> {
        for &sample in mono16k {
            if self.writer.is_none() {
                self.stats.segments += 1;
                let path = self
                    .dir
                    .join(format!("{}-{:03}.wav", self.name, self.stats.segments));
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: TARGET_RATE,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                self.writer = Some(hound::WavWriter::create(path, spec)?);
                self.segment_frames = 0;
            }
            let clamped = sample.clamp(-1.0, 1.0);
            self.stats.sum_sq += (clamped as f64) * (clamped as f64);
            self.stats.peak = self.stats.peak.max(clamped.abs());
            self.stats.written_frames += 1;
            let writer = self.writer.as_mut().expect("open segment");
            writer.write_sample((clamped * i16::MAX as f32) as i16)?;
            self.segment_frames += 1;
            if self.segment_frames >= TARGET_RATE * SEGMENT_SECONDS {
                self.writer.take().expect("open segment").finalize()?;
            }
        }
        if let Some(writer) = self.writer.as_mut() {
            writer.flush()?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Stats> {
        self.drain(true)?;
        if let Some(writer) = self.writer.take() {
            writer.finalize()?;
        }
        Ok(self.stats)
    }
}

/// Reads a 16 kHz mono WAV as f32 samples.
pub fn read_wav_16k(path: &Path) -> Result<Vec<f32>> {
    let reader =
        hound::WavReader::open(path).with_context(|| format!("open {}", path.display()))?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.sample_rate == TARGET_RATE && spec.channels == 1,
        "{}: expected 16 kHz mono",
        path.display()
    );
    Ok(match spec.sample_format {
        hound::SampleFormat::Float => reader.into_samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => reader
            .into_samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<Result<_, _>>()?,
    })
}
