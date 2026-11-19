// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Screen grabbing backends. Runs off the GTK thread and returns plain data.
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub enum Grab {
    /// BGRA (cairo ARGB32 little endian) pixels.
    Pixels { width: i32, height: i32, stride: i32, data: Vec<u8> },
    /// A PNG written by an external tool / the portal. Deleted after loading.
    File(PathBuf),
}

pub fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").map(|v| v.eq_ignore_ascii_case("wayland")).unwrap_or(false)
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub fn is_x11() -> bool {
    !is_wayland() && std::env::var_os("DISPLAY").is_some()
}

fn has_cmd(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

fn tmp_png() -> PathBuf {
    let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    dir.join(format!(
        "linshot-{}-{}.png",
        std::process::id(),
        chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

/// Grab the entire desktop. `interactive` asks the portal to let the user pick
/// (used for window capture on Wayland).
pub fn grab(interactive: bool, cursor: bool) -> Result<Grab, String> {
    if is_wayland() {
        grab_wayland(interactive, cursor)
    } else {
        grab_x11(cursor)
    }
}

// ---------------------------------------------------------------- X11

pub fn grab_x11(cursor: bool) -> Result<Grab, String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};

    let (conn, screen_num) = x11rb::connect(None).map_err(|e| format!("X11 connect: {e}"))?;
    let screen = &conn.setup().roots[screen_num];
    let (w, h) = (screen.width_in_pixels, screen.height_in_pixels);
    let root = screen.root;
    let depth = screen.root_depth;
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, w, h, !0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("X11 GetImage: {e}"))?;
    let bpp = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|f| f.depth == depth)
        .map(|f| f.bits_per_pixel)
        .unwrap_or(32);
    if bpp != 32 {
        return Err(format!("unsupported X11 pixel format ({bpp} bpp)"));
    }
    let (w, h) = (w as i32, h as i32);
    let stride = w * 4;
    let src_stride = reply.data.len() / h.max(1) as usize;
    let mut data = vec![0u8; (stride * h) as usize];
    let big_endian = conn.setup().image_byte_order == x11rb::protocol::xproto::ImageOrder::MSB_FIRST;
    for y in 0..h as usize {
        let src = &reply.data[y * src_stride..y * src_stride + (w as usize) * 4];
        let dst = &mut data[y * stride as usize..(y + 1) * stride as usize];
        for x in 0..w as usize {
            let s = &src[x * 4..x * 4 + 4];
            let (b, g, r) = if big_endian { (s[3], s[2], s[1]) } else { (s[0], s[1], s[2]) };
            let px = 0xff00_0000u32 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
            dst[x * 4..x * 4 + 4].copy_from_slice(&px.to_ne_bytes());
        }
    }
    if cursor {
        let _ = draw_x11_cursor(&conn, &mut data, w, h);
    }
    Ok(Grab::Pixels { width: w, height: h, stride, data })
}

/// Composites the current pointer image (XFixes) onto BGRA pixels.
fn draw_x11_cursor(conn: &impl x11rb::connection::Connection, data: &mut [u8], w: i32, h: i32) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::protocol::xfixes::ConnectionExt;
    conn.xfixes_query_version(5, 0)?.reply()?;
    let c = conn.xfixes_get_cursor_image()?.reply()?;
    let (ox, oy) = (c.x as i32 - c.xhot as i32, c.y as i32 - c.yhot as i32);
    for cy in 0..c.height as i32 {
        for cx in 0..c.width as i32 {
            let (x, y) = (ox + cx, oy + cy);
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let src = c.cursor_image[(cy * c.width as i32 + cx) as usize]; // premultiplied ARGB
            let a = src >> 24;
            if a == 0 {
                continue;
            }
            let off = ((y * w + x) * 4) as usize;
            let dst = u32::from_ne_bytes(data[off..off + 4].try_into()?);
            let mix = |sh: u32| -> u32 {
                let s = (src >> sh) & 0xff;
                let d = (dst >> sh) & 0xff;
                (s + d * (255 - a) / 255).min(255)
            };
            let px = 0xff00_0000 | mix(16) << 16 | mix(8) << 8 | mix(0);
            data[off..off + 4].copy_from_slice(&px.to_ne_bytes());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- Wayland

fn grab_wayland(interactive: bool, cursor: bool) -> Result<Grab, String> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    let wlroots = ["sway", "hyprland", "river", "wlroots", "wayfire", "labwc", "niri", "dwl"]
        .iter()
        .any(|d| desktop.contains(d));
    let mut errors = Vec::new();

    // wlroots compositors (Wayfire, Sway, Hyprland, labwc, ...): native, no helper needed
    if !interactive {
        match crate::wlr::grab(cursor) {
            Ok(g) => return Ok(g),
            Err(e) => errors.push(format!("wlr-screencopy: {e}")),
        }
    }
    if desktop.contains("cinnamon") && !interactive {
        match cinnamon_screenshot(cursor) {
            Ok(g) => return Ok(g),
            Err(e) => errors.push(format!("cinnamon: {e}")),
        }
    }
    let grim: &[&str] = if cursor { &["grim", "-c"] } else { &["grim"] };
    if wlroots && !interactive && has_cmd("grim") {
        match run_tool(grim, true) {
            Ok(g) => return Ok(g),
            Err(e) => errors.push(e),
        }
    }
    match portal_screenshot(interactive) {
        Ok(path) => return Ok(Grab::File(path)),
        Err(e) => errors.push(format!("portal: {e}")),
    }
    if has_cmd("grim") {
        match run_tool(grim, true) {
            Ok(g) => return Ok(g),
            Err(e) => errors.push(e),
        }
    }
    if has_cmd("spectacle") {
        let cmd: &[&str] = if cursor { &["spectacle", "-b", "-n", "-f", "-p", "-o"] } else { &["spectacle", "-b", "-n", "-f", "-o"] };
        match run_tool(cmd, true) {
            Ok(g) => return Ok(g),
            Err(e) => errors.push(e),
        }
    }
    if has_cmd("gnome-screenshot") {
        let cmd: &[&str] = if cursor { &["gnome-screenshot", "-p", "-f"] } else { &["gnome-screenshot", "-f"] };
        match run_tool(cmd, true) {
            Ok(g) => return Ok(g),
            Err(e) => errors.push(e),
        }
    }
    Err(format!("no Wayland screenshot backend worked: {}", errors.join("; ")))
}

fn run_tool(cmd: &[&str], path_last: bool) -> Result<Grab, String> {
    let out = tmp_png();
    let mut c = Command::new(cmd[0]);
    c.args(&cmd[1..]);
    if path_last {
        c.arg(&out);
    }
    let status = c
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("{}: {e}", cmd[0]))?;
    if status.success() && out.is_file() {
        Ok(Grab::File(out))
    } else {
        let _ = std::fs::remove_file(&out);
        Err(format!("{} failed", cmd[0]))
    }
}

/// Cinnamon's own screenshot D-Bus API (org.Cinnamon.Screenshot).
fn cinnamon_screenshot(cursor: bool) -> Result<Grab, String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    let out = tmp_png();
    let path = out.to_string_lossy().into_owned();
    conn.call_method(
        Some("org.Cinnamon"),
        "/org/Cinnamon",
        Some("org.Cinnamon"),
        "Screenshot",
        &(cursor, false, path.as_str()),
    )
    .map_err(|e| e.to_string())?;
    // the file is written asynchronously by the shell
    for _ in 0..50 {
        if std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            return Ok(Grab::File(out));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("no file written".into())
}

/// org.freedesktop.portal.Screenshot. Returns the path of the saved PNG.
fn portal_screenshot(interactive: bool) -> Result<PathBuf, String> {
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    let conn = Connection::session().map_err(|e| e.to_string())?;
    let unique = conn
        .unique_name()
        .ok_or("no unique bus name")?
        .trim_start_matches(':')
        .replace('.', "_");
    let token = format!("linshot{}", std::process::id() as u64 * 1000 + (chrono::Local::now().timestamp_subsec_millis() as u64));
    let req_path = format!("/org/freedesktop/portal/desktop/request/{unique}/{token}");

    let request = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        req_path.as_str(),
        "org.freedesktop.portal.Request",
    )
    .map_err(|e| e.to_string())?;
    let mut responses = request.receive_signal("Response").map_err(|e| e.to_string())?;

    let portal = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Screenshot",
    )
    .map_err(|e| e.to_string())?;
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("handle_token", Value::from(token.as_str()));
    opts.insert("interactive", Value::from(interactive));
    opts.insert("modal", Value::from(true));
    let _handle: OwnedObjectPath = portal.call("Screenshot", &("", opts)).map_err(|e| e.to_string())?;

    let msg = responses.next().ok_or("portal closed without response")?;
    let (code, results): (u32, HashMap<String, OwnedValue>) =
        msg.body().deserialize().map_err(|e| e.to_string())?;
    if code != 0 {
        return Err(if code == 1 { "cancelled".into() } else { format!("portal error {code}") });
    }
    let uri: String = results
        .get("uri")
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
        .ok_or("portal returned no uri")?;
    let path = uri.strip_prefix("file://").ok_or("non-file uri")?;
    Ok(PathBuf::from(percent_decode(path)))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------- X11 windows

/// Visible top-level window rectangles in X11 root pixel coordinates, topmost first.
pub fn x11_windows() -> Vec<(i32, i32, i32, i32)> {
    x11_windows_impl().unwrap_or_default()
}

fn x11_windows_impl() -> Result<Vec<(i32, i32, i32, i32)>, Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    let (conn, sn) = x11rb::connect(None)?;
    let root = conn.setup().roots[sn].root;
    let atom = |n: &str| -> Result<Atom, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, n.as_bytes())?.reply()?.atom)
    };
    let stacking = atom("_NET_CLIENT_LIST_STACKING")?;
    let wm_state = atom("_NET_WM_STATE")?;
    let hidden = atom("_NET_WM_STATE_HIDDEN")?;
    let frame = atom("_NET_FRAME_EXTENTS")?;
    let gtk_frame = atom("_GTK_FRAME_EXTENTS")?;

    let ids: Vec<u32> = conn
        .get_property(false, root, stacking, AtomEnum::WINDOW, 0, 4096)?
        .reply()?
        .value32()
        .map(|v| v.collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for &w in ids.iter().rev() {
        let Ok(attr) = conn.get_window_attributes(w)?.reply() else { continue };
        if attr.map_state != MapState::VIEWABLE {
            continue;
        }
        let states: Vec<u32> = conn
            .get_property(false, w, wm_state, AtomEnum::ATOM, 0, 64)?
            .reply()
            .ok()
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default();
        if states.contains(&hidden) {
            continue;
        }
        if let Some(r) = window_rect(&conn, root, w, frame, gtk_frame) {
            out.push(r);
        }
    }
    Ok(out)
}

fn window_rect(
    conn: &impl x11rb::connection::Connection,
    root: u32,
    w: u32,
    frame: u32,
    gtk_frame: u32,
) -> Option<(i32, i32, i32, i32)> {
    use x11rb::protocol::xproto::*;
    let g = conn.get_geometry(w).ok()?.reply().ok()?;
    let t = conn.translate_coordinates(w, root, 0, 0).ok()?.reply().ok()?;
    let (mut x, mut y, mut wd, mut ht) = (t.dst_x as i32, t.dst_y as i32, g.width as i32, g.height as i32);
    let ext = |a: u32| -> Option<Vec<i32>> {
        let v: Vec<u32> = conn
            .get_property(false, w, a, AtomEnum::CARDINAL, 0, 4)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .collect();
        (v.len() == 4).then(|| v.into_iter().map(|x| x as i32).collect())
    };
    if let Some(e) = ext(frame) {
        // left, right, top, bottom: server-side decorations
        x -= e[0];
        y -= e[2];
        wd += e[0] + e[1];
        ht += e[2] + e[3];
    } else if let Some(e) = ext(gtk_frame) {
        // client-side shadows: shrink
        x += e[0];
        y += e[2];
        wd -= e[0] + e[1];
        ht -= e[2] + e[3];
    }
    (wd > 1 && ht > 1).then_some((x, y, wd, ht))
}

pub fn x11_active_window() -> Option<(i32, i32, i32, i32)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    let (conn, sn) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[sn].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
    let active = atom("_NET_ACTIVE_WINDOW")?;
    let w = conn
        .get_property(false, root, active, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()?;
    if w == 0 {
        return None;
    }
    window_rect(&conn, root, w, atom("_NET_FRAME_EXTENTS")?, atom("_GTK_FRAME_EXTENTS")?)
}
