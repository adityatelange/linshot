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

/// The mouse pointer captured separately so it can be moved in the editor.
/// Position is in screenshot pixels; data is premultiplied BGRA (cairo ARGB32).
pub struct CursorImg {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub data: Vec<u8>,
}

pub fn is_wayland() -> bool {
    crate::session::kind() == crate::session::Kind::Wayland
}

pub fn is_x11() -> bool {
    crate::session::kind() == crate::session::Kind::X11
}

pub(crate) fn has_cmd(name: &str) -> bool {
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
/// Grab the screen. When `cursor` is set the pointer comes back separately
/// where the backend allows it (X11, wlroots); other backends paint it in.
pub fn grab(interactive: bool, cursor: bool) -> Result<(Grab, Option<CursorImg>), String> {
    if is_wayland() {
        let mut first_err = None;
        if !interactive {
            match crate::wlr::grab_split(cursor) {
                Ok(r) => return Ok(r),
                Err(e) => first_err = Some(format!("wlr-screencopy: {e}")),
            }
        }
        grab_wayland(interactive, cursor)
            .map(|g| (g, None))
            .map_err(|e| match first_err {
                Some(f) => format!("{f}; {e}"),
                None => e,
            })
    } else {
        let g = grab_x11()?;
        let c = if cursor { x11_cursor().ok() } else { None };
        Ok((g, c))
    }
}

// ---------------------------------------------------------------- X11

pub fn grab_x11() -> Result<Grab, String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};

    let (conn, screen_num) = x11rb::connect(None).map_err(|e| format!("X11 connect: {e}"))?;
    let screen = &conn.setup().roots[screen_num];
    let (w, h) = (screen.width_in_pixels, screen.height_in_pixels);
    let root = screen.root;
    let depth = screen.root_depth;
    let t = std::time::Instant::now();
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
    crate::trace("x11 GetImage", t);
    let t = std::time::Instant::now();
    // channel masks of the root visual: 0xff0000/0xff00/0xff for depth 24,
    // 0x3ff00000/0xffc00/0x3ff for 30-bit deep colour, 565 for depth 16
    let masks = screen
        .allowed_depths
        .iter()
        .flat_map(|d| d.visuals.iter())
        .find(|v| v.visual_id == screen.root_visual)
        .map(|v| [v.red_mask, v.green_mask, v.blue_mask])
        .unwrap_or([0xff0000, 0xff00, 0xff]);
    let (w, h) = (w as i32, h as i32);
    let src_stride = reply.data.len() / h.max(1) as usize;
    let msb_first = conn.setup().image_byte_order == x11rb::protocol::xproto::ImageOrder::MSB_FIRST;
    let data = x11_to_argb(reply.data, (w as usize, h as usize, src_stride), bpp, masks, msb_first)?;
    crate::trace("x11 convert", t);
    Ok(Grab::Pixels { width: w, height: h, stride: w * 4, data })
}

/// Converts a ZPixmap image (16, 24 or 32 bits per pixel, any channel masks)
/// to opaque cairo ARGB32 in native byte order. When the layout already
/// matches (BGRX in memory on a little-endian host, the usual case) the
/// buffer is reused and only alpha is set.
fn x11_to_argb(mut src: Vec<u8>, (w, h, src_stride): (usize, usize, usize), bpp: u8, masks: [u32; 3], msb_first: bool) -> Result<Vec<u8>, String> {
    let bytes = match bpp {
        16 | 24 | 32 => bpp as usize / 8,
        _ => return Err(format!("unsupported X11 pixel format ({bpp} bpp)")),
    };
    if src_stride < w * bytes || src.len() < src_stride * h {
        return Err("short X11 image".into());
    }
    let row = w * 4;
    if bytes == 4 && !msb_first && masks == [0xff0000, 0xff00, 0xff] && cfg!(target_endian = "little") {
        if src_stride != row {
            // drop row padding in place
            for y in 1..h {
                src.copy_within(y * src_stride..y * src_stride + row, y * row);
            }
        }
        src.truncate(row * h);
        for px in src.chunks_exact_mut(4) {
            px[3] = 0xff;
        }
        return Ok(src);
    }
    // (shift, bits) per channel
    let chan = |m: u32| (m.trailing_zeros(), (m >> m.trailing_zeros()).trailing_ones());
    let chans = masks.map(chan);
    let to8 = |px: u32, (shift, bits): (u32, u32)| -> u32 {
        let v = (px >> shift) & ((1u64 << bits) - 1) as u32;
        match bits {
            0 => 0,
            8 => v,
            b if b > 8 => v >> (b - 8),
            b => (v * 255 + ((1 << b) - 1) / 2) / ((1 << b) - 1),
        }
    };
    let mut out = vec![0u8; row * h];
    if bytes == 4 && !msb_first && masks == [0xff0000, 0xff00, 0xff] {
        // BGRX in memory, big-endian host
        for (s, d) in src.chunks(src_stride).zip(out.chunks_exact_mut(row)) {
            for (s, d) in s[..row].chunks_exact(4).zip(d.chunks_exact_mut(4)) {
                let v = 0xff00_0000 | u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                d.copy_from_slice(&v.to_ne_bytes());
            }
        }
        return Ok(out);
    }
    for (srow, drow) in src.chunks(src_stride).zip(out.chunks_exact_mut(row)) {
        for (p, d) in srow[..w * bytes].chunks_exact(bytes).zip(drow.chunks_exact_mut(4)) {
            let px = p.iter().enumerate().fold(0u32, |acc, (i, &b)| {
                let sh = if msb_first { (bytes - 1 - i) * 8 } else { i * 8 };
                acc | (b as u32) << sh
            });
            let v = 0xff00_0000 | to8(px, chans[0]) << 16 | to8(px, chans[1]) << 8 | to8(px, chans[2]);
            d.copy_from_slice(&v.to_ne_bytes());
        }
    }
    Ok(out)
}

/// The current pointer image and position (XFixes).
fn x11_cursor() -> Result<CursorImg, Box<dyn std::error::Error>> {
    use x11rb::protocol::xfixes::ConnectionExt;
    let (conn, _) = x11rb::connect(None)?;
    conn.xfixes_query_version(5, 0)?.reply()?;
    let c = conn.xfixes_get_cursor_image()?.reply()?;
    let mut data = Vec::with_capacity(c.cursor_image.len() * 4);
    for px in &c.cursor_image {
        data.extend_from_slice(&px.to_ne_bytes()); // premultiplied ARGB, same as cairo
    }
    if c.width == 0 || c.height == 0 {
        return Err("no cursor".into());
    }
    Ok(CursorImg {
        x: c.x as i32 - c.xhot as i32,
        y: c.y as i32 - c.yhot as i32,
        w: c.width as i32,
        h: c.height as i32,
        data,
    })
}

// ---------------------------------------------------------------- Wayland

fn grab_wayland(interactive: bool, cursor: bool) -> Result<Grab, String> {
    let wlroots = crate::session::is_wlroots();
    let mut errors = Vec::new();

    if crate::session::desktop_is("cinnamon") && !interactive {
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
    let mut child = c
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", cmd[0]))?;
    // a tool that hangs (e.g. waiting for a dialog nobody sees) must not block captures forever
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() < TOOL_TIMEOUT => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let Some(status) = status else {
        let _ = std::fs::remove_file(&out);
        return Err(format!("{} timed out", cmd[0]));
    };
    if status.success() && out.is_file() {
        Ok(Grab::File(out))
    } else {
        let _ = std::fs::remove_file(&out);
        Err(format!("{} failed", cmd[0]))
    }
}

const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs `f` on a helper thread and stops waiting after `limit`, so a D-Bus
/// peer that never answers cannot block captures forever. A result that
/// arrives too late is handed to `late` (to clean up files).
fn with_timeout<T: Send + 'static>(
    limit: std::time::Duration,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
    late: impl FnOnce(T) + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Err(std::sync::mpsc::SendError(Ok(v))) = tx.send(f()) {
            late(v);
        }
    });
    match rx.recv_timeout(limit) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!("no answer within {} s", limit.as_secs())),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("helper thread failed".into()),
    }
}

/// Cinnamon's own screenshot D-Bus API (org.Cinnamon.Screenshot).
fn cinnamon_screenshot(cursor: bool) -> Result<Grab, String> {
    with_timeout(std::time::Duration::from_secs(10), move || cinnamon_screenshot_impl(cursor), |g| {
        if let Grab::File(p) = g {
            let _ = std::fs::remove_file(p);
        }
    })
}

fn cinnamon_screenshot_impl(cursor: bool) -> Result<Grab, String> {
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
/// Gives up when the portal does not answer (broken or missing backend); the
/// interactive picker gets longer since the user is choosing a window.
fn portal_screenshot(interactive: bool) -> Result<PathBuf, String> {
    let limit = std::time::Duration::from_secs(if interactive { 300 } else { 60 });
    with_timeout(limit, move || portal_screenshot_impl(interactive), |p| {
        let _ = std::fs::remove_file(p);
    })
}

fn portal_screenshot_impl(interactive: bool) -> Result<PathBuf, String> {
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
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
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
    let wm_desktop = atom("_NET_WM_DESKTOP")?;
    let icccm_state = atom("WM_STATE")?;
    let card = |w: u32, a: Atom, ty: AtomEnum| -> Option<u32> {
        conn.get_property(false, w, a, ty, 0, 1).ok()?.reply().ok()?.value32()?.next()
    };
    let current = card(root, atom("_NET_CURRENT_DESKTOP")?, AtomEnum::CARDINAL);
    let screen = &conn.setup().roots[sn];
    let (sw, sh) = (screen.width_in_pixels as i32, screen.height_in_pixels as i32);

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
        // mutter / muffin (GNOME, Cinnamon) keep windows of other workspaces
        // mapped: skip windows that are not on the current one (0xFFFFFFFF =
        // on all workspaces), and iconic ones (ICCCM WM_STATE 3)
        if let (Some(cur), Some(d)) = (current, card(w, wm_desktop, AtomEnum::CARDINAL)) {
            if d != cur && d != 0xFFFF_FFFF {
                continue;
            }
        }
        if conn
            .get_property(false, w, icccm_state, icccm_state, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32()?.next())
            == Some(3)
        {
            continue;
        }
        if let Some(r) = window_rect(&conn, root, w, frame, gtk_frame) {
            // off-screen (e.g. on another viewport)
            if r.0 < sw && r.1 < sh && r.0 + r.2 > 0 && r.1 + r.3 > 0 {
                out.push(r);
            }
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
    // left, right, top, bottom. A window drawing its own decorations has no
    // frame, but xfwm4 still sets _NET_FRAME_EXTENTS (to zeros) on it, so the
    // client-side shadow margins are checked first.
    let nonzero = |e: &Option<Vec<i32>>| e.as_ref().is_some_and(|e| e.iter().any(|&v| v != 0));
    let (gtk_ext, net_ext) = (ext(gtk_frame), ext(frame));
    if nonzero(&gtk_ext) {
        // client-side shadows: shrink
        let e = gtk_ext.unwrap();
        x += e[0];
        y += e[2];
        wd -= e[0] + e[1];
        ht -= e[2] + e[3];
    } else if let Some(e) = net_ext {
        // server-side decorations: grow
        x -= e[0];
        y -= e[2];
        wd += e[0] + e[1];
        ht += e[2] + e[3];
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

#[cfg(test)]
mod tests {
    use super::*;

    fn px(v: &[u8], i: usize) -> u32 {
        u32::from_ne_bytes([v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]])
    }

    fn conv(src: &[u8], w: usize, bpp: u8, masks: [u32; 3], msb: bool) -> Vec<u32> {
        let bytes = bpp as usize / 8;
        x11_to_argb(src.to_vec(), (w, 1, w * bytes), bpp, masks, msb)
            .unwrap()
            .chunks(4)
            .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn x11_pixel_formats() {
        let rgb24 = [0xff0000, 0xff00, 0xff];
        // depth 24 in 32 bpp, little and big endian servers
        assert_eq!(conv(&[0x30, 0x20, 0x10, 0x00], 1, 32, rgb24, false), vec![0xff102030]);
        assert_eq!(conv(&[0x00, 0x10, 0x20, 0x30], 1, 32, rgb24, true), vec![0xff102030]);
        // depth 30: 10 bits per channel
        let v: u32 = 0x3ff << 20 | 0x200 << 10 | 0x004;
        assert_eq!(conv(&v.to_le_bytes(), 1, 32, [0x3ff0_0000, 0xffc00, 0x3ff], false), vec![0xffff8001]);
        // depth 16, 565: white and pure green
        assert_eq!(conv(&[0xff, 0xff, 0xe0, 0x07], 2, 16, [0xf800, 0x7e0, 0x1f], false), vec![0xffffffff, 0xff00ff00]);
        // 24 bpp packed
        assert_eq!(conv(&[0x30, 0x20, 0x10], 1, 24, rgb24, false), vec![0xff102030]);
        assert!(x11_to_argb(vec![0], (1, 1, 1), 8, rgb24, false).is_err());
    }

    #[test]
    fn x11_conversion() {
        // 2x2, optionally padded rows; pixel i = (r, g, b) = (10*i, 20+i, 30+i)
        for &(big, pad) in &[(false, 0usize), (false, 4), (true, 0), (true, 4)] {
            let stride = 8 + pad;
            let mut src = vec![0u8; stride * 2];
            for i in 0..4 {
                let (r, g, b) = (10 * i as u8, 20 + i as u8, 30 + i as u8);
                let o = (i / 2) * stride + (i % 2) * 4;
                let bytes = if big { [0x55, r, g, b] } else { [b, g, r, 0x55] };
                src[o..o + 4].copy_from_slice(&bytes);
            }
            let out = x11_to_argb(src, (2, 2, stride), 32, [0xff0000, 0xff00, 0xff], big).unwrap();
            assert_eq!(out.len(), 16);
            for i in 0..4u32 {
                assert_eq!(px(&out, i as usize), 0xff00_0000 | (10 * i) << 16 | (20 + i) << 8 | (30 + i), "big={big} pad={pad}");
            }
        }
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("/tmp/a%20b%C3%A9.png"), "/tmp/a bé.png");
        assert_eq!(percent_decode("/x%2"), "/x%2");
        assert_eq!(percent_decode("/%é/%zz"), "/%é/%zz");
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn with_timeout_returns_and_cleans_up_late_results() {
        assert_eq!(with_timeout(Duration::from_secs(5), || Ok(7), |_| {}), Ok(7));
        assert!(with_timeout(Duration::from_secs(5), || Err::<i32, _>("x".into()), |_| {}).is_err());
        let (tx, rx) = std::sync::mpsc::channel();
        let r = with_timeout(
            Duration::from_millis(20),
            || {
                std::thread::sleep(Duration::from_millis(200));
                Ok(42)
            },
            move |v| tx.send(v).unwrap(),
        );
        assert!(r.unwrap_err().contains("no answer"));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(42));
    }
}
