use crate::audio_io::Audio;
use std::{
    io::{BufWriter, Write},
    path::Path,
};

fn pcm(v: f32) -> i16 {
    (v.clamp(-1., 1.) * 32767.).round() as i16
}
pub fn write(path: &Path, audio: &Audio, start: usize, end: usize) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "wav" && ext != "flac" {
        return Err("Choose .wav or .flac".into());
    }
    let data = audio
        .samples
        .get(start..end)
        .ok_or("Invalid export range")?;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut out = BufWriter::new(file);
    match ext.as_str() {
        "wav" => wav(&mut out, audio.rate, data)?,
        "flac" => flac(&mut out, audio.rate, data)?,
        _ => return Err("Choose .wav or .flac".into()),
    }
    out.flush().map_err(|e| e.to_string())
}
fn wav<W: Write>(out: &mut W, rate: u32, data: &[f32]) -> Result<(), String> {
    let bytes = data.len().checked_mul(2).ok_or("File too large")?;
    if bytes > u32::MAX as usize - 36 {
        return Err("WAV exceeds 4 GB".into());
    }
    out.write_all(b"RIFF").map_err(|e| e.to_string())?;
    out.write_all(&(36 + bytes as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(b"WAVEfmt ").map_err(|e| e.to_string())?;
    out.write_all(&16u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&1u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&1u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&rate.to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&(rate * 2).to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&2u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&16u16.to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(b"data").map_err(|e| e.to_string())?;
    out.write_all(&(bytes as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    for &v in data {
        out.write_all(&pcm(v).to_le_bytes())
            .map_err(|e| e.to_string())?
    }
    Ok(())
}
fn crc8(bytes: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in bytes {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 {
                (c << 1) ^ 0x07
            } else {
                c << 1
            }
        }
    }
    c
}
fn crc16(bytes: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in bytes {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 {
                (c << 1) ^ 0x8005
            } else {
                c << 1
            }
        }
    }
    c
}
fn utf8_number(mut n: u64) -> Vec<u8> {
    if n < 128 {
        return vec![n as u8];
    }
    let count = if n < 1 << 11 {
        2
    } else if n < 1 << 16 {
        3
    } else if n < 1 << 21 {
        4
    } else if n < 1 << 26 {
        5
    } else if n < 1 << 31 {
        6
    } else {
        7
    };
    let mut out = vec![0; count];
    for i in (1..count).rev() {
        out[i] = 0x80 | (n as u8 & 0x3f);
        n >>= 6;
    }
    out[0] = ((0xffu16 << (8 - count)) as u8) | (n as u8 & ((1 << (7 - count)) - 1));
    out
}
fn flac<W: Write>(out: &mut W, rate: u32, data: &[f32]) -> Result<(), String> {
    if rate == 0 || rate >= 1 << 20 || data.len() >= 1usize << 36 {
        return Err("FLAC stream limits exceeded".into());
    }
    out.write_all(b"fLaC").map_err(|e| e.to_string())?;
    out.write_all(&[0x80, 0, 0, 34])
        .map_err(|e| e.to_string())?;
    out.write_all(&16u16.to_be_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&4096u16.to_be_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&[0; 6]).map_err(|e| e.to_string())?;
    let info = ((rate as u64) << 44) | ((15u64) << 36) | (data.len() as u64);
    out.write_all(&info.to_be_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(&[0; 16]).map_err(|e| e.to_string())?;
    for (index, chunk) in data.chunks(4096).enumerate() {
        let mut frame = Vec::with_capacity(chunk.len() * 2 + 32);
        frame.extend_from_slice(&[0xff, 0xf9, 0x70, 0x08]);
        frame.extend_from_slice(&utf8_number(index as u64 * 4096));
        frame.extend_from_slice(&((chunk.len() - 1) as u16).to_be_bytes());
        frame.push(crc8(&frame));
        frame.push(0x02);
        for &sample in chunk {
            frame.extend_from_slice(&pcm(sample).to_be_bytes())
        }
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        out.write_all(&frame).map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_flac() {
        let path = std::env::temp_dir().join(format!("flac-{}.flac", std::process::id()));
        let data: Vec<_> = (0..5000).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        write(
            &path,
            &Audio {
                samples: std::sync::Arc::new(data.clone()),
                rate: 44100,
            },
            0,
            data.len(),
        )
        .unwrap();
        let decoded = crate::audio_io::decode(&path).unwrap();
        assert_eq!(decoded.samples.len(), data.len());
        assert!((decoded.samples[100] - data[100]).abs() < 0.0001);
        std::fs::remove_file(path).unwrap();
    }
}
