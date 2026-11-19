// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Native screen capture on wlroots compositors (Wayfire, Sway, Hyprland,
//! labwc, river, ...) through zwlr_screencopy_manager_v1. All outputs are
//! composited into one image laid out by their xdg-output logical positions.
use crate::capture::Grab;
use std::os::fd::AsFd;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1};

#[derive(Default, Clone, Copy)]
struct Logical {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Default)]
struct Frame {
    format: Option<(wl_shm::Format, u32, u32, u32)>,
    buffer_done: bool,
    /// a wl_shm buffer was offered (readable or not)
    offered: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct State {
    logical: Vec<Logical>,
    transforms: Vec<wl_output::Transform>,
    frame: Frame,
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(s: &mut Self, _: &wl_output::WlOutput, ev: wl_output::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Geometry { transform: WEnum::Value(t), .. } = ev {
            s.transforms[*idx] = t;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, usize> for State {
    fn event(s: &mut Self, _: &zxdg_output_v1::ZxdgOutputV1, ev: zxdg_output_v1::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let l = &mut s.logical[*idx];
        match ev {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                l.x = x;
                l.y = y;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                l.w = width;
                l.h = height;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        s: &mut Self,
        _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        ev: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        match ev {
            Event::Buffer { format, width, height, stride } => {
                s.frame.offered = true;
                // several formats may be offered: take the first readable one,
                // but prefer plain 8-bit BGRA
                if let WEnum::Value(f) = format {
                    let better = match s.frame.format {
                        None => true,
                        Some((cur, ..)) => rank(f) < rank(cur),
                    };
                    if rank(f) < u8::MAX && better {
                        s.frame.format = Some((f, width, height, stride));
                    }
                }
            }
            Event::BufferDone => s.frame.buffer_done = true,
            Event::Flags { flags: WEnum::Value(f) } => {
                s.frame.y_invert = f.contains(zwlr_screencopy_frame_v1::Flags::YInvert)
            }
            Event::Ready { .. } => s.frame.ready = true,
            Event::Failed => s.frame.failed = true,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1);
delegate_noop!(State: ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);

/// Bytes per pixel of the wl_shm formats we can read, None otherwise.
fn bytes_per_pixel(f: wl_shm::Format) -> Option<usize> {
    use wl_shm::Format::*;
    match f {
        Argb8888 | Xrgb8888 | Abgr8888 | Xbgr8888 | Rgba8888 | Rgbx8888 | Bgra8888 | Bgrx8888 => Some(4),
        Argb2101010 | Xrgb2101010 | Abgr2101010 | Xbgr2101010 => Some(4),
        Rgb888 | Bgr888 => Some(3),
        Rgb565 | Bgr565 => Some(2),
        _ => None,
    }
}

/// Preference among readable formats (lower is better), u8::MAX if unreadable.
fn rank(f: wl_shm::Format) -> u8 {
    use wl_shm::Format::*;
    match f {
        Xrgb8888 | Argb8888 => 0,
        _ if bytes_per_pixel(f).is_some() => 1,
        _ => u8::MAX,
    }
}

/// Opaque 0xffRRGGBB from one pixel. wl_shm formats are little endian by
/// definition, whatever the host byte order.
fn decode(f: wl_shm::Format, p: &[u8]) -> u32 {
    use wl_shm::Format::*;
    let rgb = |r: u32, g: u32, b: u32| 0xff00_0000 | r << 16 | g << 8 | b;
    let ten = |v: u32| v >> 2; // 10 -> 8 bit
    match f {
        Argb8888 | Xrgb8888 => rgb(p[2] as u32, p[1] as u32, p[0] as u32),
        Abgr8888 | Xbgr8888 => rgb(p[0] as u32, p[1] as u32, p[2] as u32),
        Rgba8888 | Rgbx8888 => rgb(p[3] as u32, p[2] as u32, p[1] as u32),
        Bgra8888 | Bgrx8888 => rgb(p[1] as u32, p[2] as u32, p[3] as u32),
        Argb2101010 | Xrgb2101010 => {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            rgb(ten(v >> 20 & 0x3ff), ten(v >> 10 & 0x3ff), ten(v & 0x3ff))
        }
        Abgr2101010 | Xbgr2101010 => {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            rgb(ten(v & 0x3ff), ten(v >> 10 & 0x3ff), ten(v >> 20 & 0x3ff))
        }
        Rgb888 => rgb(p[2] as u32, p[1] as u32, p[0] as u32),
        Bgr888 => rgb(p[0] as u32, p[1] as u32, p[2] as u32),
        Rgb565 | Bgr565 => {
            let v = u16::from_le_bytes([p[0], p[1]]) as u32;
            let (hi, g, lo) = (v >> 11 & 0x1f, v >> 5 & 0x3f, v & 0x1f);
            let (hi, g, lo) = (hi << 3 | hi >> 2, g << 2 | g >> 4, lo << 3 | lo >> 2);
            if f == Rgb565 { rgb(hi, g, lo) } else { rgb(lo, g, hi) }
        }
        _ => 0xff00_0000,
    }
}

/// Converts a screencopy buffer to upright BGRA (cairo ARGB32, native endian,
/// stride = width * 4). The buffer is in the output's hardware orientation:
/// the compositor rendered it with the output transform applied, so undo that
/// (after undoing a y-inverted copy). Returns (width, height, data).
fn to_upright(
    src: &[u8],
    fmt: wl_shm::Format,
    (bw, bh, stride): (usize, usize, usize),
    y_invert: bool,
    t: wl_output::Transform,
) -> (usize, usize, Vec<u8>) {
    use wl_output::Transform::*;
    let bpp = bytes_per_pixel(fmt).unwrap_or(4);
    // counter-clockwise quarter turns that undo `t` (wl_output transforms are
    // counter-clockwise; the flipped ones are their own inverse)
    let (flip, quarter) = match t {
        Normal => (false, 0),
        _90 => (false, 3),
        _180 => (false, 2),
        _270 => (false, 1),
        Flipped => (true, 0),
        Flipped90 => (true, 1),
        Flipped180 => (true, 2),
        Flipped270 => (true, 3),
        _ => (false, 0),
    };
    let (w, h) = if quarter % 2 == 1 { (bh, bw) } else { (bw, bh) };
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            // rotate counter-clockwise by quarter turns, then mirror
            let (mut sx, sy) = match quarter {
                1 => (bw - 1 - y, x),
                2 => (bw - 1 - x, bh - 1 - y),
                3 => (y, bh - 1 - x),
                _ => (x, y),
            };
            if flip {
                sx = bw - 1 - sx;
            }
            let sy = if y_invert { bh - 1 - sy } else { sy };
            let o = sy * stride + sx * bpp;
            let px = decode(fmt, &src[o..o + bpp]);
            out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&px.to_ne_bytes());
        }
    }
    (w, h, out)
}

/// Pixel layout of a captured frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Px {
    /// (A|X)RGB8888: B G R X in memory
    Bgrx,
    /// (A|X)BGR8888: R G B X in memory
    Rgbx,
    /// already converted: opaque cairo ARGB32 in native byte order
    Native,
}

enum Buf {
    Map(memmap2::MmapMut),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Buf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Buf::Map(m) => m,
            Buf::Owned(v) => v,
        }
    }
}

/// One output's frame, upright: either the compositor's shared memory as
/// delivered (plain 8-bit formats, untransformed outputs: the fast path) or
/// already converted (other formats, rotated / flipped outputs).
struct Raw {
    l: Logical,
    w: usize,
    h: usize,
    stride: usize,
    px: Px,
    y_invert: bool,
    data: Buf,
}

impl Raw {
    /// Source row for output row `y` (handles y-inverted frames).
    fn row(&self, y: usize) -> &[u8] {
        let sy = if self.y_invert { self.h - 1 - y } else { y };
        &self.data[sy * self.stride..sy * self.stride + self.w * 4]
    }
}

struct Shot {
    l: Logical,
    w: i32,
    h: i32,
    data: Vec<u8>, // BGRA, opaque, stride = w*4
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;
type Cursor = crate::capture::CursorImg;

/// Captures without the pointer, and (if asked) a second frame with it; the
/// difference between the two is the pointer, returned as a separate sprite.
///
/// Both captures share one connection. When all outputs map 1:1 onto the
/// combined image (same scale, the usual case) the pointer is found by
/// comparing the frames per output and the frames are converted straight
/// into the final buffer: no per-output copies, no cairo pass, and the
/// pointer frame is never converted at all.
pub fn grab_split(cursor: bool) -> Result<(Grab, Option<Cursor>), String> {
    let t = std::time::Instant::now();
    let mut s = Session::connect().map_err(|e| e.to_string())?;
    let base = s.capture_all(false).map_err(|e| e.to_string())?;
    let with = if cursor { Some(s.capture_all(true).map_err(|e| e.to_string())?) } else { None };
    crate::trace("wlr capture", t);
    let t = std::time::Instant::now();
    let r = match Layout::of(&base) {
        Some(lay) if lay.direct => Ok(direct(&lay, base, with)),
        _ => {
            // mixed output scales: scale with cairo, find the pointer in the result
            let base = composite(base.iter().map(to_shot).collect());
            match with.map(|w| composite(w.iter().map(to_shot).collect())) {
                None => Ok((base, None)),
                Some(with) => match extract_cursor(&base, &with) {
                    Some(c) => Ok((base, Some(c))),
                    // content changed between frames: keep the pointer painted in
                    None if differs(&base, &with) => Ok((with, None)),
                    None => Ok((base, None)),
                },
            }
        }
    };
    crate::trace("wlr convert", t);
    r
}

/// Where each output goes in the combined image.
struct Layout {
    width: usize,
    height: usize,
    /// output origins in the combined image (pixels)
    origin: Vec<(usize, usize)>,
    /// every output lands 1:1 at integer positions inside the image
    direct: bool,
}

impl Layout {
    fn of(raws: &[Raw]) -> Option<Layout> {
        let minx = raws.iter().map(|r| r.l.x).min()?;
        let miny = raws.iter().map(|r| r.l.y).min()?;
        let maxx = raws.iter().map(|r| r.l.x + r.l.w).max()?;
        let maxy = raws.iter().map(|r| r.l.y + r.l.h).max()?;
        let scale = raws.iter().map(|r| r.w as f64 / r.l.w as f64).fold(1.0, f64::max);
        let width = ((maxx - minx) as f64 * scale).ceil() as usize;
        let height = ((maxy - miny) as f64 * scale).ceil() as usize;
        let mut direct = true;
        let mut origin = Vec::new();
        for r in raws {
            let (ox, oy) = ((r.l.x - minx) as f64 * scale, (r.l.y - miny) as f64 * scale);
            let one_to_one = (r.w as f64 - r.l.w as f64 * scale).abs() < 0.01 && (r.h as f64 - r.l.h as f64 * scale).abs() < 0.01;
            let whole = ox.fract().abs() < 1e-6 && oy.fract().abs() < 1e-6;
            let (ox, oy) = (ox.round().max(0.0) as usize, oy.round().max(0.0) as usize);
            direct &= one_to_one && whole && ox + r.w <= width && oy + r.h <= height;
            origin.push((ox, oy));
        }
        Some(Layout { width, height, origin, direct })
    }
}

/// Opaque 0xffRRGGBB of one pixel.
fn to_argb(p: &[u8], px: Px) -> u32 {
    match px {
        Px::Bgrx => 0xff00_0000 | (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32,
        Px::Rgbx => 0xff00_0000 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32,
        Px::Native => 0xff00_0000 | u32::from_ne_bytes([p[0], p[1], p[2], p[3]]),
    }
}

/// Converts one row into opaque cairo ARGB32.
fn convert_row(src: &[u8], dst: &mut [u8], px: Px) {
    let native_bgrx = px == Px::Bgrx && cfg!(target_endian = "little");
    if native_bgrx || px == Px::Native {
        dst.copy_from_slice(src);
        if native_bgrx {
            for p in dst.chunks_exact_mut(4) {
                p[3] = 0xff;
            }
        }
    } else {
        for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            d.copy_from_slice(&to_argb(s, px).to_ne_bytes());
        }
    }
}

/// Whether two pixels differ in colour (the X byte of xRGB formats may be anything).
fn px_differs(p: &[u8], q: &[u8], px: Px) -> bool {
    match px {
        Px::Native => p != q,
        _ => p[..3] != q[..3],
    }
}

/// Bounding box (x0, y0, x1, y1 inclusive, in output rows) of pixels that differ.
fn diff_box(a: &Raw, b: &Raw) -> Option<(usize, usize, usize, usize)> {
    let mut bbox: Option<(usize, usize, usize, usize)> = None;
    for y in 0..a.h {
        let (ra, rb) = (a.row(y), b.row(y));
        if ra == rb {
            continue;
        }
        let first = ra.chunks_exact(4).zip(rb.chunks_exact(4)).position(|(p, q)| px_differs(p, q, a.px));
        let last = ra.chunks_exact(4).zip(rb.chunks_exact(4)).rposition(|(p, q)| px_differs(p, q, a.px));
        if let (Some(x0), Some(x1)) = (first, last) {
            bbox = Some(match bbox {
                None => (x0, y, x1, y),
                Some((bx0, by0, bx1, _)) => (bx0.min(x0), by0, bx1.max(x1), y),
            });
        }
    }
    bbox
}

/// The pointer sprite: pixels of `with` that differ from `base` inside the box.
fn sprite(base: &Raw, with: &Raw, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> Vec<u8> {
    let mut data = Vec::with_capacity((x1 - x0 + 1) * (y1 - y0 + 1) * 4);
    for y in y0..=y1 {
        let (ra, rb) = (base.row(y), with.row(y));
        for x in x0..=x1 {
            let (p, q) = (&ra[x * 4..x * 4 + 4], &rb[x * 4..x * 4 + 4]);
            let v = if px_differs(p, q, with.px) { to_argb(q, with.px) } else { 0u32 };
            data.extend_from_slice(&v.to_ne_bytes());
        }
    }
    data
}

fn direct(lay: &Layout, base: Vec<Raw>, with: Option<Vec<Raw>>) -> (Grab, Option<Cursor>) {
    let mut pointer = None;
    let mut use_with = false;
    if let Some(with) = &with {
        let boxes: Vec<_> = base
            .iter()
            .zip(with)
            .enumerate()
            .filter(|(_, (a, b))| (a.w, a.h, a.px) == (b.w, b.h, b.px))
            .filter_map(|(i, (a, b))| diff_box(a, b).map(|bb| (i, bb)))
            .collect();
        match boxes.as_slice() {
            [] => {} // pointer hidden
            [(i, bb)] if bb.2 - bb.0 < 256 && bb.3 - bb.1 < 256 => {
                let (ox, oy) = lay.origin[*i];
                pointer = Some(Cursor {
                    x: (ox + bb.0) as i32,
                    y: (oy + bb.1) as i32,
                    w: (bb.2 - bb.0 + 1) as i32,
                    h: (bb.3 - bb.1 + 1) as i32,
                    data: sprite(&base[*i], &with[*i], *bb),
                });
            }
            // something else changed on screen (or the pointer spans outputs):
            // keep the pointer painted in
            _ => use_with = true,
        }
    }
    let frames = match with {
        Some(w) if use_with => w,
        _ => base,
    };
    let row = lay.width * 4;
    let mut data = vec![0u8; row * lay.height];
    let covered: usize = frames.iter().map(|r| r.w * r.h).sum();
    if covered < lay.width * lay.height {
        // areas no output covers (gaps between outputs) are opaque black
        let black = 0xff00_0000u32.to_ne_bytes();
        for px in data.chunks_exact_mut(4) {
            px.copy_from_slice(&black);
        }
    }
    for (r, &(ox, oy)) in frames.iter().zip(&lay.origin) {
        for y in 0..r.h {
            let d = (oy + y) * row + ox * 4;
            convert_row(r.row(y), &mut data[d..d + r.w * 4], r.px);
        }
    }
    let grab = Grab::Pixels { width: lay.width as i32, height: lay.height as i32, stride: row as i32, data };
    (grab, pointer)
}

fn to_shot(r: &Raw) -> Shot {
    let mut data = vec![0u8; r.w * r.h * 4];
    for (y, d) in data.chunks_exact_mut(r.w * 4).enumerate() {
        convert_row(r.row(y), d, r.px);
    }
    Shot { l: r.l, w: r.w as i32, h: r.h as i32, data }
}

fn pixels(g: &Grab) -> Option<(i32, i32, i32, &[u8])> {
    match g {
        Grab::Pixels { width, height, stride, data } => Some((*width, *height, *stride, data)),
        _ => None,
    }
}

fn differs(a: &Grab, b: &Grab) -> bool {
    match (pixels(a), pixels(b)) {
        (Some(x), Some(y)) => x.3 != y.3,
        _ => false,
    }
}

fn extract_cursor(base: &Grab, with: &Grab) -> Option<Cursor> {
    let (w, h, stride, a) = pixels(base)?;
    let (w2, h2, stride2, b) = pixels(with)?;
    if (w, h, stride) != (w2, h2, stride2) {
        return None;
    }
    let px = |d: &[u8], x: i32, y: i32| -> u32 {
        let o = (y * stride + x * 4) as usize;
        u32::from_ne_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
    };
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, -1, -1);
    for y in 0..h {
        let o = (y * stride) as usize;
        if a[o..o + (w * 4) as usize] == b[o..o + (w * 4) as usize] {
            continue;
        }
        for x in 0..w {
            if px(a, x, y) != px(b, x, y) {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < 0 {
        return None; // pointer hidden
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    if cw > 256 || ch > 256 {
        return None; // something else changed on screen
    }
    let mut data = Vec::with_capacity((cw * ch * 4) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (pa, pb) = (px(a, x, y), px(b, x, y));
            let v = if pa == pb { 0u32 } else { pb | 0xff00_0000 };
            data.extend_from_slice(&v.to_ne_bytes());
        }
    }
    Some(Cursor { x: x0, y: y0, w: cw, h: ch, data })
}

struct Session {
    queue: wayland_client::EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    shm: wl_shm::WlShm,
    mgr: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    outputs: Vec<wl_output::WlOutput>,
    _conn: Connection,
}

impl Session {
    fn connect() -> Res<Session> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let mgr: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 = globals
            .bind(&qh, 1..=3, ())
            .map_err(|_| "compositor has no wlr-screencopy support")?;
        let xdg: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1> = globals.bind(&qh, 2..=3, ()).ok();

        let outputs: Vec<wl_output::WlOutput> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .enumerate()
                .map(|(i, g)| globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, g.version.min(4), &qh, i))
                .collect()
        });
        if outputs.is_empty() {
            return Err("no outputs".into());
        }
        let mut state = State {
            logical: vec![Logical::default(); outputs.len()],
            transforms: vec![wl_output::Transform::Normal; outputs.len()],
            ..Default::default()
        };
        if let Some(x) = &xdg {
            for (i, o) in outputs.iter().enumerate() {
                x.get_xdg_output(o, &qh, i);
            }
        }
        queue.roundtrip(&mut state)?;
        Ok(Session { queue, qh, state, shm, mgr, outputs, _conn: conn })
    }

    fn capture_all(&mut self, cursor: bool) -> Res<Vec<Raw>> {
        let mut raws = Vec::new();
        let mut next_x = 0;
        for i in 0..self.outputs.len() {
            let mut r = self.capture(i, cursor)?;
            if r.l.w <= 0 || r.l.h <= 0 {
                // no xdg-output: lay outputs out left to right at buffer size
                r.l = Logical { x: next_x, y: 0, w: r.w as i32, h: r.h as i32 };
            }
            next_x = next_x.max(r.l.x + r.l.w);
            raws.push(r);
        }
        Ok(raws)
    }

    fn capture(&mut self, i: usize, cursor: bool) -> Res<Raw> {
        let (queue, state, qh) = (&mut self.queue, &mut self.state, &self.qh);
        state.frame = Frame::default();
        let frame = self.mgr.capture_output(cursor as i32, &self.outputs[i], qh, ());
        // v3 lists every buffer type, then buffer_done; older versions send
        // a single wl_shm offer
        while !state.frame.failed && !state.frame.buffer_done && !(state.frame.offered && self.mgr.version() < 3) {
            queue.blocking_dispatch(state)?;
        }
        if state.frame.failed {
            return Err("screencopy failed".into());
        }
        let (fmt, w, h, stride) = state.frame.format.ok_or("no usable buffer format")?;
        let bpp = bytes_per_pixel(fmt).ok_or("no usable buffer format")?;
        if w == 0 || h == 0 || (stride as usize) < w as usize * bpp {
            return Err("bad screencopy buffer size".into());
        }
        let size = (stride * h) as usize;

        let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("linshot-shm-{}-{i}-{}", std::process::id(), cursor as u8));
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path)?;
        let _ = std::fs::remove_file(&path);
        file.set_len(size as u64)?;
        // populate: map all pages up front instead of faulting them in while converting
        let map = unsafe { memmap2::MmapOptions::new().populate().map_mut(&file)? };
        let pool = self.shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, fmt, qh, ());
        frame.copy(&buffer);
        while !state.frame.ready && !state.frame.failed {
            queue.blocking_dispatch(state)?;
        }
        buffer.destroy();
        pool.destroy();
        frame.destroy();
        if state.frame.failed {
            return Err("screencopy copy failed".into());
        }
        let l = state.logical[i];
        let transform = state.transforms[i];
        let (w, h, stride) = (w as usize, h as usize, stride as usize);
        use wl_shm::Format::*;
        let plain = match fmt {
            Argb8888 | Xrgb8888 => Some(Px::Bgrx),
            Abgr8888 | Xbgr8888 => Some(Px::Rgbx),
            _ => None,
        };
        Ok(match plain {
            // fast path: keep the shared memory, convert later straight into the result
            Some(px) if transform == wl_output::Transform::Normal => {
                Raw { l, w, h, stride, px, y_invert: state.frame.y_invert, data: Buf::Map(map) }
            }
            // rotated / flipped output or another pixel format: decode and turn upright now
            _ => {
                let (w, h, data) = to_upright(&map, fmt, (w, h, stride), state.frame.y_invert, transform);
                Raw { l, w, h, stride: w * 4, px: Px::Native, y_invert: false, data: Buf::Owned(data) }
            }
        })
    }
}

fn composite(shots: Vec<Shot>) -> Grab {
    if shots.len() == 1 {
        let s = shots.into_iter().next().unwrap();
        return Grab::Pixels { width: s.w, height: s.h, stride: s.w * 4, data: s.data };
    }
    let minx = shots.iter().map(|s| s.l.x).min().unwrap_or(0);
    let miny = shots.iter().map(|s| s.l.y).min().unwrap_or(0);
    let maxx = shots.iter().map(|s| s.l.x + s.l.w).max().unwrap_or(1);
    let maxy = shots.iter().map(|s| s.l.y + s.l.h).max().unwrap_or(1);
    let scale = shots.iter().map(|s| s.w as f64 / s.l.w as f64).fold(1.0, f64::max);
    let width = ((maxx - minx) as f64 * scale).ceil() as i32;
    let height = ((maxy - miny) as f64 * scale).ceil() as i32;
    let Ok(out) = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height) else {
        // too large for cairo: return the first output only rather than nothing
        let s = shots.into_iter().next().unwrap();
        return Grab::Pixels { width: s.w, height: s.h, stride: s.w * 4, data: s.data };
    };
    if let Ok(cr) = cairo::Context::new(&out) {
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.paint().ok();
        for s in shots {
            let Ok(src) = cairo::ImageSurface::create_for_data(s.data, cairo::Format::ARgb32, s.w, s.h, s.w * 4) else { continue };
            cr.save().ok();
            cr.translate((s.l.x - minx) as f64 * scale, (s.l.y - miny) as f64 * scale);
            cr.scale(s.l.w as f64 * scale / s.w as f64, s.l.h as f64 * scale / s.h as f64);
            cr.set_source_surface(&src, 0.0, 0.0).ok();
            cr.paint().ok();
            cr.restore().ok();
        }
    }
    out.flush();
    let stride = out.stride();
    let mut data = Vec::new();
    let _ = out.with_data(|d| data.extend_from_slice(d));
    Grab::Pixels { width, height, stride, data }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wl_output::Transform;
    use wl_shm::Format;

    const R: u32 = 0xffff0000;
    const G: u32 = 0xff00ff00;
    const B: u32 = 0xff0000ff;
    const W: u32 = 0xffffffff;

    /// XRGB8888 buffer from rows of 0xffRRGGBB pixels.
    fn buf(rows: &[&[u32]]) -> (Vec<u8>, (usize, usize, usize)) {
        let (w, h) = (rows[0].len(), rows.len());
        let data = rows.iter().flat_map(|r| r.iter().flat_map(|p| p.to_le_bytes())).collect();
        (data, (w, h, w * 4))
    }

    fn upright(rows: &[&[u32]], y_invert: bool, t: Transform) -> (usize, usize, Vec<u32>) {
        let (data, geom) = buf(rows);
        let (w, h, out) = to_upright(&data, Format::Xrgb8888, geom, y_invert, t);
        (w, h, out.chunks(4).map(|c| u32::from_ne_bytes(c.try_into().unwrap())).collect())
    }

    #[test]
    fn transforms() {
        // what sway (headless, pixman) hands out for a screen showing
        // R G / B W with `transform 90` (clockwise in sway's config, which
        // is wl_output 270) and `transform 270` (wl_output 90)
        assert_eq!(upright(&[&[B, R], &[W, G]], false, Transform::_270), (2, 2, vec![R, G, B, W]));
        assert_eq!(upright(&[&[G, W], &[R, B]], false, Transform::_90), (2, 2, vec![R, G, B, W]));
        assert_eq!(upright(&[&[W, B], &[G, R]], false, Transform::_180), (2, 2, vec![R, G, B, W]));
        assert_eq!(upright(&[&[G, R], &[W, B]], false, Transform::Flipped), (2, 2, vec![R, G, B, W]));
        assert_eq!(upright(&[&[B, W], &[R, G]], true, Transform::Normal), (2, 2, vec![R, G, B, W]));
        // non-square: a 3x1 buffer on a portrait output becomes 1x3
        assert_eq!(upright(&[&[R, G, B]], false, Transform::_270), (1, 3, vec![B, G, R]));
        assert_eq!(upright(&[&[R, G, B]], false, Transform::_90), (1, 3, vec![R, G, B]));
        // flipped 90 / 270 are a transpose / anti-transpose
        assert_eq!(upright(&[&[R, G], &[B, W]], false, Transform::Flipped90), (2, 2, vec![R, B, G, W]));
        assert_eq!(upright(&[&[R, G], &[B, W]], false, Transform::Flipped270), (2, 2, vec![W, G, B, R]));
        // every flipped transform is its own inverse
        for t in [Transform::Flipped90, Transform::Flipped180, Transform::Flipped270] {
            let (w, h, once) = upright(&[&[R, G, B], &[W, R, G]], false, t);
            let rows: Vec<&[u32]> = once.chunks(w).collect();
            assert_eq!(rows.len(), h);
            assert_eq!(upright(&rows, false, t), (3, 2, vec![R, G, B, W, R, G]), "{t:?}");
        }
    }

    #[test]
    fn formats() {
        let px = |f, b: &[u8]| decode(f, b);
        assert_eq!(px(Format::Xrgb8888, &[0x30, 0x20, 0x10, 0]), 0xff102030);
        assert_eq!(px(Format::Abgr8888, &[0x10, 0x20, 0x30, 0x80]), 0xff102030);
        assert_eq!(px(Format::Rgbx8888, &[0, 0x30, 0x20, 0x10]), 0xff102030);
        assert_eq!(px(Format::Bgrx8888, &[0, 0x10, 0x20, 0x30]), 0xff102030);
        assert_eq!(px(Format::Rgb888, &[0x30, 0x20, 0x10]), 0xff102030);
        assert_eq!(px(Format::Bgr888, &[0x10, 0x20, 0x30]), 0xff102030);
        // 10 bit: R = 0x3ff, G = 0x200, B = 0x004
        let v: u32 = 0x3ff << 20 | 0x200 << 10 | 0x004;
        assert_eq!(px(Format::Xrgb2101010, &v.to_le_bytes()), 0xffff8001);
        assert_eq!(px(Format::Xbgr2101010, &v.to_le_bytes()), 0xff0180ff);
        // 565: pure red / pure blue
        assert_eq!(px(Format::Rgb565, &0xf800u16.to_le_bytes()), 0xffff0000);
        assert_eq!(px(Format::Bgr565, &0xf800u16.to_le_bytes()), 0xff0000ff);
        assert!(rank(Format::Xrgb8888) < rank(Format::Xrgb2101010));
        assert_eq!(rank(Format::Yuyv), u8::MAX);
        // the fast path's per-pixel decoding agrees with `decode`
        assert_eq!(to_argb(&[0x30, 0x20, 0x10, 0x55], Px::Bgrx), px(Format::Xrgb8888, &[0x30, 0x20, 0x10, 0]));
        assert_eq!(to_argb(&[0x10, 0x20, 0x30, 0x55], Px::Rgbx), px(Format::Xbgr8888, &[0x10, 0x20, 0x30, 0]));
    }

    /// A fake output frame: shared-memory layout (B G R X, or R G B X), or
    /// already converted when `px` is Native.
    fn raw(w: usize, h: usize, l: Logical, px: Px, y_invert: bool, f: impl Fn(usize, usize) -> (u8, u8, u8)) -> Raw {
        let stride = w * 4 + 16;
        let mut data = vec![0u8; stride * h];
        for y in 0..h {
            let sy = if y_invert { h - 1 - y } else { y };
            for x in 0..w {
                let (r, g, b) = f(x, y);
                let p = match px {
                    Px::Bgrx => [b, g, r, 0],
                    Px::Rgbx => [r, g, b, 0],
                    Px::Native => (0xff00_0000u32 | (r as u32) << 16 | (g as u32) << 8 | b as u32).to_ne_bytes(),
                };
                data[sy * stride + x * 4..sy * stride + x * 4 + 4].copy_from_slice(&p);
            }
        }
        Raw { l, w, h, stride, px, y_invert, data: Buf::Owned(data) }
    }

    fn at(g: &Grab, x: usize, y: usize) -> u32 {
        let (_, _, stride, d) = pixels(g).unwrap();
        let o = y * stride as usize + x * 4;
        u32::from_ne_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
    }

    fn bg(x: usize, y: usize) -> (u8, u8, u8) {
        (x as u8, y as u8, 7)
    }

    #[test]
    fn direct_layout_conversion_and_pointer() {
        let l1 = Logical { x: 0, y: 0, w: 40, h: 30 };
        let l2 = Logical { x: 40, y: 10, w: 20, h: 20 };
        let l3 = Logical { x: 60, y: 0, w: 10, h: 10 };
        let base = vec![raw(40, 30, l1, Px::Bgrx, false, bg), raw(20, 20, l2, Px::Rgbx, true, bg), raw(10, 10, l3, Px::Native, false, bg)];
        // pointer: a 3x2 white block on the second output at (5, 6)
        let ptr = |x: usize, y: usize| if (5..8).contains(&x) && (6..8).contains(&y) { (255, 255, 255) } else { bg(x, y) };
        let with = vec![raw(40, 30, l1, Px::Bgrx, false, bg), raw(20, 20, l2, Px::Rgbx, true, ptr), raw(10, 10, l3, Px::Native, false, bg)];
        let lay = Layout::of(&base).unwrap();
        assert!(lay.direct);
        assert_eq!((lay.width, lay.height), (70, 30));
        let (g, c) = direct(&lay, base, Some(with));
        // first output as is, second at (40, 10), y-inverted and swizzled correctly, third converted
        assert_eq!(at(&g, 3, 4), 0xff03_0407);
        assert_eq!(at(&g, 40 + 2, 10 + 5), 0xff02_0507);
        assert_eq!(at(&g, 60 + 9, 8), 0xff09_0807);
        // the gap above the second output is opaque black
        assert_eq!(at(&g, 45, 2), 0xff00_0000);
        let c = c.expect("pointer found");
        assert_eq!((c.x, c.y, c.w, c.h), (45, 16, 3, 2));
        assert!(c.data.chunks_exact(4).all(|p| u32::from_ne_bytes([p[0], p[1], p[2], p[3]]) == 0xffff_ffff));
        // the base image has no pointer in it
        assert_eq!(at(&g, 45, 16), 0xff05_0607);
    }

    #[test]
    fn pointer_on_converted_output() {
        let l = Logical { x: 0, y: 0, w: 20, h: 20 };
        let base = vec![raw(20, 20, l, Px::Native, false, bg)];
        let with = vec![raw(20, 20, l, Px::Native, false, |x, y| if x == 3 && y == 4 { (9, 9, 9) } else { bg(x, y) })];
        let (_, c) = direct(&Layout::of(&base).unwrap(), base, Some(with));
        let c = c.expect("pointer found");
        assert_eq!((c.x, c.y, c.w, c.h), (3, 4, 1, 1));
        assert_eq!(u32::from_ne_bytes([c.data[0], c.data[1], c.data[2], c.data[3]]), 0xff09_0909);
    }

    #[test]
    fn large_change_keeps_pointer_painted_in() {
        let l = Logical { x: 0, y: 0, w: 300, h: 10 };
        let base = vec![raw(300, 10, l, Px::Bgrx, false, bg)];
        let with = vec![raw(300, 10, l, Px::Bgrx, false, |x, y| if x == 0 || x == 299 { (1, 1, 1) } else { bg(x, y) })];
        let lay = Layout::of(&base).unwrap();
        let (g, c) = direct(&lay, base, Some(with));
        assert!(c.is_none());
        assert_eq!(at(&g, 0, 0), 0xff01_0101);
    }

    #[test]
    fn mixed_scales_are_not_direct() {
        // a 2x output next to a 1x output
        let base = vec![
            raw(80, 60, Logical { x: 0, y: 0, w: 40, h: 30 }, Px::Bgrx, false, bg),
            raw(40, 30, Logical { x: 40, y: 0, w: 40, h: 30 }, Px::Bgrx, false, bg),
        ];
        assert!(!Layout::of(&base).unwrap().direct);
    }
}
