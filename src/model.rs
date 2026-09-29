use crate::dsp::stft::Frame;
#[derive(Clone, Default)]
pub struct Envelope {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}
pub fn fit_envelope(values: &[(f32, f32)]) -> Envelope {
    if values.is_empty() {
        return Envelope::default();
    }
    let peak = values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
        .unwrap()
        .0;
    let start = values[0].0;
    let end = values.last().unwrap().0;
    let max = values[peak].1.max(1e-8);
    let attack = (values[peak].0 - start).max(0.);
    let sustain = if values.len() > 4 {
        let a = peak + (values.len() - peak) / 3;
        let b = peak + 2 * (values.len() - peak) / 3;
        values[a..b.max(a + 1)].iter().map(|x| x.1).sum::<f32>() / (b.max(a + 1) - a) as f32 / max
    } else {
        1.
    };
    let mut release = 0.;
    for i in (peak..values.len()).rev() {
        if values[i].1 > max * 0.1 {
            release = end - values[i].0;
            break;
        }
    }
    Envelope {
        attack,
        decay: ((end - start - release - attack) * 0.35).max(0.),
        sustain: sustain.clamp(0., 1.),
        release,
    }
}
pub fn note_name(freq: f32) -> String {
    if freq <= 0. {
        return "—".into();
    }
    let midi = 69. + 12. * (freq / 440.).log2();
    let n = midi.round() as i32;
    let names = [
        "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
    ];
    format!(
        "{}{} {:+0.0}c",
        names[n.rem_euclid(12) as usize],
        n.div_euclid(12) - 1,
        (midi - n as f32) * 100.
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn envelope() {
        let e = fit_envelope(&[
            (0., 0.),
            (0.1, 1.),
            (0.2, 0.7),
            (0.3, 0.5),
            (0.4, 0.5),
            (0.5, 0.),
        ]);
        assert!((e.attack - 0.1).abs() < 1e-5);
        assert!(e.sustain > 0. && e.sustain < 1.);
    }
}
#[derive(Clone)]
pub struct EventModel {
    pub start: f32,
    pub end: f32,
    pub envelope: Envelope,
}
pub fn segment(frames: &[Frame]) -> Vec<EventModel> {
    if frames.is_empty() {
        return Vec::new();
    }
    let mut boundaries = vec![0];
    for (i, f) in frames.iter().enumerate().skip(1) {
        if f.onset && i - boundaries.last().copied().unwrap_or(0) > 3 {
            boundaries.push(i);
        }
    }
    boundaries.push(frames.len());
    let mut events = Vec::new();
    for pair in boundaries.windows(2) {
        let slice = &frames[pair[0]..pair[1]];
        let values: Vec<_> = slice.iter().map(|f| (f.time, f.rms)).collect();
        events.push(EventModel {
            start: slice[0].time,
            end: slice.last().unwrap().time,
            envelope: fit_envelope(&values),
        });
    }
    events
}
