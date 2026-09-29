use crate::{
    app::App,
    edit::{Operation, Selection, Tool},
    render::{
        gpu::{Color, Gpu},
        plots,
    },
    theme::Palette,
};
use std::{path::PathBuf, sync::atomic::Ordering};
const TRANSITION_EDITOR_HEIGHT: f32 = 250.;
const RECENT_LIMIT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelpInfo {
    pub title: &'static str,
    pub body: &'static str,
}
#[derive(Default)]
pub struct HelpState {
    pub active: bool,
    pub topic: Option<HelpInfo>,
    pub point: (f32, f32),
}
impl HelpState {
    pub fn toggle(&mut self) {
        self.active = !self.active;
        self.topic = None;
    }
    pub fn close(&mut self) {
        self.active = false;
        self.topic = None;
    }
    pub fn inspect(&mut self, topic: HelpInfo, point: (f32, f32)) {
        self.topic = Some(topic);
        self.point = point;
    }
}
pub fn help_button_hit(width: f32, height: f32, loaded: bool, x: f32, y: f32) -> bool {
    let top = if loaded { height - 38. } else { 8. };
    x >= width - 44. && x < width - 8. && y >= top && y < top + 38.
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WelcomeAction {
    OpenFile,
    OpenProject,
    Recent(usize),
}
fn welcome_bounds(width: f32, height: f32) -> (f32, f32, f32) {
    let w = (width - 48.).min(520.);
    ((width - w) * 0.5, ((height - 430.) * 0.5).max(32.), w)
}
pub fn welcome_action(
    width: f32,
    height: f32,
    x: f32,
    y: f32,
    recent_count: usize,
) -> Option<WelcomeAction> {
    let (left, top, w) = welcome_bounds(width, height);
    if x < left || x >= left + w {
        return None;
    }
    if (top + 48. ..top + 92.).contains(&y) {
        return Some(WelcomeAction::OpenFile);
    }
    if (top + 100. ..top + 144.).contains(&y) {
        return Some(WelcomeAction::OpenProject);
    }
    let first = top + 191.;
    if y >= first {
        let index = ((y - first) / 38.) as usize;
        if index < recent_count.min(RECENT_LIMIT) && y < first + index as f32 * 38. + 36. {
            return Some(WelcomeAction::Recent(index));
        }
    }
    None
}
fn short_label(text: &str, limit: usize) -> String {
    let len = text.chars().count();
    if len <= limit {
        text.to_owned()
    } else {
        text.chars()
            .take(limit - 1)
            .chain(std::iter::once('…'))
            .collect()
    }
}
pub struct Layout {
    pub plot_w: f32,
    pub panel_x: f32,
    pub transport_y: f32,
    pub rows: [(f32, f32); 4],
}
pub fn layout(g: &Gpu) -> Layout {
    let transport_y = g.height - 38.;
    let panel_x = (g.width - 355.).max(g.width * 0.6);
    let plot_w = panel_x;
    let h = (transport_y - 8.) / 4.;
    Layout {
        plot_w,
        panel_x,
        transport_y,
        rows: [(0., h), (h, 2. * h), (2. * h, 3. * h), (3. * h, 4. * h)],
    }
}
fn axis(g: &mut Gpu, x: f32, y: f32, w: f32, h: f32, label: &str, p: Palette) {
    g.rect(x, y + h - 1., w, 1., p.line);
    g.text(x + 8., y + 17., 12., label, p.muted);
}
fn time_x(t: f32, a: f32, span: f32, w: f32) -> f32 {
    (t - a) / span * w
}
fn level_db(value: f32) -> String {
    if value <= 1e-9 {
        "-inf".into()
    } else {
        format!("{:.1}", 20. * value.log10())
    }
}
fn marquee_text(name: &str, width: usize, seconds: f32, active: bool) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= width {
        return name.to_owned();
    }
    if !active {
        return chars
            .iter()
            .take(width - 1)
            .copied()
            .chain(std::iter::once('…'))
            .collect();
    }
    let travel = chars.len() - width;
    let pause = 1.2;
    let scroll = travel as f32 / 8.;
    let cycle = 2. * (pause + scroll);
    let phase = seconds.rem_euclid(cycle);
    let offset = if phase < pause {
        0
    } else if phase < pause + scroll {
        ((phase - pause) * 8.).floor() as usize
    } else if phase < 2. * pause + scroll {
        travel
    } else {
        travel.saturating_sub(((phase - 2. * pause - scroll) * 8.).floor() as usize)
    };
    chars[offset..offset + width].iter().collect()
}
fn edit_color(operation: Operation, p: Palette) -> Color {
    match operation.family() {
        0 => p.edit_gain,
        1 => p.edit_shape,
        _ => p.edit_filter,
    }
}
fn transition_marker_rect(
    index: usize,
    start: f32,
    end: f32,
    left: f32,
    span: f32,
    width: f32,
    wave_top: f32,
) -> Option<(f32, f32, f32)> {
    if end < left || start > left + span {
        return None;
    }
    let badge_w = format!("T{index}").chars().count() as f32 * 7. + 12.;
    let badge_x = time_x(start, left, span, width)
        .clamp(0., width)
        .min((width - badge_w).max(0.));
    let badge_y = wave_top + 8. + (index.saturating_sub(1) % 2) as f32 * 19.;
    Some((badge_x, badge_y, badge_w))
}
fn draw_change_marks(
    g: &mut Gpu,
    a: &App,
    p: Palette,
    left: f32,
    span: f32,
    width: f32,
    cursor: (f32, f32),
    rows: &[(f32, f32); 4],
) {
    let Some(audio) = &a.audio else { return };
    let (wave_top, wave_bottom) = rows[0];
    let mut edit_slot = 0usize;
    for mark in a
        .edit_marks
        .iter()
        .filter(|m| m.active)
        .filter(|m| {
            let start = m.start as f32 / audio.rate as f32;
            let end = m.end as f32 / audio.rate as f32;
            end >= left && start <= left + span
        })
        .take(256)
    {
        let color = edit_color(mark.operation, p);
        let start = mark.start as f32 / audio.rate as f32;
        let end = mark.end as f32 / audio.rate as f32;
        let x1 = time_x(start, left, span, width).clamp(0., width);
        let x2 = time_x(end, left, span, width).clamp(0., width);
        g.rect(x1, wave_bottom - 7., (x2 - x1).max(1.), 3., color);
        g.rect(x1, wave_bottom - 11., 1., 7., color);
        g.rect(x2, wave_bottom - 11., 1., 7., color);
        let label = format!("{}{}", mark.operation.badge(), mark.id);
        let badge_w = (label.chars().count() as f32 * 7. + 10.).min(58.);
        let badge_x = x1.min((width - badge_w).max(0.));
        let badge_y = wave_top + 50. + (edit_slot % 3) as f32 * 19.;
        edit_slot += 1;
        g.rect(badge_x, badge_y, badge_w, 17., color);
        g.text(badge_x + 4., badge_y + 13., 11., &label, p.bg);
        let hovered = cursor.0 >= badge_x
            && cursor.0 < badge_x + badge_w
            && cursor.1 >= badge_y
            && cursor.1 < badge_y + 17.;
        if hovered {
            let text = format!("{}  {:.2}–{:.2} s", mark.operation.name(), start, end);
            let tx = (badge_x + badge_w + 6.).min((width - 220.).max(0.));
            g.rect(tx, badge_y, 216., 17., p.panel);
            g.text(tx + 4., badge_y + 13., 11., &text, p.ink);
        }
    }
    for (i, place) in a.placements.iter().enumerate().skip(1) {
        let start = place.start as f32 / audio.rate as f32;
        let end = (place.start + place.overlap) as f32 / audio.rate as f32;
        let Some((badge_x, badge_y, badge_w)) =
            transition_marker_rect(i, start, end, left, span, width, wave_top)
        else {
            continue;
        };
        let x1 = time_x(start, left, span, width).clamp(0., width);
        let x2 = time_x(end, left, span, width).clamp(0., width);
        let selected = a.transition_index() == Some(i);
        let span_color = Color(p.transition.0, p.transition.1, p.transition.2, 0.08);
        if x2 > x1 {
            g.rect(
                x1,
                wave_top + 1.,
                x2 - x1,
                wave_bottom - wave_top - 2.,
                span_color,
            );
        }
        g.rect(x1, wave_top + 2., (x2 - x1).max(1.), 3., p.transition);
        g.rect(
            x1,
            wave_top + 2.,
            1.,
            wave_bottom - wave_top - 4.,
            p.transition,
        );
        g.rect(
            x2,
            wave_top + 2.,
            1.,
            wave_bottom - wave_top - 4.,
            p.transition,
        );
        let label = format!("T{i}");
        g.rect(
            badge_x,
            badge_y,
            badge_w,
            17.,
            if selected { p.ink } else { p.transition },
        );
        g.text(badge_x + 4., badge_y + 13., 11., &label, p.bg);
        if cursor.0 >= badge_x
            && cursor.0 < badge_x + badge_w
            && cursor.1 >= badge_y
            && cursor.1 < badge_y + 17.
        {
            let t = &a.queue[i].transition;
            let curve = match t.curve {
                crate::project::Curve::Linear => "linear",
                crate::project::Curve::EqualPower => "equal power",
                crate::project::Curve::Smooth => "smooth",
            };
            let phase = if place.inverted {
                "flip"
            } else if t.phase_match {
                "phase"
            } else {
                "free"
            };
            let text = format!(
                "{:.2}s  cue {:.2}s  {curve}  {phase}",
                place.overlap as f32 / audio.rate as f32,
                t.in_cue
            );
            let tx = (badge_x + badge_w + 6.).min((width - 260.).max(0.));
            g.rect(tx, badge_y, 255., 17., p.panel);
            g.text(tx + 4., badge_y + 13., 11., &text, p.ink);
        }
    }
}
pub fn transition_marker_at(g: &Gpu, a: &App, x: f32, y: f32) -> Option<usize> {
    let audio = a.audio.as_ref()?;
    let l = layout(g);
    let (wave_top, _) = l.rows[0];
    let (left, span) = a.visible();
    let width = l.plot_w - 12.;
    for (i, place) in a.placements.iter().enumerate().skip(1).rev() {
        let start = place.start as f32 / audio.rate as f32;
        let end = (place.start + place.overlap) as f32 / audio.rate as f32;
        let Some((badge_x, badge_y, label_w)) =
            transition_marker_rect(i, start, end, left, span, width, wave_top)
        else {
            continue;
        };
        if x >= badge_x && x < badge_x + label_w && y >= badge_y && y < badge_y + 17. {
            return Some(i);
        }
    }
    None
}
fn edit_marker_at(g: &Gpu, a: &App, x: f32, y: f32) -> Option<Operation> {
    let audio = a.audio.as_ref()?;
    let l = layout(g);
    let (left, span) = a.visible();
    let width = l.plot_w - 12.;
    let mut slot = 0usize;
    for mark in a
        .edit_marks
        .iter()
        .filter(|mark| mark.active)
        .filter(|mark| {
            let start = mark.start as f32 / audio.rate as f32;
            let end = mark.end as f32 / audio.rate as f32;
            end >= left && start <= left + span
        })
        .take(256)
    {
        let start = mark.start as f32 / audio.rate as f32;
        let label = format!("{}{}", mark.operation.badge(), mark.id);
        let badge_w = (label.chars().count() as f32 * 7. + 10.).min(58.);
        let badge_x = time_x(start, left, span, width)
            .clamp(0., width)
            .min((width - badge_w).max(0.));
        let badge_y = l.rows[0].0 + 50. + (slot % 3) as f32 * 19.;
        slot += 1;
        if x >= badge_x && x < badge_x + badge_w && y >= badge_y && y < badge_y + 17. {
            return Some(mark.operation);
        }
    }
    None
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    Edit,
    Fit,
    Zoom,
    LoopSelection,
    Audition,
    Tool(Tool),
    Operation(Operation),
    Open,
    Add,
    Save,
    Export,
    Undo,
    Redo,
}
#[derive(Clone, Copy)]
struct Control {
    action: PanelAction,
    label: &'static str,
    x: f32,
    y: f32,
    w: f32,
}
impl Control {
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + 28.
    }
}
fn panel_controls(panel_x: f32, _width: f32, edit_mode: bool) -> Vec<Control> {
    let px = panel_x + 18.;
    let mut controls = vec![
        Control {
            action: PanelAction::Edit,
            label: "edit",
            x: px,
            y: 5.,
            w: 47.,
        },
        Control {
            action: PanelAction::Fit,
            label: "fit",
            x: px + 51.,
            y: 5.,
            w: 39.,
        },
        Control {
            action: PanelAction::Add,
            label: "add",
            x: px + 94.,
            y: 5.,
            w: 43.,
        },
        Control {
            action: PanelAction::Open,
            label: "open",
            x: px + 141.,
            y: 5.,
            w: 49.,
        },
        Control {
            action: PanelAction::Save,
            label: "save",
            x: px + 194.,
            y: 5.,
            w: 49.,
        },
        Control {
            action: PanelAction::Export,
            label: "export",
            x: px + 247.,
            y: 5.,
            w: 73.,
        },
    ];
    if edit_mode {
        controls.extend([
            Control {
                action: PanelAction::Tool(Tool::Range),
                label: "range",
                x: px,
                y: 37.,
                w: 70.,
            },
            Control {
                action: PanelAction::Tool(Tool::Lasso),
                label: "lasso",
                x: px + 76.,
                y: 37.,
                w: 72.,
            },
            Control {
                action: PanelAction::Operation(Operation::Gcd),
                label: "gcd",
                x: px + 154.,
                y: 37.,
                w: 55.,
            },
            Control {
                action: PanelAction::Undo,
                label: "undo",
                x: px + 214.,
                y: 37.,
                w: 54.,
            },
            Control {
                action: PanelAction::Redo,
                label: "redo",
                x: px + 274.,
                y: 37.,
                w: 54.,
            },
            Control {
                action: PanelAction::Operation(Operation::GainUp),
                label: "+3 dB",
                x: px,
                y: 69.,
                w: 62.,
            },
            Control {
                action: PanelAction::Operation(Operation::GainDown),
                label: "-3 dB",
                x: px + 68.,
                y: 69.,
                w: 62.,
            },
            Control {
                action: PanelAction::Operation(Operation::Normalize),
                label: "normal",
                x: px + 136.,
                y: 69.,
                w: 82.,
            },
            Control {
                action: PanelAction::Operation(Operation::Mute),
                label: "mute",
                x: px + 224.,
                y: 69.,
                w: 64.,
            },
            Control {
                action: PanelAction::Operation(Operation::Reverse),
                label: "reverse",
                x: px,
                y: 101.,
                w: 82.,
            },
            Control {
                action: PanelAction::Operation(Operation::FadeIn),
                label: "fade in",
                x: px + 88.,
                y: 101.,
                w: 82.,
            },
            Control {
                action: PanelAction::Operation(Operation::FadeOut),
                label: "fade out",
                x: px + 176.,
                y: 101.,
                w: 88.,
            },
            Control {
                action: PanelAction::LoopSelection,
                label: "loop",
                x: px + 270.,
                y: 101.,
                w: 58.,
            },
            Control {
                action: PanelAction::Operation(Operation::RemoveDc),
                label: "remove DC",
                x: px,
                y: 133.,
                w: 92.,
            },
            Control {
                action: PanelAction::Operation(Operation::Invert),
                label: "invert",
                x: px + 98.,
                y: 133.,
                w: 72.,
            },
            Control {
                action: PanelAction::Operation(Operation::Compress),
                label: "compress",
                x: px + 176.,
                y: 133.,
                w: 76.,
            },
            Control {
                action: PanelAction::Audition,
                label: "play sel",
                x: px + 258.,
                y: 133.,
                w: 70.,
            },
            Control {
                action: PanelAction::Operation(Operation::HighPass),
                label: "HP 120",
                x: px,
                y: 165.,
                w: 70.,
            },
            Control {
                action: PanelAction::Operation(Operation::LowPass),
                label: "LP 6k",
                x: px + 75.,
                y: 165.,
                w: 64.,
            },
            Control {
                action: PanelAction::Operation(Operation::Gate),
                label: "gate",
                x: px + 144.,
                y: 165.,
                w: 55.,
            },
            Control {
                action: PanelAction::Operation(Operation::SoftClip),
                label: "soft clip",
                x: px + 204.,
                y: 165.,
                w: 74.,
            },
            Control {
                action: PanelAction::Operation(Operation::FadeBoth),
                label: "edge",
                x: px + 283.,
                y: 165.,
                w: 45.,
            },
        ]);
    }
    controls
}
pub fn panel_action(g: &Gpu, edit_mode: bool, x: f32, y: f32) -> Option<PanelAction> {
    let l = layout(g);
    panel_controls(l.panel_x, g.width, edit_mode)
        .into_iter()
        .find(|control| control.contains(x, y))
        .map(|control| control.action)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    Theme,
    Import,
    Reload,
    Surface,
}
pub fn settings_action(g: &Gpu, a: &App, x: f32, y: f32) -> Option<SettingsAction> {
    if !a.settings {
        return None;
    }
    let l = layout(g);
    let sy = l.transport_y - 190.;
    if x < l.panel_x + 1. || y < sy || y >= l.transport_y {
        return None;
    }
    if (sy + 68. ..sy + 98.).contains(&y) {
        return if x < l.panel_x + 150. {
            Some(SettingsAction::Theme)
        } else if x < l.panel_x + 245. {
            Some(SettingsAction::Import)
        } else {
            Some(SettingsAction::Reload)
        };
    }
    Some(SettingsAction::Surface)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueAction {
    Select(usize),
    Up,
    Down,
    Remove,
    DurationDown,
    DurationUp,
    CueDown,
    CueUp,
    Curve,
    Phase,
    Preview,
    AuditionTransition,
    LoopTransition,
    PreviousTransition,
    NextTransition,
    Auto,
    AutoAll,
}
pub fn queue_action(g: &Gpu, a: &App, x: f32, y: f32) -> Option<QueueAction> {
    let l = layout(g);
    if x < l.panel_x || y >= l.transport_y {
        return None;
    }
    let top = if a.edit_mode { 228. } else { 48. };
    if y >= top && y < top + 25. {
        return if x >= l.panel_x + 175. && x < l.panel_x + 230. {
            Some(QueueAction::Up)
        } else if x < l.panel_x + 285. && x >= l.panel_x + 230. {
            Some(QueueAction::Down)
        } else if x >= l.panel_x + 285. {
            Some(QueueAction::Remove)
        } else {
            None
        };
    }
    let list_top = top + 25.;
    let editor_top = (l.transport_y - TRANSITION_EDITOR_HEIGHT).max(list_top + 32.);
    if y >= list_top && y < editor_top {
        let index = a.queue_scroll + ((y - list_top) / 28.) as usize;
        return (index < a.queue.len()).then_some(QueueAction::Select(index));
    }
    if a.transition_index().is_none() {
        return None;
    }
    transition_action(l.panel_x, g.width, x, y - editor_top)
}
fn transition_action(panel_x: f32, width: f32, x: f32, dy: f32) -> Option<QueueAction> {
    if (0. ..34.).contains(&dy) {
        return if x >= width - 100. && x < width - 52. {
            Some(QueueAction::PreviousTransition)
        } else if x >= width - 52. {
            Some(QueueAction::NextTransition)
        } else {
            None
        };
    }
    if (34. ..64.).contains(&dy) {
        return if x >= width - 50. {
            Some(QueueAction::DurationUp)
        } else if x >= width - 90. {
            Some(QueueAction::DurationDown)
        } else {
            None
        };
    }
    if (64. ..92.).contains(&dy) {
        return if x >= width - 50. {
            Some(QueueAction::CueUp)
        } else if x >= width - 90. {
            Some(QueueAction::CueDown)
        } else {
            None
        };
    }
    if (93. ..120.).contains(&dy) {
        return if x < panel_x + 150. {
            Some(QueueAction::Curve)
        } else {
            Some(QueueAction::Phase)
        };
    }
    if (120. ..214.).contains(&dy) {
        return Some(QueueAction::Preview);
    }
    if (214. ..250.).contains(&dy) {
        return if x < panel_x + 105. {
            Some(QueueAction::AuditionTransition)
        } else if x < panel_x + 165. {
            Some(QueueAction::LoopTransition)
        } else if x < panel_x + 250. {
            Some(QueueAction::Auto)
        } else {
            Some(QueueAction::AutoAll)
        };
    }
    None
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransportAction {
    Play,
    Stop,
    Loop,
    Follow,
    Settings,
    Volume(f32),
}
pub fn transport_action(width: f32, transport_y: f32, x: f32, y: f32) -> Option<TransportAction> {
    if y < transport_y || y >= transport_y + 38. {
        return None;
    }
    if (8.0..48.0).contains(&x) {
        return Some(TransportAction::Play);
    }
    if (48.0..90.0).contains(&x) {
        return Some(TransportAction::Stop);
    }
    if (214.0..320.0).contains(&x) {
        return Some(TransportAction::Loop);
    }
    if (335.0..445.0).contains(&x) {
        return Some(TransportAction::Follow);
    }
    if x >= width - 124. && x < width - 44. {
        return Some(TransportAction::Settings);
    }
    if x >= 530. && x < width - 135. {
        return Some(TransportAction::Volume(
            ((x - 580.) / (width - 770.)).clamp(0., 1.),
        ));
    }
    None
}
pub fn help_at(g: &Gpu, a: &App, recent_count: usize, x: f32, y: f32) -> HelpInfo {
    let info = |title, body| HelpInfo { title, body };
    if a.audio.is_none() {
        return match welcome_action(g.width, g.height, x, y, recent_count) {
            Some(WelcomeAction::OpenFile) => info(
                "Open audio or MIDI",
                "Choose an audio or MIDI file to analyze and play.",
            ),
            Some(WelcomeAction::OpenProject) => info(
                "Open project",
                "Open a saved .mgp session with its queue, transitions and edits.",
            ),
            Some(WelcomeAction::Recent(_)) => info(
                "Recent file",
                "Reopen this file or project. Missing files disappear from this list on restart.",
            ),
            None => info("Welcome", "Open a file or drop one anywhere in the window."),
        };
    }
    if let Some(action) = panel_action(g, a.edit_mode, x, y) {
        return match action {
            PanelAction::Open => info("Open", "Open an audio file, MIDI file or saved project."),
            PanelAction::Add => info("Add", "Add one or more audio or MIDI tracks to the queue."),
            PanelAction::Save => info(
                "Save",
                "Save the queue, transitions and edits as an .mgp project.",
            ),
            PanelAction::Export => info("Export", "Write the current mix to a WAV or FLAC file."),
            PanelAction::Edit => info(
                "Edit mode",
                "Show selection and audio editing tools. Drag on a plot to select a time range.",
            ),
            PanelAction::Fit => info("Fit", "Show the entire file on the time axis."),
            PanelAction::Zoom => info(
                "Zoom selection",
                "Fit the selected time range in the plot area.",
            ),
            PanelAction::LoopSelection => info(
                "Loop selection",
                "Repeat the selected time range during playback.",
            ),
            PanelAction::Audition => {
                info("Play selection", "Play only the selected range, then stop.")
            }
            PanelAction::Tool(Tool::Range) => info(
                "Range tool",
                "Drag between two times to select an area for editing.",
            ),
            PanelAction::Tool(Tool::Lasso) => info(
                "Lasso tool",
                "Draw around notes in the note plot to select them.",
            ),
            PanelAction::Undo => info("Undo", "Restore the previous edit."),
            PanelAction::Redo => info("Redo", "Apply the last undone edit again."),
            PanelAction::Operation(operation) => info(operation.name(), operation_help(operation)),
        };
    }
    if let Some(action) = settings_action(g, a, x, y) {
        return match action {
            SettingsAction::Theme => info(
                "Theme",
                "Cycle between system, light, dark and custom colors.",
            ),
            SettingsAction::Import => {
                info("Load theme", "Load a local theme file with custom colors.")
            }
            SettingsAction::Reload => info(
                "Reload theme",
                "Read the current theme file again after editing it.",
            ),
            SettingsAction::Surface => info(
                "Settings",
                "Analysis size, partial count and theme controls are here.",
            ),
        };
    }
    if let Some(action) = queue_action(g, a, x, y) {
        return match action {
            QueueAction::Select(_) => info("Queue track", "Select this track and jump to its start in the mix."),
            QueueAction::Up | QueueAction::Down => info("Reorder track", "Move the selected track in the queue and rebuild the mix."),
            QueueAction::Remove => info("Remove track", "Remove the selected track from the queue."),
            QueueAction::DurationDown | QueueAction::DurationUp => info("Overlap", "Change how long adjacent tracks play together. The actual overlap may be capped by track length."),
            QueueAction::CueDown | QueueAction::CueUp => info("In cue", "Skip more or less of the incoming track before its transition begins."),
            QueueAction::Curve => info("Fade curve", "Cycle between linear, equal power and smooth crossfades."),
            QueueAction::Phase => info("Phase matching", "Invert the incoming track when the overlap is strongly anticorrelated."),
            QueueAction::Preview => info("Transition preview", "Shows the fade weights and time span. Click it to jump to the overlap start."),
            QueueAction::AuditionTransition => info("Play preview", "Play from just before the selected transition and stop shortly after it."),
            QueueAction::LoopTransition => info("Loop transition", "Set a loop around the overlap, with a short lead-in and tail."),
            QueueAction::PreviousTransition | QueueAction::NextTransition => info("Navigate transitions", "Select the previous or next transition and jump to it."),
            QueueAction::Auto => info("Auto transition", "Search cues, beat lengths and fade curves for this boundary."),
            QueueAction::AutoAll => info("Auto all", "Find transitions for the whole queue in order."),
        };
    }
    if transition_marker_at(g, a, x, y).is_some() {
        return info(
            "Transition marker",
            "Marks a crossfade on the waveform. Click to select and jump to it.",
        );
    }
    if let Some(operation) = edit_marker_at(g, a, x, y) {
        return info(
            operation.name(),
            "This badge marks an edit on the waveform. Its letter identifies the tool; its number identifies the edit.",
        );
    }
    let l = layout(g);
    if x < l.plot_w && y < l.transport_y {
        let (left, span) = a.visible();
        let plot_width = l.plot_w - 12.;
        if (x - time_x(a.time(), left, span, plot_width)).abs() <= 3. {
            return info(
                "Playhead",
                "Shows the position reported by the audio stream during playback.",
            );
        }
        if let Some(Selection::Range { start, end }) = &a.selection {
            let x1 = time_x(start.min(*end), left, span, plot_width);
            let x2 = time_x(start.max(*end), left, span, plot_width);
            if x >= x1 && x <= x2 {
                return info(
                    "Selection",
                    "Edits and selection playback apply to this time range. Drag on a plot to choose a new range.",
                );
            }
        }
        return match l.rows.iter().position(|&(top, bottom)| y >= top && y < bottom) {
            Some(0) => info("Waveform", "The solid body is RMS level; thin edges show sample peaks. Click to seek, wheel to zoom, middle drag to pan."),
            Some(1) => info("Spectrum", "Frequency content of the current analysis frame. Markers indicate detected partials."),
            Some(2) => info("Spectrogram", "Frequency content over time. The vertical line follows the audio stream's playhead."),
            Some(3) => info("Note track", "Estimated pitch or MIDI notes over time. Drag here in edit mode to select notes or a time range."),
            _ => info("Time axis", "Click to seek. Use the wheel to zoom and middle drag to pan."),
        };
    }
    if let Some(action) = transport_action(g.width, l.transport_y, x, y) {
        return match action {
            TransportAction::Play => info(
                "Play / pause",
                "Start or pause playback. Space also toggles playback.",
            ),
            TransportAction::Stop => info("Stop", "Stop playback and return to the beginning."),
            TransportAction::Loop => info(
                "Loop",
                "Repeat the marked loop region. Right drag on a plot to set its bounds.",
            ),
            TransportAction::Follow => info(
                "Follow playhead",
                "Keep the playhead in view while zoomed in.",
            ),
            TransportAction::Settings => info("Settings", "Open analysis and theme controls."),
            TransportAction::Volume(_) => {
                info("Volume", "Drag or scroll here to change playback volume.")
            }
        };
    }
    info(
        "Queue",
        "Tracks play in order. Select a row to edit its incoming transition.",
    )
}
fn operation_help(operation: Operation) -> &'static str {
    match operation {
        Operation::GainUp | Operation::GainDown => "Raise or lower the selected audio by 3 dB.",
        Operation::Normalize => "Scale the selected audio so its peak reaches full level.",
        Operation::Mute => "Silence the selected audio.",
        Operation::Reverse => "Reverse the selected audio in time.",
        Operation::FadeIn | Operation::FadeOut | Operation::FadeBoth => {
            "Apply a smooth level fade to the selected audio."
        }
        Operation::Gcd => {
            "Find a shared fundamental among selected partials and retune them toward it."
        }
        Operation::RemoveDc => "Remove the average signal offset from the selected audio.",
        Operation::Invert => "Reverse the signal polarity in the selected audio.",
        Operation::Compress => "Reduce level differences in the selected audio.",
        Operation::HighPass => "Reduce frequencies below 120 Hz in the selected audio.",
        Operation::LowPass => "Reduce frequencies above 6 kHz in the selected audio.",
        Operation::Gate => "Reduce quiet background sound in the selected audio.",
        Operation::SoftClip => "Round off high peaks in the selected audio.",
    }
}
fn help_lines(body: &str, columns: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in body.split_whitespace() {
        let line = lines.last_mut().unwrap();
        if !line.is_empty() && line.len() + 1 + word.len() > columns {
            lines.push(word.to_owned());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}
fn draw_help(g: &mut Gpu, p: Palette, help: &HelpState) {
    if !help.active {
        return;
    }
    let Some(topic) = help.topic else {
        return;
    };
    let lines = help_lines(topic.body, 54);
    let w = (g.width - 32.).min(420.);
    let h = 50. + lines.len() as f32 * 16.;
    let x = (help.point.0 + 18.).min(g.width - w - 8.).max(8.);
    let y = if help.point.1 + h + 18. < g.height {
        help.point.1 + 18.
    } else {
        (help.point.1 - h - 18.).max(8.)
    };
    g.rect(x, y, w, h, p.panel);
    g.rect(x, y, w, 1., p.line);
    g.rect(x, y + h - 1., w, 1., p.line);
    g.rect(x, y, 1., h, p.line);
    g.rect(x + w - 1., y, 1., h, p.line);
    g.text(x + 12., y + 20., 13., topic.title, p.ink);
    for (index, line) in lines.iter().enumerate() {
        g.text(x + 12., y + 43. + index as f32 * 16., 11., line, p.muted);
    }
}
pub fn draw(
    g: &mut Gpu,
    a: &App,
    dark: bool,
    cursor: (f32, f32),
    recent: &[PathBuf],
    help: &HelpState,
) {
    let p = a.theme.palette(dark);
    g.clear();
    if a.audio.is_none() {
        let (x, y, w) = welcome_bounds(g.width, g.height);
        let help_hot = help_button_hit(g.width, g.height, false, cursor.0, cursor.1);
        g.text(
            g.width - 31.,
            34.,
            17.,
            "?",
            if help.active || help_hot {
                p.ink
            } else {
                p.muted
            },
        );
        if help.active || help_hot {
            g.rect(g.width - 44., 42., 36., 1., p.ink);
        }
        g.text(x, y + 22., 18., "Open a file", p.ink);
        let hover = welcome_action(g.width, g.height, cursor.0, cursor.1, recent.len());
        for (index, label, action) in [
            (0., "Open audio or MIDI", WelcomeAction::OpenFile),
            (1., "Open project", WelcomeAction::OpenProject),
        ] {
            let row_y = y + 48. + index * 52.;
            g.rect(x, row_y, w, 44., p.panel);
            g.rect(x, row_y, w, 1., p.line);
            g.rect(x, row_y + 43., w, 1., p.line);
            if hover == Some(action) && !a.busy {
                g.rect(x, row_y, 2., 44., p.ink);
            }
            g.text(x + 16., row_y + 28., 14., label, p.ink);
        }
        if !recent.is_empty() {
            g.text(x, y + 178., 11., "Recent", p.muted);
            for (index, path) in recent.iter().take(RECENT_LIMIT).enumerate() {
                let row_y = y + 191. + index as f32 * 38.;
                if hover == Some(WelcomeAction::Recent(index)) && !a.busy {
                    g.rect(x, row_y, w, 36., p.panel);
                    g.rect(x, row_y, 2., 36., p.ink);
                }
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("file");
                let parent = path.parent().and_then(|s| s.to_str()).unwrap_or("");
                g.text(x + 12., row_y + 15., 13., &short_label(name, 62), p.ink);
                g.text(x + 12., row_y + 31., 10., &short_label(parent, 82), p.muted);
            }
        }
        g.text(x, y + 382., 11., "Drop a file anywhere", p.muted);
        if a.busy {
            g.text(x, y + 409., 12., "Opening…", p.muted);
        } else if let Some(error) = &a.error {
            g.text(x, y + 409., 12., &short_label(error, 82), p.ink);
        }
        if a.hovered_file.is_some() {
            g.rect(0., 0., g.width, 1., p.line);
            g.rect(0., g.height - 1., g.width, 1., p.line);
            g.rect(0., 0., 1., g.height, p.line);
            g.rect(g.width - 1., 0., 1., g.height, p.line);
        }
        draw_help(g, p, help);
        g.render(p.bg);
        return;
    }
    let l = layout(g);
    let (left, span) = a.visible();
    let now = a.time();
    let width = l.plot_w - 12.;
    let selected = a.frame();
    for &(y0, y1) in &l.rows {
        g.rect(0., y0, l.plot_w, y1 - y0, p.bg);
        g.rect(0., y1 - 1., l.plot_w, 1., p.line);
    }
    if let Some(Selection::Range { start, end }) = &a.selection {
        let x1 = time_x(start.min(*end), left, span, width).clamp(0., width);
        let x2 = time_x(start.max(*end), left, span, width).clamp(0., width);
        if x2 > x1 {
            let shade = Color(p.ink.0, p.ink.1, p.ink.2, 0.1);
            for &(top, bottom) in &l.rows {
                g.rect(x1, top, x2 - x1, bottom - top - 1., shade);
            }
            for &(top, bottom) in &l.rows {
                g.rect(x1, top, 1., bottom - top, p.muted);
                g.rect(x2, top, 1., bottom - top, p.muted);
            }
        }
    }
    g.rect(l.panel_x, 0., g.width - l.panel_x, l.transport_y, p.panel);
    g.rect(l.panel_x, 0., 1., l.transport_y, p.line);
    let (y0, y1) = l.rows[0];
    axis(g, 0., y0, width, y1 - y0, "", p);
    if let Some(audio) = &a.audio {
        let center = (y0 + y1) * 0.5;
        let half = (y1 - y0) * 0.42;
        g.rect(0., center.round(), width, 1., p.line);
        let n = width.max(1.) as usize;
        let source = a.playback.as_ref().map(|p| &p.source);
        let sample = |i: usize| {
            source
                .map(|src| src.get(i))
                .unwrap_or_else(|| audio.samples.get(i).copied().unwrap_or(0.))
        };
        let visible_start = (left * audio.rate as f32).floor() as usize;
        let visible_end = ((left + span) * audio.rate as f32).ceil() as usize;
        if let Some(summary) = &a.waveform {
            if span * audio.rate as f32 / n as f32 > 1.25 {
                let peak_color = Color(p.muted.0, p.muted.1, p.muted.2, 0.65);
                let mut previous: Option<(f32, f32, f32)> = None;
                for px in 0..n {
                    let start =
                        ((left + px as f32 / n as f32 * span) * audio.rate as f32).floor() as usize;
                    let end = ((left + (px + 1) as f32 / n as f32 * span) * audio.rate as f32)
                        .ceil() as usize;
                    if let Some(range) = summary.range(&audio.samples, start, end.max(start + 1)) {
                        let high = center - half * range.high.value;
                        let low = center - half * range.low.value;
                        let rms = half * range.rms();
                        g.rect(px as f32, center - rms, 1., (rms * 2.).max(1.), p.ink);
                        if let Some((last_x, last_high, last_low)) = previous {
                            g.line(last_x, last_high, px as f32, high, peak_color);
                            g.line(last_x, last_low, px as f32, low, peak_color);
                        }
                        previous = Some((px as f32, high, low));
                    } else {
                        previous = None;
                    }
                }
            } else {
                let mut waveform = Vec::with_capacity(n);
                for px in 0..n {
                    let t = left + px as f32 / n as f32 * span;
                    let i = (t * audio.rate as f32) as usize;
                    waveform.push((px as f32, center - half * sample(i)));
                }
                plots::trace(g, &waveform, p.ink);
            }
            if let Some(range) = summary.range(&audio.samples, visible_start, visible_end) {
                let flat = (range.high.value - range.low.value).abs() < 0.03;
                let markers = [
                    Some((range.high, if flat { "level" } else { "peak" })),
                    (!flat).then_some((range.low, "low")),
                ];
                for (point, label) in markers.into_iter().flatten() {
                    let x = time_x(point.index as f32 / audio.rate as f32, left, span, width)
                        .clamp(0., width - 1.);
                    let y = center - half * point.value;
                    g.rect(x - 2., y - 2., 5., 5., p.ink);
                    let label_x = (x + 7.).min(width - 100.).max(5.);
                    let label_y = (y - 7.).clamp(y0 + 17., y1 - 8.);
                    g.text(
                        label_x,
                        label_y,
                        11.,
                        &format!("{label} {:+.2}", point.value),
                        p.muted,
                    );
                }
            }
        } else {
            let mut waveform = Vec::with_capacity(n);
            for px in 0..n {
                let t = left + px as f32 / n as f32 * span;
                let i = (t * audio.rate as f32) as usize;
                waveform.push((px as f32, center - half * sample(i)));
            }
            plots::trace(g, &waveform, p.ink);
        }
        g.text(6., y0 + 30., 10., "+1", p.muted);
        g.text(6., center - 3., 10., "0", p.muted);
        g.text(6., y1 - 7., 10., "-1", p.muted);
    }
    let (y0, y1) = l.rows[1];
    axis(g, 0., y0, width, y1 - y0, "Hz", p);
    if let Some(f) = selected {
        let mut pts = Vec::new();
        let max = f.spectrum.iter().copied().fold(0.0001f32, f32::max);
        for x in 0..width as usize {
            let k = (x as f32 / width).powf(2.) * (f.spectrum.len() - 1) as f32;
            let i = k as usize;
            let v = f.spectrum[i] / max;
            pts.push((x as f32, y1 - 7. - v * (y1 - y0 - 30.)));
        }
        plots::trace(g, &pts, p.ink);
        for pk in &f.peaks {
            let x = (pk.freq / (a.audio.as_ref().unwrap().rate as f32 / 2.)).sqrt() * width;
            if x < width {
                let chosen = a
                    .selection
                    .as_ref()
                    .is_some_and(|s| s.contains(a.caret.unwrap_or(now), pk.freq));
                g.rect(
                    x,
                    y0 + 25.,
                    if chosen { 2. } else { 1. },
                    y1 - y0 - 32.,
                    if chosen { p.ink } else { p.muted },
                );
            }
        }
    }
    let (y0, y1) = l.rows[2];
    axis(g, 0., y0, width, y1 - y0, "s", p);
    if a.spec_width > 0 {
        let processed = a.frames.last().map(|f| f.time).unwrap_or(0.).max(0.001);
        let start = left.max(0.);
        let end = (left + span).min(processed);
        if end > start {
            let x1 = time_x(start, left, span, width);
            let x2 = time_x(end, left, span, width);
            g.spec(
                x1,
                y0 + 22.,
                x2 - x1,
                y1 - y0 - 29.,
                [
                    start / processed * a.spec_width as f32 / 2048.,
                    0.,
                    (end - start) / processed * a.spec_width as f32 / 2048.,
                    a.spec_height as f32 / 512.,
                ],
            );
        }
    }
    if a.show_tracks {
        let visible_count = (span * a.audio.as_ref().unwrap().rate as f32 / a.hop as f32) as usize;
        let step = (visible_count / (width as usize).max(1)).max(1);
        let mut last = std::collections::HashMap::new();
        for f in a.frames.iter().step_by(step) {
            if f.time < left || f.time > left + span {
                continue;
            }
            let x = time_x(f.time, left, span, width);
            for pk in f.peaks.iter().take(6) {
                let yy = y1 - 8. - (pk.freq / 5000.).clamp(0., 1.) * (y1 - y0 - 34.);
                if let Some(&(old_x, old_y)) = last.get(&pk.track) {
                    if x - old_x < 8. {
                        g.line(old_x, old_y, x, yy, p.muted);
                    }
                }
                last.insert(pk.track, (x, yy));
            }
        }
    }
    let (y0, y1) = l.rows[3];
    axis(g, 0., y0, width, y1 - y0, "note", p);
    for midi in [48., 60., 72., 84.] {
        let y = y1 - 8. - (midi - 36.) / 60. * (y1 - y0 - 34.);
        g.rect(0., y.round(), width, 1., p.line);
        g.text(
            8.,
            y - 3.,
            10.,
            &format!("C{}", midi as i32 / 12 - 1),
            p.muted,
        );
    }
    if a.notes.is_empty() {
        let hop = a.hop as f32 / a.audio.as_ref().unwrap().rate as f32;
        let y_of = |midi: f32| y1 - 8. - ((midi - 36.) / 60.).clamp(0., 1.) * (y1 - y0 - 34.);
        for (from, to) in plots::pitch_segments(&a.frames, left, left + span, width, hop) {
            if (to.0 - from.0).abs() < 0.5 {
                g.rect(from.0 - 1., y_of(from.1) - 1., 3., 3., p.ink);
            } else {
                g.line(from.0, y_of(from.1), to.0, y_of(to.1), p.ink);
            }
        }
    } else {
        for n in &a.notes {
            let Some((start, end)) =
                plots::clipped_note(n.start as f32, n.end as f32, left, left + span)
            else {
                continue;
            };
            let x = time_x(start, left, span, width).clamp(0., width);
            let x2 = time_x(end, left, span, width).clamp(0., width);
            let yy = y1 - 8. - ((n.pitch as f32 - 36.) / 60.).clamp(0., 1.) * (y1 - y0 - 34.);
            let chosen = a.selection.as_ref().is_some_and(|s| {
                s.contains(
                    (n.start + n.end) as f32 * 0.5,
                    440. * 2f32.powf((n.pitch as f32 - 69.) / 12.),
                )
            });
            g.rect(
                x,
                yy,
                (x2 - x).max(1.).min(width - x),
                if chosen { 4. } else { 2. },
                p.ink,
            );
        }
    }
    if let Some(Selection::Lasso { points }) = &a.selection {
        let (top, bottom) = l.rows[3];
        let y_of =
            |note: f32| bottom - 8. - ((note - 36.) / 60.).clamp(0., 1.) * (bottom - top - 34.);
        for pair in points.windows(2) {
            g.line(
                time_x(pair[0].0, left, span, width),
                y_of(pair[0].1),
                time_x(pair[1].0, left, span, width),
                y_of(pair[1].1),
                p.muted,
            );
        }
        if points.len() > 2 {
            g.line(
                time_x(points[0].0, left, span, width),
                y_of(points[0].1),
                time_x(points.last().unwrap().0, left, span, width),
                y_of(points.last().unwrap().1),
                p.muted,
            );
        }
    }
    draw_change_marks(g, a, p, left, span, width, cursor, &l.rows);
    if let Some(play) = &a.playback {
        if play.looping.load(Ordering::Relaxed) {
            let x1 = time_x(a.loop_start, left, span, width);
            let x2 = time_x(a.loop_end, left, span, width);
            for &(y0, y1) in &l.rows {
                g.rect(x1, y0, 1., y1 - y0, p.muted);
                g.rect(x2, y0, 1., y1 - y0, p.muted);
            }
        }
    }
    let x = time_x(now, left, span, width);
    if x >= 0. && x < width {
        for &(y0, y1) in &l.rows {
            g.rect(x, y0, 1., y1 - y0 - 1., p.accent);
        }
    }
    if a.edit_mode {
        if let Some(caret) = a.caret {
            let cx = time_x(caret, left, span, width);
            if cx >= 0. && cx < width {
                for &(top, bottom) in &l.rows {
                    g.rect(cx, top, 1., bottom - top, p.ink);
                }
            }
        }
    }
    let px = l.panel_x + 18.;
    if let Some(e) = &a.error {
        g.text(px, l.transport_y - 12., 12., e, p.muted);
    }
    for control in panel_controls(l.panel_x, g.width, a.edit_mode) {
        let active = match control.action {
            PanelAction::Edit => a.edit_mode,
            PanelAction::Tool(tool) => a.tool == tool,
            _ => false,
        };
        let hovered = control.contains(cursor.0, cursor.1);
        g.text(
            control.x + 5.,
            control.y + 19.,
            13.,
            control.label,
            if active || hovered { p.ink } else { p.muted },
        );
        g.rect(
            control.x,
            control.y + 27.,
            control.w,
            1.,
            if active {
                p.ink
            } else if hovered {
                p.muted
            } else {
                p.line
            },
        );
    }
    if a.edit_mode {
        if let Some((start, end)) = a.selection.as_ref().and_then(Selection::bounds) {
            let label = if let Some(status) = &a.edit_status {
                format!("{start:.2}–{end:.2} s   {status}")
            } else {
                format!("{start:.2}–{end:.2} s")
            };
            g.text(px, 215., 11., &label, p.muted);
        } else if let Some(status) = &a.edit_status {
            g.text(px, 215., 11., status, p.muted);
        }
    }
    let top = if a.edit_mode { 228. } else { 48. };
    g.text(
        px,
        top + 16.,
        12.,
        &format!("queue  {}", a.queue.len()),
        p.muted,
    );
    let actions = [
        ("up", QueueAction::Up),
        ("down", QueueAction::Down),
        ("remove", QueueAction::Remove),
    ];
    for (j, (label, action)) in actions.iter().enumerate() {
        let x = l.panel_x + 175. + j as f32 * 55.;
        let hot = queue_action(g, a, cursor.0, cursor.1) == Some(*action);
        g.text(x, top + 16., 12., label, if hot { p.ink } else { p.muted });
    }
    let list_top = top + 25.;
    let editor_top = (l.transport_y - TRANSITION_EDITOR_HEIGHT).max(list_top + 32.);
    let rows = ((editor_top - list_top) / 28.).floor().max(0.) as usize;
    for (visible, i) in (a.queue_scroll..a.queue.len()).take(rows).enumerate() {
        let y = list_top + visible as f32 * 28.;
        if i == a.selected_track {
            g.rect(l.panel_x + 1., y, g.width - l.panel_x - 2., 27., p.bg);
            g.rect(l.panel_x + 1., y, 2., 27., p.ink);
        }
        let name = &a.queue[i].name;
        let active = a.marquee_hovered.unwrap_or(a.selected_track) == i;
        let short = marquee_text(name, 29, a.marquee_started.elapsed().as_secs_f32(), active);
        g.text(
            px + 4.,
            y + 19.,
            13.,
            &short,
            if i == a.selected_track {
                p.ink
            } else {
                p.muted
            },
        );
        if let (Some(place), Some(audio)) = (a.placements.get(i), a.audio.as_ref()) {
            g.text(
                g.width - 74.,
                y + 19.,
                11.,
                &format!(
                    "{:.0}s",
                    (place.end - place.start) as f32 / audio.rate as f32
                ),
                p.muted,
            );
        }
    }
    if let Some(i) = a.transition_index() {
        let t = &a.queue[i].transition;
        let place = a.placements.get(i).copied().unwrap_or_default();
        let y = editor_top;
        g.rect(l.panel_x + 1., y, g.width - l.panel_x - 1., 1., p.line);
        g.text(px, y + 22., 13., &format!("transition  {}", i), p.ink);
        let arrow_x = px + 112.;
        g.line(arrow_x, y + 17., arrow_x + 14., y + 17., p.ink);
        g.line(arrow_x + 14., y + 17., arrow_x + 9., y + 13., p.ink);
        g.line(arrow_x + 14., y + 17., arrow_x + 9., y + 21., p.ink);
        g.text(arrow_x + 23., y + 22., 13., &format!("{}", i + 1), p.ink);
        g.text(g.width - 96., y + 21., 11., "prev", p.muted);
        g.text(g.width - 47., y + 21., 11., "next", p.muted);
        let actual = a
            .audio
            .as_ref()
            .map(|audio| place.overlap as f32 / audio.rate as f32)
            .unwrap_or(t.seconds);
        let overlap_label = if (actual - t.seconds).abs() > 0.01 {
            format!("overlap  {:.2}/{:.2} s", actual, t.seconds)
        } else {
            format!("overlap  {:.2} s", actual)
        };
        g.text(px, y + 53., 13., &overlap_label, p.muted);
        g.text(g.width - 76., y + 53., 13., "−   +", p.ink);
        g.text(
            px,
            y + 81.,
            13.,
            &format!("in cue   {:.2} s", t.in_cue),
            p.muted,
        );
        g.text(g.width - 76., y + 81., 13., "−   +", p.ink);
        g.text(
            px,
            y + 110.,
            13.,
            match t.curve {
                crate::project::Curve::Linear => "linear",
                crate::project::Curve::EqualPower => "equal power",
                crate::project::Curve::Smooth => "smooth",
            },
            p.ink,
        );
        g.text(
            px + 137.,
            y + 110.,
            13.,
            if place.inverted {
                "phase flipped"
            } else if t.phase_match {
                "phase on"
            } else {
                "phase off"
            },
            p.ink,
        );
        if place.overlap > 0 {
            g.text(
                px + 246.,
                y + 110.,
                11.,
                &format!("r {:+.2}", place.correlation),
                p.muted,
            );
        }
        g.rect(
            l.panel_x + 1.,
            y + 121.,
            g.width - l.panel_x - 2.,
            92.,
            p.bg,
        );
        if queue_action(g, a, cursor.0, cursor.1) == Some(QueueAction::Preview) {
            g.rect(l.panel_x + 1., y + 121., 2., 92., p.muted);
        }
        g.text(px, y + 141., 13., "preview", p.ink);
        if let Some(audio) = &a.audio {
            let start = place.start as f32 / audio.rate as f32;
            g.text(
                px + 79.,
                y + 141.,
                11.,
                &format!("{:.1} - {:.1} s", start, start + actual),
                p.muted,
            );
        }
        let graph_y = y + 153.;
        let graph_left = px + 35.;
        let graph_width = (g.width - 18. - graph_left).max(1.);
        let graph_height = 34.;
        g.text(px, graph_y + 11., 10., "out", p.muted);
        g.text(px, graph_y + 33., 10., "in", p.transition);
        g.rect(graph_left, graph_y, graph_width, 1., p.line);
        g.rect(graph_left, graph_y + graph_height, graph_width, 1., p.line);
        g.rect(graph_left, graph_y, 1., graph_height + 1., p.line);
        g.rect(
            graph_left + graph_width - 1.,
            graph_y,
            1.,
            graph_height + 1.,
            p.line,
        );
        let correlation = place.correlation * if place.inverted { -1. } else { 1. };
        if place.overlap > 0 {
            for j in 0..50 {
                let x = graph_left + j as f32 / 50. * graph_width;
                let next_x = graph_left + (j + 1) as f32 / 50. * graph_width;
                let u = j as f32 / 50.;
                let v = (j + 1) as f32 / 50.;
                let (a1, b1) = crate::project::fade_weights(t.curve, u, correlation);
                let (a2, b2) = crate::project::fade_weights(t.curve, v, correlation);
                g.line(
                    x,
                    graph_y + graph_height * (1. - a1.clamp(0., 1.)),
                    next_x,
                    graph_y + graph_height * (1. - a2.clamp(0., 1.)),
                    p.muted,
                );
                g.line(
                    x,
                    graph_y + graph_height * (1. - b1.clamp(0., 1.)),
                    next_x,
                    graph_y + graph_height * (1. - b2.clamp(0., 1.)),
                    p.transition,
                );
            }
            if let Some(audio) = &a.audio {
                let start = place.start as f32 / audio.rate as f32;
                let progress = (a.time() - start) / actual;
                if (0. ..=1.).contains(&progress) {
                    g.rect(
                        graph_left + progress * graph_width,
                        graph_y,
                        1.,
                        graph_height,
                        p.accent,
                    );
                }
            }
        } else {
            let midpoint = graph_left + graph_width * 0.5;
            g.line(graph_left, graph_y, midpoint, graph_y, p.muted);
            g.line(midpoint, graph_y, midpoint, graph_y + graph_height, p.muted);
            g.line(
                midpoint,
                graph_y + graph_height,
                graph_left + graph_width,
                graph_y + graph_height,
                p.muted,
            );
            g.text(midpoint + 5., graph_y + 20., 10., "cut", p.transition);
        }
        g.text(graph_left, y + 207., 10., "start", p.muted);
        g.text(g.width - 47., y + 207., 10., "end", p.muted);
        g.text(px, y + 234., 11., "play preview", p.ink);
        let loop_active = a.playback.as_ref().is_some_and(|play| {
            if !play.looping.load(Ordering::Relaxed) {
                return false;
            }
            let Some(audio) = &a.audio else { return false };
            let start = (place.start as f32 / audio.rate as f32 - 0.75).max(0.);
            let end =
                ((place.start + place.overlap) as f32 / audio.rate as f32 + 0.75).min(a.duration());
            (a.loop_start - start).abs() < 0.02 && (a.loop_end - end).abs() < 0.02
        });
        g.text(
            px + 103.,
            y + 234.,
            11.,
            "loop",
            if loop_active { p.ink } else { p.muted },
        );
        g.text(
            px + 163.,
            y + 234.,
            11.,
            if a.auto_busy { "finding…" } else { "auto" },
            p.ink,
        );
        g.text(px + 248., y + 234., 11., "auto all", p.muted);
    }
    if let Some(stats) = a.selection_stats {
        let y = if a.transition_index().is_some() {
            editor_top - 53.
        } else {
            editor_top + 14.
        };
        g.text(
            px,
            y,
            11.,
            &format!(
                "peak {} dBFS   rms {} dBFS",
                level_db(stats.peak),
                level_db(stats.rms)
            ),
            p.muted,
        );
        g.text(
            px,
            y + 18.,
            11.,
            &format!("DC {:+.4}   clipped {}", stats.dc, stats.clipped),
            p.muted,
        );
    }
    if a.settings {
        let sy = l.transport_y - 190.;
        g.rect(l.panel_x + 1., sy, g.width - l.panel_x - 1., 190., p.panel);
        g.rect(l.panel_x, sy, g.width - l.panel_x, 1., p.line);
        g.text(
            px,
            sy + 22.,
            14.,
            &format!("partials  {}   [ / ]", a.partials),
            p.ink,
        );
        g.text(
            px,
            sy + 47.,
            14.,
            &format!("FFT {} 1/2   hop {} 3/4", a.fft_size, a.hop),
            p.muted,
        );
        g.text(
            px,
            sy + 87.,
            14.,
            &format!("theme  {}", a.theme.mode.label()),
            p.ink,
        );
        g.text(l.panel_x + 175., sy + 87., 13., "load", p.muted);
        g.text(l.panel_x + 260., sy + 87., 13., "reload", p.muted);
        let path = a.theme.path.to_string_lossy();
        let short: String = path
            .chars()
            .rev()
            .take(42)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        g.text(px, sy + 110., 10., &short, p.muted);
        g.text(px, sy + 133., 14., "tracks  T", p.muted);
        g.text(
            px,
            sy + 155.,
            12.,
            "events  PgUp/PgDn    select  I",
            p.muted,
        );
        g.text(px, sy + 178., 12., "play selection  Shift+Space", p.muted);
    }
    g.rect(0., l.transport_y, g.width, 1., p.line);
    g.rect(0., l.transport_y + 1., g.width, 37., p.panel);
    let playing = a
        .playback
        .as_ref()
        .map(|v| v.playing.load(Ordering::Relaxed))
        .unwrap_or(false);
    if playing {
        g.rect(19., l.transport_y + 11., 4., 15., p.ink);
        g.rect(27., l.transport_y + 11., 4., 15., p.ink);
    } else {
        for row in 0..14 {
            let edge = row.min(13 - row) + 1;
            g.rect(
                19.,
                l.transport_y + 12. + row as f32,
                (edge * 2) as f32,
                1.,
                p.ink,
            );
        }
    }
    g.text(53., l.transport_y + 25., 13., "stop", p.muted);
    g.text(
        96.,
        l.transport_y + 25.,
        13.,
        &format!(
            "{:02}:{:02} / {:02}:{:02}",
            (now / 60.) as u32,
            (now % 60.) as u32,
            (a.duration() / 60.) as u32,
            (a.duration() % 60.) as u32
        ),
        p.ink,
    );
    let looped = a
        .playback
        .as_ref()
        .map(|v| v.looping.load(Ordering::Relaxed))
        .unwrap_or(false);
    g.text(
        230.,
        l.transport_y + 25.,
        13.,
        if looped { "loop on" } else { "loop off" },
        if looped { p.ink } else { p.muted },
    );
    g.text(
        350.,
        l.transport_y + 25.,
        13.,
        if a.follow_playhead {
            "follow on"
        } else {
            "follow off"
        },
        if a.follow_playhead { p.ink } else { p.muted },
    );
    let vol = a
        .playback
        .as_ref()
        .map(|q| f32::from_bits(q.volume.load(Ordering::Relaxed)))
        .unwrap_or(0.8);
    let slider_start = 580.;
    let slider_end = g.width - 190.;
    g.text(540., l.transport_y + 25., 13., "vol", p.muted);
    g.rect(
        slider_start,
        l.transport_y + 20.,
        slider_end - slider_start,
        1.,
        p.line,
    );
    g.rect(
        slider_start + vol * (slider_end - slider_start) - 1.,
        l.transport_y + 14.,
        2.,
        13.,
        p.ink,
    );
    g.text(
        g.width - 176.,
        l.transport_y + 25.,
        13.,
        &format!("{:0.0}%", vol * 100.),
        p.muted,
    );
    g.text(
        g.width - 118.,
        l.transport_y + 25.,
        13.,
        "settings",
        if a.settings { p.ink } else { p.muted },
    );
    let help_hot = help_button_hit(g.width, g.height, true, cursor.0, cursor.1);
    g.text(
        g.width - 29.,
        l.transport_y + 25.,
        15.,
        "?",
        if help.active || help_hot {
            p.ink
        } else {
            p.muted
        },
    );
    if help.active || help_hot {
        g.rect(g.width - 44., l.transport_y + 34., 36., 1., p.ink);
    }
    if let Some(action) = transport_action(g.width, l.transport_y, cursor.0, cursor.1) {
        let region = match action {
            TransportAction::Play => Some((8., 40.)),
            TransportAction::Stop => Some((48., 42.)),
            TransportAction::Loop => Some((214., 106.)),
            TransportAction::Follow => Some((335., 110.)),
            TransportAction::Settings => Some((g.width - 124., 80.)),
            TransportAction::Volume(_) => None,
        };
        if let Some((x, w)) = region {
            g.rect(x, l.transport_y + 34., w, 1., p.muted);
        }
    }
    if a.hovered_file.is_some() {
        g.rect(0., 0., g.width, 1., p.line);
        g.rect(0., g.height - 1., g.width, 1., p.line);
        g.rect(0., 0., 1., g.height, p.line);
        g.rect(g.width - 1., 0., 1., g.height, p.line);
    }
    draw_help(g, p, help);
    g.render(p.bg);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_targets_match_controls_and_gaps() {
        let controls = panel_controls(575., 930., true);
        for control in &controls {
            let hit = controls
                .iter()
                .find(|other| other.contains(control.x + 4., control.y + 14.));
            assert_eq!(hit.map(|v| v.action), Some(control.action));
            assert!(control.x + control.w <= 930.);
        }
        for (i, a) in controls.iter().enumerate() {
            for b in controls.iter().skip(i + 1) {
                assert!(
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + 28. <= b.y || b.y + 28. <= a.y
                );
            }
        }
        assert!(controls.iter().all(|c| !c.contains(593. + 72., 51.)));
        assert_eq!(panel_controls(575., 930., false).len(), 6);
    }
    #[test]
    fn transport_targets_are_separate() {
        assert_eq!(
            transport_action(930., 522., 375., 540.),
            Some(TransportAction::Follow)
        );
        assert_eq!(
            transport_action(930., 522., 820., 540.),
            Some(TransportAction::Settings)
        );
        assert_eq!(transport_action(930., 522., 500., 540.), None);
        assert_eq!(
            transport_action(930., 522., 65., 540.),
            Some(TransportAction::Stop)
        );
        assert_eq!(level_db(0.), "-inf");
        assert_eq!(level_db(1.), "0.0");
    }
    #[test]
    fn long_queue_names_scroll_and_pause_at_both_ends() {
        let name = "♫abcdefghijklmnop";
        assert_eq!(marquee_text(name, 5, 0., true), "♫abcd");
        assert_eq!(marquee_text(name, 5, 1., true), "♫abcd");
        assert_eq!(marquee_text(name, 5, 100., false), "♫abc…");
        let travel = name.chars().count() - 5;
        let end_time = 1.2 + travel as f32 / 8. + 0.5;
        assert_eq!(marquee_text(name, 5, end_time, true), "lmnop");
    }
    #[test]
    fn transition_preview_and_audition_have_separate_targets() {
        let hit = |x, y| transition_action(575., 930., x, y);
        assert_eq!(hit(620., 135.), Some(QueueAction::Preview));
        assert_eq!(hit(900., 185.), Some(QueueAction::Preview));
        assert_eq!(hit(620., 225.), Some(QueueAction::AuditionTransition));
        assert_eq!(hit(710., 225.), Some(QueueAction::LoopTransition));
        assert_eq!(hit(750., 225.), Some(QueueAction::Auto));
        assert_eq!(hit(850., 225.), Some(QueueAction::AutoAll));
        assert_eq!(hit(845., 17.), Some(QueueAction::PreviousTransition));
        assert_eq!(hit(890., 17.), Some(QueueAction::NextTransition));
        assert_eq!(hit(620., 33.), None);
    }
    #[test]
    fn welcome_targets_match_drawn_rows() {
        let hit = |x, y| welcome_action(1200., 760., x, y, 2);
        let (left, top, width) = welcome_bounds(1200., 760.);
        assert_eq!(hit(left + 20., top + 60.), Some(WelcomeAction::OpenFile));
        assert_eq!(
            hit(left + width - 10., top + 120.),
            Some(WelcomeAction::OpenProject)
        );
        assert_eq!(hit(left + 20., top + 200.), Some(WelcomeAction::Recent(0)));
        assert_eq!(hit(left + 20., top + 240.), Some(WelcomeAction::Recent(1)));
        assert_eq!(hit(left + 20., top + 280.), None);
        assert_eq!(hit(left - 1., top + 60.), None);
    }
    #[test]
    fn transition_badge_stays_on_waveform_when_zoomed() {
        let marker = transition_marker_rect(2, 5., 7., 4., 4., 400., 0.).unwrap();
        assert_eq!(marker, (100., 27., 26.));
        let clipped = transition_marker_rect(2, 5., 7., 6., 1., 400., 0.).unwrap();
        assert_eq!(clipped.0, 0.);
        assert!(transition_marker_rect(2, 5., 7., 8., 1., 400., 0.).is_none());
    }
    #[test]
    fn help_button_does_not_overlap_settings_and_mode_can_close() {
        assert!(help_button_hit(930., 560., true, 905., 540.));
        assert_eq!(transport_action(930., 522., 905., 540.), None);
        assert_eq!(
            transport_action(930., 522., 820., 540.),
            Some(TransportAction::Settings)
        );
        assert!(help_button_hit(930., 560., false, 905., 22.));
        let mut help = HelpState::default();
        help.toggle();
        assert!(help.active);
        help.inspect(
            HelpInfo {
                title: "Waveform",
                body: "Signal level.",
            },
            (20., 30.),
        );
        assert_eq!(help.topic.unwrap().title, "Waveform");
        help.close();
        assert!(!help.active);
        assert!(help.topic.is_none());
    }
}
