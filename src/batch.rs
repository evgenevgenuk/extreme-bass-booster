//! Batch processing of every audio file inside a ZIP archive.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::audio::{self, extension_of, is_supported};
use crate::dsp::{self, BoostMode, Equalizer};

#[derive(Debug, Default)]
pub struct ZipReport {
    pub boosted: usize,
    pub failed: Vec<String>,
    pub copied: usize,
}

/// Reads `input`, boosts every supported audio file and writes `output`.
/// Audio files are re-encoded as WAV; everything else is copied unchanged.
/// `progress(done, total, name)` is called before each entry.
pub fn process_zip(
    input: &Path,
    output: &Path,
    mode: BoostMode,
    eq: Equalizer,
    mut progress: impl FnMut(usize, usize, &str),
) -> Result<ZipReport> {
    let mut archive = ZipArchive::new(File::open(input).context("cannot open input ZIP")?)
        .context("input is not a valid ZIP archive")?;
    let mut writer = ZipWriter::new(std::io::BufWriter::new(
        File::create(output).context("cannot create output ZIP")?,
    ));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut report = ZipReport::default();
    let mut used: HashSet<String> = HashSet::new();
    let total = archive.len();

    for i in 0..total {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        progress(i, total, &name);

        if entry.is_dir() {
            if used.insert(name.clone()) {
                writer.add_directory(name, opts)?;
            }
            continue;
        }

        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data)?;
        drop(entry);

        if is_supported(&name) {
            let ext = extension_of(&name);
            match audio::decode_bytes(data.clone(), ext.as_deref())
                .and_then(|a| audio::wav_bytes(&dsp::process(&a, mode, &eq)))
            {
                Ok(wav) => {
                    let mut out_name = wav_name(&name);
                    if used.contains(&out_name) {
                        // song.mp3 + song.flac -> song.wav + song_flac.wav
                        let e = ext.as_deref().unwrap_or("audio");
                        out_name = format!("{}_{e}.wav", out_name.trim_end_matches(".wav"));
                    }
                    let out_name = unique_name(&mut used, out_name);
                    writer.start_file(out_name, opts)?;
                    writer.write_all(&wav)?;
                    report.boosted += 1;
                    continue;
                }
                Err(e) => report.failed.push(format!("{name}: {e}")),
            }
        }

        let out_name = unique_name(&mut used, name);
        writer.start_file(out_name, opts)?;
        writer.write_all(&data)?;
        report.copied += 1;
    }

    writer.finish()?.flush()?;
    progress(total, total, "");
    Ok(report)
}

/// `music/song.mp3` -> `music/song.wav`
fn wav_name(name: &str) -> String {
    match name.rfind('.') {
        Some(dot) if !name[dot..].contains('/') => format!("{}.wav", &name[..dot]),
        _ => format!("{name}.wav"),
    }
}

/// Avoids duplicate entries, e.g. when both `a.mp3` and `a.wav` exist.
fn unique_name(used: &mut HashSet<String>, name: String) -> String {
    if used.insert(name.clone()) {
        return name;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) => (name[..dot].to_string(), name[dot..].to_string()),
        None => (name.clone(), String::new()),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| used.insert(candidate.clone()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boosts $EBB_TEST_ZIP into $EBB_TEST_ZIP.out.zip. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn zip_file() {
        let input =
            std::path::PathBuf::from(std::env::var("EBB_TEST_ZIP").expect("set EBB_TEST_ZIP"));
        let output = input.with_extension("out.zip");
        let r = process_zip(
            &input,
            &output,
            BoostMode::Hyper,
            Equalizer::default(),
            |_, _, _| {},
        )
        .unwrap();
        println!(
            "ZIP boosted={} copied={} failed={:?}",
            r.boosted, r.copied, r.failed
        );
        let names: Vec<String> = ZipArchive::new(File::open(&output).unwrap())
            .unwrap()
            .file_names()
            .map(String::from)
            .collect();
        println!("ZIP entries: {names:?}");
    }

    #[test]
    fn names() {
        assert_eq!(wav_name("a/b/song.mp3"), "a/b/song.wav");
        assert_eq!(wav_name("noext"), "noext.wav");
        let mut used = HashSet::new();
        assert_eq!(unique_name(&mut used, "x.wav".into()), "x.wav");
        assert_eq!(unique_name(&mut used, "x.wav".into()), "x (2).wav");
    }
}
