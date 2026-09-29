use super::peaks::Peak;
pub fn link(previous: &[Peak], current: &mut [Peak], next: &mut u32) {
    let mut used = vec![false; previous.len()];
    for p in current {
        let mut best = None;
        let mut distance = 0.09;
        for (i, q) in previous.iter().enumerate() {
            let d = (p.freq / q.freq).ln().abs();
            if !used[i] && d < distance {
                distance = d;
                best = Some(i);
            }
        }
        if let Some(i) = best {
            p.track = previous[i].track;
            used[i] = true;
        } else {
            p.track = *next;
            *next += 1;
        }
    }
}
