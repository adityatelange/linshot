// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Which display server and desktop Linshot runs under. Robust to missing or
//! odd environment variables: apps started from systemd user units, cron or
//! scripts often lack XDG_CURRENT_DESKTOP / XDG_SESSION_TYPE / WAYLAND_DISPLAY,
//! and GDK_BACKEND=x11 runs GTK through XWayland on a Wayland session.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

fn real_env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// Wayland sockets (`wayland-N`, not their lock files) in a runtime directory.
fn wayland_sockets(dir: &Path) -> Vec<String> {
    use std::os::unix::fs::FileTypeExt;
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().map(|t| t.is_socket()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.strip_prefix("wayland-").map(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// X displays with a local socket, e.g. [":0"].
fn x11_displays() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/tmp/.X11-unix")
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter_map(|n| n.strip_prefix('X').filter(|s| s.bytes().all(|b| b.is_ascii_digit())).map(|s| format!(":{s}")))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn runtime_dir(env: Env) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    env("XDG_RUNTIME_DIR").map(PathBuf::from).or_else(|| {
        let uid = std::fs::metadata("/proc/self").ok()?.uid();
        let p = PathBuf::from(format!("/run/user/{uid}"));
        p.is_dir().then_some(p)
    })
}

/// Display variables to add so GTK and helper tools (grim, swaymsg, ...) find
/// the session when they are missing from the environment.
fn missing_display_vars(env: Env, sockets: &[String], x_displays: &[String], runtime: Option<&Path>) -> Vec<(&'static str, String)> {
    let mut add = Vec::new();
    if env("WAYLAND_DISPLAY").is_some() {
        return add;
    }
    let wayland_session = env("XDG_SESSION_TYPE").map(|t| t.eq_ignore_ascii_case("wayland")).unwrap_or(false);
    // with DISPLAY set and no sign of Wayland this is an X11 session
    if wayland_session || env("DISPLAY").is_none() {
        let pick = if sockets.iter().any(|s| s == "wayland-0") {
            Some("wayland-0".to_string())
        } else if sockets.len() == 1 {
            Some(sockets[0].clone())
        } else {
            None
        };
        if let Some(s) = pick {
            if env("XDG_RUNTIME_DIR").is_none() {
                if let Some(r) = runtime {
                    add.push(("XDG_RUNTIME_DIR", r.to_string_lossy().into_owned()));
                }
            }
            add.push(("WAYLAND_DISPLAY", s));
            return add;
        }
    }
    if env("DISPLAY").is_none() && x_displays.len() == 1 {
        add.push(("DISPLAY", x_displays[0].clone()));
    }
    add
}

/// Fills in WAYLAND_DISPLAY / DISPLAY when the session's sockets exist but the
/// variables are missing. Call first thing in main(), before any threads.
pub fn init() {
    let runtime = runtime_dir(&real_env);
    let sockets = runtime.as_deref().map(wayland_sockets).unwrap_or_default();
    for (k, v) in missing_display_vars(&real_env, &sockets, &x11_displays(), runtime.as_deref()) {
        // SAFETY: init() is the first call in main(), before GTK or any other
        // thread starts, so nothing can read the environment concurrently.
        unsafe { std::env::set_var(k, v) };
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Wayland,
    X11,
    Unknown,
}

fn classify(env: Env) -> Kind {
    let wayland_session = env("XDG_SESSION_TYPE").map(|t| t.eq_ignore_ascii_case("wayland")).unwrap_or(false);
    // a Wayland session counts as Wayland even when GTK runs through XWayland
    // (GDK_BACKEND=x11): the X root there only shows X clients
    if env("WAYLAND_DISPLAY").is_some() || wayland_session {
        Kind::Wayland
    } else if env("DISPLAY").is_some() {
        Kind::X11
    } else {
        Kind::Unknown
    }
}

pub fn kind() -> Kind {
    static K: OnceLock<Kind> = OnceLock::new();
    *K.get_or_init(|| classify(&real_env))
}

/// Lower-case desktop identifiers, e.g. "ubuntu:gnome", "kde", "sway".
fn desktop_from(env: Env) -> String {
    for k in ["XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP", "DESKTOP_SESSION"] {
        if let Some(v) = env(k) {
            // DESKTOP_SESSION can be a path to a session file
            let v = v.rsplit('/').next().unwrap_or(&v).trim_end_matches(".desktop").to_lowercase();
            if !v.is_empty() && v != "default" {
                return v;
            }
        }
    }
    // compositors and desktops that announce themselves another way
    const HINTS: [(&str, &str); 9] = [
        ("SWAYSOCK", "sway"),
        ("HYPRLAND_INSTANCE_SIGNATURE", "hyprland"),
        ("NIRI_SOCKET", "niri"),
        ("WAYFIRE_SOCKET", "wayfire"),
        ("LABWC_PID", "labwc"),
        ("KDE_FULL_SESSION", "kde"),
        ("GNOME_SETUP_DISPLAY", "gnome"),
        ("MATE_DESKTOP_SESSION_ID", "mate"),
        ("CINNAMON_VERSION", "x-cinnamon"),
    ];
    HINTS.iter().find(|(k, _)| env(k).is_some()).map(|(_, d)| d.to_string()).unwrap_or_default()
}

pub fn desktop() -> String {
    static D: OnceLock<String> = OnceLock::new();
    D.get_or_init(|| desktop_from(&real_env)).clone()
}

pub fn desktop_is(name: &str) -> bool {
    desktop().split(':').any(|d| d == name || d.contains(name))
}

/// Compositors built on wlroots (or otherwise offering wlr-screencopy).
pub fn is_wlroots() -> bool {
    const WLR: [&str; 12] =
        ["sway", "hyprland", "river", "wlroots", "wayfire", "labwc", "niri", "dwl", "hikari", "qtile", "cage", "miracle-wm"];
    let d = desktop();
    WLR.iter().any(|w| d.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned().filter(|v| !v.is_empty())
    }

    #[test]
    fn desktop_names() {
        assert_eq!(desktop_from(&env(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")])), "ubuntu:gnome");
        assert_eq!(desktop_from(&env(&[("XDG_CURRENT_DESKTOP", "X-Cinnamon")])), "x-cinnamon");
        assert_eq!(desktop_from(&env(&[("DESKTOP_SESSION", "/usr/share/xsessions/plasma")])), "plasma");
        assert_eq!(desktop_from(&env(&[("XDG_SESSION_DESKTOP", "xfce")])), "xfce");
        assert_eq!(desktop_from(&env(&[("SWAYSOCK", "/run/user/1000/sway-ipc.sock")])), "sway");
        assert_eq!(desktop_from(&env(&[("DESKTOP_SESSION", "default"), ("HYPRLAND_INSTANCE_SIGNATURE", "x")])), "hyprland");
        assert_eq!(desktop_from(&env(&[("KDE_FULL_SESSION", "true")])), "kde");
        assert_eq!(desktop_from(&env(&[])), "");
    }

    #[test]
    fn session_kind() {
        assert_eq!(classify(&env(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")])), Kind::Wayland);
        assert_eq!(classify(&env(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")])), Kind::X11);
        // started from a systemd unit or ssh: no session type
        assert_eq!(classify(&env(&[("DISPLAY", ":1")])), Kind::X11);
        assert_eq!(classify(&env(&[("XDG_SESSION_TYPE", "tty"), ("DISPLAY", ":0")])), Kind::X11);
        // XWayland-only environment on a Wayland session
        assert_eq!(classify(&env(&[("XDG_SESSION_TYPE", "wayland"), ("DISPLAY", ":0")])), Kind::Wayland);
        assert_eq!(classify(&env(&[("XDG_SESSION_TYPE", "wayland")])), Kind::Wayland);
        assert_eq!(classify(&env(&[])), Kind::Unknown);
    }

    #[test]
    fn fills_in_display_vars() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let run = Some(Path::new("/run/user/1000"));
        // nothing set, compositor socket present
        assert_eq!(
            missing_display_vars(&env(&[("XDG_RUNTIME_DIR", "/run/user/1000")]), &s(&["wayland-1"]), &[], run),
            vec![("WAYLAND_DISPLAY", "wayland-1".to_string())]
        );
        // no XDG_RUNTIME_DIR either
        assert_eq!(
            missing_display_vars(&env(&[]), &s(&["wayland-0", "wayland-1"]), &[], run),
            vec![("XDG_RUNTIME_DIR", "/run/user/1000".to_string()), ("WAYLAND_DISPLAY", "wayland-0".to_string())]
        );
        // Wayland session where only DISPLAY was passed on
        assert_eq!(
            missing_display_vars(&env(&[("XDG_SESSION_TYPE", "wayland"), ("DISPLAY", ":0"), ("XDG_RUNTIME_DIR", "/r")]), &s(&["wayland-0"]), &s(&[":0"]), run),
            vec![("WAYLAND_DISPLAY", "wayland-0".to_string())]
        );
        // plain X11: leave alone
        assert!(missing_display_vars(&env(&[("DISPLAY", ":0")]), &s(&["wayland-0"]), &s(&[":0"]), run).is_empty());
        // ambiguous Wayland sockets: don't guess
        assert!(missing_display_vars(&env(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "wayland")]), &s(&["wayland-1", "wayland-2"]), &[], run).is_empty());
        // nothing at all but one X server
        assert_eq!(missing_display_vars(&env(&[]), &[], &s(&[":0"]), run), vec![("DISPLAY", ":0".to_string())]);
        assert!(missing_display_vars(&env(&[("WAYLAND_DISPLAY", "w")]), &s(&["wayland-0"]), &[], run).is_empty());
    }
}
