use super::{
    fft,
    peaks::{pick, Peak},
    tracking, yin,
};
use std::f32::consts::TAU;
#[derive(Clone, Default)]
pub struct Frame {
    pub time: f32,
    pub spectrum: Vec<f32>,
    pub peaks: Vec<Peak>,
    pub f0: Option<f32>,
    pub confidence: f32,
    pub flux: f32,
    pub onset: bool,
    pub rms: f32,
}
pub fn analyze(
    samples: &[f32],
    rate: u32,
    size: usize,
    _hop: usize,
    limit: usize,
    prev: Option<&Frame>,
    next: &mut u32,
) -> Frame {
    let mut frame = analyze_raw(samples, rate, size, limit);
    finish(&mut frame, prev, next);
    frame
}
pub fn analyze_raw(samples: &[f32], rate: u32, size: usize, limit: usize) -> Frame {
    let mut window = vec![0.; size];
    let mut power = 0.;
    for i in 0..size {
        let x = samples.get(i).copied().unwrap_or(0.);
        power += x * x;
        window[i] = x * (0.5 - 0.5 * (TAU * i as f32 / size as f32).cos());
    }
    let (re, im) = fft::real(&window);
    let spectrum: Vec<f32> = re
        .iter()
        .zip(&im)
        .map(|(r, i)| (r * r + i * i).sqrt())
        .collect();
    let f0 = yin::estimate(&samples[..samples.len().min(size)], rate);
    let peaks = pick(&re, &im, rate, size, f0.map(|v| v.0), limit);
    Frame {
        time: 0.,
        spectrum,
        peaks,
        f0: f0.map(|v| v.0),
        confidence: f0.map(|v| v.1).unwrap_or(0.),
        flux: 0.,
        onset: false,
        rms: (power / size as f32).sqrt(),
    }
}
pub fn finish(frame: &mut Frame, prev: Option<&Frame>, next: &mut u32) {
    tracking::link(
        prev.map(|f| f.peaks.as_slice()).unwrap_or(&[]),
        &mut frame.peaks,
        next,
    );
    let flux = prev
        .map(|p| {
            frame
                .spectrum
                .iter()
                .zip(&p.spectrum)
                .map(|(a, b)| (a - b).max(0.))
                .sum::<f32>()
                / (frame.spectrum.len().saturating_sub(1) * 2).max(1) as f32
        })
        .unwrap_or(0.);
    let onset = prev
        .map(|p| flux > p.flux * 1.6 && flux > 0.02)
        .unwrap_or(false);
    frame.flux = flux;
    frame.onset = onset;
}
