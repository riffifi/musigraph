use crate::render::gpu::Color;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Light,
    Dark,
    Custom,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Custom => "custom",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::Custom,
            Self::Custom => Self::Auto,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub panel: Color,
    pub ink: Color,
    pub muted: Color,
    pub line: Color,
    pub accent: Color,
    pub edit_gain: Color,
    pub edit_shape: Color,
    pub edit_filter: Color,
    pub transition: Color,
}
impl Palette {
    pub fn builtin(dark: bool) -> Self {
        if dark {
            Self {
                bg: Color(0.075, 0.08, 0.085, 1.),
                panel: Color(0.105, 0.11, 0.115, 1.),
                ink: Color(0.82, 0.83, 0.83, 1.),
                muted: Color(0.49, 0.52, 0.54, 1.),
                line: Color(0.24, 0.26, 0.27, 1.),
                accent: Color(0.32, 0.68, 0.70, 1.),
                edit_gain: Color(0.66, 0.72, 0.42, 1.),
                edit_shape: Color(0.74, 0.53, 0.69, 1.),
                edit_filter: Color(0.83, 0.62, 0.38, 1.),
                transition: Color(0.41, 0.68, 0.84, 1.),
            }
        } else {
            Self {
                bg: Color(0.975, 0.975, 0.97, 1.),
                panel: Color(0.94, 0.945, 0.94, 1.),
                ink: Color(0.18, 0.20, 0.21, 1.),
                muted: Color(0.46, 0.49, 0.5, 1.),
                line: Color(0.78, 0.8, 0.8, 1.),
                accent: Color(0.08, 0.43, 0.48, 1.),
                edit_gain: Color(0.34, 0.47, 0.13, 1.),
                edit_shape: Color(0.48, 0.25, 0.43, 1.),
                edit_filter: Color(0.58, 0.34, 0.12, 1.),
                transition: Color(0.14, 0.39, 0.60, 1.),
            }
        }
    }
    fn set(&mut self, key: &str, color: Color) -> Result<(), String> {
        match key {
            "bg" => self.bg = color,
            "panel" => self.panel = color,
            "ink" => self.ink = color,
            "muted" => self.muted = color,
            "line" => self.line = color,
            "accent" => self.accent = color,
            "edit_gain" => self.edit_gain = color,
            "edit_shape" => self.edit_shape = color,
            "edit_filter" => self.edit_filter = color,
            "transition" => self.transition = color,
            _ => return Err(format!("Unknown theme color: {key}")),
        }
        Ok(())
    }
}
fn parse_color(raw: &str) -> Result<Color, String> {
    let hex = raw.strip_prefix('#').ok_or("Theme colors need #RRGGBB")?;
    if hex.len() != 6 {
        return Err("Theme colors need #RRGGBB".into());
    }
    let n = u32::from_str_radix(hex, 16).map_err(|_| "Invalid theme color")?;
    Ok(Color(
        ((n >> 16) & 255) as f32 / 255.,
        ((n >> 8) & 255) as f32 / 255.,
        (n & 255) as f32 / 255.,
        1.,
    ))
}
fn hex(c: Color) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        (c.0 * 255.).round() as u8,
        (c.1 * 255.).round() as u8,
        (c.2 * 255.).round() as u8
    )
}
#[derive(Clone)]
pub struct ThemeSettings {
    pub mode: Mode,
    pub custom: Palette,
    pub path: PathBuf,
}
impl ThemeSettings {
    pub fn new(path: PathBuf) -> Self {
        Self {
            mode: Mode::Auto,
            custom: Palette::builtin(true),
            path,
        }
    }
    pub fn palette(&self, os_dark: bool) -> Palette {
        match self.mode {
            Mode::Auto => Palette::builtin(os_dark),
            Mode::Light => Palette::builtin(false),
            Mode::Dark => Palette::builtin(true),
            Mode::Custom => self.custom,
        }
    }
    pub fn read(&mut self) -> Result<(), String> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.write()?;
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };
        let mut parsed = self.clone();
        for (i, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("Theme line {} needs =", i + 1))?;
            let (key, value) = (key.trim(), value.trim());
            if key == "mode" {
                parsed.mode = match value {
                    "auto" => Mode::Auto,
                    "light" => Mode::Light,
                    "dark" => Mode::Dark,
                    "custom" => Mode::Custom,
                    _ => return Err(format!("Invalid theme mode on line {}", i + 1)),
                };
            } else {
                parsed.custom.set(
                    key,
                    parse_color(value).map_err(|e| format!("{e} on line {}", i + 1))?,
                )?;
            }
        }
        *self = parsed;
        Ok(())
    }
    pub fn write(&self) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?
        }
        let p = self.custom;
        std::fs::write(&self.path,format!("mode={}\nbg={}\npanel={}\nink={}\nmuted={}\nline={}\naccent={}\nedit_gain={}\nedit_shape={}\nedit_filter={}\ntransition={}\n",self.mode.label(),hex(p.bg),hex(p.panel),hex(p.ink),hex(p.muted),hex(p.line),hex(p.accent),hex(p.edit_gain),hex(p.edit_shape),hex(p.edit_filter),hex(p.transition))).map_err(|e|e.to_string())
    }
    pub fn cycle(&mut self) -> Result<(), String> {
        self.mode = self.mode.next();
        self.write()
    }
    pub fn import(&mut self, path: &std::path::Path) -> Result<(), String> {
        if !path.is_file() {
            return Err("Theme file is missing".into());
        }
        let mut imported = self.clone();
        imported.path = path.to_path_buf();
        imported.read()?;
        imported.mode = Mode::Custom;
        imported.path = self.path.clone();
        imported.write()?;
        *self = imported;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_and_reload() {
        let path = std::env::temp_dir().join(format!("theme-{}.theme", std::process::id()));
        let mut t = ThemeSettings::new(path.clone());
        t.read().unwrap();
        t.cycle().unwrap();
        t.cycle().unwrap();
        t.cycle().unwrap();
        assert_eq!(t.mode, Mode::Custom);
        std::fs::write(&path, "mode=custom\nbg=#102030\nedit_gain=#F0A020\n").unwrap();
        t.read().unwrap();
        assert!((t.custom.bg.0 - 16. / 255.).abs() < 1e-6);
        assert!(t.palette(true).edit_gain.0 > 0.9);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn importing_theme_keeps_configured_path() {
        let dir = std::env::temp_dir();
        let output = dir.join(format!("active-theme-{}.theme", std::process::id()));
        let source = dir.join(format!("source-theme-{}.theme", std::process::id()));
        std::fs::write(&source, "bg=#223344\ntransition=#884422\n").unwrap();
        let mut theme = ThemeSettings::new(output.clone());
        theme.import(&source).unwrap();
        assert_eq!(theme.mode, Mode::Custom);
        assert_eq!(theme.path, output);
        let mut reopened = ThemeSettings::new(output.clone());
        reopened.read().unwrap();
        assert!((reopened.custom.transition.0 - 136. / 255.).abs() < 1e-6);
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(source).unwrap();
    }
}
