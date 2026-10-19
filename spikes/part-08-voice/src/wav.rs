//! Minimal 16-bit PCM WAV reader/writer, enough for fixtures, controls and piper output.

use std::f64::consts::TAU;

const RIFF_HEADER_BYTES: usize = 12;
const CHUNK_HEADER_BYTES: usize = 8;
const FMT_BODY_BYTES: usize = 16;
const FORMAT_PCM: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
const BYTES_PER_SAMPLE: usize = 2;

pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Wav {
    pub fn parse(bytes: &[u8]) -> Result<Wav, String> {
        if bytes.len() < RIFF_HEADER_BYTES || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err("not a RIFF/WAVE file".into());
        }
        let mut format: Option<(u16, u32)> = None;
        let mut pos = RIFF_HEADER_BYTES;
        while pos + CHUNK_HEADER_BYTES <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let declared = u32::from_le_bytes([
                bytes[pos + 4],
                bytes[pos + 5],
                bytes[pos + 6],
                bytes[pos + 7],
            ]) as usize;
            let body = pos + CHUNK_HEADER_BYTES;
            // A streamed WAV may declare a size larger than what was written; clamp to the file.
            let end = body.saturating_add(declared).min(bytes.len());
            match id {
                b"fmt " => {
                    let fmt = &bytes[body..end];
                    if fmt.len() < FMT_BODY_BYTES {
                        return Err("fmt chunk too short".into());
                    }
                    let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
                    let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
                    let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                    let bits = u16::from_le_bytes([fmt[14], fmt[15]]);
                    if tag != FORMAT_PCM || bits != BITS_PER_SAMPLE {
                        return Err(format!("unsupported format tag {tag}, {bits} bits"));
                    }
                    if channels == 0 || rate == 0 {
                        return Err("zero channels or sample rate".into());
                    }
                    format = Some((channels, rate));
                }
                b"data" => {
                    let (channels, sample_rate) = format.ok_or("data chunk before fmt chunk")?;
                    let samples = bytes[body..end]
                        .as_chunks::<BYTES_PER_SAMPLE>()
                        .0
                        .iter()
                        .map(|b| i16::from_le_bytes(*b))
                        .collect();
                    return Ok(Wav {
                        sample_rate,
                        channels,
                        samples,
                    });
                }
                _ => {}
            }
            // Chunks are padded to an even length.
            pos = end + (declared & 1);
        }
        Err("no data chunk".into())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let data_len = self.samples.len() * BYTES_PER_SAMPLE;
        let block_align = self.channels as usize * BYTES_PER_SAMPLE;
        let riff_len = 4 + CHUNK_HEADER_BYTES + FMT_BODY_BYTES + CHUNK_HEADER_BYTES + data_len;
        let mut out = Vec::with_capacity(CHUNK_HEADER_BYTES + riff_len);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(riff_len as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&(FMT_BODY_BYTES as u32).to_le_bytes());
        out.extend_from_slice(&FORMAT_PCM.to_le_bytes());
        out.extend_from_slice(&self.channels.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&(self.sample_rate * block_align as u32).to_le_bytes());
        out.extend_from_slice(&(block_align as u16).to_le_bytes());
        out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data_len as u32).to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    pub fn duration_secs(&self) -> f64 {
        self.samples.len() as f64 / self.channels as f64 / self.sample_rate as f64
    }

    /// Root-mean-square level, normalised so full scale is 1.0.
    pub fn rms(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = self
            .samples
            .iter()
            .map(|&s| (s as f64 / 32768.0).powi(2))
            .sum();
        (sum / self.samples.len() as f64).sqrt()
    }

    pub fn peak(&self) -> f64 {
        self.samples
            .iter()
            .map(|&s| (s as f64 / 32768.0).abs())
            .fold(0.0, f64::max)
    }
}

pub fn silence(sample_rate: u32, secs: f64) -> Wav {
    Wav {
        sample_rate,
        channels: 1,
        samples: vec![0; (sample_rate as f64 * secs) as usize],
    }
}

/// A pure sine tone: audio that is loud but contains no speech.
pub fn tone(sample_rate: u32, secs: f64, hz: f64, amplitude: f64) -> Wav {
    let count = (sample_rate as f64 * secs) as usize;
    let samples = (0..count)
        .map(|i| {
            let t = i as f64 / sample_rate as f64;
            ((TAU * hz * t).sin() * amplitude * i16::MAX as f64) as i16
        })
        .collect();
    Wav {
        sample_rate,
        channels: 1,
        samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_bytes() {
        let original = tone(16_000, 0.25, 440.0, 0.5);
        let parsed = Wav::parse(&original.to_bytes()).unwrap();
        assert_eq!(parsed.sample_rate, 16_000);
        assert_eq!(parsed.channels, 1);
        assert_eq!(parsed.samples, original.samples);
    }

    #[test]
    fn duration_and_levels() {
        let t = tone(16_000, 2.0, 440.0, 0.5);
        assert!((t.duration_secs() - 2.0).abs() < 1e-9);
        // RMS of a sine is amplitude / sqrt(2).
        assert!((t.rms() - 0.5 / 2f64.sqrt()).abs() < 0.01);
        assert!((t.peak() - 0.5).abs() < 0.01);
        assert_eq!(silence(16_000, 1.0).rms(), 0.0);
    }

    #[test]
    fn rejects_garbage_and_unsupported_formats() {
        assert!(Wav::parse(b"not a wav file at all").is_err());
        let mut bytes = silence(16_000, 0.1).to_bytes();
        bytes[20] = 3; // format tag 3 = IEEE float
        assert!(Wav::parse(&bytes).is_err());
    }

    #[test]
    fn tolerates_streamed_size_larger_than_file() {
        let original = silence(22_050, 0.1);
        let mut bytes = original.to_bytes();
        // The data chunk size field sits 4 bytes before the sample bytes.
        let size_at = bytes.len() - original.samples.len() * BYTES_PER_SAMPLE - 4;
        bytes[size_at..size_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let parsed = Wav::parse(&bytes).unwrap();
        assert_eq!(parsed.samples.len(), original.samples.len());
    }
}
