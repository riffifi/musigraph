use crate::dsp::peaks::Peak;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Range,
    Lasso,
}

#[derive(Clone, Debug)]
pub enum Selection {
    Range { start: f32, end: f32 },
    Lasso { points: Vec<(f32, f32)> },
}
impl Selection {
    pub fn bounds(&self) -> Option<(f32, f32)> {
        match self {
            Self::Range { start, end } => Some((start.min(*end), start.max(*end))),
            Self::Lasso { points } => {
                if points.len() < 3 {
                    return None;
                }
                Some((
                    points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
                    points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max),
                ))
            }
        }
    }
    pub fn contains(&self, time: f32, freq: f32) -> bool {
        match self {
            Self::Range { .. } => self.bounds().is_some_and(|(a, b)| a <= time && time <= b),
            Self::Lasso { points } => {
                if points.len() < 3 || freq <= 0. {
                    return false;
                }
                let note = 69. + 12. * (freq / 440.).log2();
                let mut inside = false;
                let mut j = points.len() - 1;
                for i in 0..points.len() {
                    let (xi, yi) = points[i];
                    let (xj, yj) = points[j];
                    if (yi > note) != (yj > note) && time < (xj - xi) * (note - yi) / (yj - yi) + xi
                    {
                        inside = !inside;
                    }
                    j = i;
                }
                inside
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    GainUp,
    GainDown,
    Normalize,
    Mute,
    Reverse,
    FadeIn,
    FadeOut,
    Gcd,
    RemoveDc,
    Invert,
    Compress,
    HighPass,
    LowPass,
    Gate,
    SoftClip,
    FadeBoth,
}
impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Self::GainUp => "gain +3 dB",
            Self::GainDown => "gain -3 dB",
            Self::Normalize => "normalize",
            Self::Mute => "mute",
            Self::Reverse => "reverse",
            Self::FadeIn => "fade in",
            Self::FadeOut => "fade out",
            Self::Gcd => "gcd harmonize",
            Self::RemoveDc => "remove DC",
            Self::Invert => "invert polarity",
            Self::Compress => "compress",
            Self::HighPass => "high pass 120 Hz",
            Self::LowPass => "low pass 6 kHz",
            Self::Gate => "noise gate",
            Self::SoftClip => "soft clip",
            Self::FadeBoth => "edge fade",
        }
    }
    pub fn code(self) -> u8 {
        self as u8
    }
    pub fn from_code(code: u8) -> Option<Self> {
        const ALL: [Operation; 16] = [
            Operation::GainUp,
            Operation::GainDown,
            Operation::Normalize,
            Operation::Mute,
            Operation::Reverse,
            Operation::FadeIn,
            Operation::FadeOut,
            Operation::Gcd,
            Operation::RemoveDc,
            Operation::Invert,
            Operation::Compress,
            Operation::HighPass,
            Operation::LowPass,
            Operation::Gate,
            Operation::SoftClip,
            Operation::FadeBoth,
        ];
        ALL.get(code as usize).copied()
    }
    pub fn badge(self) -> char {
        match self {
            Self::GainUp | Self::GainDown => 'G',
            Self::Normalize => 'N',
            Self::Mute => 'M',
            Self::Reverse => 'R',
            Self::FadeIn | Self::FadeOut | Self::FadeBoth => 'F',
            Self::Gcd => 'H',
            Self::RemoveDc => 'D',
            Self::Invert => 'I',
            Self::Compress => 'C',
            Self::HighPass => 'H',
            Self::LowPass => 'L',
            Self::Gate => 'Q',
            Self::SoftClip => 'S',
        }
    }
    pub fn family(self) -> u8 {
        match self {
            Self::GainUp | Self::GainDown | Self::Normalize | Self::Compress | Self::SoftClip => 0,
            Self::HighPass | Self::LowPass | Self::Gate | Self::RemoveDc | Self::Gcd => 2,
            _ => 1,
        }
    }
}

pub fn apply_samples(samples: &mut [f32], operation: Operation, rate: u32) {
    let len = samples.len();
    if len == 0 {
        return;
    }
    let peak = samples.iter().map(|v| v.abs()).fold(0., f32::max);
    let dc = if operation == Operation::RemoveDc {
        samples.iter().map(|v| *v as f64).sum::<f64>() as f32 / len as f32
    } else {
        0.
    };
    if matches!(operation, Operation::HighPass | Operation::LowPass) {
        let original = samples.to_vec();
        let cutoff: f32 = if operation == Operation::HighPass {
            120.
        } else {
            6000.
        };
        let cutoff = cutoff.min(rate as f32 * 0.45);
        let dt = 1. / rate.max(1) as f32;
        let rc = 1. / (std::f32::consts::TAU * cutoff);
        let alpha = if operation == Operation::HighPass {
            rc / (rc + dt)
        } else {
            dt / (rc + dt)
        };
        let mut y = if operation == Operation::HighPass {
            0.
        } else {
            original[0]
        };
        let mut prev = original[0];
        let taper = (rate as usize / 200).min(len / 2).max(1);
        for (i, value) in samples.iter_mut().enumerate() {
            let x = original[i];
            y = if operation == Operation::HighPass {
                alpha * (y + x - prev)
            } else {
                y + alpha * (x - y)
            };
            prev = x;
            let mix = (i.min(len - 1 - i) as f32 / taper as f32).clamp(0., 1.);
            *value = (x * (1. - mix) + y * mix).clamp(-1., 1.);
        }
        return;
    }
    let mut gate_level = 0f32;
    let mut gate_gain = 0f32;
    let sample_rate = rate.max(1) as f32;
    let coefficient = |seconds: f32| 1. - (-1. / (seconds * sample_rate)).exp();
    let (level_rise, level_fall, gain_rise, gain_fall) = (
        coefficient(0.003),
        coefficient(0.04),
        coefficient(0.002),
        coefficient(0.025),
    );
    let taper = (rate as usize / 200).min(len / 2).max(1);
    for (i, value) in samples.iter_mut().enumerate() {
        match operation {
            Operation::GainUp => *value *= 10f32.powf(3. / 20.),
            Operation::GainDown => *value *= 10f32.powf(-3. / 20.),
            Operation::Normalize => {
                if peak > 0. {
                    *value *= 0.95 / peak;
                }
            }
            Operation::Mute => *value = 0.,
            Operation::FadeIn => *value *= i as f32 / (len - 1).max(1) as f32,
            Operation::FadeOut => *value *= 1. - i as f32 / (len - 1).max(1) as f32,
            Operation::RemoveDc => *value -= dc,
            Operation::Invert => *value = -*value,
            Operation::Compress => {
                let magnitude = value.abs();
                if magnitude > 0.25 {
                    *value = value.signum() * 0.25 * (magnitude / 0.25).powf(0.25);
                }
            }
            Operation::Gate => {
                let original = *value;
                gate_level += (value.abs() - gate_level)
                    * if value.abs() > gate_level {
                        level_rise
                    } else {
                        level_fall
                    };
                let target = if gate_level > 0.025 { 1. } else { 0. };
                gate_gain += (target - gate_gain)
                    * if target > gate_gain {
                        gain_rise
                    } else {
                        gain_fall
                    };
                let mix = (i.min(len - 1 - i) as f32 / taper as f32).clamp(0., 1.);
                *value = original * (1. - mix + gate_gain * mix);
            }
            Operation::SoftClip => *value /= 1. + 0.5 * value.abs(),
            Operation::FadeBoth => {
                let edge = (len / 20).max(1);
                *value *= (i.min(len - 1 - i) as f32 / edge as f32).clamp(0., 1.);
            }
            Operation::Reverse | Operation::Gcd | Operation::HighPass | Operation::LowPass => {}
        }
    }
    if operation == Operation::Reverse {
        samples.reverse();
    }
    for value in samples {
        *value = value.clamp(-1., 1.);
    }
}

pub fn common_fundamental(peaks: &[Peak]) -> Option<f32> {
    let mut list: Vec<_> = peaks
        .iter()
        .filter(|p| p.freq >= 30. && p.amp > 0.)
        .collect();
    if list.is_empty() {
        return None;
    }
    list.sort_by(|a, b| b.amp.total_cmp(&a.amp));
    let mut unique = Vec::new();
    for peak in list {
        if unique
            .iter()
            .all(|other: &&Peak| (other.freq - peak.freq).abs() > peak.freq * 0.005)
        {
            unique.push(peak);
            if unique.len() == 64 {
                break;
            }
        }
    }
    let mut list = unique;
    let max_amp = list[0].amp;
    list.retain(|p| p.amp >= max_amp * 0.08);
    let mut best = None;
    for p in &list {
        for harmonic in 1..=16 {
            let candidate = p.freq / harmonic as f32;
            if candidate < 30. || candidate > 1200. {
                continue;
            }
            let mut weight = 0.;
            let mut error = 0.;
            for q in &list {
                let ratio = q.freq / candidate;
                let nearest = ratio.round().clamp(1., 32.);
                error += q.amp * (ratio - nearest).abs();
                weight += q.amp;
            }
            let residual = error / weight.max(1e-9);
            if residual < 0.075 && best.is_none_or(|b: f32| candidate > b) {
                best = Some(candidate);
            }
        }
    }
    best
}
#[cfg(test)]
mod tests {
    use super::*;
    fn peak(f: f32) -> Peak {
        Peak {
            freq: f,
            amp: 1.,
            ..Default::default()
        }
    }
    #[test]
    fn gcd_fundamental() {
        let f = common_fundamental(&[peak(261.6), peak(329.6), peak(392.)]).unwrap();
        assert!((f - 65.4).abs() < 1.);
    }
    #[test]
    fn lasso_contains() {
        let s = Selection::Lasso {
            points: vec![(0., 60.), (1., 60.), (1., 70.), (0., 70.)],
        };
        assert!(s.contains(0.5, 440.));
        assert!(!s.contains(1.5, 440.));
    }
}
#[cfg(test)]
mod sample_tests {
    use super::*;
    #[test]
    fn transforms_selected_samples() {
        let mut values = [0.2, 0.4, 0.6];
        apply_samples(&mut values, Operation::Reverse, 44100);
        assert_eq!(values, [0.6, 0.4, 0.2]);
        apply_samples(&mut values, Operation::FadeIn, 44100);
        assert_eq!(values[0], 0.);
        assert!((values[2] - 0.2).abs() < 1e-6);
    }
    #[test]
    fn dc_invert_and_compression_change_only_selected_buffer() {
        let mut values = [0.25, 0.5, 0.75];
        apply_samples(&mut values, Operation::RemoveDc, 44100);
        assert!(values.iter().sum::<f32>().abs() < 1e-6);
        apply_samples(&mut values, Operation::Invert, 44100);
        assert!((values[0] - 0.25).abs() < 1e-6);
        let mut loud = [0.1, 1.];
        apply_samples(&mut loud, Operation::Compress, 44100);
        assert_eq!(loud[0], 0.1);
        assert!(loud[1] < 1. && loud[1] > 0.25);
    }
    #[test]
    fn filters_gate_and_edge_tools_change_the_selected_signal() {
        let mut dc = vec![0.5; 44100];
        apply_samples(&mut dc, Operation::HighPass, 44100);
        assert!(dc[22050].abs() < 0.01);
        let mut alternating: Vec<_> = (0..44100)
            .map(|i| if i % 2 == 0 { 0.8 } else { -0.8 })
            .collect();
        apply_samples(&mut alternating, Operation::LowPass, 44100);
        assert!(alternating[22050].abs() < 0.4);
        let mut quiet = vec![0.005; 44100];
        apply_samples(&mut quiet, Operation::Gate, 44100);
        assert!(quiet[22050].abs() < 0.001);
        let mut edge = vec![0.8; 1000];
        apply_samples(&mut edge, Operation::FadeBoth, 44100);
        assert_eq!(edge[0], 0.);
        assert_eq!(edge[999], 0.);
        let mut loud = [0.1, 1.];
        apply_samples(&mut loud, Operation::SoftClip, 44100);
        assert!(loud[0] > 0.09 && loud[1] < 0.7);
    }
}
