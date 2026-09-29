use musigraph::{
    app::App,
    edit::{Operation, Selection, Tool},
    render::gpu::Gpu,
    theme::ThemeSettings,
    ui,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
#[cfg(target_os = "linux")]
use winit::platform::x11::EventLoopBuilderExtX11;
use winit::{
    event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{CursorIcon, Theme, WindowBuilder},
};
fn config_path() -> PathBuf {
    let dir = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
    .unwrap_or_else(|| PathBuf::from("."));
    dir.join("harmonic_scope.conf")
}
fn read_config() -> (u32, u32, i32, i32, usize, usize, usize, bool) {
    let s = std::fs::read_to_string(config_path()).unwrap_or_default();
    let mut v = s.split_whitespace();
    let read = |v: &mut std::str::SplitWhitespace<'_>, default: i32| {
        v.next().and_then(|x| x.parse().ok()).unwrap_or(default)
    };
    let w = read(&mut v, 1200).max(930) as u32;
    let h = read(&mut v, 760).max(560) as u32;
    let x = read(&mut v, -1);
    let y = read(&mut v, -1);
    let n = read(&mut v, 16).clamp(1, 24) as usize;
    let fft = read(&mut v, 2048).clamp(512, 8192) as usize;
    let hop = read(&mut v, 512).clamp(128, 4096) as usize;
    let follow = read(&mut v, 0) != 0;
    (w, h, x, y, n, fft, hop, follow)
}
fn save_config(window: &winit::window::Window, a: &App) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let size = window.inner_size();
    let pos = window.outer_position().ok();
    let (x, y) = pos.map(|p| (p.x, p.y)).unwrap_or((-1, -1));
    let _ = std::fs::write(
        path,
        format!(
            "{} {} {} {} {} {} {} {}\n",
            size.width,
            size.height,
            x,
            y,
            a.partials,
            a.fft_size,
            a.hop,
            u8::from(a.follow_playhead)
        ),
    );
}
const RECENT_LIMIT: usize = 4;
fn recent_path() -> PathBuf {
    config_path().with_extension("recent")
}
fn read_recent() -> Vec<PathBuf> {
    std::fs::read_to_string(recent_path())
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .take(RECENT_LIMIT)
        .collect()
}
fn remember_recent(recent: &mut Vec<PathBuf>, path: &Path) {
    if !path.is_file() {
        return;
    }
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    recent.retain(|old| old != &path);
    recent.insert(0, path);
    recent.truncate(RECENT_LIMIT);
}
fn save_recent(recent: &[PathBuf]) {
    let path = recent_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = recent
        .iter()
        .filter_map(|path| path.to_str())
        .filter(|path| !path.contains('\n'))
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(path, data);
}
fn choose_media(tx: std::sync::mpsc::Sender<PathBuf>) {
    std::thread::spawn(move || {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Audio and MIDI",
                &["mid", "midi", "mp3", "wav", "ogg", "flac"],
            )
            .pick_file()
        {
            let _ = tx.send(path);
        }
    });
}
fn choose_project(tx: std::sync::mpsc::Sender<PathBuf>) {
    std::thread::spawn(move || {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Project", &["mgp"])
            .pick_file()
        {
            let _ = tx.send(path);
        }
    });
}
fn choose_file(tx: std::sync::mpsc::Sender<PathBuf>) {
    std::thread::spawn(move || {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Audio and MIDI",
                &["mid", "midi", "mp3", "wav", "ogg", "flac", "mgp"],
            )
            .pick_file()
        {
            let _ = tx.send(path);
        }
    });
}
fn choose_tracks(tx: std::sync::mpsc::Sender<Vec<PathBuf>>) {
    std::thread::spawn(move || {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "Audio and MIDI",
                &["mid", "midi", "mp3", "wav", "ogg", "flac"],
            )
            .pick_files()
        {
            let _ = tx.send(paths);
        }
    });
}
fn choose_package(tx: std::sync::mpsc::Sender<PathBuf>) {
    std::thread::spawn(move || {
        if let Some(mut path) = rfd::FileDialog::new()
            .add_filter("Project", &["mgp"])
            .set_file_name("session.mgp")
            .save_file()
        {
            if path.extension().is_none() {
                path.set_extension("mgp");
            }
            let _ = tx.send(path);
        }
    });
}
fn choose_theme(tx: std::sync::mpsc::Sender<PathBuf>) {
    std::thread::spawn(move || {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Theme", &["theme", "txt"])
            .pick_file()
        {
            let _ = tx.send(path);
        }
    });
}
fn choose_export(tx: std::sync::mpsc::Sender<(PathBuf, bool)>, selection_only: bool) {
    std::thread::spawn(move || {
        if let Some(mut path) = rfd::FileDialog::new()
            .add_filter("WAV or FLAC", &["wav", "flac"])
            .set_file_name("output.wav")
            .save_file()
        {
            if path.extension().is_none() {
                path.set_extension("wav");
            }
            let _ = tx.send((path, selection_only));
        }
    });
}
fn main() {
    let (w, h, x, y, n, fft, hop, follow) = read_config();
    let event_loop = {
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("DISPLAY").is_some() {
                let mut builder = EventLoopBuilder::new();
                builder.with_x11();
                builder.build().or_else(|_| EventLoop::new()).unwrap()
            } else {
                EventLoop::new().unwrap()
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            EventLoop::new().unwrap()
        }
    };
    let mut builder = WindowBuilder::new()
        .with_title(" ")
        .with_min_inner_size(winit::dpi::LogicalSize::new(930.0, 560.0))
        .with_inner_size(winit::dpi::PhysicalSize::new(w, h));
    if x >= 0 && y >= 0 {
        builder = builder.with_position(winit::dpi::PhysicalPosition::new(x, y));
    }
    let window = Arc::new(builder.build(&event_loop).unwrap());
    let mut gpu = match Gpu::new(window.clone()) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let size = window.inner_size();
    gpu.resize(size.width, size.height);
    let (dialog_tx, dialog_rx) = std::sync::mpsc::channel::<PathBuf>();
    let (add_tx, add_rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let (package_tx, package_rx) = std::sync::mpsc::channel::<PathBuf>();
    let (theme_tx, theme_rx) = std::sync::mpsc::channel::<PathBuf>();
    let (save_tx, save_rx) = std::sync::mpsc::channel::<(PathBuf, bool)>();
    let mut app = App::new();
    let mut recent = read_recent();
    let mut help = ui::HelpState::default();
    app.theme = ThemeSettings::new(config_path().with_extension("theme"));
    if let Err(error) = app.theme.read() {
        app.edit_status = Some(error);
    }
    app.partials = n;
    app.fft_size = fft;
    app.hop = hop;
    app.follow_playhead = follow;
    let mut dark = window.theme().unwrap_or(Theme::Dark) == Theme::Dark;
    let mut last_spec_upload = std::time::Instant::now() - Duration::from_secs(1);
    let mut cursor = (0f32, 0f32);
    let mut dragging = false;
    let mut volume_dragging = false;
    let mut selecting = false;
    let mut modifiers = ModifiersState::empty();
    let mut panning = false;
    let mut loop_drag: Option<f32> = None;
    let mut last_x = 0f32;
    if let Some(arg) = std::env::args_os().nth(1) {
        let path = PathBuf::from(arg);
        remember_recent(&mut recent, &path);
        app.open(path);
    }
    event_loop.set_control_flow(ControlFlow::WaitUntil(
        std::time::Instant::now() + Duration::from_millis(16),
    ));
    event_loop
        .run(move |event, target| match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => {
                    save_config(&window, &app);
                    save_recent(&recent);
                    target.exit();
                }
                WindowEvent::Resized(s) => gpu.resize(s.width, s.height),
                WindowEvent::ThemeChanged(t) => {
                    dark = t == Theme::Dark;
                    app.spec_dirty = true;
                }
                WindowEvent::ModifiersChanged(m) => modifiers = m.state(),
                WindowEvent::HoveredFile(p) => app.hovered_file = Some(p),
                WindowEvent::HoveredFileCancelled => app.hovered_file = None,
                WindowEvent::DroppedFile(p) => {
                    app.hovered_file = None;
                    remember_recent(&mut recent, &p);
                    if p.extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.eq_ignore_ascii_case("mgp"))
                    {
                        app.open(p)
                    } else {
                        app.add_paths(vec![p])
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let scale = window.scale_factor() as f32;
                    cursor = (position.x as f32 / scale, position.y as f32 / scale);
                    let hovered = match ui::queue_action(&gpu, &app, cursor.0, cursor.1) {
                        Some(ui::QueueAction::Select(index)) => Some(index),
                        _ => None,
                    };
                    app.set_marquee_hovered(hovered);
                    if selecting {
                        let l = ui::layout(&gpu);
                        let (left, span) = app.visible();
                        let time = (left + cursor.0.clamp(0., l.plot_w) / l.plot_w * span)
                            .clamp(0., app.duration());
                        if let Some(selection) = &mut app.selection {
                            match selection {
                                Selection::Range { end, .. } => *end = time,
                                Selection::Lasso { points } => {
                                    let (top, bottom) = l.rows[3];
                                    let note = 36.
                                        + ((bottom - 8. - cursor.1) / (bottom - top - 34.) * 60.)
                                            .clamp(0., 60.);
                                    if points.last().is_none_or(|p| {
                                        (p.0 - time).abs() > 0.002 || (p.1 - note).abs() > 0.2
                                    }) {
                                        points.push((time, note));
                                    }
                                }
                            }
                        }
                    }
                    if dragging {
                        let l = ui::layout(&gpu);
                        if cursor.1 < l.transport_y && cursor.0 < l.plot_w {
                            let (start, span) = app.visible();
                            app.seek(start + cursor.0 / l.plot_w * span);
                        }
                    }
                    if let Some(start) = loop_drag {
                        let l = ui::layout(&gpu);
                        if cursor.0 < l.plot_w {
                            let (left, span) = app.visible();
                            app.loop_start = start.min(left + cursor.0 / l.plot_w * span).max(0.);
                            app.loop_end = start
                                .max(left + cursor.0 / l.plot_w * span)
                                .min(app.duration());
                        }
                    }
                    if panning {
                        let span = app.visible().1;
                        app.pan = (app.pan - (cursor.0 - last_x) / ui::layout(&gpu).plot_w * span)
                            .max(0.);
                        last_x = cursor.0;
                    }
                    if volume_dragging {
                        app.set_volume((cursor.0 - 580.) / (gpu.width - 770.));
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let l = ui::layout(&gpu);
                    if state == ElementState::Pressed {
                        if button == MouseButton::Left
                            && ui::help_button_hit(
                                gpu.width,
                                gpu.height,
                                app.audio.is_some(),
                                cursor.0,
                                cursor.1,
                            )
                        {
                            help.toggle();
                            window.set_cursor_icon(if help.active {
                                CursorIcon::Help
                            } else {
                                CursorIcon::Default
                            });
                        } else if help.active {
                            if button == MouseButton::Left {
                                let topic =
                                    ui::help_at(&gpu, &app, recent.len(), cursor.0, cursor.1);
                                help.inspect(topic, cursor);
                            }
                        } else if button == MouseButton::Left {
                            if app.audio.is_none() {
                                if !app.busy {
                                    match ui::welcome_action(
                                        gpu.width,
                                        gpu.height,
                                        cursor.0,
                                        cursor.1,
                                        recent.len(),
                                    ) {
                                        Some(ui::WelcomeAction::OpenFile) => {
                                            choose_media(dialog_tx.clone())
                                        }
                                        Some(ui::WelcomeAction::OpenProject) => {
                                            choose_project(dialog_tx.clone())
                                        }
                                        Some(ui::WelcomeAction::Recent(index)) => {
                                            if let Some(path) = recent.get(index).cloned() {
                                                remember_recent(&mut recent, &path);
                                                app.open(path);
                                            }
                                        }
                                        None => {}
                                    }
                                }
                            } else if let Some(action) =
                                ui::panel_action(&gpu, app.edit_mode, cursor.0, cursor.1)
                            {
                                match action {
                                    ui::PanelAction::Open => choose_file(dialog_tx.clone()),
                                    ui::PanelAction::Save => choose_package(package_tx.clone()),
                                    ui::PanelAction::Export => {
                                        choose_export(save_tx.clone(), false)
                                    }
                                    ui::PanelAction::Add => choose_tracks(add_tx.clone()),
                                    ui::PanelAction::Edit => app.edit_mode = !app.edit_mode,
                                    ui::PanelAction::Fit => app.fit_all(),
                                    ui::PanelAction::Zoom => app.zoom_selection(),
                                    ui::PanelAction::LoopSelection => app.loop_selection(),
                                    ui::PanelAction::Audition => app.audition_selection(),
                                    ui::PanelAction::Tool(tool) => app.tool = tool,
                                    ui::PanelAction::Operation(op) => app.apply_operation(op),
                                    ui::PanelAction::Undo => app.undo(),
                                    ui::PanelAction::Redo => app.redo(),
                                }
                            } else if let Some(action) =
                                ui::settings_action(&gpu, &app, cursor.0, cursor.1)
                            {
                                let result = match action {
                                    ui::SettingsAction::Theme => app.theme.cycle(),
                                    ui::SettingsAction::Import => {
                                        choose_theme(theme_tx.clone());
                                        Ok(())
                                    }
                                    ui::SettingsAction::Reload => app.theme.read(),
                                    ui::SettingsAction::Surface => Ok(()),
                                };
                                if let Err(error) = result {
                                    app.edit_status = Some(error);
                                }
                                app.spec_dirty = true;
                            } else if let Some(action) =
                                ui::queue_action(&gpu, &app, cursor.0, cursor.1)
                            {
                                match action {
                                    ui::QueueAction::Select(i) => app.queue_select(i),
                                    ui::QueueAction::Up => app.queue_move(true),
                                    ui::QueueAction::Down => app.queue_move(false),
                                    ui::QueueAction::Remove => app.queue_remove(),
                                    ui::QueueAction::DurationDown => app.transition_duration(-0.25),
                                    ui::QueueAction::DurationUp => app.transition_duration(0.25),
                                    ui::QueueAction::CueDown => app.transition_cue(-0.1),
                                    ui::QueueAction::CueUp => app.transition_cue(0.1),
                                    ui::QueueAction::Curve => app.transition_curve(),
                                    ui::QueueAction::Phase => app.transition_phase(),
                                    ui::QueueAction::Preview => app.preview_transition(),
                                    ui::QueueAction::AuditionTransition => {
                                        app.audition_transition()
                                    }
                                    ui::QueueAction::LoopTransition => app.loop_transition(),
                                    ui::QueueAction::PreviousTransition => {
                                        app.step_transition(false)
                                    }
                                    ui::QueueAction::NextTransition => app.step_transition(true),
                                    ui::QueueAction::Auto => app.auto_transition(false),
                                    ui::QueueAction::AutoAll => app.auto_transition(true),
                                }
                            } else if let Some(index) =
                                ui::transition_marker_at(&gpu, &app, cursor.0, cursor.1)
                            {
                                app.queue_select(index);
                            } else if cursor.1 < l.transport_y && cursor.0 < l.plot_w {
                                let (start, span) = app.visible();
                                let time = (start + cursor.0.clamp(0., l.plot_w) / l.plot_w * span)
                                    .clamp(0., app.duration());
                                if app.edit_mode && app.audio.is_some() {
                                    if app.tool == Tool::Lasso && cursor.1 >= l.rows[3].0 {
                                        let (top, bottom) = l.rows[3];
                                        let note = 36.
                                            + ((bottom - 8. - cursor.1) / (bottom - top - 34.)
                                                * 60.)
                                                .clamp(0., 60.);
                                        app.selection = Some(Selection::Lasso {
                                            points: vec![(time, note)],
                                        });
                                    } else {
                                        let anchor = if modifiers.shift_key() {
                                            app.caret.unwrap_or(time)
                                        } else {
                                            time
                                        };
                                        app.selection = Some(Selection::Range {
                                            start: anchor,
                                            end: time,
                                        });
                                    }
                                    app.caret = Some(time);
                                    app.invalidate_selection_stats();
                                    selecting = true;
                                } else {
                                    app.seek(time);
                                    dragging = true;
                                }
                            } else if let Some(action) =
                                ui::transport_action(gpu.width, l.transport_y, cursor.0, cursor.1)
                            {
                                match action {
                                    ui::TransportAction::Play => app.play_pause(),
                                    ui::TransportAction::Stop => app.stop(),
                                    ui::TransportAction::Loop => app.toggle_loop(),
                                    ui::TransportAction::Follow => {
                                        app.follow_playhead = !app.follow_playhead
                                    }
                                    ui::TransportAction::Settings => app.settings = !app.settings,
                                    ui::TransportAction::Volume(value) => {
                                        app.set_volume(value);
                                        volume_dragging = true;
                                    }
                                }
                            }
                        } else if button == MouseButton::Middle {
                            panning = true;
                            app.follow_playhead = false;
                            last_x = cursor.0;
                        } else if button == MouseButton::Right
                            && cursor.0 < l.plot_w
                            && cursor.1 < l.transport_y
                        {
                            let (left, span) = app.visible();
                            loop_drag = Some(left + cursor.0 / l.plot_w * span);
                        }
                    } else {
                        if button == MouseButton::Left {
                            dragging = false;
                            if selecting {
                                app.request_selection_stats();
                            }
                            selecting = false;
                            volume_dragging = false;
                        }
                        if button == MouseButton::Middle {
                            panning = false;
                        }
                        if button == MouseButton::Right
                            && loop_drag.take().is_some()
                            && app.loop_end - app.loop_start > 0.02
                        {
                            if let (Some(audio), Some(p)) = (&app.audio, &app.playback) {
                                p.loop_start.store(
                                    (app.loop_start * audio.rate as f32) as u64,
                                    std::sync::atomic::Ordering::Release,
                                );
                                p.loop_end.store(
                                    (app.loop_end * audio.rate as f32) as u64,
                                    std::sync::atomic::Ordering::Release,
                                );
                                p.looping.store(true, std::sync::atomic::Ordering::Release);
                            }
                        }
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let v = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.,
                    };
                    if cursor.1 < ui::layout(&gpu).transport_y && cursor.0 < ui::layout(&gpu).plot_w
                    {
                        let (before, span) = app.visible();
                        let anchor = before + cursor.0 / ui::layout(&gpu).plot_w * span;
                        app.zoom = (app.zoom * 1.2f32.powf(v)).clamp(1., 1000.);
                        let new_span = app.visible().1;
                        app.pan = (anchor - cursor.0 / ui::layout(&gpu).plot_w * new_span).max(0.);
                    } else if cursor.1 < ui::layout(&gpu).transport_y
                        && cursor.0 >= ui::layout(&gpu).panel_x
                    {
                        if v < 0. {
                            app.queue_scroll = (app.queue_scroll + (-v).ceil() as usize)
                                .min(app.queue.len().saturating_sub(1));
                        } else {
                            app.queue_scroll = app.queue_scroll.saturating_sub(v.ceil() as usize);
                        }
                        let hovered = match ui::queue_action(&gpu, &app, cursor.0, cursor.1) {
                            Some(ui::QueueAction::Select(index)) => Some(index),
                            _ => None,
                        };
                        app.set_marquee_hovered(hovered);
                    } else if cursor.1 >= ui::layout(&gpu).transport_y {
                        let vol = app
                            .playback
                            .as_ref()
                            .map(|p| {
                                f32::from_bits(p.volume.load(std::sync::atomic::Ordering::Relaxed))
                            })
                            .unwrap_or(0.8);
                        app.set_volume(vol + v * 0.05);
                    }
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::F1) => {
                            help.toggle();
                            window.set_cursor_icon(if help.active {
                                CursorIcon::Help
                            } else {
                                CursorIcon::Default
                            });
                        }
                        PhysicalKey::Code(KeyCode::Escape) if help.active => {
                            help.close();
                            window.set_cursor_icon(CursorIcon::Default);
                        }
                        PhysicalKey::Code(KeyCode::KeyZ)
                            if modifiers.control_key() || modifiers.super_key() =>
                        {
                            app.undo()
                        }
                        PhysicalKey::Code(KeyCode::KeyY)
                            if modifiers.control_key() || modifiers.super_key() =>
                        {
                            app.redo()
                        }
                        PhysicalKey::Code(KeyCode::KeyS)
                            if modifiers.control_key() || modifiers.super_key() =>
                        {
                            if modifiers.shift_key() {
                                choose_export(save_tx.clone(), true)
                            } else {
                                choose_package(package_tx.clone())
                            }
                        }
                        PhysicalKey::Code(KeyCode::KeyA)
                            if modifiers.control_key() || modifiers.super_key() =>
                        {
                            app.select_all()
                        }
                        PhysicalKey::Code(KeyCode::Space) if modifiers.shift_key() => {
                            app.audition_selection()
                        }
                        PhysicalKey::Code(KeyCode::Space) => app.play_pause(),
                        PhysicalKey::Code(KeyCode::KeyK) => app.stop(),
                        PhysicalKey::Code(KeyCode::Home) => app.seek(0.),
                        PhysicalKey::Code(KeyCode::End) => app.seek(app.duration()),
                        PhysicalKey::Code(KeyCode::PageUp) => app.jump_event(false),
                        PhysicalKey::Code(KeyCode::PageDown) => app.jump_event(true),
                        PhysicalKey::Code(KeyCode::KeyI) => app.select_event(),
                        PhysicalKey::Code(KeyCode::Digit0) => app.fit_all(),
                        PhysicalKey::Code(KeyCode::KeyZ) => app.zoom_selection(),
                        PhysicalKey::Code(KeyCode::KeyL) if modifiers.shift_key() => {
                            app.loop_selection()
                        }
                        PhysicalKey::Code(KeyCode::ArrowLeft) => app.seek(app.time() - 5.),
                        PhysicalKey::Code(KeyCode::ArrowRight) => app.seek(app.time() + 5.),
                        PhysicalKey::Code(KeyCode::KeyL) => app.toggle_loop(),
                        PhysicalKey::Code(KeyCode::KeyP) => {
                            app.follow_playhead = !app.follow_playhead
                        }
                        PhysicalKey::Code(KeyCode::KeyT) => app.show_tracks = !app.show_tracks,
                        PhysicalKey::Code(KeyCode::KeyE) => app.edit_mode = !app.edit_mode,
                        PhysicalKey::Code(KeyCode::KeyR) if app.edit_mode => app.tool = Tool::Range,
                        PhysicalKey::Code(KeyCode::KeyV) if app.edit_mode => app.tool = Tool::Lasso,
                        PhysicalKey::Code(KeyCode::KeyG) if app.edit_mode => {
                            app.apply_operation(Operation::Gcd)
                        }
                        PhysicalKey::Code(KeyCode::Equal) if app.edit_mode => {
                            app.apply_operation(Operation::GainUp)
                        }
                        PhysicalKey::Code(KeyCode::Minus) if app.edit_mode => {
                            app.apply_operation(Operation::GainDown)
                        }
                        PhysicalKey::Code(KeyCode::KeyN) if app.edit_mode => {
                            app.apply_operation(Operation::Normalize)
                        }
                        PhysicalKey::Code(KeyCode::KeyM) if app.edit_mode => {
                            app.apply_operation(Operation::Mute)
                        }
                        PhysicalKey::Code(KeyCode::KeyX) if app.edit_mode => {
                            app.apply_operation(Operation::Reverse)
                        }
                        PhysicalKey::Code(KeyCode::KeyF) if app.edit_mode => {
                            app.apply_operation(Operation::FadeIn)
                        }
                        PhysicalKey::Code(KeyCode::KeyD) if app.edit_mode => {
                            app.apply_operation(Operation::FadeOut)
                        }
                        PhysicalKey::Code(KeyCode::Escape) if app.edit_mode => {
                            app.clear_selection();
                        }
                        PhysicalKey::Code(KeyCode::BracketLeft) if modifiers.shift_key() => {
                            app.step_transition(false);
                        }
                        PhysicalKey::Code(KeyCode::BracketRight) if modifiers.shift_key() => {
                            app.step_transition(true);
                        }
                        PhysicalKey::Code(KeyCode::BracketLeft) => {
                            app.partials = app.partials.saturating_sub(1).max(1);
                            app.reanalyze();
                        }
                        PhysicalKey::Code(KeyCode::BracketRight) => {
                            app.partials = (app.partials + 1).min(24);
                            app.reanalyze();
                        }
                        PhysicalKey::Code(KeyCode::Digit1) => {
                            app.fft_size = (app.fft_size / 2).max(512);
                            app.hop = app.fft_size / 4;
                            app.reanalyze();
                        }
                        PhysicalKey::Code(KeyCode::Digit2) => {
                            app.fft_size = (app.fft_size * 2).min(8192);
                            app.hop = app.fft_size / 4;
                            app.reanalyze();
                        }
                        PhysicalKey::Code(KeyCode::Digit3) => {
                            app.hop = (app.hop / 2).max(128);
                            app.reanalyze();
                        }
                        PhysicalKey::Code(KeyCode::Digit4) => {
                            app.hop = (app.hop * 2).min(app.fft_size);
                            app.reanalyze();
                        }
                        PhysicalKey::Code(KeyCode::KeyO) => {
                            choose_file(dialog_tx.clone());
                        }
                        _ => {}
                    }
                }
                WindowEvent::RedrawRequested => {
                    app.poll();
                    app.sync_follow();
                    if app.spec_dirty
                        && !app.frames.is_empty()
                        && last_spec_upload.elapsed() >= Duration::from_millis(250)
                    {
                        last_spec_upload = std::time::Instant::now();
                        let palette = app.theme.palette(dark);
                        let data = app.spec_pixels(palette);
                        gpu.upload_spec(&data, app.spec_width, app.spec_height);
                    }
                    ui::draw(&mut gpu, &app, dark, cursor, &recent, &help);
                }
                _ => {}
            },
            Event::AboutToWait => {
                while let Ok(p) = dialog_rx.try_recv() {
                    remember_recent(&mut recent, &p);
                    app.open(p);
                }
                while let Ok(paths) = add_rx.try_recv() {
                    for path in &paths {
                        remember_recent(&mut recent, path);
                    }
                    app.add_paths(paths);
                }
                while let Ok(path) = package_rx.try_recv() {
                    app.save_package(path);
                }
                while let Ok(path) = theme_rx.try_recv() {
                    if let Err(error) = app.theme.import(&path) {
                        app.edit_status = Some(error)
                    }
                    app.spec_dirty = true;
                }
                while let Ok((path, selection_only)) = save_rx.try_recv() {
                    app.export_audio(path, selection_only);
                }
                window.request_redraw();
                target.set_control_flow(ControlFlow::WaitUntil(
                    std::time::Instant::now() + Duration::from_millis(16),
                ));
            }
            _ => {}
        })
        .unwrap();
}
