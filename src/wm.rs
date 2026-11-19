// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Window geometry on Wayland compositors that expose it over IPC
//! (Wayfire, Sway, Hyprland). Rectangles are in global logical coordinates,
//! topmost / most recently focused first.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::Command;

pub type Rect = (f64, f64, f64, f64);

pub struct Windows {
    pub list: Vec<Rect>,
    pub active: Option<Rect>,
}

fn wayfire_socket() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("WAYFIRE_SOCKET") {
        return Some(p.into());
    }
    let d = std::env::var("WAYLAND_DISPLAY").ok()?;
    let p = std::path::PathBuf::from(format!("/tmp/wayfire-{d}.socket"));
    p.exists().then_some(p)
}

pub fn windows() -> Option<Windows> {
    if wayfire_socket().is_some() {
        if let Some(w) = wayfire() {
            return Some(w);
        }
    }
    if std::env::var_os("SWAYSOCK").is_some() {
        if let Some(w) = sway() {
            return Some(w);
        }
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        if let Some(w) = hyprland() {
            return Some(w);
        }
    }
    None
}

fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

fn rect(v: &Value) -> Option<Rect> {
    let r = (num(v, "x"), num(v, "y"), num(v, "width"), num(v, "height"));
    (r.2 > 1.0 && r.3 > 1.0).then_some(r)
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

// ---------------------------------------------------------------- Wayfire
// Needs the `ipc` and `ipc-rules` plugins (enabled in most Wayfire setups
// that use wf-shell / third-party panels).

fn wayfire_call(method: &str) -> Option<Value> {
    let path = wayfire_socket()?;
    let mut s = std::os::unix::net::UnixStream::connect(path).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(2))).ok()?;
    let msg = serde_json::to_vec(&json!({ "method": method, "data": {} })).ok()?;
    // the length header is in host byte order
    s.write_all(&(msg.len() as u32).to_ne_bytes()).ok()?;
    s.write_all(&msg).ok()?;
    let mut len = [0u8; 4];
    s.read_exact(&mut len).ok()?;
    let mut buf = vec![0u8; u32::from_ne_bytes(len) as usize];
    s.read_exact(&mut buf).ok()?;
    let v: Value = serde_json::from_slice(&buf).ok()?;
    if v.get("error").is_some() {
        return None;
    }
    Some(v)
}

fn wayfire() -> Option<Windows> {
    let outputs = wayfire_call("window-rules/list-outputs")?;
    let outs: Vec<(i64, Rect)> = outputs
        .as_array()?
        .iter()
        .filter_map(|o| Some((o.get("id")?.as_i64()?, rect(o.get("geometry")?)?)))
        .collect();
    let views = wayfire_call("window-rules/list-views")?;
    let mut list: Vec<(f64, Rect, bool)> = Vec::new();
    for v in views.as_array()? {
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let role = v.get("role").and_then(Value::as_str).unwrap_or("toplevel");
        if !v.get("mapped").and_then(Value::as_bool).unwrap_or(true) || flag("minimized") || role != "toplevel" {
            continue;
        }
        let Some(g) = v.get("geometry").and_then(rect) else { continue };
        // view geometry is relative to its output
        let out = v
            .get("output-id")
            .and_then(Value::as_i64)
            .and_then(|id| outs.iter().find(|o| o.0 == id))
            .map(|o| o.1);
        let (ox, oy) = out.map(|o| (o.0, o.1)).unwrap_or((0.0, 0.0));
        let r = (g.0 + ox, g.1 + oy, g.2, g.3);
        // skip views on other workspaces (outside their output)
        if let Some(o) = out {
            if !intersects(&r, &o) {
                continue;
            }
        }
        let ts = num(v, "last-focus-timestamp");
        list.push((ts, r, flag("activated")));
    }
    list.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let active = list.iter().find(|x| x.2).or(list.first()).map(|x| x.1);
    Some(Windows { list: list.into_iter().map(|x| x.1).collect(), active })
}

// ---------------------------------------------------------------- Sway

fn sway() -> Option<Windows> {
    let out = Command::new("swaymsg").args(["-t", "get_tree", "-r"]).output().ok()?;
    let tree: Value = serde_json::from_slice(&out.stdout).ok()?;
    let mut list = Vec::new();
    let mut active = None;
    fn walk(n: &Value, list: &mut Vec<(bool, Rect)>, active: &mut Option<Rect>) {
        let is_window = n.get("pid").is_some() && n.get("visible").and_then(Value::as_bool).unwrap_or(false);
        if is_window {
            if let Some(mut r) = n.get("rect").and_then(rect) {
                // include the title bar
                if let Some(d) = n.get("deco_rect").and_then(rect) {
                    r.1 -= d.3;
                    r.3 += d.3;
                }
                let focused = n.get("focused").and_then(Value::as_bool).unwrap_or(false);
                if focused {
                    *active = Some(r);
                }
                list.push((n.get("type").and_then(Value::as_str) == Some("floating_con"), r));
            }
        }
        for key in ["floating_nodes", "nodes"] {
            if let Some(ch) = n.get(key).and_then(Value::as_array) {
                for c in ch {
                    walk(c, list, active);
                }
            }
        }
    }
    let mut raw = Vec::new();
    walk(&tree, &mut raw, &mut active);
    // floating windows sit above tiled ones
    raw.sort_by_key(|x| !x.0);
    list.extend(raw.into_iter().map(|x| x.1));
    Some(Windows { list, active })
}

// ---------------------------------------------------------------- Hyprland

fn hyprland() -> Option<Windows> {
    let get = |what: &str| -> Option<Value> {
        let o = Command::new("hyprctl").args(["-j", what]).output().ok()?;
        serde_json::from_slice(&o.stdout).ok()
    };
    let monitors = get("monitors")?;
    let active_ws: Vec<i64> = monitors
        .as_array()?
        .iter()
        .filter_map(|m| m.get("activeWorkspace")?.get("id")?.as_i64())
        .collect();
    let clients = get("clients")?;
    let mut list: Vec<(i64, Rect)> = clients
        .as_array()?
        .iter()
        .filter(|c| c.get("mapped").and_then(Value::as_bool).unwrap_or(true))
        .filter(|c| !c.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .filter(|c| {
            c.get("workspace")
                .and_then(|w| w.get("id"))
                .and_then(Value::as_i64)
                .map(|id| active_ws.contains(&id))
                .unwrap_or(false)
        })
        .filter_map(|c| {
            let at = c.get("at")?.as_array()?;
            let sz = c.get("size")?.as_array()?;
            let r = (at[0].as_f64()?, at[1].as_f64()?, sz[0].as_f64()?, sz[1].as_f64()?);
            Some((c.get("focusHistoryID").and_then(Value::as_i64).unwrap_or(99), r))
        })
        .collect();
    list.sort_by_key(|x| x.0);
    let active = list.first().map(|x| x.1);
    Some(Windows { list: list.into_iter().map(|x| x.1).collect(), active })
}
