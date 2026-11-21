// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! StatusNotifierItem tray icon (KDE, XFCE, Cinnamon, MATE, GNOME with the
//! AppIndicator extension, waybar, ...).
use crate::{Cmd, Mode};
use ksni::menu::{StandardItem, SubMenu};
use ksni::MenuItem;

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

/// The running tray icon (none if it could not be started).
pub struct TrayHandle {
    handle: Option<ksni::blocking::Handle<Tray>>,
}

impl TrayHandle {
    pub fn set_dark_panel(&self, dark: bool) {
        if let Some(h) = &self.handle {
            // None once the service has stopped: nothing to update then
            h.update(|t| t.dark_panel = dark);
        }
    }
}

/// Starts the tray service on its own thread. ksni keeps the icon registered
/// across panel / StatusNotifierWatcher restarts and waits for a watcher that
/// is not there yet; if the service cannot start at all (no session bus),
/// Linshot carries on without a tray icon.
pub fn spawn(tx: async_channel::Sender<Cmd>) -> TrayHandle {
    use ksni::blocking::TrayMethods;
    let tray = Tray { tx, dark_panel: true };
    let handle = match tray.assume_sni_available(true).spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("linshot: could not start the tray icon: {e}");
            None
        }
    };
    TrayHandle { handle }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tray_is_a_no_op() {
        TrayHandle { handle: None }.set_dark_panel(false);
    }
}
