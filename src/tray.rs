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
    /// seconds left of a delayed capture
    pub countdown: Option<u32>,
}

/// The remaining seconds as a tray icon: a green disc with the number.
fn countdown_icon(n: u32, size: i32) -> Option<ksni::Icon> {
    let mut surf = cairo::ImageSurface::create(cairo::Format::ARgb32, size, size).ok()?;
    {
        let cr = cairo::Context::new(&surf).ok()?;
        let s = size as f64;
        cr.set_source_rgb(0.13, 0.65, 0.40);
        cr.arc(s / 2.0, s / 2.0, s / 2.0, 0.0, std::f64::consts::TAU);
        cr.fill().ok()?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        let text = n.to_string();
        cr.set_font_size(s * if text.len() > 1 { 0.5 } else { 0.62 });
        let e = cr.text_extents(&text).ok()?;
        cr.move_to((s - e.width()) / 2.0 - e.x_bearing(), (s - e.height()) / 2.0 - e.y_bearing());
        cr.show_text(&text).ok()?;
    }
    surf.flush();
    let stride = surf.stride() as usize;
    let data = surf.data().ok()?;
    // cairo: native-endian premultiplied ARGB; SNI: straight ARGB, big-endian
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size as usize {
        for x in 0..size as usize {
            let p = u32::from_ne_bytes(data[y * stride + x * 4..][..4].try_into().ok()?);
            let a = p >> 24;
            let un = |c: u32| if a == 0 { 0 } else { ((c * 255 + a / 2) / a).min(255) };
            let (r, g, b) = (un((p >> 16) & 0xff), un((p >> 8) & 0xff), un(p & 0xff));
            out.extend_from_slice(&[a as u8, r as u8, g as u8, b as u8]);
        }
    }
    Some(ksni::Icon { width: size, height: size, data: out })
}

impl Tray {
    fn item(label: &str, cmd: Cmd) -> MenuItem<Self> {
        StandardItem {
            label: label.into(),
            activate: Box::new(move |t: &mut Self| {
                let _ = t.tx.send_blocking(cmd.clone().from_menu());
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
        if self.countdown.is_some() {
            // panels prefer a named icon over the pixmap
            return String::new();
        }
        if self.dark_panel { "linshot-tray-light" } else { "linshot-tray-dark" }.into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        match self.countdown {
            Some(n) => [22, 44].into_iter().filter_map(|s| countdown_icon(n, s)).collect(),
            None => Vec::new(),
        }
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Linshot".into(),
            description: match self.countdown {
                Some(n) => format!("Capturing in {n} s"),
                None => "Click to capture a region".into(),
            },
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send_blocking(Cmd::cap(Mode::Region));
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items = Vec::new();
        if let Some(n) = self.countdown {
            items.push(Self::item(&format!("Cancel capture ({n} s)"), Cmd::CancelCapture));
            items.push(MenuItem::Separator);
        }
        items.extend(vec![
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
        ]);
        items
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

    /// Shows the seconds left of a delayed capture on the icon (None: normal icon).
    pub fn set_countdown(&self, n: Option<u32>) {
        if let Some(h) = &self.handle {
            h.update(|t| t.countdown = n);
        }
    }
}

/// Starts the tray service on its own thread. ksni keeps the icon registered
/// across panel / StatusNotifierWatcher restarts and waits for a watcher that
/// is not there yet; if the service cannot start at all (no session bus),
/// Linshot carries on without a tray icon.
pub fn spawn(tx: async_channel::Sender<Cmd>) -> TrayHandle {
    use ksni::blocking::TrayMethods;
    let tray = Tray { tx, dark_panel: true, countdown: None };
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
        TrayHandle { handle: None }.set_countdown(Some(3));
    }

    #[test]
    fn countdown_icon_is_argb_big_endian() {
        let i = countdown_icon(3, 22).unwrap();
        assert_eq!(i.data.len(), 22 * 22 * 4);
        // centre-left of the disc is opaque green: A, R, G, B
        let p = &i.data[(11 * 22 + 3) * 4..][..4];
        assert_eq!(p[0], 255);
        assert!(p[2] > p[1] && p[2] > p[3]);
    }
}
