use super::gpu::{Color, Gpu};

pub fn trace(g: &mut Gpu, points: &[(f32, f32)], c: Color) {
    for w in points.windows(2) {
        g.line(w[0].0, w[0].1, w[1].0, w[1].1, c);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extremum {
    pub value: f32,
    pub index: usize,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangePeaks {
    pub low: Extremum,
    pub high: Extremum,
    sum_squares: f64,
    count: usize,
}
impl RangePeaks {
    pub fn rms(self) -> f32 {
        (self.sum_squares / self.count as f64).sqrt() as f32
    }
    fn sample(value: f32, index: usize) -> Self {
        let point = Extremum { value, index };
        Self {
            low: point,
            high: point,
            sum_squares: (value as f64) * (value as f64),
            count: 1,
        }
    }
    fn merge(self, other: Self) -> Self {
        Self {
            low: if self.low.value <= other.low.value {
                self.low
            } else {
                other.low
            },
            high: if self.high.value >= other.high.value {
                self.high
            } else {
                other.high
            },
            sum_squares: self.sum_squares + other.sum_squares,
            count: self.count + other.count,
        }
    }
}
const BLOCK: usize = 256;
pub struct WaveformPeaks {
    levels: Vec<Vec<RangePeaks>>,
}
impl WaveformPeaks {
    pub fn build(samples: &[f32]) -> Self {
        let mut base = Vec::with_capacity(samples.len().div_ceil(BLOCK));
        for (block, slice) in samples.chunks(BLOCK).enumerate() {
            let mut peaks = RangePeaks::sample(slice[0], block * BLOCK);
            for (offset, &value) in slice.iter().enumerate().skip(1) {
                peaks = peaks.merge(RangePeaks::sample(value, block * BLOCK + offset));
            }
            base.push(peaks);
        }
        let mut levels = vec![base];
        while levels.last().is_some_and(|level| level.len() > 1) {
            let prev = levels.last().unwrap();
            let next = prev
                .chunks(2)
                .map(|pair| {
                    if pair.len() == 2 {
                        pair[0].merge(pair[1])
                    } else {
                        pair[0]
                    }
                })
                .collect();
            levels.push(next);
        }
        Self { levels }
    }
    pub fn range(&self, samples: &[f32], start: usize, end: usize) -> Option<RangePeaks> {
        let start = start.min(samples.len());
        let end = end.min(samples.len());
        if start >= end {
            return None;
        }
        let mut result = None;
        let mut add = |part: RangePeaks| {
            result = Some(match result {
                Some(current) => RangePeaks::merge(current, part),
                None => part,
            });
        };
        let first_full = start.div_ceil(BLOCK);
        let last_full = end / BLOCK;
        if first_full > last_full {
            for i in start..end {
                add(RangePeaks::sample(samples[i], i));
            }
            return result;
        }
        let left_end = (first_full * BLOCK).min(end);
        for i in start..left_end {
            add(RangePeaks::sample(samples[i], i));
        }
        let mut block = first_full;
        while block < last_full {
            let remaining = last_full - block;
            let level =
                (usize::BITS - 1 - remaining.leading_zeros()).min(block.trailing_zeros()) as usize;
            add(self.levels[level][block >> level]);
            block += 1 << level;
        }
        for i in last_full * BLOCK..end {
            add(RangePeaks::sample(samples[i], i));
        }
        result
    }
}

pub fn pitch_segments(
    frames: &[crate::dsp::stft::Frame],
    left: f32,
    right: f32,
    width: f32,
    hop: f32,
) -> Vec<((f32, f32), (f32, f32))> {
    if right <= left || width <= 0. {
        return Vec::new();
    }
    let first = frames
        .partition_point(|frame| frame.time < left)
        .saturating_sub(1);
    let end = (frames.partition_point(|frame| frame.time <= right) + 1).min(frames.len());
    let mut result = Vec::new();
    let mut previous: Option<(usize, f32, f32)> = None;
    let mut anchor: Option<(f32, f32)> = None;
    let mut drew = false;
    let x = |time: f32| (time - left) / (right - left) * width;
    for (i, frame) in frames[first..end].iter().enumerate() {
        let index = first + i;
        let midi = frame
            .f0
            .filter(|freq| freq.is_finite() && *freq > 0.)
            .map(|freq| 69. + 12. * (freq / 440.).log2());
        let Some(midi) = midi else {
            if !drew {
                if let Some((time, note)) = anchor {
                    if (left..=right).contains(&time) {
                        result.push(((x(time), note), (x(time), note)));
                    }
                }
            }
            previous = None;
            anchor = None;
            drew = false;
            continue;
        };
        let joins = previous.is_some_and(|(last_index, time, pitch)| {
            index == last_index + 1 && frame.time - time <= hop * 1.6 && (midi - pitch).abs() <= 5.
        });
        if !joins {
            if !drew {
                if let Some((time, note)) = anchor {
                    if (left..=right).contains(&time) {
                        result.push(((x(time), note), (x(time), note)));
                    }
                }
            }
            anchor = Some((frame.time, midi));
            drew = false;
        } else if let Some((from_time, from_midi)) = anchor {
            if x(frame.time) - x(from_time) >= 0.75 {
                let a = from_time.max(left);
                let b = frame.time.min(right);
                if b >= a {
                    let ratio_a = ((a - from_time) / (frame.time - from_time)).clamp(0., 1.);
                    let ratio_b = ((b - from_time) / (frame.time - from_time)).clamp(0., 1.);
                    result.push((
                        (x(a), from_midi + (midi - from_midi) * ratio_a),
                        (x(b), from_midi + (midi - from_midi) * ratio_b),
                    ));
                    drew = true;
                }
                anchor = Some((frame.time, midi));
            }
        }
        previous = Some((index, frame.time, midi));
    }
    if !drew {
        if let Some((time, note)) = anchor {
            if (left..=right).contains(&time) {
                result.push(((x(time), note), (x(time), note)));
            }
        }
    }
    result
}

pub fn clipped_note(start: f32, end: f32, left: f32, right: f32) -> Option<(f32, f32)> {
    let a = start.max(left);
    let b = end.min(right);
    (b > a).then_some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waveform_peaks_match_direct_scan() {
        let samples: Vec<_> = (0..4099)
            .map(|i| ((i * 173 % 997) as f32 - 500.) / 500.)
            .collect();
        let peaks = WaveformPeaks::build(&samples);
        let mut ranges = vec![
            (0, 1),
            (0, 4099),
            (7, 8),
            (7, 257),
            (256, 769),
            (511, 2048),
            (4000, 4099),
        ];
        let mut seed = 17u32;
        for _ in 0..256 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let start = seed as usize % samples.len();
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let end = start + 1 + seed as usize % (samples.len() - start);
            ranges.push((start, end));
        }
        for (start, end) in ranges {
            let got = peaks.range(&samples, start, end).unwrap();
            let low = samples[start..end]
                .iter()
                .copied()
                .fold(f32::INFINITY, f32::min);
            let high = samples[start..end]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            assert_eq!((got.low.value, got.high.value), (low, high));
            assert_eq!(samples[got.low.index], low);
            assert_eq!(samples[got.high.index], high);
            let rms = (samples[start..end]
                .iter()
                .map(|&v| (v as f64) * (v as f64))
                .sum::<f64>()
                / (end - start) as f64)
                .sqrt() as f32;
            assert!((got.rms() - rms).abs() < 0.00001);
        }
    }
    #[test]
    fn isolated_peak_does_not_fill_rms_envelope() {
        let mut samples = vec![0.; 256];
        samples[83] = 1.;
        let range = WaveformPeaks::build(&samples)
            .range(&samples, 0, samples.len())
            .unwrap();
        assert_eq!(range.high.value, 1.);
        assert!((range.rms() - 0.0625).abs() < 0.00001);
    }
    #[test]
    fn clipped_notes_and_visible_frames() {
        assert_eq!(clipped_note(-10., 2., 1., 3.), Some((1., 2.)));
        assert_eq!(clipped_note(2., 10., 1., 3.), Some((2., 3.)));
        assert_eq!(clipped_note(-10., 0., 1., 3.), None);
    }
    #[test]
    fn pitch_breaks_at_silence_and_stays_inside_zoom() {
        let frames: Vec<_> = [Some(440.), Some(440.), None, Some(660.), Some(660.)]
            .into_iter()
            .enumerate()
            .map(|(i, f0)| crate::dsp::stft::Frame {
                time: i as f32 * 0.1,
                f0,
                ..Default::default()
            })
            .collect();
        let segments = pitch_segments(&frames, 0.05, 0.35, 300., 0.1);
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().all(|(a, b)| a.0 >= 0. && b.0 <= 300.));
        let first_end = (segments[0].1).0;
        let second_start = (segments[1].0).0;
        assert!(first_end < second_start);
    }
}
