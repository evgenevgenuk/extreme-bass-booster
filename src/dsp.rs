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
            .map(|i| amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32)
            .collect();
        AudioData { samples, channels: 1, sample_rate: rate }
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
    fn output_is_clipped_and_same_length() {
        let a = sine(50.0, 0.5, 48_000, 0.9);
        let b = boost(&a, BoostMode::Hyper);
        assert_eq!(a.samples.len(), b.samples.len());
        assert!(b.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
    }
}
