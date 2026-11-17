# Extreme Bass Booster 9000

A small desktop app written in Rust that boosts the bass of your music. It opens almost any audio format, lets you preview the result, saves it as WAV, and can process every track inside a ZIP archive at once.

> **Warning:** high boost levels and HYPER MAX mode are extremely loud and heavily distorted on purpose. Turn your volume down before you press Play. Your speakers and ears will thank you.

## Features

- **Opens almost any audio format:** MP3, FLAC, WAV, OGG Vorbis, AAC / M4A, ALAC, AIFF, CAF, MKV / WebM audio, ADPCM and more (decoding by [Symphonia](https://github.com/pdeljanov/Symphonia), pure Rust).
- **Boost slider from 0 to 400 %**, plus a fixed **HYPER MAX** preset.
- **Preview:** play the boosted track, seek with the progress bar, stop at any time.
- **Save:** export the boosted track as 16-bit WAV.
- **Batch ZIP mode:** pick a ZIP, and every supported audio file inside is boosted and written to a new ZIP as WAV. Folder structure is kept and other files are copied unchanged.
- **Responsive UI:** decoding and processing run on a background thread.
- **Single executable,** no runtime or installer needed.

## Download

Grab the latest Windows build from the [Releases](../../releases) page and run `extreme-bass-booster.exe`.

## Build from source

You need [Rust](https://rustup.rs/) (stable).

```bash
git clone https://github.com/evgenevgenuk/extreme-bass-booster.git
cd extreme-bass-booster
cargo build --release
```

The binary is in `target/release/`.

On Linux you also need the ALSA and GTK development packages:

```bash
sudo apt install libasound2-dev libgtk-3-dev
```

## How to use

1. Click **Load audio file** and pick a track.
2. Move the slider to set the boost strength.
3. Click **Play (slider)** to preview, or **Play HYPER MAX** if you are brave.
4. Click **Save (slider)** or **Save HYPER MAX** to export a WAV file.

For many files at once, put them in a ZIP and use **Boost ZIP (slider)** or **HYPER MAX for ZIP**.

Settings at 300 % and above, and HYPER MAX, ask for confirmation first.

## How it works

1. The track is decoded to 32-bit float PCM.
2. A one-pole low-pass filter (cutoff 180 Hz, or 150 Hz in HYPER MAX) extracts the low end of each channel separately.
3. The low end is amplified and mixed back on top of the original signal:

   | Mode | Gain | Layers | Cutoff |
   |------|------|--------|--------|
   | Slider `p` % | `1 + p/100 * 4` | `1 + p/80` | 180 Hz |
   | HYPER MAX | 25 | 10 | 150 Hz |

4. The result is hard-clipped to the [-1, 1] range and written as 16-bit PCM.

## Project layout

```
src/
  main.rs    GUI (egui / eframe) and background tasks
  audio.rs   decoding (Symphonia) and WAV writing (hound)
  dsp.rs     bass boost algorithm
  player.rs  playback with seeking (rodio)
  batch.rs   ZIP batch processing
```

## Running tests

```bash
cargo test
```

## Limitations

- Output is always WAV (16-bit PCM).
- Opus and WMA files are not supported by the decoder.
- The whole track is loaded into memory, which is fine for songs but not for multi-hour recordings.

## License

[MIT](LICENSE)
