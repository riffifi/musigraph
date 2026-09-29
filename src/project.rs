use crate::{
    art::{self, CoverArt},
    audio_io::{self, Audio},
    dsp::yin,
    midi::{self, Note},
    synth,
};
use std::{path::Path, sync::Arc};

pub fn load_track(path: &Path) -> Result<QueueTrack, String> {
    let extension = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (audio, notes, cover) = if extension == "mid" || extension == "midi" {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let notes = midi::parse(&bytes)?;
        let samples = synth::render_midi(&notes, 44100);
        (
            Audio {
                samples: Arc::new(samples),
                rate: 44100,
            },
            notes,
            None,
        )
    } else {
        let (audio, cover) = audio_io::decode_with_art(path)?;
        (audio, Vec::new(), cover)
    };
    let name = path
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("track")
        .to_owned();
    let cover = cover.or_else(|| art::from_sidecar(path));
    Ok(QueueTrack {
        name,
        audio,
        notes,
        cover,
        transition: Transition::default(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    Linear,
    EqualPower,
    Smooth,
}

#[derive(Clone, Copy, Debug)]
pub struct Transition {
    pub seconds: f32,
    pub in_cue: f32,
    pub curve: Curve,
    pub phase_match: bool,
}
impl Default for Transition {
    fn default() -> Self {
        Self {
            seconds: 2.,
            in_cue: 0.,
            curve: Curve::EqualPower,
            phase_match: true,
        }
    }
}

#[derive(Clone)]
pub struct QueueTrack {
    pub name: String,
    pub audio: Audio,
    pub notes: Vec<Note>,
    pub cover: Option<Arc<CoverArt>>,
    pub transition: Transition,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Placement {
    pub start: usize,
    pub end: usize,
    pub overlap: usize,
    pub correlation: f32,
    pub inverted: bool,
}
pub fn fade_weights(curve: Curve, u: f32, correlation: f32) -> (f32, f32) {
    let u = u.clamp(0., 1.);
    let (mut a, mut b) = match curve {
        Curve::Linear => (1. - u, u),
        Curve::EqualPower => (
            (u * std::f32::consts::FRAC_PI_2).cos(),
            (u * std::f32::consts::FRAC_PI_2).sin(),
        ),
        Curve::Smooth => {
            let v = u * u * (3. - 2. * u);
            (1. - v, v)
        }
    };
    if curve == Curve::EqualPower {
        let level = (a * a + b * b + 2. * correlation.clamp(-0.75, 1.) * a * b)
            .sqrt()
            .max(1e-6);
        a /= level;
        b /= level;
    }
    (a, b)
}

fn sample_at(audio: &Audio, position: f64) -> f32 {
    if audio.samples.is_empty() {
        return 0.;
    }
    let i = position.floor() as isize;
    let t = (position - i as f64) as f32;
    let get = |index: isize| -> f32 {
        audio.samples[index.clamp(0, audio.samples.len() as isize - 1) as usize]
    };
    let (a, b, c, d) = (get(i - 1), get(i), get(i + 1), get(i + 2));
    let slope_b = 0.5 * (c - a);
    let slope_c = 0.5 * (d - b);
    let t2 = t * t;
    let t3 = t2 * t;
    ((2. * t3 - 3. * t2 + 1.) * b
        + (t3 - 2. * t2 + t) * slope_b
        + (-2. * t3 + 3. * t2) * c
        + (t3 - t2) * slope_c)
        .clamp(-1., 1.)
}

fn output_len(track: &QueueTrack, rate: u32, first: bool) -> usize {
    let cue = if first {
        0
    } else {
        (track.transition.in_cue.max(0.) * track.audio.rate as f32) as usize
    };
    let remaining = track
        .audio
        .samples
        .len()
        .saturating_sub(cue.min(track.audio.samples.len()));
    ((remaining as f64 * rate as f64 / track.audio.rate as f64).round() as usize).max(1)
}

pub fn layout(tracks: &[QueueTrack], rate: u32) -> Result<Vec<Placement>, String> {
    if tracks.is_empty() || rate == 0 {
        return Err("Queue is empty".into());
    }
    let lengths: Vec<_> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| output_len(t, rate, i == 0))
        .collect();
    let mut places = vec![Placement::default(); tracks.len()];
    places[0].end = lengths[0];
    for i in 1..tracks.len() {
        let requested = (tracks[i].transition.seconds.clamp(0., 30.) * rate as f32) as usize;
        let overlap = requested.min(lengths[i - 1] / 2).min(lengths[i] / 2);
        let start = places[i - 1].end.saturating_sub(overlap);
        let end = start.checked_add(lengths[i]).ok_or("Project is too long")?;
        places[i] = Placement {
            start,
            end,
            overlap,
            ..Placement::default()
        };
    }
    let total = places.last().unwrap().end;
    if total > 1_500_000_000 {
        return Err("Project is too long".into());
    }
    Ok(places)
}

pub fn mix(tracks: &[QueueTrack], rate: u32) -> Result<(Audio, Vec<Placement>), String> {
    let mut places = layout(tracks, rate)?;
    let lengths: Vec<_> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| output_len(t, rate, i == 0))
        .collect();
    let total = places.last().unwrap().end;
    let mut out = vec![0f32; total];
    for (i, track) in tracks.iter().enumerate() {
        let place = places[i];
        let cue = if i == 0 {
            0.
        } else {
            (track.transition.in_cue.max(0.) * track.audio.rate as f32) as f64
        };
        let ratio = track.audio.rate as f64 / rate as f64;
        let overlap = place.overlap;
        let mut rho = 0.;
        if overlap > 0 {
            let stride = (overlap / 2048).max(1);
            let (mut dot, mut aa, mut bb) = (0f64, 0f64, 0f64);
            for j in (0..overlap).step_by(stride) {
                let a = out[place.start + j] as f64;
                let b = sample_at(&track.audio, cue + j as f64 * ratio) as f64;
                dot += a * b;
                aa += a * a;
                bb += b * b;
            }
            if aa > 1e-12 && bb > 1e-12 {
                rho = (dot / (aa * bb).sqrt()).clamp(-1., 1.) as f32;
            }
        }
        let inverted = track.transition.phase_match && rho < -0.7;
        let sign = if inverted { -1. } else { 1. };
        places[i].correlation = rho;
        places[i].inverted = inverted;
        let correlation = (rho * sign).clamp(-0.75, 1.);
        for j in 0..lengths[i] {
            let value = sample_at(&track.audio, cue + j as f64 * ratio) * sign;
            let target = &mut out[place.start + j];
            if j < overlap {
                let u = (j as f32 + 0.5) / overlap as f32;
                let (wa, wb) = fade_weights(track.transition.curve, u, correlation);
                *target = (*target * wa + value * wb).clamp(-1., 1.);
            } else {
                *target = value;
            }
        }
    }
    let edge = (rate as usize / 200).min(total / 2);
    if edge > 0 {
        for i in 0..edge {
            let gain = i as f32 / edge as f32;
            out[i] *= gain;
            out[total - 1 - i] *= gain;
        }
    }
    Ok((
        Audio {
            samples: Arc::new(out),
            rate,
        },
        places,
    ))
}

fn onset_times(audio: &Audio, start_time: f32, end_time: f32) -> Vec<f32> {
    let step = (audio.rate as usize / 40).max(1);
    let start = ((start_time.max(0.) * audio.rate as f32) as usize / step) * step;
    let end = ((end_time.max(start_time) * audio.rate as f32) as usize).min(audio.samples.len());
    let count = end.saturating_sub(start) / step;
    if count < 3 {
        return Vec::new();
    }
    let mut power = Vec::with_capacity(count);
    for i in 0..count {
        let block_start = start + i * step;
        let block_end = (block_start + step).min(audio.samples.len());
        let stride = ((block_end - block_start) / 64).max(1);
        let mut sum = 0f32;
        let mut n = 0usize;
        for j in (block_start..block_end).step_by(stride) {
            let v = audio.samples[j];
            sum += v * v;
            n += 1;
        }
        power.push((sum / n.max(1) as f32).sqrt());
    }
    let threshold = (2..count.saturating_sub(1))
        .map(|i| (power[i] - power[i - 2]).max(0.))
        .fold(0., f32::max)
        * 0.15;
    let mut onset = Vec::new();
    for i in 2..count.saturating_sub(1) {
        let jump = (power[i] - power[i - 2]).max(0.);
        if jump > threshold.max(0.0001) && jump >= (power[i + 1] - power[i - 1]).max(0.) {
            onset.push((jump, (start + i * step) as f32 / audio.rate as f32));
        }
    }
    onset.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut times = Vec::new();
    for (_, time) in onset {
        if times.iter().all(|t: &f32| (*t - time).abs() > 0.12) {
            times.push(time);
            if times.len() >= 16 {
                break;
            }
        }
    }
    times.sort_by(f32::total_cmp);
    times
}
fn beat_period(onsets: &[f32]) -> Option<f32> {
    if onsets.len() < 3 {
        return None;
    }
    let mut bins = [0f32; 46];
    for (i, &a) in onsets.iter().enumerate() {
        for &b in onsets.iter().skip(i + 1) {
            let difference = b - a;
            if !(0.3..=1.2).contains(&difference) {
                continue;
            }
            let bin = ((difference - 0.3) / 0.02).round() as usize;
            bins[bin.min(45)] += 1. / difference.sqrt();
        }
    }
    let (index, votes) = bins
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    (votes >= 1.5).then_some(0.3 + index as f32 * 0.02)
}
fn beat_alignment_penalty(
    duration: f32,
    cue: f32,
    previous_length: f32,
    period: f32,
    previous_onsets: &[f32],
    next_onsets: &[f32],
) -> f32 {
    let beats = duration / period;
    let length_error = [0.5, 1., 2., 4., 8., 16.]
        .into_iter()
        .map(|count| ((beats - count) / count).abs())
        .fold(f32::INFINITY, f32::min)
        .min(1.);
    let mut penalty = 0.12 * length_error;
    if let (Some(&outgoing), Some(&incoming)) = (
        previous_onsets.iter().min_by(|a, b| {
            (*a - (previous_length - duration))
                .abs()
                .total_cmp(&(*b - (previous_length - duration)).abs())
        }),
        next_onsets
            .iter()
            .min_by(|a, b| (*a - cue).abs().total_cmp(&(*b - cue).abs())),
    ) {
        let outgoing_phase = (previous_length - duration - outgoing).rem_euclid(period);
        let incoming_phase = (cue - incoming).rem_euclid(period);
        let difference = (outgoing_phase - incoming_phase).abs();
        penalty += 0.10 * difference.min(period - difference) / period;
    }
    penalty
}
fn candidate_score(previous: &QueueTrack, next: &QueueTrack, transition: Transition) -> f32 {
    let prev_audio = &previous.audio;
    let next_audio = &next.audio;
    let prev_len = prev_audio.samples.len() as f32 / prev_audio.rate as f32;
    let overlap = transition.seconds;
    if overlap <= 0.01 {
        return 20.;
    }
    let mut aa = 0f64;
    let mut bb = 0f64;
    let mut ab = 0f64;
    let mut level_a = 0f64;
    let mut level_b = 0f64;
    let mut contrast = 0f32;
    let mut prior_a = 0f32;
    let mut prior_b = 0f32;
    let n = 128;
    for j in 0..n {
        let u = (j as f32 + 0.5) / n as f32;
        let ta = (prev_len - overlap + u * overlap).max(0.);
        let tb = transition.in_cue + u * overlap;
        let energy = |audio: &Audio, time: f32| {
            let mut sum = 0.;
            for k in 0..8 {
                let offset = (k as f32 - 3.5) * 0.0015;
                let v = sample_at(audio, ((time + offset).max(0.) * audio.rate as f32) as f64);
                sum += v * v;
            }
            (sum / 8.).sqrt()
        };
        let ea = energy(prev_audio, ta);
        let eb = energy(next_audio, tb);
        level_a += (ea * ea) as f64;
        level_b += (eb * eb) as f64;
        aa += (ea * ea) as f64;
        bb += (eb * eb) as f64;
        ab += (ea * eb) as f64;
        if j > 0 {
            contrast += ((ea - prior_a).abs() - (eb - prior_b).abs()).abs();
        }
        prior_a = ea;
        prior_b = eb;
    }
    let balance = ((level_a + 0.01) / (level_b + 0.01)).ln().abs() as f32;
    let coherence = if aa > 1e-7 && bb > 1e-7 {
        (ab / (aa * bb).sqrt()) as f32
    } else {
        0.
    };
    let boundary_rms = |audio: &Audio, time: f32| {
        let mut sum = 0.;
        for k in 0..16 {
            let offset = (k as f32 - 7.5) * 0.001;
            let v = sample_at(audio, ((time + offset).max(0.) * audio.rate as f32) as f64);
            sum += v * v;
        }
        (sum / 16.).sqrt()
    };
    let outgoing_start = boundary_rms(prev_audio, (prev_len - overlap).max(0.));
    let incoming_start = boundary_rms(next_audio, transition.in_cue);
    let duration_cost = (overlap / 2.).ln().abs() * 0.06;
    let correlation_samples =
        ((overlap * prev_audio.rate.min(next_audio.rate) as f32) as usize).clamp(64, 1024);
    let (mut raw_a, mut raw_b, mut raw_ab) = (0f64, 0f64, 0f64);
    for j in 0..correlation_samples {
        let u = (j as f32 + 0.5) / correlation_samples as f32;
        let ta = (prev_len - overlap + u * overlap).max(0.);
        let tb = transition.in_cue + u * overlap;
        let a = sample_at(prev_audio, ta as f64 * prev_audio.rate as f64) as f64;
        let b = sample_at(next_audio, tb as f64 * next_audio.rate as f64) as f64;
        raw_a += a * a;
        raw_b += b * b;
        raw_ab += a * b;
    }
    let mut correlation = if raw_a > 1e-9 && raw_b > 1e-9 {
        (raw_ab / (raw_a * raw_b).sqrt()) as f32
    } else {
        0.
    };
    let inverted = transition.phase_match && correlation < -0.7;
    if inverted {
        correlation = -correlation;
    }
    let sign = if inverted { -1. } else { 1. };
    let mut peak = 0f32;
    let mut mixed_power = 0f64;
    for j in 0..n {
        for sub in [0.25, 0.75] {
            let u = (j as f32 + sub) / n as f32;
            let ta = (prev_len - overlap + u * overlap).max(0.);
            let tb = transition.in_cue + u * overlap;
            let a = sample_at(prev_audio, ta as f64 * prev_audio.rate as f64);
            let b = sample_at(next_audio, tb as f64 * next_audio.rate as f64) * sign;
            let (wa, wb) = fade_weights(transition.curve, u, correlation);
            let mixed = a * wa + b * wb;
            peak = peak.max(mixed.abs());
            mixed_power += (mixed * mixed) as f64;
        }
    }
    let mix_rms = (mixed_power / (n * 2) as f64).sqrt() as f32;
    let target_rms = ((level_a / n as f64).sqrt() + (level_b / n as f64).sqrt()) as f32 * 0.5;
    let level_shift = ((mix_rms + 0.01) / (target_rms + 0.01)).ln().abs();
    let mid_gain = if transition.curve == Curve::EqualPower {
        1.
    } else {
        (0.5 + 0.5 * correlation).sqrt()
    };
    0.4 * balance
        + 0.4 * (1. - coherence)
        + 0.35 * (outgoing_start + incoming_start)
        + 3. * (peak - 0.95).max(0.)
        + 0.1 * contrast
        + 0.6 * (1. - mid_gain).abs()
        + 0.5 * level_shift
        + duration_cost
}
fn stable_pitch(audio: &Audio, time: f32) -> Option<f32> {
    let size = (audio.rate as usize / 10).clamp(2048, 8192);
    if audio.samples.len() < size + audio.rate as usize / 20 {
        return None;
    }
    let first = ((time.max(0.) * audio.rate as f32) as usize)
        .min(audio.samples.len() - size - audio.rate as usize / 20);
    let next = first + audio.rate as usize / 20;
    let (a, confidence_a) = yin::estimate(&audio.samples[first..first + size], audio.rate)?;
    let (b, confidence_b) = yin::estimate(&audio.samples[next..next + size], audio.rate)?;
    if confidence_a < 0.85 || confidence_b < 0.85 || (12. * (a / b).log2()).abs() > 0.5 {
        return None;
    }
    Some((a + b) * 0.5)
}
fn harmonic_penalty(previous: &QueueTrack, next: &QueueTrack, transition: Transition) -> f32 {
    let prev_len = previous.audio.samples.len() as f32 / previous.audio.rate as f32;
    let Some(a) = stable_pitch(&previous.audio, prev_len - transition.seconds * 0.4) else {
        return 0.;
    };
    let Some(b) = stable_pitch(&next.audio, transition.in_cue + transition.seconds * 0.4) else {
        return 0.;
    };
    let interval = (12. * (a / b).log2()).round() as i32;
    let class = interval.rem_euclid(12);
    if [0, 3, 4, 5, 7, 8, 9].contains(&class) {
        0.
    } else {
        0.12
    }
}
pub fn suggest_transition(previous: &QueueTrack, next: &QueueTrack) -> Transition {
    let prev_len = previous.audio.samples.len() as f32 / previous.audio.rate as f32;
    let next_len = next.audio.samples.len() as f32 / next.audio.rate as f32;
    let previous_available = (prev_len - previous.transition.in_cue.max(0.)).max(0.);
    let max_duration = (previous_available.min(next_len) * 0.5).min(12.);
    if max_duration <= 0.05 {
        return Transition {
            seconds: 0.,
            ..next.transition
        };
    }
    let next_onsets = onset_times(&next.audio, 0., next_len.min(12.));
    let previous_onsets = onset_times(&previous.audio, (prev_len - 12.).max(0.), prev_len);
    let beat = beat_period(&previous_onsets).or_else(|| beat_period(&next_onsets));
    let mut cues = vec![0.];
    cues.extend(next_onsets);
    let mut durations = vec![0.25, 0.5, 1., 2., 3., 4., 6., 8., 12.];
    if let Some(period) = beat {
        durations.extend([0.5, 1., 2., 4., 8., 16.].map(|beats| beats * period));
    }
    durations.sort_by(f32::total_cmp);
    durations.dedup_by(|a, b| (*a - *b).abs() < 0.04);
    let baseline_duration = next.transition.seconds.clamp(0.05, max_duration);
    let best = Transition {
        seconds: baseline_duration,
        in_cue: next
            .transition
            .in_cue
            .clamp(0., (next_len - 2. * baseline_duration).max(0.)),
        ..next.transition
    };
    let baseline = candidate_score(previous, next, best) + harmonic_penalty(previous, next, best);
    let adjusted = |transition: Transition| {
        beat.map(|period| {
            beat_alignment_penalty(
                transition.seconds,
                transition.in_cue,
                prev_len,
                period,
                &previous_onsets,
                &cues[1..],
            )
        })
        .unwrap_or(0.)
    };
    let mut ranked = Vec::new();
    for duration in durations {
        if duration > max_duration {
            continue;
        }
        for &onset in &cues {
            for fraction in [0., 0.25, 0.5, 0.75] {
                let cue =
                    (onset - duration * fraction).clamp(0., (next_len - 2. * duration).max(0.));
                for curve in [Curve::Linear, Curve::EqualPower, Curve::Smooth] {
                    let candidate = Transition {
                        seconds: duration,
                        in_cue: cue,
                        curve,
                        phase_match: true,
                    };
                    let score = candidate_score(previous, next, candidate);
                    ranked.push((score, candidate));
                }
            }
        }
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut chosen = best;
    let mut best_score = baseline + adjusted(best);
    for (score, candidate) in ranked.into_iter().take(24) {
        let total = score + harmonic_penalty(previous, next, candidate);
        let preferred = total + adjusted(candidate);
        if total <= baseline + 1e-6 && preferred < best_score {
            best_score = preferred;
            chosen = candidate;
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track(name: &str, data: Vec<f32>, transition: Transition) -> QueueTrack {
        QueueTrack {
            name: name.into(),
            audio: Audio {
                samples: Arc::new(data),
                rate: 1000,
            },
            notes: Vec::new(),
            cover: None,
            transition,
        }
    }
    #[test]
    fn overlapping_tracks_are_continuous_and_phase_matched() {
        let a = track("a", vec![0.5; 1000], Transition::default());
        let b = track(
            "b",
            vec![-0.5; 1000],
            Transition {
                seconds: 0.2,
                ..Transition::default()
            },
        );
        let (mix, places) = mix(&[a, b], 1000).unwrap();
        assert_eq!(mix.samples.len(), 1800);
        assert_eq!(places[1].start, 800);
        assert!(places[1].inverted);
        assert!((mix.samples[900] - 0.5).abs() < 0.02);
        assert!((mix.samples[799] - mix.samples[800]).abs() < 0.02);
        assert!((mix.samples[999] - mix.samples[1000]).abs() < 0.02);
    }
    #[test]
    fn cue_and_resampling_change_placement() {
        let a = track("a", vec![0.; 1000], Transition::default());
        let mut b = track(
            "b",
            vec![0., 0., 1., 1.],
            Transition {
                seconds: 0.,
                in_cue: 0.002,
                ..Transition::default()
            },
        );
        b.audio.rate = 1000;
        let (mix, places) = mix(&[a, b], 1000).unwrap();
        assert_eq!(places[1].start, 1000);
        assert_eq!(mix.samples.len(), 1002);
    }
    #[test]
    fn automatic_transition_respects_bounds_and_improves_candidate_score() {
        let pulse = |shift: usize| {
            (0..8000)
                .map(|i| {
                    let phase = (i + shift) % 500;
                    if phase < 40 {
                        0.6 * (1. - phase as f32 / 40.)
                    } else {
                        0.
                    }
                })
                .collect()
        };
        let a = track("a", pulse(0), Transition::default());
        let b = track("b", pulse(210), Transition::default());
        let suggested = suggest_transition(&a, &b);
        assert!(suggested.seconds > 0. && suggested.seconds <= 4.);
        assert!(suggested.in_cue >= 0. && suggested.in_cue + 2. * suggested.seconds <= 8.);
        assert!(
            candidate_score(&a, &b, suggested) + harmonic_penalty(&a, &b, suggested)
                <= candidate_score(&a, &b, b.transition)
                    + harmonic_penalty(&a, &b, b.transition)
                    + 1e-6
        );
        let mut b = b;
        b.transition = suggested;
        let (_, places) = mix(&[a, b], 1000).unwrap();
        assert!((places[1].overlap as f32 / 1000. - suggested.seconds).abs() < 0.002);
    }
    #[test]
    fn quiet_onsets_provide_a_beat_grid() {
        let samples: Vec<f32> = (0..8000)
            .map(|i| {
                let phase = i % 500;
                if phase < 25 {
                    0.008 * (1. - phase as f32 / 25.)
                } else {
                    0.
                }
            })
            .collect();
        let audio = Audio {
            samples: Arc::new(samples),
            rate: 1000,
        };
        let onsets = onset_times(&audio, 0., 8.);
        assert!(onsets.len() >= 8);
        assert!((beat_period(&onsets).unwrap() - 0.5).abs() < 0.05);
    }
    #[test]
    fn beat_alignment_prefers_matching_downbeats() {
        let outgoing = [6., 6.5, 7., 7.5, 8.];
        let incoming = [0., 0.5, 1., 1.5, 2.];
        let aligned = beat_alignment_penalty(1., 1.5, 8., 0.5, &outgoing, &incoming);
        let shifted = beat_alignment_penalty(1., 1.25, 8., 0.5, &outgoing, &incoming);
        assert!(aligned < shifted);
    }
    #[test]
    fn automatic_transition_skips_a_silent_intro() {
        let pulse = |i: usize| {
            let phase = i % 500;
            if phase < 40 {
                0.55 * (1. - phase as f32 / 40.)
            } else {
                0.
            }
        };
        let a = track("out", (0..8000).map(pulse).collect(), Transition::default());
        let b = track(
            "in",
            (0..8000)
                .map(|i| if i < 2000 { 0. } else { pulse(i - 2000) })
                .collect(),
            Transition::default(),
        );
        let suggested = suggest_transition(&a, &b);
        assert!(suggested.in_cue >= 1.5, "cue was {}", suggested.in_cue);
    }
    #[test]
    fn fade_weights_have_expected_endpoints_and_middle() {
        for curve in [Curve::Linear, Curve::EqualPower, Curve::Smooth] {
            assert_eq!(fade_weights(curve, 0., 0.), (1., 0.));
            let end = fade_weights(curve, 1., 0.);
            assert!(end.0.abs() < 1e-6 && (end.1 - 1.).abs() < 1e-6);
        }
        let (a, b) = fade_weights(Curve::EqualPower, 0.5, 0.);
        assert!((a * a + b * b - 1.).abs() < 1e-5);
    }
    #[test]
    fn first_track_has_no_incoming_cue_after_reordering() {
        let first = track(
            "first",
            vec![0.25; 1000],
            Transition {
                in_cue: 0.5,
                ..Transition::default()
            },
        );
        let places = layout(&[first.clone()], 1000).unwrap();
        assert_eq!(places[0].end, 1000);
        let (audio, _) = mix(&[first], 1000).unwrap();
        assert_eq!(audio.samples.len(), 1000);
    }
}
