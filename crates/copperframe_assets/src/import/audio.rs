//! Audio asset checks (WAV and Ogg Vorbis).
//!
//! Import reads the container headers to confirm the format, channel count,
//! sample rate and length, and keeps the file bytes for the runtime, which
//! decodes them with its audio library. Nothing here touches an audio
//! device, so validation works headless.

use std::path::Path;

use serde::Serialize;

use super::{ImportError, read_source};
use crate::meta::ImportSettings;

/// Largest accepted audio file (the runtime decodes whole files).
pub const MAX_AUDIO_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Wav,
    OggVorbis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedAudio {
    pub format: AudioFormat,
    pub channels: u16,
    pub sample_rate: u32,
    pub duration_secs: f64,
    /// The source file, for the runtime decoder.
    pub bytes: std::sync::Arc<[u8]>,
}

pub fn import_audio(path: &Path, _settings: &ImportSettings) -> Result<ImportedAudio, ImportError> {
    let bytes = read_source(path)?;
    if bytes.len() as u64 > MAX_AUDIO_BYTES {
        return Err(ImportError::Malformed {
            path: path.to_path_buf(),
            what: "audio".into(),
            message: format!(
                "{} bytes is over the {MAX_AUDIO_BYTES}-byte audio limit",
                bytes.len()
            ),
        });
    }
    let invalid = |message: String| ImportError::Malformed {
        path: path.to_path_buf(),
        what: "audio".into(),
        message,
    };
    let (format, channels, sample_rate, duration_secs) = if bytes.starts_with(b"RIFF") {
        let (c, r, d) = parse_wav(&bytes).map_err(invalid)?;
        (AudioFormat::Wav, c, r, d)
    } else if bytes.starts_with(b"OggS") {
        let (c, r, d) = parse_ogg_vorbis(&bytes).map_err(invalid)?;
        (AudioFormat::OggVorbis, c, r, d)
    } else {
        return Err(invalid(
            "not a WAV (RIFF) or Ogg file; supported audio formats are WAV and Ogg Vorbis".into(),
        ));
    };
    Ok(ImportedAudio {
        format,
        channels,
        sample_rate,
        duration_secs,
        bytes: bytes.into(),
    })
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Channels, sample rate and duration of a PCM or IEEE-float WAV file.
fn parse_wav(b: &[u8]) -> Result<(u16, u32, f64), String> {
    if b.get(8..12) != Some(b"WAVE") {
        return Err("RIFF file is not WAVE audio".into());
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32_at(b, pos + 4).ok_or("truncated chunk header")? as usize;
        let body = pos + 8;
        let end = body
            .checked_add(size)
            .filter(|&e| e <= b.len())
            .ok_or_else(|| format!("chunk `{}` runs past the end of the file", ascii(id)))?;
        match id {
            b"fmt " => {
                let format = u16_at(b, body).ok_or("truncated fmt chunk")?;
                let channels = u16_at(b, body + 2).ok_or("truncated fmt chunk")?;
                let rate = u32_at(b, body + 4).ok_or("truncated fmt chunk")?;
                let bits = u16_at(b, body + 14).ok_or("truncated fmt chunk")?;
                // 1 = PCM, 3 = IEEE float, 0xFFFE = extensible.
                if ![1, 3, 0xFFFE].contains(&format) {
                    return Err(format!(
                        "WAV encoding {format:#x} is not supported (use PCM or float)"
                    ));
                }
                fmt = Some((format, channels, rate, bits));
            }
            b"data" => {
                let (_, channels, rate, bits) =
                    fmt.ok_or("the data chunk comes before the fmt chunk")?;
                if channels == 0 || channels > 8 || rate == 0 || bits == 0 || bits % 8 != 0 {
                    return Err(format!(
                        "unsupported WAV layout: {channels} channels, {rate} Hz, {bits} bits"
                    ));
                }
                let frame = usize::from(channels) * usize::from(bits / 8);
                let frames = size / frame;
                if frames == 0 {
                    return Err("WAV file has no samples".into());
                }
                return Ok((channels, rate, frames as f64 / f64::from(rate)));
            }
            _ => {}
        }
        // Chunks are padded to even sizes.
        pos = end + (size & 1);
    }
    Err("WAV file has no data chunk".into())
}

fn ascii(id: &[u8]) -> String {
    id.iter()
        .map(|&c| if c.is_ascii_graphic() { c as char } else { '?' })
        .collect()
}

/// Channels, sample rate and duration of an Ogg Vorbis stream, from the
/// identification header and the last page's granule position.
fn parse_ogg_vorbis(b: &[u8]) -> Result<(u16, u32, f64), String> {
    // First page: 27-byte header + segment table, then the identification
    // packet "\x01vorbis".
    let segments = usize::from(*b.get(26).ok_or("truncated Ogg page")?);
    let packet = 27 + segments;
    let id = b.get(packet..packet + 30).ok_or("truncated Ogg page")?;
    if &id[0..7] != b"\x01vorbis" {
        return Err("Ogg stream is not Vorbis (Opus and FLAC in Ogg are not supported)".into());
    }
    let channels = u16::from(id[11]);
    let rate = u32_at(id, 12).ok_or("truncated Vorbis header")?;
    if channels == 0 || rate == 0 {
        return Err("Vorbis header has zero channels or sample rate".into());
    }
    // The last page that starts with "OggS" carries the total sample count.
    let last = b
        .windows(4)
        .rposition(|w| w == b"OggS")
        .ok_or("no Ogg pages")?;
    let granule = b
        .get(last + 6..last + 14)
        .and_then(|g| g.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or("truncated last Ogg page")?;
    if granule == 0 || granule == u64::MAX {
        return Err("Ogg stream has no audio samples".into());
    }
    Ok((channels, rate, granule as f64 / f64::from(rate)))
}

/// A mono 16-bit PCM WAV file, for fixtures and generated example sounds.
pub fn wav_bytes(sample_rate: u32, samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone() -> Vec<u8> {
        let samples: Vec<f32> = (0..22050).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        wav_bytes(22050, &samples)
    }

    #[test]
    fn wav_headers_are_read() {
        let (c, r, d) = parse_wav(&tone()).unwrap();
        assert_eq!((c, r), (1, 22050));
        assert!((d - 1.0).abs() < 1e-9);
    }

    #[test]
    fn broken_audio_is_rejected_with_a_reason() {
        let wav = tone();
        // Truncated inside the data chunk.
        assert!(parse_wav(&wav[..100]).unwrap_err().contains("past the end"));
        // Data before fmt.
        let mut swapped = wav[..12].to_vec();
        swapped.extend_from_slice(b"data\x02\x00\x00\x00\x00\x00");
        assert!(parse_wav(&swapped).unwrap_err().contains("before the fmt"));
        // Compressed encodings are refused.
        let mut adpcm = wav.clone();
        adpcm[20] = 2;
        assert!(parse_wav(&adpcm).unwrap_err().contains("not supported"));
        // Ogg but not Vorbis.
        let mut opus = b"OggS".to_vec();
        opus.extend_from_slice(&[0; 22]);
        opus.push(1);
        opus.push(19);
        opus.extend_from_slice(b"OpusHead");
        opus.extend_from_slice(&[0; 30]);
        assert!(parse_ogg_vorbis(&opus).unwrap_err().contains("not Vorbis"));
        assert!(parse_ogg_vorbis(b"OggS").is_err());
    }

    #[test]
    fn import_reads_files_and_rejects_other_formats() {
        let dir = std::env::temp_dir().join(format!("psx_audio_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("tone.wav");
        std::fs::write(&good, tone()).unwrap();
        let audio = import_audio(&good, &ImportSettings::default()).unwrap();
        assert_eq!(audio.format, AudioFormat::Wav);
        assert_eq!(audio.bytes.len(), tone().len());
        let bad = dir.join("fake.wav");
        std::fs::write(&bad, b"ID3 this is an mp3").unwrap();
        let err = import_audio(&bad, &ImportSettings::default()).unwrap_err();
        assert!(err.to_string().contains("WAV"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
