//! A streaming windowed-sinc resampler from any device rate to the 16 kHz
//! that recordings are written at and whisper reads.
//!
//! **Filter.** A Kaiser-windowed sinc low-pass:
//! - cutoff [`CUTOFF_HZ`] (7.5 kHz), with a [`TRANSITION_HZ`] (1 kHz) wide
//!   transition band centered on it, so the passband is flat to 7 kHz and
//!   the stopband begins at 8 kHz, the output's Nyquist frequency. Speech
//!   carries little above 7 kHz, and anything at or above 8 kHz would
//!   otherwise alias back into the band whisper listens to.
//! - [`STOPBAND_DB`] (90 dB) of stopband attenuation, giving Kaiser
//!   β = 0.1102 × (90 − 8.7) ≈ 8.96.
//! - Half-width H = ⌈(A − 7.95) / (2.285 · 2π · Δf / f_in) / 2⌉ input
//!   samples, from Kaiser's length estimate: 138 at 48 kHz, 127 at
//!   44.1 kHz, and 275 at 96 kHz, so 2H taps per output sample.
//!
//! The kernel is tabulated at [`TABLE_STEPS`] points per input sample and
//! read with linear interpolation, so any ratio works without a polyphase
//! bank per ratio.
//!
//! **Timing.** Output sample `n` is centered on input position
//! `n × f_in / f_out`, computed exactly in integers, so there is no
//! group delay and no accumulated rounding over a long recording. Input
//! before the first sample counts as silence.
//!
//! **Streaming.** One instance runs per stream for a whole run of
//! contiguous audio. meetily found that a new resampler per chunk raised the
//! volume by about 173%. Output never depends on how the input was chunked:
//! feeding a run in any chunk sizes gives the same samples as feeding it at
//! once. [`Resampler::finish`] pads the end with silence to emit the last
//! samples.

/// Center of the transition band, in hertz.
pub const CUTOFF_HZ: f64 = 7_500.0;
/// Width of the transition band, in hertz.
pub const TRANSITION_HZ: f64 = 1_000.0;
/// Stopband attenuation, in decibels.
pub const STOPBAND_DB: f64 = 90.0;
/// Kernel table points per input sample.
pub const TABLE_STEPS: usize = 256;

pub struct Resampler {
    in_rate: u64,
    out_rate: u64,
    /// Kernel half-width, in input samples.
    half: usize,
    /// The kernel at τ = j / TABLE_STEPS for j in 0..=half × TABLE_STEPS,
    /// plus one zero so interpolation at the edge reads in bounds.
    table: Vec<f32>,
    /// Input not yet consumed; `buf[0]` is input sample `buf_start`.
    buf: Vec<f32>,
    buf_start: u64,
    /// Input samples received in this run.
    received: u64,
    /// Output samples produced in this run.
    produced: u64,
}

fn bessel_i0(x: f64) -> f64 {
    // The power series converges quickly for the β used here.
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..64 {
        term *= half / k as f64;
        let add = term * term;
        sum += add;
        if add < sum * 1e-17 {
            break;
        }
    }
    sum
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// The kernel half-width in input samples for `in_rate`.
pub fn half_width(in_rate: u32) -> usize {
    let delta_omega = 2.0 * std::f64::consts::PI * TRANSITION_HZ / in_rate as f64;
    let order = (STOPBAND_DB - 7.95) / (2.285 * delta_omega);
    (order / 2.0).ceil() as usize
}

impl Resampler {
    /// A resampler from `in_rate` to `out_rate`. When they are equal it
    /// copies input through unchanged.
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        assert!(in_rate > 0 && out_rate > 0, "sample rates must be positive");
        let passthrough = in_rate == out_rate;
        let half = if passthrough { 0 } else { half_width(in_rate) };
        let mut table = Vec::new();
        if !passthrough {
            // The cutoff is capped below the input's own Nyquist frequency
            // for inputs slower than 16 kHz, which then only interpolate.
            let cutoff = CUTOFF_HZ.min(0.45 * in_rate as f64);
            let fc = cutoff / in_rate as f64;
            let beta = 0.1102 * (STOPBAND_DB - 8.7);
            let norm = bessel_i0(beta);
            let points = half * TABLE_STEPS;
            table.reserve(points + 2);
            for j in 0..=points {
                let tau = j as f64 / TABLE_STEPS as f64;
                let x = tau / half as f64;
                let window = bessel_i0(beta * (1.0 - x * x).max(0.0).sqrt()) / norm;
                table.push((2.0 * fc * sinc(2.0 * fc * tau) * window) as f32);
            }
            table.push(0.0);
        }
        Self {
            in_rate: in_rate as u64,
            out_rate: out_rate as u64,
            half,
            table,
            buf: Vec::new(),
            buf_start: 0,
            received: 0,
            produced: 0,
        }
    }

    /// Output samples produced since the run began.
    pub fn produced(&self) -> u64 {
        self.produced
    }

    /// Input position, in input samples since the run began, that output
    /// sample `n` is centered on.
    pub fn input_position(&self, n: u64) -> f64 {
        (n as f64) * self.in_rate as f64 / self.out_rate as f64
    }

    fn kernel(&self, tau: f64) -> f64 {
        let a = tau.abs() * TABLE_STEPS as f64;
        let index = a as usize;
        if index + 1 >= self.table.len() {
            return 0.0;
        }
        let frac = a - index as f64;
        self.table[index] as f64 * (1.0 - frac) + self.table[index + 1] as f64 * frac
    }

    /// Feeds input samples and appends every output sample that is now
    /// complete to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.half == 0 {
            out.extend_from_slice(input);
            self.received += input.len() as u64;
            self.produced += input.len() as u64;
            return;
        }
        self.buf.extend_from_slice(input);
        self.received += input.len() as u64;
        self.emit(false, out);
    }

    /// Ends the run: treats the input as followed by silence, appends the
    /// remaining output samples, and resets for a new run.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.half > 0 {
            self.emit(true, out);
        }
        self.buf.clear();
        self.buf_start = 0;
        self.received = 0;
        self.produced = 0;
    }

    fn emit(&mut self, finishing: bool, out: &mut Vec<f32>) {
        let half = self.half as i64;
        loop {
            let n = self.produced;
            let numerator = n as u128 * self.in_rate as u128;
            // Output n is centered on input t = center + frac.
            let center = (numerator / self.out_rate as u128) as i64;
            if finishing {
                // Every output centered inside the input is emitted.
                if numerator >= self.received as u128 * self.out_rate as u128 {
                    break;
                }
            } else if center + half >= self.received as i64 {
                break;
            }
            let frac = (numerator % self.out_rate as u128) as f64 / self.out_rate as f64;
            let mut sum = 0.0f64;
            for m in -half..half {
                let k = center - m;
                if k < self.buf_start as i64 {
                    // Before the run began (silence) or already trimmed,
                    // which only happens for k < 0.
                    continue;
                }
                let Some(&x) = self.buf.get((k - self.buf_start as i64) as usize) else {
                    continue; // after the end, while finishing
                };
                sum += x as f64 * self.kernel(frac + m as f64);
            }
            out.push(sum as f32);
            self.produced += 1;
        }
        // Keep only input the next output still needs.
        let next_center =
            ((self.produced as u128 * self.in_rate as u128) / self.out_rate as u128) as i64;
        let keep_from = (next_center - half + 1).max(0) as u64;
        if keep_from > self.buf_start {
            let drop = ((keep_from - self.buf_start) as usize).min(self.buf.len());
            self.buf.drain(..drop);
            self.buf_start += drop as u64;
        }
    }
}

/// Converts a sample in −1.0..=1.0 to 16-bit PCM, clamping overs.
pub fn to_i16(sample: f32) -> i16 {
    let scaled = (sample.clamp(-1.0, 1.0) * 32767.0).round();
    scaled as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: u32 = 16_000;

    fn sine(rate: u32, hz: f64, seconds: f64) -> Vec<f32> {
        let count = (rate as f64 * seconds) as usize;
        (0..count)
            .map(|i| {
                (0.5 * (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin()) as f32
            })
            .collect()
    }

    fn one_shot(rate: u32, input: &[f32]) -> Vec<f32> {
        let mut resampler = Resampler::new(rate, OUT);
        let mut out = Vec::new();
        resampler.process(input, &mut out);
        resampler.finish(&mut out);
        out
    }

    /// RMS of the middle of a signal, away from the edges' ramps.
    fn rms_middle(signal: &[f32]) -> f64 {
        let middle = &signal[signal.len() / 4..signal.len() * 3 / 4];
        (middle.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / middle.len() as f64).sqrt()
    }

    /// Amplitude of `hz` in a 16 kHz signal's middle, by correlation.
    fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
        let start = signal.len() / 4;
        let middle = &signal[start..signal.len() * 3 / 4];
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in middle.iter().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * hz * (start + i) as f64 / OUT as f64;
            re += x as f64 * phase.cos();
            im += x as f64 * phase.sin();
        }
        2.0 * (re * re + im * im).sqrt() / middle.len() as f64
    }

    #[test]
    fn half_widths_follow_the_kaiser_estimate() {
        assert_eq!(half_width(48_000), 138);
        assert_eq!(half_width(44_100), 127);
        assert_eq!(half_width(96_000), 275);
    }

    #[test]
    fn a_tone_in_the_passband_keeps_its_frequency_and_level() {
        for rate in [48_000, 44_100, 96_000] {
            for hz in [440.0, 1_000.0, 3_000.0, 6_500.0] {
                let input = sine(rate, hz, 1.0);
                let out = one_shot(rate, &input);
                let expected_len = (input.len() as u64 * OUT as u64).div_ceil(rate as u64);
                assert_eq!(out.len() as u64, expected_len, "{rate} Hz");
                let amplitude = amplitude_at(&out, hz);
                assert!(
                    (amplitude - 0.5).abs() < 0.5 * 0.01,
                    "{hz} Hz from {rate} Hz came out at {amplitude}, not 0.5"
                );
                // Nearly all of the energy is at that frequency.
                let rms = rms_middle(&out);
                assert!(
                    (rms - 0.5 / 2f64.sqrt()).abs() < 0.005,
                    "{hz} Hz from {rate} Hz: rms {rms}"
                );
            }
        }
    }

    #[test]
    fn energy_above_8_khz_is_strongly_attenuated() {
        for rate in [48_000, 44_100, 96_000] {
            for hz in [8_200.0, 9_000.0, 12_000.0, 20_000.0] {
                let input = sine(rate, hz, 1.0);
                let out = one_shot(rate, &input);
                let ratio_db = 20.0 * (rms_middle(&out) / rms_middle(&input)).log10();
                assert!(
                    ratio_db < -70.0,
                    "{hz} Hz from {rate} Hz is only {ratio_db:.1} dB down"
                );
            }
        }
    }

    #[test]
    fn streaming_in_odd_chunks_equals_one_shot() {
        for rate in [48_000, 44_100, 96_000] {
            let mut input = sine(rate, 1_234.0, 0.7);
            for (i, sample) in input.iter_mut().enumerate() {
                *sample += 0.2 * (((i * 7919) % 101) as f32 / 101.0 - 0.5);
            }
            let expected = one_shot(rate, &input);
            for sizes in [&[1usize][..], &[7, 333, 2, 1021], &[4096], &[13, 1]] {
                let mut resampler = Resampler::new(rate, OUT);
                let mut out = Vec::new();
                let mut position = 0;
                let mut size = sizes.iter().cycle();
                while position < input.len() {
                    let end = (position + size.next().unwrap()).min(input.len());
                    resampler.process(&input[position..end], &mut out);
                    position = end;
                }
                resampler.finish(&mut out);
                assert_eq!(out, expected, "{rate} Hz in chunks of {sizes:?}");
            }
        }
    }

    #[test]
    fn finish_resets_for_a_new_run() {
        let input = sine(48_000, 500.0, 0.2);
        let mut resampler = Resampler::new(48_000, OUT);
        let mut first = Vec::new();
        resampler.process(&input, &mut first);
        resampler.finish(&mut first);
        assert_eq!(resampler.produced(), 0);
        let mut second = Vec::new();
        resampler.process(&input, &mut second);
        resampler.finish(&mut second);
        assert_eq!(first, second);
    }

    #[test]
    fn equal_rates_pass_through() {
        let input = sine(16_000, 1_000.0, 0.1);
        assert_eq!(one_shot(16_000, &input), input);
    }

    #[test]
    fn output_is_centered_without_delay() {
        // An impulse at input 4800 (100 ms at 48 kHz) peaks at output 1600.
        let mut input = vec![0.0f32; 9600];
        input[4800] = 1.0;
        let out = one_shot(48_000, &input);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, 1600);
    }

    #[test]
    fn i16_conversion_rounds_and_clamps() {
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-1.0), -32767);
        assert_eq!(to_i16(2.5), 32767);
        assert_eq!(to_i16(-3.0), -32767);
        assert_eq!(to_i16(0.5), 16384);
    }
}
