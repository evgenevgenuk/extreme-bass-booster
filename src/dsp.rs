//! Bass boost DSP.
//!
//! The algorithm: a one-pole low-pass filter extracts the low end of the
//! signal, which is amplified and mixed back on top of the original,
//! then the result is hard-clipped to [-1, 1].

use crate::audio::AudioData;

/// How strong the boost should be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoostMode {
    /// Boost controlled by the slider, 0..=400 percent.
    Slider(u32),
    /// Fixed, absurdly strong preset.
    Hyper,
}

impl BoostMode {
    /// Returns `true` if this setting is loud enough to deserve a warning.
    pub fn is_dangerous(self) -> bool {
        match self {
            BoostMode::Hyper => true,
            BoostMode::Slider(p) => p >= 300,
        }
    }

    /// (gain, layers, cutoff Hz)
    fn params(self) -> (f64, u32, f64) {
        match self {
            BoostMode::Hyper => (25.0, 10, 150.0),
            BoostMode::Slider(p) => (1.0 + p as f64 / 100.0 * 4.0, 1 + p / 80, 180.0),
        }
    }

    /// Short prefix used for output file names.
    pub fn file_prefix(self) -> &'static str {
        match self {
            BoostMode::Hyper => "hyper_",
            BoostMode::Slider(_) => "boost_",
        }
    }
}

/// Center frequencies of the graphic equalizer bands, in Hz.
pub const EQ_BANDS: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
/// Maximum boost or cut per band, in dB.
pub const EQ_RANGE_DB: f32 = 12.0;

/// 10-band graphic equalizer settings (gain per band in dB).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Equalizer {
    pub gains: [f32; 10],
}

impl Default for Equalizer {
    fn default() -> Self {
        Self { gains: [0.0; 10] }
    }
}

impl Equalizer {
    pub const PRESETS: &'static [(&'static str, [f32; 10])] = &[
        ("Flat", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ("Bass", [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        (
            "Sub Rumble",
            [10.0, 8.0, 3.0, 0.0, -2.0, -2.0, 0.0, 0.0, 0.0, 0.0],
        ),
        (
            "Vocal",
            [-3.0, -2.0, -1.0, 1.0, 3.0, 4.0, 3.0, 1.0, 0.0, -1.0],
        ),
        ("Treble", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0]),
        (
            "Rock",
            [4.0, 3.0, 1.0, -1.0, -2.0, -1.0, 1.0, 3.0, 4.0, 4.0],
        ),
        (
            "Electronic",
            [5.0, 4.0, 1.0, 0.0, -2.0, 1.0, 0.0, 2.0, 4.0, 5.0],
        ),
        (
            "Hip-Hop",
            [5.0, 4.0, 2.0, 3.0, -1.0, -1.0, 1.0, 0.0, 2.0, 3.0],
        ),
        (
            "Loudness",
            [6.0, 4.0, 0.0, 0.0, -2.0, 0.0, -1.0, 0.0, 4.0, 6.0],
        ),
    ];

    pub fn is_flat(&self) -> bool {
        self.gains.iter().all(|g| g.abs() < 0.05)
    }

    /// Returns the name of the preset matching the current gains, if any.
    pub fn preset_name(&self) -> Option<&'static str> {
        Self::PRESETS
            .iter()
            .find(|(_, g)| g.iter().zip(&self.gains).all(|(a, b)| (a - b).abs() < 0.05))
            .map(|(n, _)| *n)
    }
}

/// RBJ "Audio EQ Cookbook" peaking filter, normalised so a0 = 1.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    fn peaking(freq: f64, gain_db: f64, q: f64, rate: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

/// Applies the equalizer. Values are not clipped here; clipping happens later.
pub fn equalize(input: &AudioData, eq: &Equalizer) -> AudioData {
    let rate = input.sample_rate.max(1) as f64;
    let channels = input.channels.max(1);
    let filters: Vec<Biquad> = EQ_BANDS
        .iter()
        .zip(eq.gains)
        .filter(|(f, g)| g.abs() >= 0.05 && (**f as f64) < rate * 0.45)
        .map(|(f, g)| Biquad::peaking(*f as f64, g as f64, std::f64::consts::SQRT_2, rate))
        .collect();

    let mut samples = input.samples.clone();
    for f in &filters {
        // Direct form I state per channel: x1, x2, y1, y2
        let mut st = vec![[0.0f64; 4]; channels];
        for (i, s) in samples.iter_mut().enumerate() {
            let z = &mut st[i % channels];
            let x = *s as f64;
            let y = f.b0 * x + f.b1 * z[0] + f.b2 * z[1] - f.a1 * z[2] - f.a2 * z[3];
            *z = [x, z[0], y, z[2]];
            *s = y as f32;
        }
    }
    AudioData {
        samples,
        channels: input.channels,
        sample_rate: input.sample_rate,
    }
}

/// Full processing chain: equalizer, then bass boost and clipping.
pub fn process(input: &AudioData, mode: BoostMode, eq: &Equalizer) -> AudioData {
    if eq.is_flat() {
        boost(input, mode)
    } else {
        boost(&equalize(input, eq), mode)
    }
}

/// Applies the bass boost and returns a new buffer with the same layout.
pub fn boost(input: &AudioData, mode: BoostMode) -> AudioData {
    let (gain, layers, cutoff) = mode.params();
    let amount = gain * layers as f64;

    let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff);
    let dt = 1.0 / input.sample_rate.max(1) as f64;
    let alpha = dt / (rc + dt);

    let channels = input.channels.max(1);
    // One filter state per channel, so stereo channels don't bleed into each other.
    let mut state = vec![0.0f64; channels];

    let samples = input
        .samples
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let c = i % channels;
            let x = s as f64;
            state[c] += alpha * (x - state[c]);
            (x + state[c] * amount).clamp(-1.0, 1.0) as f32
        })
        .collect();

    AudioData {
        samples,
        channels: input.channels,
        sample_rate: input.sample_rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, secs: f64, rate: u32, amp: f32) -> AudioData {
        let n = (secs * rate as f64) as usize;
        let samples = (0..n)
            .map(|i| {
                amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32
            })
            .collect();
        AudioData {
            samples,
            channels: 1,
            sample_rate: rate,
        }
    }

    fn rms(s: &[f32]) -> f64 {
        (s.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    }

    #[test]
    fn boosts_lows_more_than_highs() {
        let low = sine(60.0, 1.0, 44_100, 0.05);
        let high = sine(5_000.0, 1.0, 44_100, 0.05);
        let mode = BoostMode::Slider(100);
        let low_gain = rms(&boost(&low, mode).samples) / rms(&low.samples);
        let high_gain = rms(&boost(&high, mode).samples) / rms(&high.samples);
        assert!(low_gain > 3.0, "low gain {low_gain}");
        assert!(high_gain < 1.3, "high gain {high_gain}");
    }

    #[test]
    fn equalizer_boosts_and_cuts_its_band() {
        let tone = sine(1000.0, 1.0, 44_100, 0.1);
        let mut eq = Equalizer::default();
        eq.gains[5] = 12.0; // 1 kHz
        let up = rms(&equalize(&tone, &eq).samples) / rms(&tone.samples);
        eq.gains[5] = -12.0;
        let down = rms(&equalize(&tone, &eq).samples) / rms(&tone.samples);
        assert!((up - 3.98).abs() < 0.3, "up {up}"); // +12 dB ~ x3.98
        assert!((down - 0.251).abs() < 0.03, "down {down}"); // -12 dB ~ x0.251

        let far = sine(60.0, 1.0, 44_100, 0.1);
        eq.gains[5] = 12.0;
        let far_gain = rms(&equalize(&far, &eq).samples) / rms(&far.samples);
        assert!((far_gain - 1.0).abs() < 0.1, "far {far_gain}");
    }

    #[test]
    fn output_is_clipped_and_same_length() {
        let a = sine(50.0, 0.5, 48_000, 0.9);
        let b = boost(&a, BoostMode::Hyper);
        assert_eq!(a.samples.len(), b.samples.len());
        assert!(b.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
    }
}
