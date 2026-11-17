//! Simple seekable playback of an in-memory buffer using rodio.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, OutputStreamHandle, Sink};

use crate::audio::AudioData;

pub struct Player {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    audio: Option<Arc<AudioData>>,
    /// Position (seconds) where the current sink started playing from.
    offset: f64,
}

impl Player {
    pub fn new() -> Result<Self> {
        let (stream, handle) =
            OutputStream::try_default().map_err(|e| anyhow!("no audio output device: {e}"))?;
        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
            audio: None,
            offset: 0.0,
        })
    }

    /// Starts playing `audio` from `secs` seconds.
    pub fn play_at(&mut self, audio: Arc<AudioData>, secs: f64) -> Result<()> {
        self.audio = Some(audio);
        self.play_from(secs)
    }

    fn play_from(&mut self, secs: f64) -> Result<()> {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        let Some(audio) = self.audio.clone() else {
            return Ok(());
        };
        let secs = secs.clamp(0.0, audio.duration_secs());
        let frame = (secs * audio.sample_rate as f64) as usize;
        let start = (frame * audio.channels).min(audio.samples.len());

        let sink =
            Sink::try_new(&self.handle).map_err(|e| anyhow!("cannot start playback: {e}"))?;
        sink.append(SamplesBuffer::new(
            audio.channels as u16,
            audio.sample_rate,
            audio.samples[start..].to_vec(),
        ));
        sink.play();
        self.sink = Some(sink);
        self.offset = secs;
        Ok(())
    }

    pub fn seek(&mut self, secs: f64) -> Result<()> {
        if self.is_playing() {
            self.play_from(secs)
        } else {
            Ok(())
        }
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.audio = None;
        self.offset = 0.0;
    }

    pub fn is_playing(&self) -> bool {
        self.sink.as_ref().is_some_and(|s| !s.empty())
    }

    pub fn position(&self) -> f64 {
        let played = self.sink.as_ref().map_or(Duration::ZERO, |s| s.get_pos());
        self.offset + played.as_secs_f64()
    }

    pub fn duration(&self) -> f64 {
        self.audio.as_ref().map_or(0.0, |a| a.duration_secs())
    }
}
