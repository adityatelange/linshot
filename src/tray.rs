// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! StatusNotifierItem tray icon (KDE, XFCE, Cinnamon, MATE, GNOME with the
//! AppIndicator extension, waybar, ...).
use crate::{Cmd, Mode};
use ksni::menu::{StandardItem, SubMenu};
use ksni::MenuItem;

struct Tray {
    tx: async_channel::Sender<Cmd>,
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
        "linshot".into()
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

pub fn spawn(tx: async_channel::Sender<Cmd>) {
    let service = ksni::TrayService::new(Tray { tx });
    service.spawn();
}
