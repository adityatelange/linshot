// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterCapture {
    Editor,
    Save,
    Clipboard,
    SaveAndClipboard,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub output_dir: String,
    pub filename_pattern: String,
    pub format: String,
    pub jpeg_quality: u8,
    pub after_capture: AfterCapture,
    pub notifications: bool,
    pub magnifier: bool,
    /// after dragging a region it can be resized / moved and is confirmed
    /// with Enter or the buttons, instead of being captured on release
    pub adjust_selection: bool,
    pub hotkeys: bool,
    pub last_region: Option<[i32; 4]>,
    pub line_color: [f64; 4],
    pub fill_color: [f64; 4],
    pub fill_enabled: bool,
    pub line_width: f64,
    pub font_size: f64,
    pub shadow: bool,
    /// "system", "light" or "dark"
    pub theme: String,
    /// asked once whether to register desktop shortcuts
    pub shortcuts_prompted: bool,
    pub capture_cursor: bool,
    /// user wants Linshot's shortcuts in the desktop config (re-applied if lost)
    pub desktop_shortcuts: Option<bool>,
}

impl Default for Config {
    fn default() -> Self {
        let pics = dirs::picture_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Pictures")))
            .unwrap_or_else(|| PathBuf::from("."));
        Config {
            output_dir: pics.join("Screenshots").to_string_lossy().into_owned(),
            filename_pattern: DEFAULT_PATTERN.into(),
            format: "png".into(),
            jpeg_quality: 90,
            after_capture: AfterCapture::Editor,
            notifications: true,
            magnifier: true,
            adjust_selection: true,
            hotkeys: true,
            last_region: None,
            line_color: [0.937, 0.267, 0.267, 1.0],
            fill_color: [1.0, 1.0, 1.0, 1.0],
            fill_enabled: false,
            line_width: 3.0,
            font_size: 18.0,
            shadow: true,
            theme: "system".into(),
            shortcuts_prompted: false,
            capture_cursor: true,
            desktop_shortcuts: None,
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("linshot")
        .join("config.json")
}

impl Config {
    pub fn load() -> Config {
        let p = config_path();
        let Ok(s) = std::fs::read_to_string(&p) else { return Config::default() };
        match serde_json::from_str::<Config>(&s) {
            Ok(mut c) => {
                // hand-edited values the encoder would reject
                c.jpeg_quality = c.jpeg_quality.clamp(1, 100);
                c
            }
            Err(e) => {
                // keep the broken file for the user instead of overwriting it on the next save
                let bak = p.with_extension("json.bad");
                eprintln!("linshot: {} is invalid ({e}); using defaults, old file kept as {}", p.display(), bak.display());
                let _ = std::fs::rename(&p, &bak);
                Config::default()
            }
        }
    }

    pub fn save(&self) {
        let p = config_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            if let Err(e) = write_atomic(&p, s.as_bytes(), true) {
                eprintln!("linshot: could not save {}: {e}", p.display());
            }
        }
    }

    pub fn extension(&self) -> &str {
        if self.format == "jpg" || self.format == "jpeg" {
            "jpg"
        } else {
            "png"
        }
    }

    /// Builds a unique file path in the output directory from the filename pattern.
    pub fn next_output_path(&self) -> PathBuf {
        let dir = PathBuf::from(shellexpand(&self.output_dir));
        let _ = std::fs::create_dir_all(&dir);
        let name = file_stem(&self.filename_pattern, chrono::Local::now());
        let ext = self.extension();
        let mut path = dir.join(format!("{name}.{ext}"));
        let mut n = 1;
        while path.exists() {
            path = dir.join(format!("{name}_{n}.{ext}"));
            n += 1;
        }
        path
    }
}

/// Writes a file so that readers (and a crash or full disk midway) only ever
/// see the old or the complete new content: temp file in the same folder,
/// then rename over the target. `sync` flushes to disk before the rename.
pub fn write_atomic(path: &std::path::Path, data: &[u8], sync: bool) -> std::io::Result<()> {
    use std::io::Write;
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    let tmp = path.with_file_name(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id()));
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        if sync {
            f.sync_all()?;
        }
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

pub const DEFAULT_PATTERN: &str = "Screenshot_%Y-%m-%d_%H-%M-%S";

/// Expands a strftime pattern, or None if it is invalid (e.g. "%Q" or a
/// trailing "%"): chrono reports those as a formatting error, and
/// `to_string()` on them panics.
pub fn format_pattern(pattern: &str, t: chrono::DateTime<chrono::Local>) -> Option<String> {
    use std::fmt::Write;
    let mut out = String::new();
    write!(out, "{}", t.format(pattern)).ok()?;
    Some(out)
}

/// The file name (without extension) for a capture taken at `t`. Falls back
/// to the default pattern when the configured one is invalid or empty.
fn file_stem(pattern: &str, t: chrono::DateTime<chrono::Local>) -> String {
    format_pattern(pattern, t)
        .map(|n| n.replace('/', "_"))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            eprintln!("linshot: invalid file name pattern {pattern:?}, using the default");
            format_pattern(DEFAULT_PATTERN, t).unwrap_or_default()
        })
}

pub fn shellexpand(s: &str) -> String {
    let home = || dirs::home_dir().map(|h| h.to_string_lossy().into_owned());
    for prefix in ["~", "$HOME", "${HOME}"] {
        if let Some(rest) = s.strip_prefix(prefix)
            && (rest.is_empty() || rest.starts_with('/'))
            && let Some(h) = home()
        {
            return format!("{h}{rest}");
        }
    }
    s.to_string()
}

const AUTOSTART_FILE: &str = "linshot.desktop";

pub fn autostart_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("autostart")
        .join(AUTOSTART_FILE)
}

pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let p = autostart_path();
    if enabled {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(
            p,
            "[Desktop Entry]\nType=Application\nName=Linshot\nExec=linshot --tray\nIcon=linshot\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        )
    } else {
        match std::fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("linshot-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");
        write_atomic(&p, b"one", true).unwrap();
        write_atomic(&p, b"two", false).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        // only the target is left behind
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // unwritable target folder: error, no stray temp file
        assert!(write_atomic(&dir.join("missing").join("x.png"), b"x", false).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn config_roundtrip_and_partial() {
        let c = Config::default();
        let s = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back.filename_pattern, c.filename_pattern);
        // missing fields fall back to defaults
        let partial: Config = serde_json::from_str(r#"{"jpeg_quality": 50}"#).unwrap();
        assert_eq!(partial.jpeg_quality, 50);
        assert!(partial.notifications);
    }

    #[test]
    fn invalid_patterns_fall_back_instead_of_panicking() {
        let t = chrono::Local::now();
        for p in ["%Q", "shot_%", "", "   "] {
            let n = file_stem(p, t);
            assert!(n.starts_with("Screenshot_"), "{p:?} -> {n}");
        }
        assert_eq!(file_stem("a/b_%Y", t), format!("a_b_{}", t.format("%Y")));
        assert!(format_pattern("%Q", t).is_none());
    }

    #[test]
    fn home_is_expanded() {
        let h = dirs::home_dir().unwrap().to_string_lossy().into_owned();
        assert_eq!(shellexpand("~"), h);
        assert_eq!(shellexpand("~/Pictures"), format!("{h}/Pictures"));
        assert_eq!(shellexpand("$HOME/x"), format!("{h}/x"));
        assert_eq!(shellexpand("~user/x"), "~user/x");
        assert_eq!(shellexpand("/tmp"), "/tmp");
    }
}
