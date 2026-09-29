use crate::{
    audio_io::{Audio, EditSource, Playback},
    dsp::stft::{self, Frame},
    edit::{self, Operation, Selection, Tool},
    encode,
    midi::Note,
    model::EventModel,
    package,
    project::{self, Curve, Placement, QueueTrack, Transition},
    render::plots::WaveformPeaks,
    theme::{Palette, ThemeSettings},
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Instant,
};
pub enum LoadData {
    Track(QueueTrack),
    Package(package::Package, Audio),
}
pub enum Message {
    Loaded(u64, LoadData),
    QueueAdded(u64, Vec<QueueTrack>, Option<String>),
    MixReady(u64, u64, Audio, Audio, Vec<Placement>, Vec<package::Patch>),
    MixFailed(u64, u64, String),
    AutoReady(u64, u64, u64, Vec<(usize, Transition)>),
    SavedPackage(u64, Result<PathBuf, String>),
    Frames(u64, Vec<Frame>),
    Waveform(u64, Arc<WaveformPeaks>),
    Events(u64, Vec<EventModel>),
    Error(u64, String),
    SourceEdited(u64, usize, Vec<f32>, Vec<f32>, Operation, String),
    SourceSnapshot(u64, Audio),
    EditComplete(u64, String),
    Exported(u64, Result<PathBuf, String>),
    SelectionMeasured(u64, u64, SelectionStats),
}
#[derive(Clone, Copy, Debug)]
pub struct SelectionStats {
    pub samples: usize,
    pub peak: f32,
    pub rms: f32,
    pub dc: f32,
    pub clipped: usize,
}
enum HistoryEntry {
    Source {
        start: usize,
        before: Arc<Vec<f32>>,
        after: Arc<Vec<f32>>,
        marker_id: u32,
    },
}
impl From<package::Patch> for HistoryEntry {
    fn from(p: package::Patch) -> Self {
        Self::Source {
            start: p.start,
            before: p.before,
            after: p.after,
            marker_id: p.marker_id,
        }
    }
}
impl From<&HistoryEntry> for package::Patch {
    fn from(h: &HistoryEntry) -> Self {
        match h {
            HistoryEntry::Source {
                start,
                before,
                after,
                marker_id,
            } => Self {
                start: *start,
                before: before.clone(),
                after: after.clone(),
                marker_id: *marker_id,
            },
        }
    }
}
struct PendingEdit {
    selection: Selection,
    next: usize,
    end: usize,
    frames: Vec<(usize, Frame)>,
}
pub struct App {
    pub audio: Option<Audio>,
    base_audio: Option<Audio>,
    pub playback: Option<Playback>,
    pub frames: Vec<Frame>,
    pub waveform: Option<Arc<WaveformPeaks>>,
    pub events: Vec<EventModel>,
    pub notes: Vec<Note>,
    pub edit_marks: Vec<package::EditMark>,
    pub queue: Vec<QueueTrack>,
    pub placements: Vec<Placement>,
    pub selected_track: usize,
    pub queue_scroll: usize,
    pub marquee_hovered: Option<usize>,
    pub marquee_started: Instant,
    pub project_path: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub hovered_file: Option<PathBuf>,
    pub edit_mode: bool,
    pub tool: Tool,
    pub selection: Option<Selection>,
    pub selection_stats: Option<SelectionStats>,
    pub caret: Option<f32>,
    pub edit_busy: bool,
    pub auto_busy: bool,
    pub edit_status: Option<String>,
    pub error: Option<String>,
    pub busy: bool,
    pub partials: usize,
    pub fft_size: usize,
    pub hop: usize,
    pub zoom: f32,
    pub pan: f32,
    pub follow_playhead: bool,
    pub show_tracks: bool,
    pub settings: bool,
    pub theme: ThemeSettings,
    pub loop_start: f32,
    pub loop_end: f32,
    pub spec_dirty: bool,
    pub spec_width: u32,
    pub spec_height: u32,
    generation: Arc<AtomicU64>,
    project_epoch: Arc<AtomicU64>,
    mix_version: Arc<AtomicU64>,
    auto_version: u64,
    pending_rebuild: bool,
    mix_busy: bool,
    pending_paths: Vec<PathBuf>,
    measurement_token: Arc<AtomicU64>,
    pending_edit: Option<PendingEdit>,
    history: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    next_mark_id: u32,
    tx: Sender<Message>,
    rx: Receiver<Message>,
}
impl App {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            audio: None,
            base_audio: None,
            playback: None,
            frames: Vec::new(),
            waveform: None,
            events: Vec::new(),
            notes: Vec::new(),
            edit_marks: Vec::new(),
            queue: Vec::new(),
            placements: Vec::new(),
            selected_track: 0,
            queue_scroll: 0,
            marquee_hovered: None,
            marquee_started: Instant::now(),
            project_path: None,
            path: None,
            hovered_file: None,
            edit_mode: false,
            tool: Tool::Range,
            selection: None,
            selection_stats: None,
            caret: None,
            edit_busy: false,
            auto_busy: false,
            edit_status: None,
            error: None,
            busy: false,
            partials: 16,
            fft_size: 2048,
            hop: 512,
            zoom: 1.,
            pan: 0.,
            follow_playhead: false,
            show_tracks: true,
            settings: false,
            theme: ThemeSettings::new(PathBuf::new()),
            loop_start: 0.,
            loop_end: 0.,
            spec_dirty: false,
            spec_width: 0,
            spec_height: 0,
            generation: Arc::new(AtomicU64::new(0)),
            project_epoch: Arc::new(AtomicU64::new(0)),
            mix_version: Arc::new(AtomicU64::new(0)),
            auto_version: 0,
            pending_rebuild: false,
            mix_busy: false,
            pending_paths: Vec::new(),
            measurement_token: Arc::new(AtomicU64::new(0)),
            pending_edit: None,
            history: Vec::new(),
            redo: Vec::new(),
            next_mark_id: 1,
            tx,
            rx,
        }
    }
    pub fn open(&mut self, path: PathBuf) {
        let epoch = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.project_epoch.fetch_add(1, Ordering::AcqRel);
        self.mix_version.fetch_add(1, Ordering::AcqRel);
        self.auto_version = self.auto_version.wrapping_add(1);
        while self.rx.try_recv().is_ok() {}
        let generation = self.generation.clone();
        self.path = Some(path.clone());
        self.hovered_file = None;
        self.selection = None;
        self.selection_stats = None;
        self.measurement_token.fetch_add(1, Ordering::AcqRel);
        self.caret = None;
        self.edit_busy = false;
        self.auto_busy = false;
        self.pending_edit = None;
        self.history.clear();
        self.redo.clear();
        self.edit_marks.clear();
        self.next_mark_id = 1;
        self.edit_status = None;
        self.audio = None;
        self.base_audio = None;
        self.playback = None;
        self.frames.clear();
        self.waveform = None;
        self.events.clear();
        self.spec_width = 0;
        self.spec_height = 0;
        self.spec_dirty = false;
        self.notes.clear();
        self.queue.clear();
        self.placements.clear();
        self.selected_track = 0;
        self.queue_scroll = 0;
        self.marquee_hovered = None;
        self.marquee_started = Instant::now();
        self.project_path = None;
        self.pending_rebuild = false;
        self.mix_busy = false;
        self.pending_paths.clear();
        self.busy = true;
        self.error = None;
        self.pan = 0.;
        self.zoom = 1.;
        self.loop_start = 0.;
        self.loop_end = 0.;
        let tx = self.tx.clone();
        let size = self.fft_size;
        let hop = self.hop;
        let limit = self.partials;
        thread::spawn(move || {
            let package_file = path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("mgp"));
            let loaded = if package_file {
                package::load(&path).and_then(|saved| {
                    let base = if saved.tracks.len() == 1
                        && saved.tracks[0].audio.rate == saved.master.rate
                    {
                        saved.tracks[0].audio.clone()
                    } else {
                        project::mix(&saved.tracks, saved.master.rate)?.0
                    };
                    Ok(LoadData::Package(saved, base))
                })
            } else {
                project::load_track(&path).map(LoadData::Track)
            };
            if generation.load(Ordering::Acquire) != epoch {
                return;
            }
            let data = match loaded {
                Ok(x) => x,
                Err(e) => {
                    let _ = tx.send(Message::Error(epoch, e));
                    return;
                }
            };
            let (audio, skip_analysis, fft, hop, limit) = match &data {
                LoadData::Track(track) => (
                    track.audio.clone(),
                    !track.notes.is_empty(),
                    size,
                    hop,
                    limit,
                ),
                LoadData::Package(project, _) => (
                    project.master.clone(),
                    project.tracks.len() == 1 && !project.tracks[0].notes.is_empty(),
                    project.fft_size,
                    project.hop,
                    project.partials,
                ),
            };
            if tx.send(Message::Loaded(epoch, data)).is_err() {
                return;
            }
            if !skip_analysis {
                run_analysis(audio, fft, hop, limit, tx, generation, epoch);
            }
        });
    }
    pub fn poll(&mut self) {
        for _ in 0..16 {
            let Ok(msg) = self.rx.try_recv() else {
                break;
            };
            match msg {
                Message::Loaded(message_epoch, data)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    let saved_looping = match &data {
                        LoadData::Package(saved, _) => saved.looping,
                        _ => false,
                    };
                    let audio = match data {
                        LoadData::Track(track) => {
                            self.notes = track.notes.clone();
                            let audio = track.audio.clone();
                            self.queue = vec![track];
                            self.base_audio = Some(audio.clone());
                            self.placements =
                                project::layout(&self.queue, audio.rate).unwrap_or_default();
                            audio
                        }
                        LoadData::Package(saved, base) => {
                            self.queue = saved.tracks;
                            self.selected_track = saved.selected_track;
                            self.marquee_started = Instant::now();
                            self.notes = if self.queue.len() == 1 {
                                self.queue[0].notes.clone()
                            } else {
                                Vec::new()
                            };
                            self.placements =
                                project::layout(&self.queue, saved.master.rate).unwrap_or_default();
                            self.selection = saved
                                .selection
                                .map(|(start, end)| Selection::Range { start, end });
                            self.loop_start = saved.loop_region.0;
                            self.loop_end = saved.loop_region.1;
                            self.zoom = saved.zoom;
                            self.pan = saved.pan;
                            self.partials = saved.partials;
                            self.fft_size = saved.fft_size;
                            self.hop = saved.hop;
                            self.history = saved.undo.into_iter().map(HistoryEntry::from).collect();
                            self.redo = saved.redo.into_iter().map(HistoryEntry::from).collect();
                            self.edit_marks = saved.marks;
                            self.next_mark_id = self
                                .edit_marks
                                .iter()
                                .map(|m| m.id)
                                .max()
                                .unwrap_or(0)
                                .saturating_add(1);
                            self.project_path = self.path.clone();
                            self.base_audio = Some(base);
                            let audio = saved.master;
                            audio
                        }
                    };
                    if self.project_path.is_none() {
                        self.loop_end = audio.samples.len() as f32 / audio.rate as f32;
                    }
                    self.playback = match Playback::new(audio.clone()) {
                        Ok(p) => Some(p),
                        Err(e) => {
                            self.error = Some(e);
                            None
                        }
                    };
                    if let Some(p) = &self.playback {
                        p.loop_start.store(
                            (self.loop_start * audio.rate as f32) as u64,
                            Ordering::Release,
                        );
                        p.loop_end.store(
                            (self.loop_end * audio.rate as f32) as u64,
                            Ordering::Release,
                        );
                        p.looping.store(saved_looping, Ordering::Release);
                    }
                    self.start_waveform(audio.clone());
                    self.audio = Some(audio);
                    self.busy = false;
                    self.spec_dirty = true;
                    if !self.pending_paths.is_empty() {
                        let paths = std::mem::take(&mut self.pending_paths);
                        self.add_paths(paths);
                    }
                }
                Message::QueueAdded(epoch, mut tracks, error)
                    if epoch == self.project_epoch.load(Ordering::Acquire) =>
                {
                    if !tracks.is_empty() {
                        self.queue.append(&mut tracks);
                        self.selected_track = self.queue.len() - 1;
                        self.marquee_started = Instant::now();
                        self.request_mix();
                    }
                    if let Some(error) = error {
                        self.edit_status = Some(error);
                    }
                }
                Message::MixReady(epoch, version, audio, base, placements, patches)
                    if epoch == self.project_epoch.load(Ordering::Acquire)
                        && version == self.mix_version.load(Ordering::Acquire) =>
                {
                    if self.pending_rebuild {
                        self.mix_busy = false;
                        continue;
                    }
                    let old_time = self.time();
                    let old_duration = self.duration();
                    let playing = self
                        .playback
                        .as_ref()
                        .is_some_and(|p| p.playing.load(Ordering::Acquire));
                    let volume = self
                        .playback
                        .as_ref()
                        .map(|p| p.volume.load(Ordering::Relaxed))
                        .unwrap_or(0.8f32.to_bits());
                    let looping = self
                        .playback
                        .as_ref()
                        .is_some_and(|p| p.looping.load(Ordering::Acquire));
                    self.generation.fetch_add(1, Ordering::AcqRel);
                    self.placements = placements;
                    self.notes = if self.queue.len() == 1 {
                        self.queue[0].notes.clone()
                    } else {
                        Vec::new()
                    };
                    self.history = patches.into_iter().map(HistoryEntry::from).collect();
                    self.redo.clear();
                    let new_duration = audio.samples.len() as f32 / audio.rate as f32;
                    self.loop_start = self.loop_start.min(new_duration);
                    self.loop_end = if (self.loop_end - old_duration).abs() < 0.01 {
                        new_duration
                    } else {
                        self.loop_end.min(new_duration)
                    };
                    self.playback = Playback::new(audio.clone()).ok();
                    if let Some(p) = &self.playback {
                        p.volume.store(volume, Ordering::Release);
                        p.looping.store(looping, Ordering::Release);
                        p.loop_start.store(
                            (self.loop_start * audio.rate as f32) as u64,
                            Ordering::Release,
                        );
                        p.loop_end.store(
                            (self.loop_end * audio.rate as f32) as u64,
                            Ordering::Release,
                        );
                        p.seek_to(
                            (old_time.min(audio.samples.len() as f32 / audio.rate as f32)
                                * audio.rate as f32) as u64,
                        );
                        p.playing.store(playing, Ordering::Release);
                    }
                    self.audio = Some(audio.clone());
                    self.base_audio = Some(base);
                    self.edit_marks.retain_mut(|mark| {
                        if mark.start >= audio.samples.len() {
                            return false;
                        }
                        mark.end = mark.end.min(audio.samples.len());
                        true
                    });
                    self.waveform = None;
                    self.start_waveform(audio.clone());
                    self.frames.clear();
                    self.events.clear();
                    self.spec_width = 0;
                    self.spec_height = 0;
                    self.spec_dirty = false;
                    self.edit_status = Some("queue updated".into());
                    self.mix_busy = false;
                    self.start_analysis(audio);
                }
                Message::SavedPackage(epoch, result)
                    if epoch == self.project_epoch.load(Ordering::Acquire) =>
                {
                    self.edit_status = Some(match result {
                        Ok(path) => {
                            self.project_path = Some(path);
                            "project saved".into()
                        }
                        Err(e) => e,
                    });
                }
                Message::MixFailed(epoch, version, error)
                    if epoch == self.project_epoch.load(Ordering::Acquire)
                        && version == self.mix_version.load(Ordering::Acquire) =>
                {
                    self.mix_busy = false;
                    self.edit_status = Some(error);
                }
                Message::AutoReady(epoch, mix_version, auto_version, suggestions)
                    if epoch == self.project_epoch.load(Ordering::Acquire)
                        && mix_version == self.mix_version.load(Ordering::Acquire)
                        && auto_version == self.auto_version =>
                {
                    self.auto_busy = false;
                    for (i, transition) in suggestions {
                        if let Some(track) = self.queue.get_mut(i) {
                            track.transition = transition;
                        }
                    }
                    self.edit_status = Some("transitions set".into());
                    self.request_mix();
                }
                Message::Frames(message_epoch, f)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    self.frames.extend(f);
                    self.spec_dirty = true;
                }
                Message::Waveform(message_epoch, peaks)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    self.waveform = Some(peaks);
                }
                Message::Events(message_epoch, events)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    self.events = events
                }
                Message::Error(message_epoch, e)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    self.error = Some(e);
                    self.busy = false;
                }
                Message::SourceEdited(message_epoch, start, before, after, operation, name)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    self.edit_busy = false;
                    self.edit_status = Some(name);
                    let marker_id = self.next_mark_id;
                    self.next_mark_id = self.next_mark_id.saturating_add(1);
                    self.edit_marks.push(package::EditMark {
                        id: marker_id,
                        start,
                        end: start + after.len(),
                        operation,
                        active: true,
                    });
                    self.push_history(HistoryEntry::Source {
                        start,
                        before: Arc::new(before),
                        after: Arc::new(after),
                        marker_id,
                    });
                    self.request_selection_stats();
                }
                Message::EditComplete(message_epoch, name)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    self.edit_busy = false;
                    self.edit_status = Some(name);
                }
                Message::Exported(message_epoch, result)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    self.edit_status = Some(match result {
                        Ok(path) => format!(
                            "saved {}",
                            path.file_name().and_then(|v| v.to_str()).unwrap_or("file")
                        ),
                        Err(error) => error,
                    });
                }
                Message::SourceSnapshot(message_epoch, audio)
                    if message_epoch == self.generation.load(Ordering::Acquire) =>
                {
                    self.waveform = None;
                    self.start_waveform(audio.clone());
                    self.audio = Some(audio);
                    self.frames.clear();
                    self.events.clear();
                    self.spec_width = 0;
                    self.spec_height = 0;
                    self.spec_dirty = false;
                }
                Message::SelectionMeasured(message_epoch, token, stats)
                    if message_epoch == self.generation.load(Ordering::Acquire)
                        && token == self.measurement_token.load(Ordering::Acquire) =>
                {
                    self.selection_stats = Some(stats);
                }
                _ => {}
            }
        }
        self.advance_edit_snapshot();
        if self.pending_rebuild && !self.edit_busy && !self.mix_busy {
            self.pending_rebuild = false;
            self.request_mix();
        }
    }
    fn start_analysis(&mut self, audio: Audio) {
        if !self.notes.is_empty() {
            return;
        }
        let tx = self.tx.clone();
        let generation = self.generation.clone();
        let epoch = generation.load(Ordering::Acquire);
        let (size, hop, limit) = (self.fft_size, self.hop, self.partials);
        thread::spawn(move || run_analysis(audio, size, hop, limit, tx, generation, epoch));
    }
    fn start_waveform(&self, audio: Audio) {
        let tx = self.tx.clone();
        let generation = self.generation.clone();
        let epoch = generation.load(Ordering::Acquire);
        thread::spawn(move || {
            let peaks = Arc::new(WaveformPeaks::build(&audio.samples));
            if generation.load(Ordering::Acquire) == epoch {
                let _ = tx.send(Message::Waveform(epoch, peaks));
            }
        });
    }
    pub fn reanalyze(&mut self) {
        let Some(audio) = self.audio.clone() else {
            return;
        };
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.frames.clear();
        self.events.clear();
        self.spec_width = 0;
        self.spec_height = 0;
        self.spec_dirty = false;
        self.start_analysis(audio);
    }
    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.audio.is_none() {
            if self.busy {
                self.pending_paths.extend(paths);
            } else {
                self.open(paths[0].clone());
                self.pending_paths.extend(paths.into_iter().skip(1));
            }
            return;
        }
        let epoch = self.project_epoch.load(Ordering::Acquire);
        let tx = self.tx.clone();
        self.edit_status = Some("adding tracks…".into());
        thread::spawn(move || {
            let mut tracks = Vec::new();
            let mut errors = Vec::new();
            for path in paths {
                match project::load_track(&path) {
                    Ok(t) => tracks.push(t),
                    Err(e) => errors.push(e),
                }
            }
            let _ = tx.send(Message::QueueAdded(
                epoch,
                tracks,
                (!errors.is_empty()).then(|| errors.join("; ")),
            ));
        });
    }
    fn request_mix(&mut self) {
        if self.edit_busy || self.mix_busy {
            self.pending_rebuild = true;
            return;
        }
        if self.queue.is_empty() {
            return;
        }
        let epoch = self.project_epoch.load(Ordering::Acquire);
        let version = self.mix_version.fetch_add(1, Ordering::AcqRel) + 1;
        self.auto_busy = false;
        self.mix_busy = true;
        let tracks = self.queue.clone();
        let rate = self
            .audio
            .as_ref()
            .map(|a| a.rate)
            .unwrap_or(tracks[0].audio.rate);
        let old_base = self.base_audio.clone();
        let old_master = self.audio.clone();
        let patches: Vec<package::Patch> = self.history.iter().map(package::Patch::from).collect();
        let tx = self.tx.clone();
        self.edit_status = Some("mixing…".into());
        thread::spawn(move || {
            let result = if tracks.len() == 1 && tracks[0].audio.rate == rate {
                project::layout(&tracks, rate).map(|places| (tracks[0].audio.clone(), places))
            } else {
                project::mix(&tracks, rate)
            };
            match result {
                Ok((base, placements)) => {
                    let (audio, rebased) = match (old_base, old_master) {
                        (Some(old_base), Some(old_master))
                            if old_base.rate == base.rate && old_master.rate == base.rate =>
                        {
                            rebase_edits(&base, &old_base, &old_master, patches)
                        }
                        _ => (base.clone(), Vec::new()),
                    };
                    let _ = tx.send(Message::MixReady(
                        epoch, version, audio, base, placements, rebased,
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::MixFailed(epoch, version, e));
                }
            }
        });
    }
    pub fn queue_select(&mut self, index: usize) {
        if index < self.queue.len() {
            if self.selected_track != index {
                self.marquee_started = Instant::now();
            }
            self.selected_track = index;
            if let Some(p) = self.placements.get(index) {
                let t = p.start as f32 / self.audio.as_ref().unwrap().rate as f32;
                self.seek(t);
                self.reveal_time(t);
            }
        }
    }
    pub fn queue_move(&mut self, up: bool) {
        let i = self.selected_track;
        let j = if up {
            i.checked_sub(1)
        } else {
            i.checked_add(1)
        };
        if let Some(j) = j.filter(|&j| j < self.queue.len()) {
            self.queue.swap(i, j);
            self.selected_track = j;
            self.marquee_started = Instant::now();
            self.request_mix();
        }
    }
    pub fn queue_remove(&mut self) {
        if self.queue.len() > 1 {
            self.queue.remove(self.selected_track);
            self.selected_track = self.selected_track.min(self.queue.len() - 1);
            self.marquee_started = Instant::now();
            self.request_mix();
        }
    }
    pub fn set_marquee_hovered(&mut self, index: Option<usize>) {
        if self.marquee_hovered != index {
            self.marquee_hovered = index;
            self.marquee_started = Instant::now();
        }
    }
    pub fn transition_index(&self) -> Option<usize> {
        if self.queue.len() > 1 {
            Some(self.selected_track.max(1))
        } else {
            None
        }
    }
    pub fn step_transition(&mut self, forward: bool) {
        let Some(current) = self.transition_index() else {
            return;
        };
        let next = if forward {
            if current + 1 < self.queue.len() {
                current + 1
            } else {
                1
            }
        } else if current > 1 {
            current - 1
        } else {
            self.queue.len() - 1
        };
        self.queue_select(next);
    }
    pub fn loop_transition(&mut self) {
        let Some(i) = self.transition_index() else {
            return;
        };
        let (Some(place), Some(audio), Some(play)) = (
            self.placements.get(i),
            self.audio.as_ref(),
            self.playback.as_ref(),
        ) else {
            return;
        };
        let rate = audio.rate as f32;
        let start = (place.start as f32 / rate - 0.75).max(0.);
        let end = ((place.start + place.overlap) as f32 / rate + 0.75).min(self.duration());
        if end <= start {
            return;
        }
        self.loop_start = start;
        self.loop_end = end;
        play.loop_start
            .store((start * rate) as u64, Ordering::Release);
        play.loop_end.store((end * rate) as u64, Ordering::Release);
        play.audition_end.store(u64::MAX, Ordering::Release);
        play.looping.store(true, Ordering::Release);
        self.seek(start);
        self.reveal_time(start);
    }
    pub fn transition_duration(&mut self, delta: f32) {
        if let Some(i) = self.transition_index() {
            let t = &mut self.queue[i].transition;
            t.seconds = (t.seconds + delta).clamp(0., 30.);
            self.request_mix();
        }
    }
    pub fn transition_cue(&mut self, delta: f32) {
        if let Some(i) = self.transition_index() {
            let max = (self.queue[i].audio.samples.len() as f32 / self.queue[i].audio.rate as f32
                - 0.05)
                .max(0.);
            let t = &mut self.queue[i].transition;
            t.in_cue = (t.in_cue + delta).clamp(0., max);
            self.request_mix();
        }
    }
    pub fn transition_curve(&mut self) {
        if let Some(i) = self.transition_index() {
            let t = &mut self.queue[i].transition;
            t.curve = match t.curve {
                Curve::Linear => Curve::EqualPower,
                Curve::EqualPower => Curve::Smooth,
                Curve::Smooth => Curve::Linear,
            };
            self.request_mix();
        }
    }
    pub fn auto_transition(&mut self, all: bool) {
        if self.queue.len() < 2 || self.edit_busy || self.mix_busy {
            return;
        }
        let mut tracks = self.queue.clone();
        let selected = self.transition_index().unwrap_or(1);
        let epoch = self.project_epoch.load(Ordering::Acquire);
        let mix_version = self.mix_version.load(Ordering::Acquire);
        self.auto_version = self.auto_version.wrapping_add(1);
        let auto_version = self.auto_version;
        let tx = self.tx.clone();
        self.edit_status = Some("finding transitions…".into());
        self.auto_busy = true;
        thread::spawn(move || {
            let indices: Vec<_> = if all {
                (1..tracks.len()).collect()
            } else {
                vec![selected]
            };
            let mut suggestions = Vec::with_capacity(indices.len());
            for i in indices {
                let transition = project::suggest_transition(&tracks[i - 1], &tracks[i]);
                tracks[i].transition = transition;
                suggestions.push((i, transition));
            }
            let _ = tx.send(Message::AutoReady(
                epoch,
                mix_version,
                auto_version,
                suggestions,
            ));
        });
    }
    pub fn transition_phase(&mut self) {
        if let Some(i) = self.transition_index() {
            self.queue[i].transition.phase_match = !self.queue[i].transition.phase_match;
            self.request_mix();
        }
    }
    pub fn preview_transition(&mut self) {
        let Some(i) = self.transition_index() else {
            return;
        };
        let (Some(place), Some(audio)) = (self.placements.get(i), self.audio.as_ref()) else {
            return;
        };
        let start = place.start as f32 / audio.rate as f32;
        self.seek(start);
        let span = self.visible().1;
        self.pan = (start - span * 0.25).clamp(0., (self.duration() - span).max(0.));
    }
    pub fn audition_transition(&mut self) {
        let Some(i) = self.transition_index() else {
            return;
        };
        let Some(p) = self.placements.get(i) else {
            return;
        };
        let Some(a) = &self.audio else { return };
        let Some(play) = &self.playback else { return };
        let start = (p.start as f32 / a.rate as f32 - 1.).max(0.);
        let end = (p.start + p.overlap) as f32 / a.rate as f32 + 1.;
        play.playing.store(false, Ordering::Release);
        play.audition_end.store(
            (end.min(self.duration()) * a.rate as f32) as u64,
            Ordering::Release,
        );
        play.seek_to((start * a.rate as f32) as u64);
        play.playing.store(true, Ordering::Release);
    }
    pub fn save_package(&mut self, path: PathBuf) {
        let Some(master) = self.audio.clone() else {
            return;
        };
        if self.edit_busy || self.mix_busy {
            self.edit_status = Some("Wait for audio to finish updating".into());
            return;
        }
        let state = package::Package {
            tracks: self.queue.clone(),
            master,
            undo: self.history.iter().map(package::Patch::from).collect(),
            redo: self.redo.iter().map(package::Patch::from).collect(),
            marks: self.edit_marks.clone(),
            selected_track: self.selected_track,
            selection: self.selection.as_ref().and_then(Selection::bounds),
            loop_region: (self.loop_start, self.loop_end),
            looping: self
                .playback
                .as_ref()
                .is_some_and(|p| p.looping.load(Ordering::Acquire)),
            zoom: self.zoom,
            pan: self.pan,
            partials: self.partials,
            fft_size: self.fft_size,
            hop: self.hop,
        };
        let epoch = self.project_epoch.load(Ordering::Acquire);
        let tx = self.tx.clone();
        self.edit_status = Some("saving project…".into());
        thread::spawn(move || {
            let result = package::save(&path, &state).map(|_| path);
            let _ = tx.send(Message::SavedPackage(epoch, result));
        });
    }
    fn push_history(&mut self, entry: HistoryEntry) {
        self.history.push(entry);
        self.redo.clear();
        if self.history.len() > 8 {
            self.history.remove(0);
        }
    }
    fn set_mark_active(&mut self, id: u32, active: bool) {
        if let Some(mark) = self.edit_marks.iter_mut().find(|mark| mark.id == id) {
            mark.active = active;
        }
    }
    pub fn undo(&mut self) {
        if self.edit_busy || self.mix_busy {
            return;
        }
        let Some(entry) = self.history.pop() else {
            return;
        };
        match &entry {
            HistoryEntry::Source {
                start,
                before,
                marker_id,
                ..
            } => {
                self.restore_source(*start, before.clone(), "undo");
                self.set_mark_active(*marker_id, false);
            }
        }
        self.redo.push(entry);
    }
    pub fn redo(&mut self) {
        if self.edit_busy || self.mix_busy {
            return;
        }
        let Some(entry) = self.redo.pop() else {
            return;
        };
        match &entry {
            HistoryEntry::Source {
                start,
                after,
                marker_id,
                ..
            } => {
                self.restore_source(*start, after.clone(), "redo");
                self.set_mark_active(*marker_id, true);
            }
        }
        self.history.push(entry);
    }
    pub fn export_audio(&mut self, path: PathBuf, selection_only: bool) {
        if self.edit_busy || self.mix_busy {
            self.edit_status = Some("Wait for audio to finish updating".into());
            return;
        }
        let Some(audio) = self.audio.clone() else {
            return;
        };
        let (start, end) = if selection_only {
            let Some((a, b)) = self.selected_times() else {
                self.edit_status = Some("Select a region first".into());
                return;
            };
            (
                (a * audio.rate as f32) as usize,
                ((b * audio.rate as f32) as usize).min(audio.samples.len()),
            )
        } else {
            (0, audio.samples.len())
        };
        let tx = self.tx.clone();
        let epoch = self.generation.load(Ordering::Acquire);
        self.edit_status = Some("exporting…".into());
        thread::spawn(move || {
            let result = encode::write(&path, &audio, start, end).map(|_| path);
            let _ = tx.send(Message::Exported(epoch, result));
        });
    }
    fn restore_source(&mut self, start: usize, values: Arc<Vec<f32>>, label: &'static str) {
        let (Some(audio), Some(playback)) = (&self.audio, &self.playback) else {
            return;
        };
        self.edit_busy = true;
        self.edit_status = Some(format!("{label}…"));
        let source = playback.source.clone();
        let analyze_after_edit = self.notes.is_empty();
        let rate = audio.rate;
        let size = self.fft_size;
        let hop = self.hop;
        let limit = self.partials;
        let tx = self.tx.clone();
        let generation = self.generation.clone();
        let epoch = generation.load(Ordering::Acquire);
        thread::spawn(move || {
            for (offset, &value) in values.iter().enumerate() {
                if offset % 4096 == 0 && generation.load(Ordering::Acquire) != epoch {
                    return;
                }
                source.set(start + offset, value);
            }
            let samples: Vec<_> = (0..source.len()).map(|i| source.get(i)).collect();
            if generation
                .compare_exchange(epoch, epoch + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }
            let new_epoch = epoch + 1;
            let audio = Audio {
                samples: Arc::new(samples),
                rate,
            };
            let _ = tx.send(Message::SourceSnapshot(new_epoch, audio.clone()));
            let _ = tx.send(Message::EditComplete(new_epoch, label.into()));
            if analyze_after_edit {
                run_analysis(audio, size, hop, limit, tx, generation, new_epoch);
            }
        });
    }
    pub fn apply_operation(&mut self, operation: Operation) {
        let Some(selection) = self.selection.clone() else {
            self.edit_status = Some("Select a region first".into());
            return;
        };
        if self.edit_busy || self.mix_busy {
            return;
        }
        let Some((begin, end)) = selection.bounds() else {
            return;
        };
        if end - begin < 0.001 {
            return;
        }
        let Some(audio) = &self.audio else {
            return;
        };
        let Some(playback) = &self.playback else {
            return;
        };
        self.edit_busy = true;
        self.edit_status = Some(format!("{}…", operation.name()));
        if operation != Operation::Gcd {
            let start = (begin.max(0.) * audio.rate as f32) as usize;
            let finish = ((end.min(self.duration()) * audio.rate as f32) as usize)
                .min(playback.source.len());
            let source = playback.source.clone();
            let analyze_after_edit = self.notes.is_empty();
            let tx = self.tx.clone();
            let generation = self.generation.clone();
            let epoch = generation.load(Ordering::Acquire);
            let rate = audio.rate;
            let size = self.fft_size;
            let hop = self.hop;
            let limit = self.partials;
            thread::spawn(move || {
                let before: Vec<_> = (start..finish).map(|i| source.get(i)).collect();
                let mut after = before.clone();
                edit::apply_samples(&mut after, operation, rate);
                for (offset, &value) in after.iter().enumerate() {
                    if offset % 4096 == 0 && generation.load(Ordering::Acquire) != epoch {
                        return;
                    }
                    source.set(start + offset, value);
                }
                if generation.load(Ordering::Acquire) != epoch {
                    return;
                }
                let samples: Vec<_> = (0..source.len()).map(|i| source.get(i)).collect();
                if generation
                    .compare_exchange(epoch, epoch + 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return;
                }
                let new_epoch = epoch + 1;
                let audio = Audio {
                    samples: Arc::new(samples),
                    rate,
                };
                let _ = tx.send(Message::SourceSnapshot(new_epoch, audio.clone()));
                let _ = tx.send(Message::SourceEdited(
                    new_epoch,
                    start,
                    before,
                    after,
                    operation,
                    operation.name().into(),
                ));
                if analyze_after_edit {
                    run_analysis(audio, size, hop, limit, tx, generation, new_epoch);
                }
            });
        } else {
            let start =
                ((begin.max(0.) * audio.rate as f32) as usize / self.hop).min(self.frames.len());
            let end = (((end.max(0.) * audio.rate as f32) as usize / self.hop) + 1)
                .min(self.frames.len());
            if start >= end {
                self.edit_busy = false;
                self.edit_status = Some("Selection has no analyzed frames yet".into());
                return;
            }
            self.pending_edit = Some(PendingEdit {
                selection,
                next: start,
                end,
                frames: Vec::new(),
            });
        }
    }
    fn advance_edit_snapshot(&mut self) {
        let Some(pending) = &mut self.pending_edit else {
            return;
        };
        let end = (pending.next + 64).min(pending.end);
        for index in pending.next..end {
            let mut frame = self.frames[index].clone();
            frame.spectrum.clear();
            pending.frames.push((index, frame));
        }
        pending.next = end;
        if pending.next != pending.end {
            return;
        }
        let pending = self.pending_edit.take().unwrap();
        let Some(audio) = self.audio.clone() else {
            return;
        };
        let Some(playback) = &self.playback else {
            return;
        };
        let source = playback.source.clone();
        let tx = self.tx.clone();
        let generation = self.generation.clone();
        let epoch = generation.load(Ordering::Acquire);
        let hop = self.hop;
        let size = self.fft_size;
        let limit = self.partials;
        thread::spawn(move || {
            let mut selected = Vec::new();
            for (_, frame) in &pending.frames {
                for peak in &frame.peaks {
                    if pending.selection.contains(frame.time, peak.freq) {
                        selected.push(peak.clone());
                    }
                }
            }
            let Some(base) = edit::common_fundamental(&selected) else {
                let _ = tx.send(Message::EditComplete(epoch, "No common fundamental".into()));
                return;
            };
            let (begin, end) = pending.selection.bounds().unwrap();
            let start = (begin.max(0.) * audio.rate as f32) as usize;
            let finish = ((end.min(audio.samples.len() as f32 / audio.rate as f32)
                * audio.rate as f32) as usize)
                .min(source.len());
            let before: Vec<_> = (start..finish).map(|i| source.get(i)).collect();
            let mut after = before.clone();
            let taper = (audio.rate as usize / 200).max(1);
            for (index, frame) in &pending.frames {
                if generation.load(Ordering::Acquire) != epoch {
                    return;
                }
                let frame_start = (index * hop).max(start);
                let frame_end = ((index + 1) * hop).min(finish);
                if frame_start >= frame_end {
                    continue;
                }
                let changes: Vec<_> = frame
                    .peaks
                    .iter()
                    .filter(|peak| pending.selection.contains(frame.time, peak.freq))
                    .map(|peak| {
                        (
                            peak.amp,
                            peak.freq,
                            (peak.freq / base).round().max(1.) * base,
                            peak.phase,
                        )
                    })
                    .collect();
                for i in frame_start..frame_end {
                    let t = i as f32 / audio.rate as f32 - frame.time;
                    let delta: f32 = changes
                        .iter()
                        .map(|(amp, old, new, phase)| {
                            amp * ((std::f32::consts::TAU * new * t + phase).sin()
                                - (std::f32::consts::TAU * old * t + phase).sin())
                        })
                        .sum();
                    let fade = (i - start).min(finish - 1 - i).min(taper) as f32 / taper as f32;
                    after[i - start] = (before[i - start] + delta * fade).clamp(-1., 1.);
                }
            }
            for (offset, &value) in after.iter().enumerate() {
                if offset % 4096 == 0 && generation.load(Ordering::Acquire) != epoch {
                    return;
                }
                source.set(start + offset, value);
            }
            let samples: Vec<_> = (0..source.len()).map(|i| source.get(i)).collect();
            if generation
                .compare_exchange(epoch, epoch + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }
            let new_epoch = epoch + 1;
            let audio = Audio {
                samples: Arc::new(samples),
                rate: audio.rate,
            };
            let _ = tx.send(Message::SourceSnapshot(new_epoch, audio.clone()));
            let _ = tx.send(Message::SourceEdited(
                new_epoch,
                start,
                before,
                after,
                Operation::Gcd,
                format!("GCD {:.2} Hz", base),
            ));
            run_analysis(audio, size, hop, limit, tx, generation, new_epoch);
        });
    }
    pub fn duration(&self) -> f32 {
        self.audio
            .as_ref()
            .map(|a| a.samples.len() as f32 / a.rate as f32)
            .unwrap_or(0.)
    }
    pub fn time(&self) -> f32 {
        match (&self.audio, &self.playback) {
            (Some(a), Some(p)) => p.position.load(Ordering::Acquire) as f32 / a.rate as f32,
            _ => 0.,
        }
    }
    pub fn seek(&self, t: f32) {
        if let (Some(a), Some(p)) = (&self.audio, &self.playback) {
            p.audition_end.store(u64::MAX, Ordering::Release);
            p.seek_to((t.clamp(0., self.duration()) * a.rate as f32) as u64);
        }
    }
    pub fn play_pause(&self) {
        if let Some(p) = &self.playback {
            if !p.playing.load(Ordering::Relaxed) && self.time() >= self.duration() - 0.001 {
                p.seek_to(0);
            }
            p.playing.fetch_xor(true, Ordering::Relaxed);
        }
    }
    pub fn stop(&self) {
        if let Some(p) = &self.playback {
            p.playing.store(false, Ordering::Release);
            p.audition_end.store(u64::MAX, Ordering::Release);
            p.seek_to(0);
            p.position.store(0, Ordering::Release);
        }
    }
    fn selected_times(&self) -> Option<(f32, f32)> {
        let (start, end) = self.selection.as_ref()?.bounds()?;
        let start = start.clamp(0., self.duration());
        let end = end.clamp(0., self.duration());
        (end - start >= 0.001).then_some((start, end))
    }
    pub fn audition_selection(&mut self) {
        let Some((start, end)) = self.selected_times() else {
            self.edit_status = Some("Select a region first".into());
            return;
        };
        let (Some(audio), Some(p)) = (&self.audio, &self.playback) else {
            return;
        };
        p.playing.store(false, Ordering::Release);
        p.audition_end
            .store((end * audio.rate as f32) as u64, Ordering::Release);
        p.seek_to((start * audio.rate as f32) as u64);
        p.playing.store(true, Ordering::Release);
        self.edit_status = Some("playing selection".into());
    }
    pub fn loop_selection(&mut self) {
        let Some((start, end)) = self.selected_times() else {
            self.edit_status = Some("Select a region first".into());
            return;
        };
        let (Some(audio), Some(p)) = (&self.audio, &self.playback) else {
            return;
        };
        self.loop_start = start;
        self.loop_end = end;
        p.loop_start
            .store((start * audio.rate as f32) as u64, Ordering::Release);
        p.loop_end
            .store((end * audio.rate as f32) as u64, Ordering::Release);
        p.audition_end.store(u64::MAX, Ordering::Release);
        p.looping.store(true, Ordering::Release);
        self.edit_status = Some("loop selection".into());
    }
    pub fn fit_all(&mut self) {
        self.zoom = 1.;
        self.pan = 0.;
    }
    pub fn zoom_selection(&mut self) {
        let duration = self.duration();
        if duration <= 0. {
            return;
        }
        let (center, width) = if let Some((start, end)) = self.selected_times() {
            ((start + end) * 0.5, (end - start) * 1.1)
        } else {
            (
                self.caret.unwrap_or_else(|| self.time()),
                self.visible().1 * 0.5,
            )
        };
        let span = width.clamp(0.1, duration);
        self.zoom = (duration / span).clamp(1., 1000.);
        self.pan = (center - span * 0.5).clamp(0., (duration - span).max(0.));
    }
    pub fn select_all(&mut self) {
        if self.audio.is_none() {
            return;
        }
        self.edit_mode = true;
        self.selection = Some(Selection::Range {
            start: 0.,
            end: self.duration(),
        });
        self.caret = Some(0.);
        self.request_selection_stats();
    }
    pub fn jump_event(&mut self, forward: bool) {
        let now = self.time();
        let target = if !self.notes.is_empty() {
            let mut times: Vec<_> = self.notes.iter().map(|n| n.start as f32).collect();
            times.sort_by(f32::total_cmp);
            if forward {
                times.into_iter().find(|&t| t > now + 0.03)
            } else {
                times.into_iter().rev().find(|&t| t < now - 0.03)
            }
        } else if !self.events.is_empty() {
            if forward {
                self.events
                    .iter()
                    .map(|e| e.start)
                    .find(|&t| t > now + 0.03)
            } else {
                self.events
                    .iter()
                    .rev()
                    .map(|e| e.start)
                    .find(|&t| t < now - 0.03)
            }
        } else if forward {
            self.frames
                .iter()
                .filter(|f| f.onset)
                .map(|f| f.time)
                .find(|&t| t > now + 0.03)
        } else {
            self.frames
                .iter()
                .rev()
                .filter(|f| f.onset)
                .map(|f| f.time)
                .find(|&t| t < now - 0.03)
        };
        if let Some(time) = target {
            self.seek(time);
            self.reveal_time(time);
        }
    }
    pub fn select_event(&mut self) {
        let now = self.time();
        let bounds = if !self.notes.is_empty() {
            self.notes
                .iter()
                .filter(|n| n.start <= now as f64 && (now as f64) < n.end)
                .min_by(|a, b| a.start.total_cmp(&b.start))
                .map(|n| (n.start as f32, n.end as f32))
        } else if !self.events.is_empty() {
            self.events
                .iter()
                .find(|e| e.start <= now && now <= e.end)
                .map(|e| (e.start, e.end))
        } else if self.frames.last().is_some_and(|frame| {
            now <= frame.time + self.hop as f32 / self.audio.as_ref().unwrap().rate as f32
        }) {
            let start = self
                .frames
                .iter()
                .take_while(|f| f.time <= now)
                .filter(|f| f.onset)
                .map(|f| f.time)
                .last()
                .unwrap_or(0.);
            let end = self
                .frames
                .iter()
                .find(|f| f.onset && f.time > now)
                .map(|f| f.time)
                .unwrap_or_else(|| {
                    (self.frames.last().unwrap().time
                        + self.hop as f32 / self.audio.as_ref().unwrap().rate as f32)
                        .min(self.duration())
                });
            (end > start).then_some((start, end))
        } else {
            None
        };
        if let Some((start, end)) = bounds.filter(|(a, b)| b - a >= 0.001) {
            self.edit_mode = true;
            self.selection = Some(Selection::Range { start, end });
            self.caret = Some(start);
            self.edit_status = Some("event selected".into());
            self.reveal_time(start);
            self.request_selection_stats();
        } else {
            self.edit_status = Some("No event at playhead".into());
        }
    }
    fn reveal_time(&mut self, time: f32) {
        let (left, span) = self.visible();
        if time < left || time > left + span {
            self.pan = (time - span * 0.5).clamp(0., (self.duration() - span).max(0.));
        }
    }
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.caret = None;
        self.invalidate_selection_stats();
    }
    pub fn invalidate_selection_stats(&mut self) {
        self.selection_stats = None;
        self.measurement_token.fetch_add(1, Ordering::AcqRel);
    }
    pub fn request_selection_stats(&mut self) {
        self.invalidate_selection_stats();
        let token = self.measurement_token.load(Ordering::Acquire);
        if !matches!(self.selection, Some(Selection::Range { .. })) {
            return;
        }
        let (Some((start, end)), Some(audio), Some(p)) =
            (self.selected_times(), &self.audio, &self.playback)
        else {
            return;
        };
        let first = (start * audio.rate as f32) as usize;
        let last = ((end * audio.rate as f32) as usize).min(p.source.len());
        let source = p.source.clone();
        let cancel = self.measurement_token.clone();
        let tx = self.tx.clone();
        let epoch = self.generation.load(Ordering::Acquire);
        thread::spawn(move || {
            if let Some(stats) = measure_region(&source, first, last, &cancel, token) {
                let _ = tx.send(Message::SelectionMeasured(epoch, token, stats));
            }
        });
    }
    pub fn toggle_loop(&self) {
        if let Some(p) = &self.playback {
            p.looping.fetch_xor(true, Ordering::Relaxed);
        }
    }
    pub fn set_volume(&self, v: f32) {
        if let Some(p) = &self.playback {
            p.volume.store(v.clamp(0., 1.).to_bits(), Ordering::Relaxed);
        }
    }
    pub fn visible(&self) -> (f32, f32) {
        let dur = self.duration().max(1.);
        let span = (dur / self.zoom).max(0.1);
        (self.pan.clamp(0., (dur - span).max(0.)), span)
    }
    pub fn sync_follow(&mut self) {
        if !self.follow_playhead || self.zoom <= 1. {
            return;
        }
        if !self
            .playback
            .as_ref()
            .is_some_and(|p| p.playing.load(Ordering::Relaxed))
        {
            return;
        }
        let (left, span) = self.visible();
        self.pan = follow_pan(left, span, self.time(), self.duration());
    }
    pub fn frame(&self) -> Option<&Frame> {
        let a = self.audio.as_ref()?;
        let time = if self.edit_mode {
            self.caret.unwrap_or_else(|| self.time())
        } else {
            self.time()
        };
        self.frames.get(
            ((time * a.rate as f32 - self.fft_size as f32 * 0.5).max(0.) / self.hop as f32)
                as usize,
        )
    }
    pub fn spec_pixels(&mut self, palette: Palette) -> Vec<u8> {
        let width = self.frames.len().min(2048).max(1) as u32;
        let height = 512u32;
        let mut out = vec![0u8; (width * height * 4) as usize];
        for x in 0..width {
            let frame = &self.frames
                [(x as usize * self.frames.len() / width as usize).min(self.frames.len() - 1)];
            for y in 0..height {
                let bin = ((height - 1 - y) as usize * frame.spectrum.len() / height as usize)
                    .min(frame.spectrum.len() - 1);
                let db = (20. * (frame.spectrum[bin] / self.fft_size as f32 + 1e-7).log10())
                    .clamp(-90., 0.);
                let v = ((db + 90.) / 90.).powf(1.5);
                let i = ((y * width + x) * 4) as usize;
                for (channel, (bg, fg)) in [
                    (palette.bg.0, palette.ink.0),
                    (palette.bg.1, palette.ink.1),
                    (palette.bg.2, palette.ink.2),
                ]
                .into_iter()
                .enumerate()
                {
                    out[i + channel] = ((bg * (1. - v) + fg * v).clamp(0., 1.) * 255.) as u8;
                }
                out[i + 3] = 255;
            }
        }
        self.spec_width = width;
        self.spec_height = height;
        self.spec_dirty = false;
        out
    }
}
fn rebase_edits(
    base: &Audio,
    old_base: &Audio,
    old_master: &Audio,
    patches: Vec<package::Patch>,
) -> (Audio, Vec<package::Patch>) {
    let mut data = (*base.samples).clone();
    let count = data
        .len()
        .min(old_base.samples.len())
        .min(old_master.samples.len());
    for i in 0..count {
        let difference = old_master.samples[i] - old_base.samples[i];
        if difference.abs() > 1e-7 {
            data[i] = (data[i] + difference).clamp(-1., 1.);
        }
    }
    let mut rebased = Vec::new();
    for patch in patches {
        let end = patch.start.saturating_add(patch.after.len());
        if end > count {
            continue;
        }
        let mut before = Vec::with_capacity(patch.before.len());
        let mut after = Vec::with_capacity(patch.after.len());
        for i in patch.start..end {
            let delta = base.samples[i] - old_base.samples[i];
            before.push((patch.before[i - patch.start] + delta).clamp(-1., 1.));
            after.push((patch.after[i - patch.start] + delta).clamp(-1., 1.));
        }
        rebased.push(package::Patch {
            start: patch.start,
            before: Arc::new(before),
            after: Arc::new(after),
            marker_id: patch.marker_id,
        });
    }
    (
        Audio {
            samples: Arc::new(data),
            rate: base.rate,
        },
        rebased,
    )
}
fn follow_pan(left: f32, span: f32, time: f32, duration: f32) -> f32 {
    let next = if time > left + span * 0.78 {
        time - span * 0.78
    } else if time < left + span * 0.12 {
        time - span * 0.2
    } else {
        left
    };
    next.clamp(0., (duration - span).max(0.))
}
fn measure_region(
    source: &EditSource,
    start: usize,
    end: usize,
    cancel: &AtomicU64,
    token: u64,
) -> Option<SelectionStats> {
    if end <= start || end > source.len() {
        return None;
    }
    let mut peak = 0f32;
    let mut power = 0f64;
    let mut sum = 0f64;
    let mut clipped = 0;
    for i in start..end {
        if i % 4096 == 0 && cancel.load(Ordering::Relaxed) != token {
            return None;
        }
        let value = source.get(i);
        peak = peak.max(value.abs());
        power += (value as f64) * (value as f64);
        sum += value as f64;
        clipped += usize::from(value.abs() >= 0.999);
    }
    let samples = end - start;
    Some(SelectionStats {
        samples,
        peak,
        rms: (power / samples as f64).sqrt() as f32,
        dc: (sum / samples as f64) as f32,
        clipped,
    })
}
fn run_analysis(
    audio: Audio,
    size: usize,
    hop: usize,
    limit: usize,
    tx: Sender<Message>,
    generation: Arc<AtomicU64>,
    epoch: u64,
) {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    let len = audio.samples.len();
    let total = len.div_ceil(hop);
    let batches = total.div_ceil(64);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).clamp(1, 4))
        .unwrap_or(1);
    let (job_tx, job_rx) = mpsc::channel::<(usize, usize, usize)>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (result_tx, result_rx) = mpsc::sync_channel::<(usize, Vec<Frame>)>(workers * 2);
    thread::scope(|scope| {
        for _ in 0..workers {
            let jobs = job_rx.clone();
            let results = result_tx.clone();
            let audio = audio.clone();
            let generation = generation.clone();
            scope.spawn(move || loop {
                let job = { jobs.lock().unwrap().recv() };
                let Ok((batch, start, end)) = job else {
                    break;
                };
                if generation.load(Ordering::Acquire) != epoch {
                    break;
                }
                let mut raw = Vec::with_capacity(end - start);
                for i in start..end {
                    if generation.load(Ordering::Acquire) != epoch {
                        return;
                    }
                    raw.push(stft::analyze_raw(
                        &audio.samples[i * hop..],
                        audio.rate,
                        size,
                        limit,
                    ));
                }
                if results.send((batch, raw)).is_err() {
                    break;
                }
            });
        }
        drop(result_tx);
        for batch in 0..batches {
            let start = batch * 64;
            let end = (start + 64).min(total);
            if job_tx.send((batch, start, end)).is_err() {
                break;
            }
        }
        drop(job_tx);
        let mut pending = BTreeMap::new();
        let mut next_batch = 0;
        let mut frames = Vec::with_capacity(total);
        let mut previous: Option<Frame> = None;
        let mut next_track = 1;
        while let Ok((batch, raw)) = result_rx.recv() {
            if generation.load(Ordering::Acquire) != epoch {
                break;
            }
            pending.insert(batch, raw);
            while let Some(raw) = pending.remove(&next_batch) {
                let mut ui_batch = Vec::with_capacity(raw.len());
                for (offset, mut frame) in raw.into_iter().enumerate() {
                    let i = next_batch * 64 + offset;
                    frame.time = (i * hop + size / 2) as f32 / audio.rate as f32;
                    stft::finish(&mut frame, previous.as_ref(), &mut next_track);
                    previous = Some(frame.clone());
                    ui_batch.push(frame.clone());
                    frame.spectrum.clear();
                    frames.push(frame);
                }
                if tx.send(Message::Frames(epoch, ui_batch)).is_err() {
                    return;
                }
                next_batch += 1;
            }
        }
        drop(result_rx);
        if generation.load(Ordering::Acquire) == epoch && next_batch == batches {
            let _ = tx.send(Message::Events(epoch, crate::model::segment(&frames)));
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_frames_are_ordered() {
        let rate = 44100;
        let samples: Vec<_> = (0..rate)
            .map(|i| (std::f32::consts::TAU * 440. * i as f32 / rate as f32).sin() * 0.5)
            .collect();
        let audio = Audio {
            samples: Arc::new(samples),
            rate,
        };
        let (tx, rx) = mpsc::channel();
        run_analysis(audio, 2048, 512, 16, tx, Arc::new(AtomicU64::new(1)), 1);
        let mut frames = Vec::new();
        while let Ok(message) = rx.try_recv() {
            match message {
                Message::Frames(_, batch) => frames.extend(batch),
                _ => {}
            }
        }
        assert_eq!(frames.len(), rate as usize / 512 + 1);
        assert!(frames.windows(2).all(|w| w[0].time < w[1].time));
        assert!((frames[10].f0.unwrap() - 440.).abs() < 5.);
    }
    #[test]
    fn follow_keeps_playhead_in_view_and_handles_loop_jump() {
        assert_eq!(follow_pan(0., 10., 5., 60.), 0.);
        assert!((follow_pan(0., 10., 8., 60.) - 0.2).abs() < 1e-5);
        assert_eq!(follow_pan(40., 10., 2., 60.), 0.);
        assert_eq!(follow_pan(50., 10., 60., 60.), 50.);
    }
    #[test]
    fn selection_measurements_include_levels_dc_and_clipping() {
        let source = EditSource::new(Arc::new(vec![1., 0., -1., 0., 0.5]));
        let token = AtomicU64::new(7);
        let stats = measure_region(&source, 0, 4, &token, 7).unwrap();
        assert_eq!(stats.samples, 4);
        assert_eq!(stats.peak, 1.);
        assert!((stats.rms - 0.5f32.sqrt()).abs() < 1e-6);
        assert_eq!(stats.dc, 0.);
        assert_eq!(stats.clipped, 2);
        token.store(8, Ordering::Release);
        assert!(measure_region(&source, 0, 4, &token, 7).is_none());
    }
    #[test]
    fn select_event_uses_active_midi_note() {
        let mut app = App::new();
        app.audio = Some(Audio {
            samples: Arc::new(vec![0.; 44100]),
            rate: 44100,
        });
        app.notes.push(Note {
            start: 0.,
            end: 0.5,
            pitch: 69,
            velocity: 100,
            channel: 0,
        });
        app.select_event();
        assert!(matches!(
            app.selection,
            Some(Selection::Range {
                start: 0.,
                end: 0.5
            })
        ));
        assert!(app.edit_mode);
    }
    #[test]
    fn rebasing_preserves_edits_beyond_short_undo_history() {
        let audio = |samples: Vec<f32>| Audio {
            samples: Arc::new(samples),
            rate: 1000,
        };
        let old_base = audio(vec![0.2, 0.2, 0.2, 0.2]);
        let old_master = audio(vec![0.2, 0.5, 0.6, 0.2]);
        let new_base = audio(vec![0.1, 0.1, 0.1, 0.1]);
        let patch = package::Patch {
            start: 2,
            before: Arc::new(vec![0.2]),
            after: Arc::new(vec![0.6]),
            marker_id: 7,
        };
        let (result, history) = rebase_edits(&new_base, &old_base, &old_master, vec![patch]);
        assert!((result.samples[1] - 0.4).abs() < 1e-6);
        assert!((result.samples[2] - 0.5).abs() < 1e-6);
        assert!((history[0].before[0] - 0.1).abs() < 1e-6);
        assert!((history[0].after[0] - 0.5).abs() < 1e-6);
        assert_eq!(history[0].marker_id, 7);
    }
}
