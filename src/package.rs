use crate::{
    audio_io::Audio,
    edit::Operation,
    midi::Note,
    project::{Curve, QueueTrack, Transition},
};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
    sync::Arc,
};

const MAX_TRACKS: usize = 128;
const MAX_SAMPLES: usize = 1_500_000_000;

#[derive(Clone)]
pub struct Patch {
    pub start: usize,
    pub before: Arc<Vec<f32>>,
    pub after: Arc<Vec<f32>>,
    pub marker_id: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct EditMark {
    pub id: u32,
    pub start: usize,
    pub end: usize,
    pub operation: Operation,
    pub active: bool,
}

#[derive(Clone)]
pub struct Package {
    pub tracks: Vec<QueueTrack>,
    pub master: Audio,
    pub undo: Vec<Patch>,
    pub redo: Vec<Patch>,
    pub marks: Vec<EditMark>,
    pub selected_track: usize,
    pub selection: Option<(f32, f32)>,
    pub loop_region: (f32, f32),
    pub looping: bool,
    pub zoom: f32,
    pub pan: f32,
    pub partials: usize,
    pub fft_size: usize,
    pub hop: usize,
}

fn put<W: Write>(w: &mut W, bytes: &[u8]) -> Result<(), String> {
    w.write_all(bytes).map_err(|e| e.to_string())
}
fn take<R: Read>(r: &mut R, bytes: &mut [u8]) -> Result<(), String> {
    r.read_exact(bytes).map_err(|e| e.to_string())
}
fn put_u32<W: Write>(w: &mut W, v: u32) -> Result<(), String> {
    put(w, &v.to_le_bytes())
}
fn put_u64<W: Write>(w: &mut W, v: u64) -> Result<(), String> {
    put(w, &v.to_le_bytes())
}
fn put_f32<W: Write>(w: &mut W, v: f32) -> Result<(), String> {
    put(w, &v.to_le_bytes())
}
fn get_u32<R: Read>(r: &mut R) -> Result<u32, String> {
    let mut b = [0; 4];
    take(r, &mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn get_u64<R: Read>(r: &mut R) -> Result<u64, String> {
    let mut b = [0; 8];
    take(r, &mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn get_f32<R: Read>(r: &mut R) -> Result<f32, String> {
    let mut b = [0; 4];
    take(r, &mut b)?;
    Ok(f32::from_le_bytes(b))
}
fn checksum(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
        }
    }
    crc
}
fn write_samples<W: Write>(w: &mut W, samples: &[f32]) -> Result<(), String> {
    put_u64(w, samples.len() as u64)?;
    let mut crc = !0u32;
    let mut buffer = Vec::with_capacity(16384);
    for &sample in samples {
        let bytes = sample.to_le_bytes();
        crc = checksum(crc, &bytes);
        buffer.extend_from_slice(&bytes);
        if buffer.len() >= 16384 {
            put(w, &buffer)?;
            buffer.clear();
        }
    }
    put(w, &buffer)?;
    put_u32(w, !crc)
}
fn read_samples<R: Read>(r: &mut R) -> Result<Arc<Vec<f32>>, String> {
    let len = usize::try_from(get_u64(r)?).map_err(|_| "Audio is too long")?;
    if len > MAX_SAMPLES {
        return Err("Audio is too long".into());
    }
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(len)
        .map_err(|_| "Not enough memory")?;
    let mut crc = !0u32;
    let mut bytes = [0u8; 4];
    for _ in 0..len {
        take(r, &mut bytes)?;
        crc = checksum(crc, &bytes);
        let value = f32::from_le_bytes(bytes);
        if !value.is_finite() {
            return Err("Invalid audio sample".into());
        }
        samples.push(value);
    }
    if !crc != get_u32(r)? {
        return Err("Audio checksum does not match".into());
    }
    Ok(Arc::new(samples))
}
fn write_patch<W: Write>(w: &mut W, patch: &Patch) -> Result<(), String> {
    put_u64(w, patch.start as u64)?;
    put_u32(w, patch.marker_id)?;
    write_samples(w, &patch.before)?;
    write_samples(w, &patch.after)
}
fn read_patch<R: Read>(r: &mut R, master_len: usize, version: u8) -> Result<Patch, String> {
    let start = usize::try_from(get_u64(r)?).map_err(|_| "Invalid edit")?;
    let marker_id = if version >= 2 { get_u32(r)? } else { 0 };
    let before = read_samples(r)?;
    let after = read_samples(r)?;
    if before.len() != after.len()
        || start
            .checked_add(before.len())
            .is_none_or(|end| end > master_len)
    {
        return Err("Invalid edit range".into());
    }
    Ok(Patch {
        start,
        before,
        after,
        marker_id,
    })
}

pub fn save(path: &Path, project: &Package) -> Result<(), String> {
    if project.tracks.is_empty() || project.tracks.len() > MAX_TRACKS {
        return Err("Invalid queue size".into());
    }
    if project.marks.len() > 100_000
        || project
            .marks
            .iter()
            .any(|m| m.id == 0 || m.start >= m.end || m.end > project.master.samples.len())
    {
        return Err("Invalid edit markers".into());
    }
    let file = File::create(path).map_err(|e| e.to_string())?;
    let mut w = BufWriter::new(file);
    put(&mut w, b"MGP2")?;
    put_u32(&mut w, project.tracks.len() as u32)?;
    put_u32(&mut w, project.selected_track as u32)?;
    put_f32(&mut w, project.selection.map(|s| s.0).unwrap_or(f32::NAN))?;
    put_f32(&mut w, project.selection.map(|s| s.1).unwrap_or(f32::NAN))?;
    put_f32(&mut w, project.loop_region.0)?;
    put_f32(&mut w, project.loop_region.1)?;
    put_u32(&mut w, u32::from(project.looping))?;
    put_f32(&mut w, project.zoom)?;
    put_f32(&mut w, project.pan)?;
    put_u32(&mut w, project.partials as u32)?;
    put_u32(&mut w, project.fft_size as u32)?;
    put_u32(&mut w, project.hop as u32)?;
    for track in &project.tracks {
        let name = track.name.as_bytes();
        if name.len() > 1024 {
            return Err("Track name is too long".into());
        }
        put_u32(&mut w, name.len() as u32)?;
        put(&mut w, name)?;
        put_u32(&mut w, track.audio.rate)?;
        put_f32(&mut w, track.transition.seconds)?;
        put_f32(&mut w, track.transition.in_cue)?;
        put_u32(
            &mut w,
            match track.transition.curve {
                Curve::Linear => 0,
                Curve::EqualPower => 1,
                Curve::Smooth => 2,
            },
        )?;
        put_u32(&mut w, u32::from(track.transition.phase_match))?;
        put_u32(&mut w, track.notes.len() as u32)?;
        for note in &track.notes {
            put(&mut w, &note.start.to_le_bytes())?;
            put(&mut w, &note.end.to_le_bytes())?;
            put(&mut w, &[note.pitch, note.velocity, note.channel])?;
        }
        write_samples(&mut w, &track.audio.samples)?;
    }
    put_u32(&mut w, project.master.rate)?;
    write_samples(&mut w, &project.master.samples)?;
    put_u32(&mut w, project.undo.len() as u32)?;
    for patch in &project.undo {
        write_patch(&mut w, patch)?;
    }
    put_u32(&mut w, project.redo.len() as u32)?;
    for patch in &project.redo {
        write_patch(&mut w, patch)?;
    }
    put_u32(&mut w, project.marks.len() as u32)?;
    for mark in &project.marks {
        put_u32(&mut w, mark.id)?;
        put_u64(&mut w, mark.start as u64)?;
        put_u64(&mut w, mark.end as u64)?;
        put(&mut w, &[mark.operation.code(), u8::from(mark.active)])?;
    }
    w.flush().map_err(|e| e.to_string())
}

pub fn load(path: &Path) -> Result<Package, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(file);
    let mut magic = [0; 4];
    take(&mut r, &mut magic)?;
    let version = if &magic == b"MGP1" {
        1
    } else if &magic == b"MGP2" {
        2
    } else {
        0
    };
    if version == 0 {
        return Err("Unsupported project file".into());
    }
    let count = get_u32(&mut r)? as usize;
    if count == 0 || count > MAX_TRACKS {
        return Err("Invalid queue size".into());
    }
    let selected_track = get_u32(&mut r)? as usize;
    if selected_track >= count {
        return Err("Invalid selected track".into());
    }
    let (sel_a, sel_b) = (get_f32(&mut r)?, get_f32(&mut r)?);
    let selection = if sel_a.is_finite() && sel_b.is_finite() {
        Some((sel_a, sel_b))
    } else {
        None
    };
    let loop_region = (get_f32(&mut r)?, get_f32(&mut r)?);
    let looping = get_u32(&mut r)? != 0;
    let zoom = get_f32(&mut r)?;
    let pan = get_f32(&mut r)?;
    let partials = get_u32(&mut r)? as usize;
    let fft_size = get_u32(&mut r)? as usize;
    let hop = get_u32(&mut r)? as usize;
    if !loop_region.0.is_finite()
        || !loop_region.1.is_finite()
        || !zoom.is_finite()
        || !pan.is_finite()
        || zoom < 1.
        || hop == 0
        || fft_size < 512
    {
        return Err("Invalid project settings".into());
    }
    let mut tracks = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = get_u32(&mut r)? as usize;
        if name_len > 1024 {
            return Err("Invalid track name".into());
        }
        let mut name = vec![0; name_len];
        take(&mut r, &mut name)?;
        let name = String::from_utf8(name).map_err(|_| "Invalid track name")?;
        let rate = get_u32(&mut r)?;
        let seconds = get_f32(&mut r)?;
        let in_cue = get_f32(&mut r)?;
        let curve = match get_u32(&mut r)? {
            0 => Curve::Linear,
            1 => Curve::EqualPower,
            2 if version >= 2 => Curve::Smooth,
            _ => return Err("Invalid transition curve".into()),
        };
        let phase_match = get_u32(&mut r)? != 0;
        if rate == 0
            || !seconds.is_finite()
            || !in_cue.is_finite()
            || !(0. ..=30.).contains(&seconds)
            || in_cue < 0.
        {
            return Err("Invalid transition".into());
        }
        let note_count = get_u32(&mut r)? as usize;
        if note_count > 1_000_000 {
            return Err("Too many notes".into());
        }
        let mut notes = Vec::with_capacity(note_count);
        for _ in 0..note_count {
            let mut a = [0; 8];
            let mut b = [0; 8];
            let mut rest = [0; 3];
            take(&mut r, &mut a)?;
            take(&mut r, &mut b)?;
            take(&mut r, &mut rest)?;
            let start = f64::from_le_bytes(a);
            let end = f64::from_le_bytes(b);
            if !start.is_finite() || !end.is_finite() || end < start {
                return Err("Invalid note".into());
            }
            notes.push(Note {
                start,
                end,
                pitch: rest[0],
                velocity: rest[1],
                channel: rest[2],
            });
        }
        let samples = read_samples(&mut r)?;
        tracks.push(QueueTrack {
            name,
            audio: Audio { samples, rate },
            notes,
            transition: Transition {
                seconds,
                in_cue,
                curve,
                phase_match,
            },
        });
    }
    let master_rate = get_u32(&mut r)?;
    if master_rate == 0 {
        return Err("Invalid sample rate".into());
    }
    let master = Audio {
        samples: read_samples(&mut r)?,
        rate: master_rate,
    };
    let undo_count = get_u32(&mut r)? as usize;
    if undo_count > 8 {
        return Err("Too many edits".into());
    }
    let mut undo = Vec::with_capacity(undo_count);
    for _ in 0..undo_count {
        undo.push(read_patch(&mut r, master.samples.len(), version)?);
    }
    let redo_count = get_u32(&mut r)? as usize;
    if redo_count > 8 {
        return Err("Too many edits".into());
    }
    let mut redo = Vec::with_capacity(redo_count);
    for _ in 0..redo_count {
        redo.push(read_patch(&mut r, master.samples.len(), version)?);
    }
    let mut marks = Vec::new();
    if version >= 2 {
        let count = get_u32(&mut r)? as usize;
        if count > 100_000 {
            return Err("Too many edit markers".into());
        }
        marks.reserve(count);
        for _ in 0..count {
            let id = get_u32(&mut r)?;
            let start = usize::try_from(get_u64(&mut r)?).map_err(|_| "Invalid edit marker")?;
            let end = usize::try_from(get_u64(&mut r)?).map_err(|_| "Invalid edit marker")?;
            let mut b = [0; 2];
            take(&mut r, &mut b)?;
            let operation = Operation::from_code(b[0]).ok_or("Invalid edit operation")?;
            if id == 0
                || start >= end
                || end > master.samples.len()
                || marks.iter().any(|m: &EditMark| m.id == id)
            {
                return Err("Invalid edit marker".into());
            }
            marks.push(EditMark {
                id,
                start,
                end,
                operation,
                active: b[1] != 0,
            });
        }
    }
    Ok(Package {
        tracks,
        master,
        undo,
        redo,
        marks,
        selected_track,
        selection,
        loop_region,
        looping,
        zoom,
        pan,
        partials,
        fft_size,
        hop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_round_trip_and_checksum() {
        let path = std::env::temp_dir().join(format!("project-{}-{}.mgp", std::process::id(), 17));
        let samples = Arc::new(vec![0., 0.25, -0.5, 1.]);
        let track = QueueTrack {
            name: "test.wav".into(),
            audio: Audio {
                samples: samples.clone(),
                rate: 44100,
            },
            notes: Vec::new(),
            transition: Transition::default(),
        };
        let patch = Patch {
            start: 1,
            before: Arc::new(vec![0.]),
            after: Arc::new(vec![0.25]),
            marker_id: 1,
        };
        let state = Package {
            tracks: vec![track],
            master: Audio {
                samples,
                rate: 44100,
            },
            undo: vec![patch],
            redo: Vec::new(),
            marks: vec![EditMark {
                id: 1,
                start: 1,
                end: 2,
                operation: Operation::GainUp,
                active: true,
            }],
            selected_track: 0,
            selection: Some((0., 0.1)),
            loop_region: (0., 0.1),
            looping: true,
            zoom: 2.,
            pan: 0.,
            partials: 16,
            fft_size: 2048,
            hop: 512,
        };
        save(&path, &state).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.tracks[0].name, "test.wav");
        assert_eq!(
            loaded.master.samples.as_slice(),
            state.master.samples.as_slice()
        );
        assert_eq!(loaded.undo[0].after.as_slice(), &[0.25]);
        assert_eq!(loaded.undo[0].marker_id, 1);
        assert_eq!(loaded.marks[0].operation, Operation::GainUp);
        assert_eq!(loaded.selection, Some((0., 0.1)));
        let mut bytes = std::fs::read(&path).unwrap();
        let offset = bytes
            .windows(4)
            .position(|w| w == 0.25f32.to_le_bytes())
            .unwrap();
        bytes[offset] ^= 0x01;
        std::fs::write(&path, bytes).unwrap();
        assert!(load(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn older_projects_remain_openable() {
        let path = std::env::temp_dir().join(format!("legacy-project-{}.mgp", std::process::id()));
        let mut w = BufWriter::new(File::create(&path).unwrap());
        put(&mut w, b"MGP1").unwrap();
        put_u32(&mut w, 1).unwrap();
        put_u32(&mut w, 0).unwrap();
        put_f32(&mut w, f32::NAN).unwrap();
        put_f32(&mut w, f32::NAN).unwrap();
        put_f32(&mut w, 0.).unwrap();
        put_f32(&mut w, 1.).unwrap();
        put_u32(&mut w, 0).unwrap();
        put_f32(&mut w, 1.).unwrap();
        put_f32(&mut w, 0.).unwrap();
        put_u32(&mut w, 16).unwrap();
        put_u32(&mut w, 2048).unwrap();
        put_u32(&mut w, 512).unwrap();
        put_u32(&mut w, 3).unwrap();
        put(&mut w, b"old").unwrap();
        put_u32(&mut w, 44100).unwrap();
        put_f32(&mut w, 0.).unwrap();
        put_f32(&mut w, 0.).unwrap();
        put_u32(&mut w, 1).unwrap();
        put_u32(&mut w, 1).unwrap();
        put_u32(&mut w, 0).unwrap();
        write_samples(&mut w, &[0., 0.5]).unwrap();
        put_u32(&mut w, 44100).unwrap();
        write_samples(&mut w, &[0., 0.5]).unwrap();
        put_u32(&mut w, 0).unwrap();
        put_u32(&mut w, 0).unwrap();
        w.flush().unwrap();
        drop(w);
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.tracks[0].name, "old");
        assert!(loaded.marks.is_empty());
        std::fs::remove_file(path).unwrap();
    }
}
