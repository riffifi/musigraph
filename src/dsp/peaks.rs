#[derive(Clone, Debug, Default)]
pub struct Peak {
    pub freq: f32,
    pub amp: f32,
    pub phase: f32,
    pub ratio: f32,
    pub track: u32,
}
pub fn pick(
    re: &[f32],
    im: &[f32],
    rate: u32,
    n: usize,
    f0: Option<f32>,
    count: usize,
) -> Vec<Peak> {
    let mags: Vec<f32> = re
        .iter()
        .zip(im)
        .map(|(r, i)| (r * r + i * i).sqrt())
        .collect();
    let mut peaks = Vec::new();
    for k in 2..mags.len() - 1 {
        if mags[k] > mags[k - 1] && mags[k] >= mags[k + 1] && mags[k] > 0.001 {
            let a = mags[k - 1].max(1e-9).ln();
            let b = mags[k].ln();
            let c = mags[k + 1].max(1e-9).ln();
            let offset = (0.5 * (a - c) / (a - 2. * b + c)).clamp(-0.5, 0.5);
            let freq = (k as f32 + offset) * rate as f32 / n as f32;
            let amp = 2. * mags[k] / (n as f32 * 0.5);
            peaks.push(Peak {
                freq,
                amp,
                phase: im[k].atan2(re[k]),
                ratio: f0.map(|f| freq / f).unwrap_or(0.),
                track: 0,
            });
        }
    }
    peaks.sort_by(|a, b| b.amp.total_cmp(&a.amp));
    peaks.truncate(count);
    peaks
}
