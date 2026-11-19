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
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct State {
    logical: Vec<Logical>,
    frame: Frame,
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
            Event::Buffer { format: WEnum::Value(f), width, height, stride } => {
                if s.frame.format.is_none() && swizzle(f).is_some() {
                    s.frame.format = Some((f, width, height, stride));
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

delegate_noop!(State: ignore wl_output::WlOutput);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1);
delegate_noop!(State: ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);

/// Some(swap_red_blue) for the 32-bit formats we can read.
fn swizzle(f: wl_shm::Format) -> Option<bool> {
    use wl_shm::Format::*;
    match f {
        Argb8888 | Xrgb8888 => Some(false),
        Abgr8888 | Xbgr8888 => Some(true),
        _ => None,
    }
}

struct Shot {
    l: Logical,
    w: i32,
    h: i32,
    data: Vec<u8>, // BGRA, opaque, stride = w*4
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Captures without the pointer, and (if asked) a second frame with it; the
/// difference between the two is the pointer, returned as a separate sprite.
pub fn grab_split(cursor: bool) -> Result<(Grab, Option<crate::capture::CursorImg>), String> {
    let base = grab_impl(false).map_err(|e| e.to_string())?;
    if !cursor {
        return Ok((base, None));
    }
    let with = grab_impl(true).map_err(|e| e.to_string())?;
    match extract_cursor(&base, &with) {
        Some(c) => Ok((base, Some(c))),
        // content changed between frames: keep the pointer painted in
        None if differs(&base, &with) => Ok((with, None)),
        None => Ok((base, None)),
    }
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

fn extract_cursor(base: &Grab, with: &Grab) -> Option<crate::capture::CursorImg> {
    let (w, h, stride, a) = pixels(base)?;
    let (w2, h2, stride2, b) = pixels(with)?;
    if (w, h, stride) != (w2, h2, stride2) {
        return None;
    }
    let px = |d: &[u8], x: i32, y: i32| -> u32 {
        let o = (y * stride + x * 4) as usize;
        u32::from_ne_bytes(d[o..o + 4].try_into().unwrap())
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
    Some(crate::capture::CursorImg { x: x0, y: y0, w: cw, h: ch, data })
}

fn grab_impl(cursor: bool) -> Res<Grab> {
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
            .map(|g| globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, g.version.min(4), &qh, ()))
            .collect()
    });
    if outputs.is_empty() {
        return Err("no outputs".into());
    }
    let mut state = State { logical: vec![Logical::default(); outputs.len()], ..Default::default() };
    if let Some(x) = &xdg {
        for (i, o) in outputs.iter().enumerate() {
            x.get_xdg_output(o, &qh, i);
        }
    }
    queue.roundtrip(&mut state)?;

    let mut shots = Vec::new();
    let mut next_x = 0;
    for (i, out) in outputs.iter().enumerate() {
        state.frame = Frame::default();
        let frame = mgr.capture_output(cursor as i32, out, &qh, ());
        while !state.frame.failed && !(state.frame.format.is_some() && (state.frame.buffer_done || mgr.version() < 3)) {
            queue.blocking_dispatch(&mut state)?;
        }
        if state.frame.failed {
            return Err("screencopy failed".into());
        }
        let (fmt, w, h, stride) = state.frame.format.ok_or("no usable buffer format")?;
        let size = (stride * h) as usize;

        let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("linshot-shm-{}-{i}", std::process::id()));
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path)?;
        let _ = std::fs::remove_file(&path);
        file.set_len(size as u64)?;
        let map = unsafe { memmap2::MmapMut::map_mut(&file)? };
        let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, fmt, &qh, ());
        frame.copy(&buffer);
        while !state.frame.ready && !state.frame.failed {
            queue.blocking_dispatch(&mut state)?;
        }
        if state.frame.failed {
            return Err("screencopy copy failed".into());
        }

        let swap = swizzle(fmt).unwrap_or(false);
        let (wi, hi) = (w as usize, h as usize);
        let mut data = vec![0u8; wi * hi * 4];
        for y in 0..hi {
            let sy = if state.frame.y_invert { hi - 1 - y } else { y };
            let src = &map[sy * stride as usize..sy * stride as usize + wi * 4];
            let dst = &mut data[y * wi * 4..(y + 1) * wi * 4];
            for x in 0..wi {
                let p = &src[x * 4..x * 4 + 4];
                // memory order of (A|X)RGB8888 on little endian: B G R A
                let (b, g, r) = if swap { (p[2], p[1], p[0]) } else { (p[0], p[1], p[2]) };
                let px = 0xff00_0000u32 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
                dst[x * 4..x * 4 + 4].copy_from_slice(&px.to_ne_bytes());
            }
        }
        buffer.destroy();
        pool.destroy();
        frame.destroy();

        let mut l = state.logical[i];
        if l.w <= 0 || l.h <= 0 {
            // no xdg-output: lay outputs out left to right at buffer size
            l = Logical { x: next_x, y: 0, w: w as i32, h: h as i32 };
        }
        next_x = next_x.max(l.x + l.w);
        shots.push(Shot { l, w: w as i32, h: h as i32, data });
    }
    Ok(composite(shots))
}

fn composite(shots: Vec<Shot>) -> Grab {
    if shots.len() == 1 {
        let s = shots.into_iter().next().unwrap();
        return Grab::Pixels { width: s.w, height: s.h, stride: s.w * 4, data: s.data };
    }
    let minx = shots.iter().map(|s| s.l.x).min().unwrap();
    let miny = shots.iter().map(|s| s.l.y).min().unwrap();
    let maxx = shots.iter().map(|s| s.l.x + s.l.w).max().unwrap();
    let maxy = shots.iter().map(|s| s.l.y + s.l.h).max().unwrap();
    let scale = shots.iter().map(|s| s.w as f64 / s.l.w as f64).fold(1.0, f64::max);
    let width = ((maxx - minx) as f64 * scale).ceil() as i32;
    let height = ((maxy - miny) as f64 * scale).ceil() as i32;
    let out = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).expect("surface");
    {
        let cr = cairo::Context::new(&out).expect("ctx");
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.paint().ok();
        for s in shots {
            let src = cairo::ImageSurface::create_for_data(s.data, cairo::Format::ARgb32, s.w, s.h, s.w * 4).expect("surface");
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
