// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! StatusNotifierItem tray icon (KDE, XFCE, Cinnamon, MATE, GNOME with the
//! AppIndicator extension, waybar, ...).
use crate::{Cmd, Mode};
use ksni::menu::{StandardItem, SubMenu};
use ksni::MenuItem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Tray {
    tx: async_channel::Sender<Cmd>,
    /// panel is dark → use the white glyph
    pub dark_panel: bool,
}

impl Tray {
    fn item(label: &str, cmd: Cmd) -> MenuItem<Self> {
        StandardItem {
            label: label.into(),
            activate: Box::new(move |t: &mut Self| {
                let _ = t.tx.send_blocking(cmd.clone());
            }),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "linshot".into()
    }
    fn title(&self) -> String {
        "Linshot".into()
    }
    fn icon_name(&self) -> String {
        if self.dark_panel { "linshot-tray-light" } else { "linshot-tray-dark" }.into()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Linshot".into(),
            description: "Click to capture a region".into(),
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send_blocking(Cmd::cap(Mode::Region));
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            Self::item("Capture region  (Print)", Cmd::cap(Mode::Region)),
            Self::item("Capture last region  (Shift+Print)", Cmd::cap(Mode::Last)),
            Self::item("Capture window  (Alt+Print)", Cmd::cap(Mode::Window)),
            Self::item("Capture active window", Cmd::cap(Mode::Active)),
            Self::item("Capture full screen  (Ctrl+Print)", Cmd::cap(Mode::Full)),
            SubMenu {
                label: "Capture region after delay".into(),
                submenu: vec![
                    Self::item("3 seconds", Cmd::delayed(Mode::Region, 3)),
                    Self::item("5 seconds", Cmd::delayed(Mode::Region, 5)),
                    Self::item("10 seconds", Cmd::delayed(Mode::Region, 10)),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            Self::item("Open image in editor…", Cmd::OpenDialog),
            Self::item("Open screenshots folder", Cmd::OpenFolder),
            MenuItem::Separator,
            Self::item("Preferences…", Cmd::Settings),
            Self::item("Quit", Cmd::Quit),
        ]
    }
}

/// Whether a panel shows StatusNotifierItems right now. Plain GNOME (no
/// AppIndicator extension), many tiling setups and some Wayland compositors
/// have none, so the launcher window is then the way back into Linshot.
pub fn host_available() -> bool {
    let check = || -> zbus::Result<bool> {
        let conn = zbus::blocking::Connection::session()?;
        let p = zbus::blocking::Proxy::new(&conn, "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher")?;
        p.get_property("IsStatusNotifierHostRegistered")
    };
    check().unwrap_or(false)
}

/// GNOME's top bar is dark regardless of the theme.
pub fn panel_always_dark() -> bool {
    crate::session::desktop_is("gnome")
}

/// The running tray, which may be restarted underneath (see `spawn`).
pub struct TrayHandle {
    current: Arc<Mutex<Option<ksni::Handle<Tray>>>>,
    dark: Arc<AtomicBool>,
}

impl TrayHandle {
    pub fn set_dark_panel(&self, dark: bool) {
        self.dark.store(dark, Ordering::Relaxed);
        let h = self.current.lock().ok().and_then(|c| c.clone());
        if let Some(h) = h {
            // ksni unwraps its model lock, which is poisoned if its thread
            // panicked while holding it; that must not abort the GTK thread
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.update(|t| t.dark_panel = dark)));
            if r.is_err() {
                eprintln!("linshot: tray icon is not responding");
            }
        }
    }
}

/// Runs the tray service on its own thread and restarts it when it fails.
/// ksni panics or returns an error when the D-Bus session connection breaks
/// (e.g. the bus or the panel restarts); after repeated quick failures it
/// gives up and Linshot carries on without a tray icon.
pub fn spawn(tx: async_channel::Sender<Cmd>) -> TrayHandle {
    let current = Arc::new(Mutex::new(None));
    let dark = Arc::new(AtomicBool::new(true));
    let handle = TrayHandle { current: current.clone(), dark: dark.clone() };
    let r = std::thread::Builder::new().name("linshot-tray".into()).spawn(move || {
        let mut quick_failures = 0u32;
        loop {
            let service = ksni::TrayService::new(Tray { tx: tx.clone(), dark_panel: dark.load(Ordering::Relaxed) });
            if let Ok(mut c) = current.lock() {
                *c = Some(service.handle());
            }
            let started = Instant::now();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| service.run()));
            if let Ok(mut c) = current.lock() {
                *c = None;
            }
            if tx.is_closed() {
                return;
            }
            match r {
                Ok(Ok(())) => return, // shut down on request
                Ok(Err(e)) => eprintln!("linshot: tray icon stopped: {e}"),
                Err(_) => eprintln!("linshot: tray icon crashed"),
            }
            quick_failures = if started.elapsed() > Duration::from_secs(60) { 1 } else { quick_failures + 1 };
            if quick_failures > 5 {
                eprintln!("linshot: giving up on the tray icon");
                return;
            }
            std::thread::sleep(Duration::from_secs(1 << quick_failures.min(5)));
        }
    });
    if let Err(e) = r {
        eprintln!("linshot: could not start the tray icon: {e}");
    }
    handle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_tray_does_not_panic_caller() {
        let (tx, _rx) = async_channel::unbounded();
        let service = ksni::TrayService::new(Tray { tx, dark_panel: true });
        let h = service.handle();
        // a panic while ksni holds the model lock poisons it, as a D-Bus error in its thread does
        let h2 = h.clone();
        let _ = std::thread::spawn(move || h2.update(|_| panic!("simulated tray failure"))).join();
        let th = TrayHandle { current: Arc::new(Mutex::new(Some(h))), dark: Arc::new(AtomicBool::new(true)) };
        th.set_dark_panel(false);
        assert!(!th.dark.load(Ordering::Relaxed));
    }
}
