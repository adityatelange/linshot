// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Follows the desktop's light/dark preference
//! (org.freedesktop.appearance color-scheme via xdg-desktop-portal,
//! falling back to GNOME's gsettings key).
use std::process::Command;

/// Sends `true` when the desktop prefers dark, on startup and on every change.
pub fn spawn_watcher(tx: async_channel::Sender<bool>) {
    std::thread::spawn(move || {
        if watch_portal(&tx).is_err() {
            let _ = tx.send_blocking(gsettings_dark());
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

fn watch_portal(tx: &async_channel::Sender<bool>) -> Result<(), Box<dyn std::error::Error>> {
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
    let _ = tx.send_blocking(if initial == 0 { gsettings_dark() } else { initial == 1 });

    for msg in changes {
        let Ok((ns, k, val)) = msg.body().deserialize::<(String, String, OwnedValue)>() else { continue };
        if ns == key.0 && k == key.1 {
            if let Some(n) = to_u32(&val) {
                if tx.send_blocking(n == 1).is_err() {
                    break;
                }
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
