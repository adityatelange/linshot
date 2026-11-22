// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

use crate::capture::Grab;
use cairo::{Format, ImageSurface};
use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf, glib};
use std::path::Path;

#[cfg(target_endian = "little")]
const MEM_FMT: gdk::MemoryFormat = gdk::MemoryFormat::B8g8r8a8Premultiplied;
#[cfg(target_endian = "big")]
const MEM_FMT: gdk::MemoryFormat = gdk::MemoryFormat::A8r8g8b8Premultiplied;

pub fn surface_from_grab(g: Grab) -> Result<ImageSurface, String> {
    match g {
        Grab::Pixels { width, height, stride, data } => {
            ImageSurface::create_for_data(data, Format::ARgb32, width, height, stride).map_err(|e| e.to_string())
        }
        Grab::File(path) => {
            let r = load_file(&path);
            let _ = std::fs::remove_file(&path);
            r
        }
    }
}

/// Load any image format gdk-pixbuf / GTK understands.
pub fn load_file(path: &Path) -> Result<ImageSurface, String> {
    let tex = gdk::Texture::from_filename(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (tex.width(), tex.height());
    let mut surf = ImageSurface::create(Format::ARgb32, w, h).map_err(|e| e.to_string())?;
    let stride = surf.stride() as usize;
    {
        let mut data = surf.data().map_err(|e| e.to_string())?;
        // download() writes cairo's ARGB32 layout (premultiplied, native endian)
        tex.download(&mut data, stride);
    }
    surf.mark_dirty();
    Ok(surf)
}

/// Copies a w×h area starting at (x, y). Fails for sizes cairo cannot
/// allocate (over 32767 px or out of memory) instead of panicking.
pub fn crop(src: &ImageSurface, x: i32, y: i32, w: i32, h: i32) -> Result<ImageSurface, String> {
    let w = w.max(1);
    let h = h.max(1);
    let out = ImageSurface::create(Format::ARgb32, w, h).map_err(|e| format!("cannot create a {w}×{h} image: {e}"))?;
    let cr = cairo::Context::new(&out).map_err(|e| e.to_string())?;
    cr.set_source_surface(src, -x as f64, -y as f64).map_err(|e| e.to_string())?;
    cr.paint().map_err(|e| e.to_string())?;
    drop(cr);
    Ok(out)
}

fn raw_bytes(s: &ImageSurface) -> (Vec<u8>, usize) {
    s.flush();
    let stride = s.stride() as usize;
    let mut v = Vec::new();
    let _ = s.with_data(|d| v.extend_from_slice(d));
    (v, stride)
}

pub fn to_texture(s: &ImageSurface) -> gdk::MemoryTexture {
    let (data, stride) = raw_bytes(s);
    gdk::MemoryTexture::new(s.width(), s.height(), MEM_FMT, &glib::Bytes::from_owned(data), stride)
}

pub fn copy_to_clipboard(s: &ImageSurface) {
    if let Some(d) = gdk::Display::default() {
        d.clipboard().set_texture(&to_texture(s));
    }
}

/// Copies to the clipboard from anywhere, including the background.
///
/// Wayland compositors only accept a new clipboard selection from a client
/// that has had input, so when no Linshot window has focus (`full -c`, or
/// "copy" as the after-capture action) the plain GTK call is silently
/// dropped. Then this goes through `wl-copy` if installed, or else shows a
/// small window just long enough to get focus and set the clipboard from it.
pub async fn copy_anywhere(app: &gtk::Application, s: &ImageSurface) -> Result<(), String> {
    let focused = app.windows().iter().any(|w| w.is_active());
    if !crate::capture::is_wayland() || focused {
        copy_to_clipboard(s);
        return Ok(());
    }
    if glib::find_program_in_path("wl-copy").is_some() {
        let png = encode(s, Path::new("x.png"), 100)?;
        let r = gtk::gio::spawn_blocking(move || {
            use std::io::Write;
            let mut child = std::process::Command::new("wl-copy")
                .args(["--type", "image/png"])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            child.stdin.take().ok_or("no stdin")?.write_all(&png).map_err(|e| e.to_string())?;
            // wl-copy forks a server and exits once it has read the image
            match child.wait() {
                Ok(st) if st.success() => Ok(()),
                Ok(st) => Err(format!("wl-copy failed ({st})")),
                Err(e) => Err(e.to_string()),
            }
        })
        .await
        .map_err(|_| "wl-copy thread panicked".to_string())?;
        match r {
            Ok(()) => return Ok(()),
            Err(e) => eprintln!("linshot: {e}; trying a focused window"),
        }
    }
    const NEED_WL_COPY: &str = "the desktop only accepts clipboard data from a focused window. Install wl-clipboard (wl-copy) to copy without opening the editor";
    // wlroots also wants the serial of an input event, which a window that
    // was merely focused never received: the selection would be rejected
    if crate::session::is_wlroots() {
        return Err(NEED_WL_COPY.into());
    }
    let win = gtk::Window::builder()
        .application(app)
        .title("Linshot")
        .resizable(false)
        .default_width(260)
        .build();
    let label = gtk::Label::new(Some("Copying screenshot\u{2026}"));
    label.set_margin_top(18);
    label.set_margin_bottom(18);
    label.set_margin_start(24);
    label.set_margin_end(24);
    win.set_child(Some(&label));
    win.present();
    let start = std::time::Instant::now();
    while !win.is_active() && start.elapsed() < std::time::Duration::from_millis(1500) {
        glib::timeout_future(std::time::Duration::from_millis(30)).await;
    }
    let ok = win.is_active();
    if ok {
        // the clipboard stays owned by Linshot after the window is gone
        copy_to_clipboard(s);
        glib::timeout_future(std::time::Duration::from_millis(150)).await;
    }
    win.destroy();
    if ok {
        Ok(())
    } else {
        Err(NEED_WL_COPY.into())
    }
}

pub fn save(s: &ImageSurface, path: &Path, jpeg_quality: u8) -> Result<(), String> {
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create folder {}: {e}", d.display()))?;
    }
    let bytes = encode(s, path, jpeg_quality)?;
    crate::config::write_atomic(path, &bytes, false).map_err(|e| e.to_string())
}

/// The image encoded in the format named by the file extension (PNG by default).
fn encode(s: &ImageSurface, path: &Path, jpeg_quality: u8) -> Result<Vec<u8>, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    s.flush();
    let (w, h, stride) = (s.width(), s.height(), s.stride() as usize);
    match ext.as_str() {
        "jpg" | "jpeg" | "bmp" | "tiff" | "tif" | "webp" => {
            let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
            s.with_data(|data| {
                for row in data.chunks(stride).take(h as usize) {
                    for p in row[..w as usize * 4].chunks_exact(4) {
                        let px = u32::from_ne_bytes([p[0], p[1], p[2], p[3]]);
                        // composite on white
                        let a = (px >> 24) & 0xff;
                        let c = |sh: u32| (((px >> sh) & 0xff) + (255 - a)) as u8;
                        rgb.extend_from_slice(&[c(16), c(8), c(0)]);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
            let pb = gdk_pixbuf::Pixbuf::from_bytes(
                &glib::Bytes::from_owned(rgb),
                gdk_pixbuf::Colorspace::Rgb,
                false,
                8,
                w,
                h,
                w * 3,
            );
            let (ty, opts): (&str, Vec<(&str, String)>) = match ext.as_str() {
                "jpg" | "jpeg" => ("jpeg", vec![("quality", jpeg_quality.to_string())]),
                "tif" | "tiff" => ("tiff", vec![]),
                other => (other, vec![]),
            };
            let opts: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (*k, v.as_str())).collect();
            pb.save_to_bufferv(ty, &opts).map_err(|e| e.to_string())
        }
        _ => {
            let mut out = None;
            s.with_data(|d| out = Some(crate::png::encode(w as usize, h as usize, stride, d)))
                .map_err(|e| e.to_string())?;
            match out {
                Some(Ok(v)) => Ok(v),
                // fall back to cairo's own (single-threaded) encoder
                _ => {
                    let mut v = Vec::new();
                    s.write_to_png(&mut v).map_err(|e| e.to_string())?;
                    Ok(v)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(s: &mut ImageSurface, x: usize, y: usize) -> u32 {
        let stride = s.stride() as usize;
        let d = s.data().unwrap();
        u32::from_ne_bytes(d[y * stride + x * 4..y * stride + x * 4 + 4].try_into().unwrap())
    }

    /// load_file must hand back cairo's native ARGB32 (premultiplied) layout.
    #[test]
    fn load_png_round_trip() {
        let dir = std::env::temp_dir().join(format!("linshot-imgtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        let mut src = ImageSurface::create(Format::ARgb32, 3, 2).unwrap();
        {
            let cr = cairo::Context::new(&src).unwrap();
            for (x, (r, g, b, a)) in [(1.0, 0.0, 0.0, 1.0), (0.0, 1.0, 0.0, 1.0), (0.0, 0.0, 1.0, 0.5)].into_iter().enumerate() {
                cr.set_source_rgba(r, g, b, a);
                cr.rectangle(x as f64, 0.0, 1.0, 2.0);
                cr.fill().unwrap();
            }
        }
        save(&src, &path, 90).unwrap();
        let mut got = load_file(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((got.width(), got.height()), (3, 2));
        for (x, y) in [(0, 0), (1, 1), (2, 0)] {
            let (a, b) = (px(&mut src, x, y), px(&mut got, x, y));
            let close = (0..4).all(|i| ((a >> (i * 8)) & 0xff).abs_diff((b >> (i * 8)) & 0xff) <= 1);
            assert!(close, "pixel {x},{y}: {a:08x} vs {b:08x}");
        }
        assert_eq!(px(&mut got, 0, 0), 0xffff0000);
    }
}
