//! Decoding (via Symphonia) and WAV encoding (via hound).

use std::fs::File;
use std::io::{Cursor, Seek, Write};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// File extensions the app will try to open.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "mp3", "mp2", "mp1", "wav", "wave", "flac", "ogg", "oga", "m4a", "m4b", "mp4", "aac", "alac",
    "aif", "aiff", "aifc", "caf", "mka", "mkv", "webm", "adpcm",
];

/// Decoded interleaved PCM audio in the range [-1, 1].
#[derive(Clone, Debug)]
pub struct AudioData {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub sample_rate: u32,
}

impl AudioData {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }
}

pub fn is_supported(name: &str) -> bool {
    extension_of(name)
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.as_str()))
        .unwrap_or(false)
}

pub fn extension_of(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Decodes an audio file from disk.
pub fn decode_file(path: &Path) -> Result<AudioData> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let ext = extension_of(&path.to_string_lossy());
    decode(Box::new(file), ext.as_deref())
}

/// Decodes an audio file that is already in memory (e.g. from a ZIP entry).
pub fn decode_bytes(bytes: Vec<u8>, ext: Option<&str>) -> Result<AudioData> {
    decode(Box::new(Cursor::new(bytes)), ext)
}

fn decode(source: Box<dyn MediaSource>, ext: Option<&str>) -> Result<AudioData> {
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(e) = ext {
        hint.with_extension(e);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| anyhow!("unsupported or corrupted file: {e}"))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("no audio track found"))?;
    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate;
    let mut channels = track.codec_params.channels.map(|c| c.count());

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| anyhow!("unsupported codec: {e}"))?;

    let mut samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(anyhow!("read error: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let mut b = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                b.copy_interleaved_ref(decoded);
                samples.extend_from_slice(b.samples());
                sample_rate = Some(spec.rate);
                channels = Some(spec.channels.count());
            }
            // Skip broken frames instead of failing the whole file.
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(anyhow!("decode error: {e}")),
        }
    }

    let channels = channels.ok_or_else(|| anyhow!("unknown channel count"))?;
    let sample_rate = sample_rate.ok_or_else(|| anyhow!("unknown sample rate"))?;
    if samples.is_empty() {
        return Err(anyhow!("file contains no audio"));
    }
    Ok(AudioData { samples, channels, sample_rate })
}

fn write_wav_to<W: Write + Seek>(audio: &AudioData, out: W) -> Result<()> {
    let spec = hound::WavSpec {
        channels: audio.channels as u16,
        sample_rate: audio.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::new(out, spec)?;
    for &s in &audio.samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
    }
    w.finalize()?;
    Ok(())
}

/// Saves audio as a 16-bit PCM WAV file.
pub fn save_wav(audio: &AudioData, path: &Path) -> Result<()> {
    let file = std::io::BufWriter::new(
        File::create(path).with_context(|| format!("cannot create {}", path.display()))?,
    );
    write_wav_to(audio, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_roundtrip() {
        let a = AudioData { samples: vec![0.0, 0.5, -0.5, 0.25], channels: 2, sample_rate: 22_050 };
        let b = decode_bytes(wav_bytes(&a).unwrap(), Some("wav")).unwrap();
        assert_eq!((b.channels, b.sample_rate, b.samples.len()), (2, 22_050, 4));
        assert!((b.samples[1] - 0.5).abs() < 1e-3);
    }

    /// Decodes every file in $EBB_TEST_DIR. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn decode_test_dir() {
        let dir = std::env::var("EBB_TEST_DIR").expect("set EBB_TEST_DIR");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            match decode_file(&path) {
                Ok(a) => println!("OK   {:<12} {:.2}s {}Hz {}ch", path.file_name().unwrap().to_string_lossy(), a.duration_secs(), a.sample_rate, a.channels),
                Err(e) => println!("FAIL {:<12} {e}", path.file_name().unwrap().to_string_lossy()),
            }
        }
    }
}

/// Encodes audio as a 16-bit PCM WAV file in memory.
pub fn wav_bytes(audio: &AudioData) -> Result<Vec<u8>> {
    let mut cur = Cursor::new(Vec::new());
    write_wav_to(audio, &mut cur)?;
    Ok(cur.into_inner())
}
