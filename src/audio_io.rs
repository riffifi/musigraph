use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    fs::File,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};
#[derive(Clone)]
pub struct Audio {
    pub samples: Arc<Vec<f32>>,
    pub rate: u32,
}
const EDIT_CHUNK: usize = 4096;
pub struct EditSource {
    original: Arc<Vec<f32>>,
    chunks: Vec<AtomicPtr<Vec<AtomicU32>>>,
}
impl EditSource {
    pub fn new(original: Arc<Vec<f32>>) -> Self {
        let chunks = (0..original.len().div_ceil(EDIT_CHUNK))
            .map(|_| AtomicPtr::new(std::ptr::null_mut()))
            .collect();
        Self { original, chunks }
    }
    pub fn len(&self) -> usize {
        self.original.len()
    }
    pub fn get(&self, index: usize) -> f32 {
        let Some(&value) = self.original.get(index) else {
            return 0.;
        };
        let ptr = self.chunks[index / EDIT_CHUNK].load(Ordering::Acquire);
        if ptr.is_null() {
            value
        } else {
            f32::from_bits(unsafe { (&*ptr)[index % EDIT_CHUNK].load(Ordering::Relaxed) })
        }
    }
    pub fn set(&self, index: usize, value: f32) {
        if index >= self.original.len() {
            return;
        }
        let slot = &self.chunks[index / EDIT_CHUNK];
        let mut ptr = slot.load(Ordering::Acquire);
        if ptr.is_null() {
            let start = index / EDIT_CHUNK * EDIT_CHUNK;
            let end = (start + EDIT_CHUNK).min(self.original.len());
            let block: Vec<_> = self.original[start..end]
                .iter()
                .map(|v| AtomicU32::new(v.to_bits()))
                .collect();
            let candidate = Box::into_raw(Box::new(block));
            match slot.compare_exchange(
                std::ptr::null_mut(),
                candidate,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => ptr = candidate,
                Err(existing) => {
                    unsafe {
                        drop(Box::from_raw(candidate));
                    }
                    ptr = existing;
                }
            }
        }
        unsafe {
            (&*ptr)[index % EDIT_CHUNK].store(value.to_bits(), Ordering::Relaxed);
        }
    }
}
impl Drop for EditSource {
    fn drop(&mut self) {
        for slot in &self.chunks {
            let ptr = slot.load(Ordering::Relaxed);
            if !ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
    }
}
pub fn decode(path: &Path) -> Result<Audio, String> {
    let f = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(f), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("no audio track")?;
    let id = track.id;
    let rate = track
        .codec_params
        .sample_rate
        .ok_or("unknown sample rate")?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(v) => v,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let spec = *decoded.spec();
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        let channels = spec.channels.count();
        for frame in buf.samples().chunks(channels) {
            out.push(frame.iter().copied().sum::<f32>() / channels as f32);
        }
    }
    Ok(Audio {
        samples: Arc::new(out),
        rate,
    })
}
pub struct Playback {
    pub playing: Arc<AtomicBool>,
    pub position: Arc<AtomicU64>,
    pub seek: Arc<AtomicU64>,
    pub volume: Arc<AtomicU32>,
    pub source: Arc<EditSource>,
    pub looping: Arc<AtomicBool>,
    pub loop_start: Arc<AtomicU64>,
    pub loop_end: Arc<AtomicU64>,
    pub audition_end: Arc<AtomicU64>,
    _stream: Option<cpal::Stream>,
}
impl Playback {
    pub fn new(audio: Audio) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no audio output device")?;
        let supported = device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .find(|c| c.sample_format() == cpal::SampleFormat::F32)
            .ok_or("no f32 output")?;
        let cfg = supported.with_max_sample_rate().config();
        let channels = cfg.channels as usize;
        let ratio = audio.rate as f64 / cfg.sample_rate.0 as f64;
        let playing = Arc::new(AtomicBool::new(false));
        let position = Arc::new(AtomicU64::new(0));
        let seek = Arc::new(AtomicU64::new(u64::MAX));
        let volume = Arc::new(AtomicU32::new(0.8f32.to_bits()));
        let source = Arc::new(EditSource::new(audio.samples.clone()));
        let looping = Arc::new(AtomicBool::new(false));
        let loop_start = Arc::new(AtomicU64::new(0));
        let loop_end = Arc::new(AtomicU64::new(audio.samples.len() as u64));
        let audition_end = Arc::new(AtomicU64::new(u64::MAX));
        let (p, pos, s, v, source_read, l, ls, le, ae) = (
            playing.clone(),
            position.clone(),
            seek.clone(),
            volume.clone(),
            source.clone(),
            looping.clone(),
            loop_start.clone(),
            loop_end.clone(),
            audition_end.clone(),
        );
        let mut cursor = 0f64;
        let samples = audio.samples;
        let stream = device
            .build_output_stream(
                &cfg,
                move |data: &mut [f32], _| {
                    let wanted = s.swap(u64::MAX, Ordering::AcqRel);
                    if wanted != u64::MAX {
                        cursor = wanted.min(samples.len() as u64) as f64;
                    }
                    let gain = f32::from_bits(v.load(Ordering::Relaxed));
                    let mut active = p.load(Ordering::Relaxed);
                    for frame in data.chunks_mut(channels) {
                        if active && cursor >= ae.load(Ordering::Relaxed) as f64 {
                            active = false;
                            p.store(false, Ordering::Relaxed);
                            ae.store(u64::MAX, Ordering::Relaxed);
                        }
                        let i = cursor as usize;
                        let frac = (cursor - i as f64) as f32;
                        let a = source_read.get(i);
                        let b = source_read.get(i + 1);
                        let sample = if active {
                            (a + (b - a) * frac) * gain
                        } else {
                            0.
                        };
                        for x in frame {
                            *x = sample;
                        }
                        if active {
                            cursor += ratio;
                            if ae.load(Ordering::Relaxed) == u64::MAX
                                && l.load(Ordering::Relaxed)
                                && cursor >= le.load(Ordering::Relaxed) as f64
                            {
                                cursor = ls.load(Ordering::Relaxed) as f64;
                            }
                            if cursor >= samples.len() as f64 {
                                cursor = samples.len() as f64;
                                active = false;
                                p.store(false, Ordering::Relaxed);
                            }
                        }
                    }
                    pos.store(cursor as u64, Ordering::Release);
                },
                move |e| eprintln!("audio: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            playing,
            position,
            seek,
            volume,
            source,
            looping,
            loop_start,
            loop_end,
            audition_end,
            _stream: Some(stream),
        })
    }
    pub fn seek_to(&self, sample: u64) {
        self.seek.store(sample, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_source_edits() {
        let source = EditSource::new(Arc::new(vec![0.25; 9000]));
        assert_eq!(source.get(5000), 0.25);
        source.set(5000, -0.5);
        assert_eq!(source.get(5000), -0.5);
        assert_eq!(source.get(100), 0.25);
        assert_eq!(source.get(5001), 0.25);
    }
}
