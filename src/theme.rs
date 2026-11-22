// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Follows the desktop's light/dark preference and accent colour
//! (org.freedesktop.appearance color-scheme / accent-color via
//! xdg-desktop-portal, falling back to GNOME's gsettings key). GNOME 47+,
//! KDE and, through xdg-desktop-portal-xapp, Cinnamon / MATE / Xfce provide
//! an accent colour.
use std::process::Command;

pub enum Appearance {
    Dark(bool),
    /// sRGB 0..1; None when the desktop has no accent colour
    Accent(Option<[f64; 3]>),
}

/// Sends the dark preference and accent colour on startup and on every change.
pub fn spawn_watcher(tx: async_channel::Sender<Appearance>) {
    std::thread::spawn(move || {
        // reconnect a few times if the portal goes away (e.g. it is restarted)
        for attempt in 0..10 {
            let r = watch_portal(&tx);
            if tx.is_closed() {
                return;
            }
            if r.is_err() && attempt == 0 {
                let _ = tx.send_blocking(Appearance::Dark(gsettings_dark()));
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}

fn to_u32(v: &zbus::zvariant::Value) -> Option<u32> {
    use zbus::zvariant::Value;
    match v {
        Value::U32(n) => Some(*n),
        Value::Value(inner) => to_u32(inner),
        _ => None,
    }
}

/// The portal's accent-color: (ddd) in 0..1, anything out of range = unset.
fn to_rgb(v: &zbus::zvariant::Value) -> Option<[f64; 3]> {
    use zbus::zvariant::{Structure, Value};
    match v {
        Value::Value(inner) => to_rgb(inner),
        Value::Structure(st) => {
            let st: &Structure = st;
            let f: Vec<f64> = st
                .fields()
                .iter()
                .filter_map(|x| if let Value::F64(n) = x { Some(*n) } else { None })
                .collect();
            (f.len() == 3 && f.iter().all(|n| (0.0..=1.0).contains(n))).then(|| [f[0], f[1], f[2]])
        }
        _ => None,
    }
}

fn watch_portal(tx: &async_channel::Sender<Appearance>) -> Result<(), Box<dyn std::error::Error>> {
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::OwnedValue;

    let conn = Connection::session()?;
    let proxy = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )?;
    let changes = proxy.receive_signal("SettingChanged")?;
    let key = ("org.freedesktop.appearance", "color-scheme");
    let v: OwnedValue = proxy
        .call("ReadOne", &key)
        .or_else(|_| proxy.call("Read", &key))?;
    // 0 = no preference, 1 = prefer dark, 2 = prefer light
    let initial = to_u32(&v).ok_or("unexpected color-scheme value")?;
    let _ = tx.send_blocking(Appearance::Dark(if initial == 0 { gsettings_dark() } else { initial == 1 }));
    let akey = ("org.freedesktop.appearance", "accent-color");
    if let Ok(v) = proxy.call::<_, _, OwnedValue>("ReadOne", &akey).or_else(|_| proxy.call("Read", &akey)) {
        let _ = tx.send_blocking(Appearance::Accent(to_rgb(&v)));
    }

    for msg in changes {
        let Ok((ns, k, val)) = msg.body().deserialize::<(String, String, OwnedValue)>() else { continue };
        let ev = match (ns == key.0, k.as_str()) {
            (true, "color-scheme") => to_u32(&val).map(|n| Appearance::Dark(n == 1)),
            (true, "accent-color") => Some(Appearance::Accent(to_rgb(&val))),
            _ => None,
        };
        if let Some(ev) = ev {
            if tx.send_blocking(ev).is_err() {
                break;
            }
        }
    }
    Ok(())
}

fn gsettings_dark() -> bool {
    Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("dark"))
        .unwrap_or(false)
}
