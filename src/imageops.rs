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
        let mut dl = gdk::TextureDownloader::new(&tex);
        dl.set_format(MEM_FMT);
        let (bytes, bstride) = dl.download_bytes();
        for y in 0..h as usize {
            let n = w as usize * 4;
            data[y * stride..y * stride + n].copy_from_slice(&bytes[y * bstride..y * bstride + n]);
        }
    }
    surf.mark_dirty();
    Ok(surf)
}

pub fn crop(src: &ImageSurface, x: i32, y: i32, w: i32, h: i32) -> ImageSurface {
    let w = w.max(1);
    let h = h.max(1);
    let out = ImageSurface::create(Format::ARgb32, w, h).expect("surface");
    let cr = cairo::Context::new(&out).expect("ctx");
    cr.set_source_surface(src, -x as f64, -y as f64).ok();
    cr.paint().ok();
    drop(cr);
    out
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

pub fn save(s: &ImageSurface, path: &Path, jpeg_quality: u8) -> Result<(), String> {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "bmp" | "tiff" | "tif" | "webp" => {
            let (w, h) = (s.width(), s.height());
            let (data, stride) = raw_bytes(s);
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let px = u32::from_ne_bytes(data[y * stride + x * 4..y * stride + x * 4 + 4].try_into().unwrap());
                    // composite on white
                    let a = (px >> 24) & 0xff;
                    let c = |sh: u32| (((px >> sh) & 0xff) + (255 - a)) as u8;
                    rgb.extend_from_slice(&[c(16), c(8), c(0)]);
                }
            }
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
            pb.savev(path, ty, &opts).map_err(|e| e.to_string())
        }
        _ => {
            let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
            s.write_to_png(&mut f).map_err(|e| e.to_string())
        }
    }
}
