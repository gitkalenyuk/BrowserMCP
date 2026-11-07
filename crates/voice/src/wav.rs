//! 16-bit PCM WAV: what the UI records (16 kHz mono) and what whisper-server and the speech
//! APIs accept.

/// Sample rate of everything the UI streams (`voice.stream_push`).
pub const SAMPLE_RATE: u32 = 16_000;

/// A WAV file (RIFF, PCM 16-bit little endian) of `samples`.
pub fn encode(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let block_align = channels * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Facts read from a WAV header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    /// Bytes of sample data.
    pub data_len: u32,
}

impl WavInfo {
    /// Length of the audio in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        let per_second =
            u64::from(self.sample_rate) * u64::from(self.channels) * u64::from(self.bits / 8);
        if per_second == 0 {
            return 0;
        }
        u64::from(self.data_len) * 1000 / per_second
    }
}

/// Read the `fmt ` and `data` chunks of a RIFF/WAVE file; `None` if it is not one.
pub fn parse(bytes: &[u8]) -> Option<WavInfo> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u32, u16)> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?);
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            let channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into().ok()?);
            let rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
            let bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().ok()?);
            fmt = Some((channels, rate, bits));
        } else if id == b"data" {
            let (channels, sample_rate, bits) = fmt?;
            // A streamed WAV may carry a placeholder length: use what is there.
            let available = (bytes.len() - body) as u32;
            return Some(WavInfo {
                sample_rate,
                channels,
                bits,
                data_len: len.min(available),
            });
        }
        // Chunks are padded to an even length.
        pos = body + len as usize + (len as usize & 1);
    }
    None
}

/// 16-bit little-endian PCM bytes → samples (an odd trailing byte is dropped).
pub fn samples_from_le_bytes(bytes: &[u8]) -> Vec<i16> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().map(|b| i16::from_le_bytes(*b)).collect()
}

/// Root-mean-square level of `samples` in 0..=1 (silence detection in tests and logs).
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let x = f64::from(s) / 32768.0;
            x * x
        })
        .sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_header_that_parses_back() {
        let samples: Vec<i16> = (0..16_000).map(|i| ((i % 100) * 300) as i16).collect();
        let wav = encode(&samples, SAMPLE_RATE, 1);
        assert_eq!(wav.len(), 44 + 32_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()),
            36 + 32_000
        );
        let info = parse(&wav).unwrap();
        assert_eq!(
            info,
            WavInfo {
                sample_rate: 16_000,
                channels: 1,
                bits: 16,
                data_len: 32_000
            }
        );
        assert_eq!(info.duration_ms(), 1000);
        assert_eq!(samples_from_le_bytes(&wav[44..]), samples);
    }

    #[test]
    fn skips_extra_chunks_and_rejects_other_files() {
        let mut wav = encode(&[1, 2, 3, 4], 8000, 1);
        // Insert a LIST chunk (odd length → padded) between fmt and data.
        let list = [b"LIST".as_slice(), &3u32.to_le_bytes(), b"abc\0"].concat();
        wav.splice(36..36, list);
        let info = parse(&wav).unwrap();
        assert_eq!(info.data_len, 8);
        assert_eq!(info.sample_rate, 8000);
        assert!(parse(b"OggS\0\0\0\0\0\0\0\0").is_none());
        assert!(parse(b"RIFF").is_none());
    }

    #[test]
    fn level_of_silence_and_noise() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0; 100]), 0.0);
        assert!(rms(&[i16::MAX, i16::MIN]) > 0.99);
    }
}
