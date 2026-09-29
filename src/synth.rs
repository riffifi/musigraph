use crate::midi::Note;
pub fn midi_sample(notes: &[Note], t: f64) -> f32 {
    let mut out = 0.;
    for n in notes.iter().filter(|n| n.start <= t && t < n.end) {
        let dt = (t - n.start) as f32;
        let f = 440. * 2f32.powf((n.pitch as f32 - 69.) / 12.);
        let env = (dt * 80.).min(1.) * ((n.end - t) as f32 * 25.).min(1.);
        let a = n.velocity as f32 / 127. * env;
        for k in 1..=5 {
            out += a * (std::f32::consts::TAU * f * k as f32 * dt).sin() / (k * k) as f32 * 0.18;
        }
    }
    out.clamp(-1., 1.)
}
pub fn render_midi(notes: &[Note], rate: u32) -> Vec<f32> {
    let end = notes.iter().map(|n| n.end).fold(0., f64::max) + 0.1;
    let mut out = vec![0.; (end * rate as f64) as usize];
    for n in notes {
        let start = (n.start * rate as f64).ceil() as usize;
        let end = (n.end * rate as f64).ceil() as usize;
        let freq = 440. * 2f32.powf((n.pitch as f32 - 69.) / 12.);
        let delta = std::f32::consts::TAU * freq / rate as f32;
        let (sd, cd) = delta.sin_cos();
        let phase = std::f32::consts::TAU * freq * (start as f64 / rate as f64 - n.start) as f32;
        let (mut si, mut co) = phase.sin_cos();
        for i in start..end.min(out.len()) {
            let dt = (i as f64 / rate as f64 - n.start) as f32;
            let tail = (n.end - i as f64 / rate as f64) as f32;
            let env = (dt * 80.).min(1.) * (tail * 25.).min(1.);
            let amp = n.velocity as f32 / 127. * env * 0.18;
            let (mut hs, mut hc) = (si, co);
            for k in 1..=5 {
                out[i] += amp * hs / (k * k) as f32;
                let next_s = hs * co + hc * si;
                hc = hc * co - hs * si;
                hs = next_s;
            }
            let next_s = si * cd + co * sd;
            co = co * cd - si * sd;
            si = next_s;
            if i % 4096 == 0 {
                let inv = (si * si + co * co).sqrt().recip();
                si *= inv;
                co *= inv;
            }
        }
    }
    for x in &mut out {
        *x = x.clamp(-1., 1.);
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn midi_oscillator_matches_direct_sum() {
        let notes = vec![
            Note {
                start: 0.,
                end: 0.5,
                pitch: 69,
                velocity: 100,
                channel: 0,
            },
            Note {
                start: 0.1,
                end: 0.4,
                pitch: 72,
                velocity: 80,
                channel: 0,
            },
        ];
        let samples = render_midi(&notes, 44100);
        for i in (0..samples.len()).step_by(97) {
            let expected = midi_sample(&notes, i as f64 / 44100.);
            assert!((samples[i] - expected).abs() < 0.01, "{i}");
        }
    }
}
