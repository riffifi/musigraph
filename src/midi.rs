use std::collections::HashMap;
#[derive(Clone, Debug)]
pub struct Note {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub velocity: u8,
    pub channel: u8,
}
#[derive(Clone, Debug)]
struct Event {
    tick: u64,
    kind: u8,
    channel: u8,
    a: u8,
    b: u8,
    tempo: u32,
}
fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
fn var(b: &[u8], p: &mut usize) -> Result<u32, String> {
    let mut v = 0;
    for _ in 0..4 {
        let c = *b.get(*p).ok_or("truncated variable length")?;
        *p += 1;
        v = (v << 7) | (c & 127) as u32;
        if c & 128 == 0 {
            return Ok(v);
        }
    }
    Err("invalid variable length".into())
}
pub fn parse(bytes: &[u8]) -> Result<Vec<Note>, String> {
    if bytes.len() < 14 || &bytes[..4] != b"MThd" {
        return Err("invalid MIDI header".into());
    }
    let h = be32(&bytes[4..8]) as usize;
    if h < 6 || bytes.len() < 8 + h {
        return Err("invalid MIDI header length".into());
    }
    let format = be16(&bytes[8..10]);
    if format > 1 {
        return Err("only SMF 0/1 supported".into());
    }
    let tracks = be16(&bytes[10..12]) as usize;
    let div = be16(&bytes[12..14]);
    if div & 0x8000 != 0 || div == 0 {
        return Err("SMPTE timing unsupported".into());
    }
    let mut pos = 8 + h;
    let mut events = Vec::new();
    for _ in 0..tracks {
        if pos + 8 > bytes.len() || &bytes[pos..pos + 4] != b"MTrk" {
            return Err("missing track".into());
        }
        let len = be32(&bytes[pos + 4..pos + 8]) as usize;
        pos += 8;
        if pos + len > bytes.len() {
            return Err("truncated track".into());
        }
        let track = &bytes[pos..pos + len];
        pos += len;
        let mut p = 0;
        let mut tick = 0u64;
        let mut running = 0u8;
        while p < track.len() {
            tick += var(track, &mut p)? as u64;
            let lead = *track.get(p).ok_or("truncated event")?;
            let status = if lead & 0x80 != 0 {
                p += 1;
                lead
            } else {
                if running == 0 {
                    return Err("missing running status".into());
                }
                running
            };
            if status == 0xff {
                let ty = *track.get(p).ok_or("truncated meta")?;
                p += 1;
                let n = var(track, &mut p)? as usize;
                if p + n > track.len() {
                    return Err("truncated meta".into());
                }
                if ty == 0x51 && n == 3 {
                    let tempo = ((track[p] as u32) << 16)
                        | ((track[p + 1] as u32) << 8)
                        | track[p + 2] as u32;
                    events.push(Event {
                        tick,
                        kind: 0xff,
                        channel: 0,
                        a: 0,
                        b: 0,
                        tempo,
                    });
                }
                p += n;
                continue;
            }
            if status == 0xf0 || status == 0xf7 {
                let n = var(track, &mut p)? as usize;
                p = p.checked_add(n).ok_or("event overflow")?;
                if p > track.len() {
                    return Err("truncated sysex".into());
                }
                continue;
            }
            if status & 0xf0 == 0xf0 {
                return Err("unsupported system event".into());
            }
            running = status;
            let kind = status & 0xf0;
            let channel = status & 15;
            let a = *track.get(p).ok_or("truncated event")?;
            p += 1;
            let b = if kind == 0xc0 || kind == 0xd0 {
                0
            } else {
                let b = *track.get(p).ok_or("truncated event")?;
                p += 1;
                b
            };
            if kind == 0x80 || kind == 0x90 {
                events.push(Event {
                    tick,
                    kind,
                    channel,
                    a,
                    b,
                    tempo: 0,
                });
            }
        }
    }
    events.sort_by_key(|e| {
        (
            e.tick,
            if e.kind == 0xff {
                0
            } else if e.kind == 0x80 || e.b == 0 {
                1
            } else {
                2
            },
        )
    });
    let mut t = 0f64;
    let mut prev = 0u64;
    let mut tempo = 500000u32;
    let mut active: HashMap<(u8, u8), Vec<(f64, u8)>> = HashMap::new();
    let mut notes = Vec::new();
    for e in events {
        t += (e.tick - prev) as f64 * tempo as f64 / (div as f64 * 1e6);
        prev = e.tick;
        if e.kind == 0xff {
            tempo = e.tempo.max(1);
            continue;
        }
        let key = (e.channel, e.a);
        if e.kind == 0x90 && e.b > 0 {
            active.entry(key).or_default().push((t, e.b));
        } else if let Some(v) = active.get_mut(&key) {
            if !v.is_empty() {
                let (start, velocity) = v.remove(0);
                notes.push(Note {
                    start,
                    end: t,
                    pitch: e.a,
                    velocity,
                    channel: e.channel,
                });
            }
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(notes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tempo_running() {
        let mut b = b"MThd\0\0\0\x06\0\x01\0\x01\x01\xe0MTrk\0\0\0\x16".to_vec();
        b.extend_from_slice(&[
            0, 0xff, 0x51, 3, 0x07, 0xa1, 0x20, 0, 0x90, 60, 100, 0x83, 0x60, 60, 0, 0, 0xff, 0x2f,
            0,
        ]);
        b[21] = 19;
        let n = parse(&b).unwrap();
        assert_eq!(n.len(), 1);
        assert!((n[0].end - 0.5).abs() < 1e-6);
    }
}
