use musigraph::{audio_io, dsp::stft, midi, model};
use std::{f32::consts::TAU, fs, path::PathBuf};
fn wav(samples: &[f32], rate: u32, path: &std::path::Path) {
    let mut b = Vec::new();
    let bytes = (samples.len() * 2) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + bytes).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&bytes.to_le_bytes());
    for &x in samples {
        b.extend_from_slice(&((x.clamp(-1., 1.) * 32767.) as i16).to_le_bytes());
    }
    fs::write(path, b).unwrap();
}
fn test_frame(path: &std::path::Path) -> stft::Frame {
    let a = audio_io::decode(path).unwrap();
    let mut next = 1;
    stft::analyze(&a.samples, a.rate, 2048, 512, 16, None, &mut next)
}
#[test]
fn sine_chord_drum_speech_and_midi() {
    let dir = std::env::temp_dir().join(format!("scope_test_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let rate = 44100;
    let sine: Vec<_> = (0..8192)
        .map(|i| (TAU * 440. * i as f32 / rate as f32).sin() * 0.5)
        .collect();
    let path = dir.join("sine.wav");
    wav(&sine, rate, &path);
    let f = test_frame(&path);
    assert!((f.f0.unwrap() - 440.).abs() < 5.);
    assert!(f.peaks.iter().any(|p| (p.freq - 440.).abs() < 25.));
    let chord: Vec<_> = (0..8192)
        .map(|i| {
            [261.63, 329.63, 392.]
                .iter()
                .map(|f| (TAU * f * i as f32 / rate as f32).sin() * 0.22)
                .sum()
        })
        .collect();
    let path = dir.join("chord.wav");
    wav(&chord, rate, &path);
    let f = test_frame(&path);
    for target in [261.63, 329.63, 392.] {
        assert!(
            f.peaks.iter().any(|p| (p.freq - target).abs() < 30.),
            "missing {target}"
        );
    }
    let mut seed = 7u32;
    let drum: Vec<_> = (0..8192)
        .map(|i| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((seed >> 8) as f32 / 16777216. - 0.5) * 2.;
            noise * (-i as f32 / 1100.).exp() * 0.8
        })
        .collect();
    let path = dir.join("drum.wav");
    wav(&drum, rate, &path);
    let f = test_frame(&path);
    assert!(f.rms > 0.01);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/speech.wav");
    let a = audio_io::decode(&path).unwrap();
    assert!(a.samples.len() > 2048);
    let mut next = 1;
    let frames: Vec<_> = (0..8)
        .map(|i| {
            let mut f = stft::analyze(
                &a.samples[i * 512..],
                a.rate,
                2048,
                512,
                16,
                None,
                &mut next,
            );
            f.time = i as f32 * 512. / a.rate as f32;
            f
        })
        .collect();
    assert!(frames.iter().any(|f| f.rms > 0.001));
    assert!(!model::segment(&frames).is_empty());
    let midi_data = [
        b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk\0\0\0\x0d".as_slice(),
        &[0, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0, 0, 0xff, 0x2f, 0],
    ]
    .concat();
    let notes = midi::parse(&midi_data).unwrap();
    assert_eq!(notes.len(), 1);
    let _ = fs::remove_dir_all(dir);
}
#[test]
fn compressed_formats() {
    let ffmpeg = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output();
    if ffmpeg.is_err() {
        return;
    }
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/speech.wav");
    let dir = std::env::temp_dir().join(format!("scope_codecs_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    for ext in ["mp3", "ogg", "flac"] {
        let output = dir.join(format!("speech.{ext}"));
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&input)
            .arg(&output)
            .status()
            .unwrap();
        assert!(status.success());
        let a = audio_io::decode(&output).unwrap();
        assert!(a.samples.len() > 2048, "{ext}");
    }
    let _ = fs::remove_dir_all(dir);
}
