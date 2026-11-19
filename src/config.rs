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
            filename_pattern: "Screenshot_%Y-%m-%d_%H-%M-%S".into(),
            format: "png".into(),
            jpeg_quality: 90,
            after_capture: AfterCapture::Editor,
            notifications: true,
            magnifier: true,
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
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = config_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
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
        let name = chrono::Local::now().format(&self.filename_pattern).to_string();
        let name = name.replace('/', "_");
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

pub fn shellexpand(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = dirs::home_dir() {
            return h.join(rest).to_string_lossy().into_owned();
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
