pub fn estimate(samples: &[f32], rate: u32) -> Option<(f32, f32)> {
    let factor = (rate / 12000).max(1) as usize;
    let reduced: Vec<f32> = samples.iter().step_by(factor).copied().collect();
    let samples = &reduced;
    let rate = rate / factor as u32;
    let min_tau = (rate / 1200).max(2) as usize;
    let max_tau = ((rate / 55) as usize).min(samples.len() / 2);
    if max_tau <= min_tau {
        return None;
    }
    let mut diff = vec![0.; max_tau + 1];
    for tau in 1..=max_tau {
        let mut d = 0.;
        for i in 0..samples.len() - max_tau {
            let v = samples[i] - samples[i + tau];
            d += v * v;
        }
        diff[tau] = d;
    }
    let mut sum = 0.;
    let mut best = None;
    for tau in 1..=max_tau {
        sum += diff[tau];
        let cmnd = if sum > 0. {
            diff[tau] * tau as f32 / sum
        } else {
            1.
        };
        diff[tau] = cmnd;
        if tau >= min_tau && cmnd < 0.15 && best.is_none() {
            best = Some(tau);
        }
    }
    let tau = best?;
    let mut t = tau;
    while t + 1 <= max_tau && diff[t + 1] < diff[t] {
        t += 1;
    }
    let refined = if t > 1 && t < max_tau {
        let a = diff[t - 1];
        let b = diff[t];
        let c = diff[t + 1];
        t as f32 + (a - c) / (2. * (a - 2. * b + c)).max(1e-9)
    } else {
        t as f32
    };
    Some((rate as f32 / refined, 1. - diff[t]))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tone() {
        let v: Vec<f32> = (0..4096)
            .map(|i| (i as f32 * 440. * std::f32::consts::TAU / 44100.).sin())
            .collect();
        assert!((estimate(&v, 44100).unwrap().0 - 440.).abs() < 2.);
    }
}
