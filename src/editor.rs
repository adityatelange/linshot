// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Annotation editor: shapes, arrows, text, highlight, obfuscation, step
//! counters, crop, undo/redo, save/copy/print.
use crate::imageops;
use crate::overlay::{rounded, R};
use crate::Ctx;
use cairo::ImageSurface;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Pen,
    Text,
    Highlight,
    Pixelate,
    Blur,
    Counter,
    Crop,
    /// captured mouse pointer (movable sprite)
    Pointer,
}

const TOOLS: &[(Tool, &str, &str, char)] = &[
    (Tool::Select, "⬉", "Select / move (S)", 's'),
    (Tool::Rect, "▭", "Rectangle (R)", 'r'),
    (Tool::Ellipse, "◯", "Ellipse (E)", 'e'),
    (Tool::Line, "╱", "Line (L)", 'l'),
    (Tool::Arrow, "➚", "Arrow (A)", 'a'),
    (Tool::Pen, "✎", "Freehand (F)", 'f'),
    (Tool::Text, "T", "Text (T)", 't'),
    (Tool::Highlight, "▌", "Highlight (H)", 'h'),
    (Tool::Counter, "①", "Step counter (I)", 'i'),
    (Tool::Pixelate, "▦", "Pixelate (O)", 'o'),
    (Tool::Blur, "◍", "Blur (B)", 'b'),
    (Tool::Crop, "⌗", "Crop (C) — drag, then Enter", 'c'),
];

type Pt = (f64, f64);

#[derive(Clone, Debug)]
pub struct Shape {
    kind: Tool,
    a: Pt,
    b: Pt,
    pts: Vec<Pt>,
    color: [f64; 4],
    fill: Option<[f64; 4]>,
    width: f64,
    font: f64,
    shadow: bool,
    text: String,
    num: u32,
    sprite: Option<ImageSurface>,
}

impl Shape {
    fn rect(&self) -> R {
        R::from_points(self.a, self.b)
    }

    fn translate(&mut self, dx: f64, dy: f64) {
        self.a = (self.a.0 + dx, self.a.1 + dy);
        self.b = (self.b.0 + dx, self.b.1 + dy);
        for p in &mut self.pts {
            *p = (p.0 + dx, p.1 + dy);
        }
    }

    fn bbox(&self) -> R {
        match self.kind {
            Tool::Pen => {
                let mut r = R { x: self.pts[0].0, y: self.pts[0].1, w: 0.0, h: 0.0 };
                for p in &self.pts {
                    r = r.union(&R { x: p.0, y: p.1, w: 0.01, h: 0.01 });
                }
                r
            }
            Tool::Text => SCRATCH.with(|cr| {
                let l = text_layout(cr, self);
                let (w, h) = l.pixel_size();
                R { x: self.a.0, y: self.a.1, w: w as f64 + 2.0 * TEXT_PAD, h: h as f64 + 2.0 * TEXT_PAD }
            }),
            Tool::Counter => {
                let r = counter_radius(self);
                R { x: self.a.0 - r, y: self.a.1 - r, w: 2.0 * r, h: 2.0 * r }
            }
            _ => self.rect(),
        }
    }

    fn hit(&self, p: Pt) -> bool {
        let tol = self.width / 2.0 + 5.0;
        match self.kind {
            Tool::Line | Tool::Arrow => seg_dist(p, self.a, self.b) <= tol,
            Tool::Pen => self.pts.windows(2).any(|w| seg_dist(p, w[0], w[1]) <= tol),
            Tool::Rect | Tool::Ellipse if self.fill.is_none() => {
                let r = self.rect();
                let outer = R { x: r.x - tol, y: r.y - tol, w: r.w + 2.0 * tol, h: r.h + 2.0 * tol };
                let inner = R { x: r.x + tol, y: r.y + tol, w: r.w - 2.0 * tol, h: r.h - 2.0 * tol };
                outer.contains(p) && !(inner.w > 0.0 && inner.h > 0.0 && inner.contains(p))
            }
            _ => self.bbox().contains(p),
        }
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
    let (x, y) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - x).powi(2) + (p.1 - y).powi(2)).sqrt()
}

thread_local! {
    static SCRATCH: cairo::Context = {
        let s = ImageSurface::create(cairo::Format::ARgb32, 1, 1).unwrap();
        cairo::Context::new(&s).unwrap()
    };
}

const TEXT_PAD: f64 = 5.0;

fn text_layout(cr: &cairo::Context, s: &Shape) -> pango::Layout {
    let l = pangocairo::functions::create_layout(cr);
    let mut fd = pango::FontDescription::from_string("Sans Bold");
    fd.set_absolute_size(s.font * pango::SCALE as f64);
    l.set_font_description(Some(&fd));
    l.set_text(&s.text);
    l
}

fn counter_radius(s: &Shape) -> f64 {
    s.font * 0.8 + 4.0
}

fn rgba(cr: &cairo::Context, c: [f64; 4]) {
    cr.set_source_rgba(c[0], c[1], c[2], c[3]);
}

fn shape_path(cr: &cairo::Context, s: &Shape) {
    match s.kind {
        Tool::Rect => {
            let r = s.rect();
            cr.rectangle(r.x, r.y, r.w, r.h);
        }
        Tool::Ellipse => {
            let r = s.rect();
            if r.w > 0.0 && r.h > 0.0 {
                cr.save().ok();
                cr.translate(r.x + r.w / 2.0, r.y + r.h / 2.0);
                cr.scale(r.w / 2.0, r.h / 2.0);
                cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
                cr.restore().ok();
            }
        }
        Tool::Line => {
            cr.move_to(s.a.0, s.a.1);
            cr.line_to(s.b.0, s.b.1);
        }
        Tool::Pen => {
            if let Some(f) = s.pts.first() {
                cr.move_to(f.0, f.1);
                for p in &s.pts[1..] {
                    cr.line_to(p.0, p.1);
                }
            }
        }
        _ => {}
    }
}

fn draw_arrow(cr: &cairo::Context, s: &Shape) {
    let ang = (s.b.1 - s.a.1).atan2(s.b.0 - s.a.0);
    let len = 10.0 + s.width * 3.0;
    let spread = 0.45;
    let base = (s.b.0 - len * 0.8 * ang.cos(), s.b.1 - len * 0.8 * ang.sin());
    cr.move_to(s.a.0, s.a.1);
    cr.line_to(base.0, base.1);
    cr.stroke().ok();
    cr.move_to(s.b.0, s.b.1);
    cr.line_to(s.b.0 - len * (ang - spread).cos(), s.b.1 - len * (ang - spread).sin());
    cr.line_to(s.b.0 - len * (ang + spread).cos(), s.b.1 - len * (ang + spread).sin());
    cr.close_path();
    cr.fill().ok();
}

fn draw_shape(cr: &cairo::Context, s: &Shape, bg: &ImageSurface) {
    cr.save().ok();
    cr.set_line_width(s.width);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    match s.kind {
        Tool::Rect | Tool::Ellipse | Tool::Line | Tool::Pen | Tool::Arrow => {
            let passes: &[(f64, bool)] = if s.shadow { &[(3.0, true), (0.0, false)] } else { &[(0.0, false)] };
            for &(off, shadow) in passes {
                cr.save().ok();
                cr.translate(off, off);
                if s.kind == Tool::Arrow {
                    rgba(cr, if shadow { [0.0, 0.0, 0.0, 0.3] } else { s.color });
                    draw_arrow(cr, s);
                } else {
                    shape_path(cr, s);
                    if let (Some(f), false) = (s.fill, shadow) {
                        rgba(cr, f);
                        cr.fill_preserve().ok();
                    }
                    rgba(cr, if shadow { [0.0, 0.0, 0.0, 0.3] } else { s.color });
                    cr.stroke().ok();
                }
                cr.restore().ok();
            }
        }
        Tool::Highlight => {
            let r = s.rect();
            cr.set_operator(cairo::Operator::Multiply);
            rgba(cr, s.color);
            cr.rectangle(r.x, r.y, r.w, r.h);
            cr.fill().ok();
        }
        Tool::Pixelate | Tool::Blur => obfuscate(cr, s, bg),
        Tool::Text => {
            let r = s.bbox();
            if let Some(f) = s.fill {
                if s.shadow {
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
                    rounded(cr, r.x + 3.0, r.y + 3.0, r.w, r.h, 4.0);
                    cr.fill().ok();
                }
                rgba(cr, f);
                rounded(cr, r.x, r.y, r.w, r.h, 4.0);
                cr.fill().ok();
            }
            let l = text_layout(cr, s);
            if s.shadow && s.fill.is_none() {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
                cr.move_to(s.a.0 + TEXT_PAD + 1.5, s.a.1 + TEXT_PAD + 1.5);
                pangocairo::functions::show_layout(cr, &l);
            }
            rgba(cr, s.color);
            cr.move_to(s.a.0 + TEXT_PAD, s.a.1 + TEXT_PAD);
            pangocairo::functions::show_layout(cr, &l);
        }
        Tool::Counter => {
            let rad = counter_radius(s);
            if s.shadow {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
                cr.arc(s.a.0 + 2.5, s.a.1 + 2.5, rad, 0.0, std::f64::consts::TAU);
                cr.fill().ok();
            }
            rgba(cr, s.color);
            cr.arc(s.a.0, s.a.1, rad, 0.0, std::f64::consts::TAU);
            cr.fill_preserve().ok();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(2.0);
            cr.stroke().ok();
            let t = Shape { text: s.num.to_string(), font: s.font * 0.95, ..s.clone() };
            let l = text_layout(cr, &t);
            let (w, h) = l.pixel_size();
            let lum = 0.299 * s.color[0] + 0.587 * s.color[1] + 0.114 * s.color[2];
            if lum > 0.6 {
                cr.set_source_rgb(0.0, 0.0, 0.0);
            } else {
                cr.set_source_rgb(1.0, 1.0, 1.0);
            }
            cr.move_to(s.a.0 - w as f64 / 2.0, s.a.1 - h as f64 / 2.0);
            pangocairo::functions::show_layout(cr, &l);
        }
        Tool::Pointer => {
            if let Some(sp) = &s.sprite {
                cr.set_source_surface(sp, s.a.0, s.a.1).ok();
                cr.paint().ok();
            }
        }
        Tool::Select | Tool::Crop => {}
    }
    cr.restore().ok();
}

fn obfuscate(cr: &cairo::Context, s: &Shape, bg: &ImageSurface) {
    let full = R { x: 0.0, y: 0.0, w: bg.width() as f64, h: bg.height() as f64 };
    let r = s.rect().intersect(&full);
    let (rx, ry, rw, rh) = (r.x.floor(), r.y.floor(), r.w.ceil(), r.h.ceil());
    if rw < 1.0 || rh < 1.0 {
        return;
    }
    let block = if s.kind == Tool::Pixelate { (s.width * 4.0).clamp(4.0, 64.0) } else { (s.width * 2.5).clamp(3.0, 40.0) };
    let sw = ((rw / block).ceil() as i32).max(1);
    let sh = ((rh / block).ceil() as i32).max(1);
    let Some(small) = obfuscated(bg, [rx as i64, ry as i64, rw as i64, rh as i64, sw as i64, sh as i64]) else { return };
    cr.save().ok();
    cr.rectangle(rx, ry, rw, rh);
    cr.clip();
    cr.translate(rx, ry);
    cr.scale(rw / sw as f64, rh / sh as f64);
    cr.set_source_surface(&small, 0.0, 0.0).ok();
    let pat = cr.source();
    pat.set_extend(cairo::Extend::Pad);
    pat.set_filter(if s.kind == Tool::Pixelate { cairo::Filter::Nearest } else { cairo::Filter::Bilinear });
    cr.paint().ok();
    cr.restore().ok();
}

/// Downscaled copies of background areas for pixelate / blur, kept on the
/// background surface itself so they live exactly as long as it does.
/// Recomputing them (a filtered downscale of the area) on every redraw made
/// dragging or drawing anything slow once a few obfuscations existed.
#[derive(Default)]
struct ObfCache(RefCell<Vec<([i64; 6], ImageSurface)>>);

static OBF_KEY: cairo::UserDataKey<ObfCache> = cairo::UserDataKey::new();
const OBF_CACHE_MAX: usize = 32;

/// The area [x, y, w, h] of `bg` scaled down to [.., .., .., .., sw, sh].
fn obfuscated(bg: &ImageSurface, key: [i64; 6]) -> Option<ImageSurface> {
    let cache = match bg.user_data(&OBF_KEY) {
        Some(c) => c,
        None => {
            let c = std::rc::Rc::new(ObfCache::default());
            bg.set_user_data(&OBF_KEY, c.clone()).ok()?;
            c
        }
    };
    let mut entries = cache.0.borrow_mut();
    if let Some(i) = entries.iter().position(|(k, _)| *k == key) {
        // most recently used last
        let e = entries.remove(i);
        let img = e.1.clone();
        entries.push(e);
        return Some(img);
    }
    let [rx, ry, rw, rh, sw, sh] = key;
    let small = ImageSurface::create(cairo::Format::ARgb32, sw as i32, sh as i32).ok()?;
    {
        let c = cairo::Context::new(&small).ok()?;
        c.scale(sw as f64 / rw as f64, sh as f64 / rh as f64);
        c.set_source_surface(bg, -rx as f64, -ry as f64).ok()?;
        c.source().set_filter(cairo::Filter::Good);
        c.paint().ok()?;
    }
    if entries.len() >= OBF_CACHE_MAX {
        entries.remove(0);
    }
    entries.push((key, small.clone()));
    Some(small)
}

#[derive(Clone)]
struct Doc {
    bg: ImageSurface,
    shapes: Vec<Shape>,
}

fn draw_doc(cr: &cairo::Context, d: &Doc) {
    cr.set_source_surface(&d.bg, 0.0, 0.0).ok();
    cr.paint().ok();
    for s in &d.shapes {
        draw_shape(cr, s, &d.bg);
    }
}

fn render(d: &Doc) -> Result<ImageSurface, String> {
    let out = ImageSurface::create(cairo::Format::ARgb32, d.bg.width(), d.bg.height()).map_err(|e| e.to_string())?;
    {
        let cr = cairo::Context::new(&out).map_err(|e| e.to_string())?;
        draw_doc(&cr, d);
    }
    Ok(out)
}

#[derive(Clone)]
struct Style {
    color: [f64; 4],
    fill: [f64; 4],
    fill_on: bool,
    width: f64,
    font: f64,
    shadow: bool,
}

struct Ed {
    doc: Doc,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    tool: Tool,
    sel: Option<usize>,
    preview: Option<Shape>,
    move_orig: Option<Shape>,
    move_saved: bool,
    drag_start: Pt,
    crop: Option<R>,
    zoom: f64,
    dirty: bool,
    path: Option<PathBuf>,
    style: Style,
    allow_close: bool,
}

impl Ed {
    fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        // snapshots share the background unless a crop made a new one; keep the
        // images only the undo history holds under a memory budget
        while self.undo.len() > 1 && history_bytes(&self.doc, &self.undo) > UNDO_IMAGE_BUDGET {
            self.undo.remove(0);
        }
    }
    fn renumber(&mut self) {
        let mut n = 0;
        for s in self.doc.shapes.iter_mut().filter(|s| s.kind == Tool::Counter) {
            n += 1;
            s.num = n;
        }
    }
    fn new_shape(&self, kind: Tool, p: Pt) -> Shape {
        let st = &self.style;
        let (color, fill) = match kind {
            Tool::Highlight => ([1.0, 0.92, 0.23, 1.0], None),
            Tool::Pixelate | Tool::Blur => ([0.0; 4], None),
            _ => (st.color, st.fill_on.then_some(st.fill)),
        };
        Shape {
            kind,
            a: p,
            b: p,
            pts: vec![p],
            color,
            fill,
            width: st.width,
            font: st.font,
            shadow: st.shadow,
            text: String::new(),
            num: 0,
            sprite: None,
        }
    }
}

/// Memory the undo history may spend on backgrounds the current document no
/// longer uses (each crop step keeps the previous full image).
const UNDO_IMAGE_BUDGET: usize = 768 << 20;

/// Bytes of distinct background images in `history` other than `cur`'s.
fn history_bytes(cur: &Doc, history: &[Doc]) -> usize {
    let mut seen = vec![cur.bg.to_raw_none()];
    let mut total = 0;
    for d in history {
        let p = d.bg.to_raw_none();
        if !seen.contains(&p) {
            seen.push(p);
            total += d.bg.stride().max(0) as usize * d.bg.height().max(0) as usize;
        }
    }
    total
}

/// Space around the image on the canvas (logical px).
const PAD: f64 = 40.0;
/// Room kept free for the floating dock (left) and property bar (bottom).
const DOCK_W: i32 = 76;
const BAR_H: i32 = 72;
/// Extra room at the top so the zoom pill does not cover the image.
const TOP_H: i32 = 20;

const PALETTE_NAMES: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Black", "White"];
const PALETTE: [[f64; 4]; 8] = [
    [0.937, 0.267, 0.267, 1.0],
    [0.976, 0.451, 0.086, 1.0],
    [0.980, 0.800, 0.082, 1.0],
    [0.133, 0.773, 0.369, 1.0],
    [0.231, 0.510, 0.965, 1.0],
    [0.659, 0.333, 0.969, 1.0],
    [0.067, 0.094, 0.153, 1.0],
    [1.0, 1.0, 1.0, 1.0],
];

struct Groups {
    color: gtk::Box,
    fill: gtk::Box,
    width: gtk::Box,
    font: gtk::Box,
    shadow: gtk::Box,
    hint: gtk::Label,
    sep1: gtk::Separator,
    sep2: gtk::Separator,
}

struct Ui {
    win: gtk::ApplicationWindow,
    area: gtk::DrawingArea,
    scroller: gtk::ScrolledWindow,
    subtitle: gtk::Label,
    zoom_label: gtk::Label,
    crop_bar: gtk::Box,
    toast: gtk::Revealer,
    toast_label: gtk::Label,
    toast_gen: Cell<u32>,
    undo_btn: gtk::Button,
    redo_btn: gtk::Button,
    del_btn: gtk::Button,
    tools: Vec<(Tool, gtk::ToggleButton)>,
    color: gtk::ColorButton,
    swatches: Vec<gtk::Button>,
    fill: gtk::ColorButton,
    fill_chk: gtk::ToggleButton,
    width: gtk::Scale,
    width_cap: gtk::Label,
    width_val: gtk::Label,
    width_dot: gtk::DrawingArea,
    font: gtk::SpinButton,
    shadow: gtk::ToggleButton,
    groups: Groups,
}

pub struct Editor {
    ed: RefCell<Ed>,
    ui: Ui,
    ctx: Rc<Ctx>,
    syncing: Cell<bool>,
    /// pointer position over the canvas (scroller coordinates)
    pointer: Cell<Option<(f64, f64)>>,
    last_style_ckpt: Cell<Option<std::time::Instant>>,
    /// background pre-scaled for the current zoom (see `scaled_bg`)
    view: RefCell<Option<ScaledBg>>,
}

struct ScaledBg {
    src: ImageSurface,
    scale: f64,
    img: ImageSurface,
}

thread_local! {
    static OPEN: RefCell<Vec<Rc<Editor>>> = const { RefCell::new(Vec::new()) };
}

fn to_rgba(c: [f64; 4]) -> gdk::RGBA {
    gdk::RGBA::new(c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32)
}
fn from_rgba(c: &gdk::RGBA) -> [f64; 4] {
    [c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64]
}

fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tip));
    b.add_css_class("flat");
    b
}

/// Crisp, theme-coloured tool glyphs drawn with cairo on a 16-unit grid,
/// like GNOME's symbolic icons: 1.5 px strokes, one shared live area
/// (about 2..14) so every tool has the same optical size, outlines rather
/// than solid shapes.
fn glyph(kind: Tool, size: i32) -> gtk::DrawingArea {
    use std::f64::consts::{PI, TAU};
    let a = gtk::DrawingArea::new();
    a.set_content_width(size);
    a.set_content_height(size);
    a.set_can_target(false);
    a.set_draw_func(move |w, cr, width, _| {
        let c = crate::compat::fg_color(w);
        let (r, g, b, al) = (c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
        let s = width as f64 / 16.0;
        cr.scale(s, s);
        cr.set_source_rgba(r, g, b, al);
        cr.set_line_width(1.5);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        let stroke = |cr: &cairo::Context| {
            cr.stroke().ok();
        };
        let faint = |cr: &cairo::Context, a: f64| cr.set_source_rgba(r, g, b, al * a);
        match kind {
            Tool::Select | Tool::Pointer => {
                cr.move_to(4.0, 2.0);
                cr.line_to(4.0, 13.0);
                cr.line_to(6.9, 10.3);
                cr.line_to(8.9, 14.0);
                cr.line_to(10.6, 13.1);
                cr.line_to(8.7, 9.5);
                cr.line_to(12.5, 9.5);
                cr.close_path();
                cr.fill_preserve().ok();
                cr.set_line_width(1.0);
                stroke(cr);
            }
            Tool::Rect => {
                rounded(cr, 2.25, 3.25, 11.5, 9.5, 1.75);
                stroke(cr);
            }
            Tool::Ellipse => {
                cr.save().ok();
                cr.translate(8.0, 8.0);
                cr.scale(5.75, 4.75);
                cr.arc(0.0, 0.0, 1.0, 0.0, TAU);
                cr.restore().ok();
                stroke(cr);
            }
            Tool::Line => {
                cr.move_to(3.0, 13.0);
                cr.line_to(13.0, 3.0);
                stroke(cr);
            }
            Tool::Arrow => {
                cr.move_to(3.0, 13.0);
                cr.line_to(12.5, 3.5);
                cr.move_to(7.0, 3.0);
                cr.line_to(13.0, 3.0);
                cr.line_to(13.0, 9.0);
                stroke(cr);
            }
            Tool::Pen => {
                cr.move_to(2.5, 12.0);
                cr.curve_to(4.5, 5.0, 7.0, 14.5, 9.5, 8.0);
                cr.curve_to(10.7, 5.0, 12.0, 4.6, 13.5, 3.5);
                stroke(cr);
            }
            Tool::Text => {
                cr.move_to(3.5, 3.25);
                cr.line_to(12.5, 3.25);
                cr.move_to(8.0, 3.25);
                cr.line_to(8.0, 13.0);
                cr.move_to(6.25, 13.0);
                cr.line_to(9.75, 13.0);
                stroke(cr);
            }
            Tool::Highlight => {
                // marker nib above a translucent stroke
                cr.move_to(5.25, 8.75);
                cr.line_to(9.75, 2.25);
                cr.line_to(12.5, 4.2);
                cr.line_to(8.0, 10.7);
                cr.close_path();
                cr.move_to(5.25, 8.75);
                cr.line_to(4.25, 10.75);
                cr.line_to(6.5, 10.75);
                stroke(cr);
                faint(cr, 0.45);
                cr.set_line_width(2.0);
                cr.move_to(3.0, 13.25);
                cr.line_to(13.0, 13.25);
                stroke(cr);
            }
            Tool::Counter => {
                cr.arc(8.0, 8.0, 5.5, 0.0, TAU);
                stroke(cr);
                cr.move_to(6.7, 6.4);
                cr.line_to(8.3, 5.2);
                cr.line_to(8.3, 10.8);
                stroke(cr);
            }
            Tool::Pixelate => {
                // outlined square with a 2x2 checker inside
                rounded(cr, 2.5, 2.5, 11.0, 11.0, 2.0);
                stroke(cr);
                cr.save().ok();
                rounded(cr, 2.5, 2.5, 11.0, 11.0, 2.0);
                cr.clip();
                faint(cr, 0.6);
                cr.rectangle(2.5, 2.5, 5.5, 5.5);
                cr.rectangle(8.0, 8.0, 5.5, 5.5);
                cr.fill().ok();
                cr.restore().ok();
            }
            Tool::Blur => {
                // drop with a faint inner ring
                cr.move_to(8.0, 2.0);
                cr.curve_to(9.6, 4.3, 12.5, 6.9, 12.5, 9.5);
                cr.arc(8.0, 9.5, 4.5, 0.0, PI);
                cr.curve_to(3.5, 6.9, 6.4, 4.3, 8.0, 2.0);
                cr.close_path();
                stroke(cr);
                faint(cr, 0.5);
                cr.set_line_width(1.25);
                cr.arc(8.0, 9.6, 2.1, 0.0, TAU);
                stroke(cr);
            }
            Tool::Crop => {
                cr.move_to(5.0, 2.0);
                cr.line_to(5.0, 11.0);
                cr.line_to(14.0, 11.0);
                cr.move_to(2.0, 5.0);
                cr.line_to(11.0, 5.0);
                cr.line_to(11.0, 14.0);
                stroke(cr);
            }
        }
    });
    a
}

/// Small glyphs for the property bar.
fn prop_glyph(name: &'static str) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_content_width(18);
    a.set_content_height(18);
    a.set_can_target(false);
    a.set_draw_func(move |w, cr, width, _| {
        let c = crate::compat::fg_color(w);
        cr.scale(width as f64 / 20.0, width as f64 / 20.0);
        cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
        cr.set_line_width(1.7);
        match name {
            "fill" => {
                rounded(cr, 3.5, 3.5, 13.0, 13.0, 3.0);
                cr.stroke_preserve().ok();
                cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, 0.45);
                cr.fill().ok();
            }
            _ => {
                // shadow
                cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, 0.4);
                rounded(cr, 6.0, 6.0, 11.0, 11.0, 2.5);
                cr.fill().ok();
                cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
                rounded(cr, 3.0, 3.0, 11.0, 11.0, 2.5);
                cr.stroke().ok();
            }
        }
    });
    a
}

fn cap(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("ls-cap");
    l
}

/// Makes the swatch inside a colour button a small circle like the palette
/// swatches (GTK sizes it as a wide rectangle by default).
fn round_color_button(w: &impl IsA<gtk::Widget>) {
    let mut child = w.as_ref().first_child();
    while let Some(c) = child {
        if c.css_name() == "colorswatch" {
            c.set_size_request(16, 16);
        } else {
            round_color_button(&c);
        }
        child = c.next_sibling();
    }
}

fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

pub fn open(ctx: &Rc<Ctx>, img: ImageSurface, title: Option<String>, pointer: Option<(ImageSurface, f64, f64)>) {
    let cfg = ctx.cfg.borrow().clone();
    let title_text = title.unwrap_or_else(|| "Screenshot".into());
    let win = gtk::ApplicationWindow::builder()
        .application(&ctx.app)
        .title(format!("{title_text} — Linshot"))
        .icon_name("linshot")
        .build();
    win.add_css_class("ls-editor");

    // ---------------------------------------------------------------- header
    let header = gtk::HeaderBar::new();
    let tbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tbox.set_valign(gtk::Align::Center);
    let tlabel = gtk::Label::new(Some(&title_text));
    tlabel.add_css_class("ls-title");
    tlabel.set_ellipsize(pango::EllipsizeMode::Middle);
    tlabel.set_max_width_chars(40);
    let subtitle = gtk::Label::new(None);
    subtitle.add_css_class("ls-subtitle");
    tbox.append(&tlabel);
    tbox.append(&subtitle);
    header.set_title_widget(Some(&tbox));

    let open_btn = icon_button("document-open-symbolic", "Open image (Ctrl+O)");
    let undo_btn = icon_button("edit-undo-symbolic", "Undo (Ctrl+Z)");
    let redo_btn = icon_button("edit-redo-symbolic", "Redo (Ctrl+Y)");
    header.pack_start(&open_btn);
    header.pack_start(&undo_btn);
    header.pack_start(&redo_btn);

    let menu = gio::Menu::new();
    let sec1 = gio::Menu::new();
    sec1.append(Some("Save As…"), Some("win.save-as"));
    sec1.append(Some("Print…"), Some("win.print"));
    menu.append_section(None, &sec1);
    let sec2 = gio::Menu::new();
    sec2.append(Some("New capture"), Some("win.capture"));
    sec2.append(Some("Preferences"), Some("win.prefs"));
    menu.append_section(None, &sec2);
    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("More")
        .build();
    menu_btn.add_css_class("flat");
    let save_btn = gtk::Button::with_label("Save");
    save_btn.add_css_class("ls-primary");
    save_btn.set_tooltip_text(Some("Save to the screenshots folder (Ctrl+S)"));
    let copy_btn = gtk::Button::new();
    let cb = hbox(6);
    cb.append(&gtk::Image::from_icon_name("edit-copy-symbolic"));
    cb.append(&gtk::Label::new(Some("Copy")));
    copy_btn.set_child(Some(&cb));
    copy_btn.add_css_class("ls-soft");
    copy_btn.set_tooltip_text(Some("Copy image to clipboard (Ctrl+C)"));
    header.pack_end(&menu_btn);
    header.pack_end(&save_btn);
    header.pack_end(&copy_btn);
    win.set_titlebar(Some(&header));

    // ---------------------------------------------------------------- canvas
    let area = gtk::DrawingArea::new();
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_focusable(true);
    area.set_margin_start(DOCK_W);
    area.set_margin_bottom(BAR_H);
    area.set_margin_top(TOP_H);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&area));
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);
    scroller.add_css_class("ls-canvas");
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&scroller));

    // ---------------------------------------------------------------- tool dock
    let dock = gtk::Box::new(gtk::Orientation::Vertical, 4);
    dock.add_css_class("ls-dock-tools");
    // scrolls instead of being clipped when the window is too short for all tools
    let dock_frame = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .propagate_natural_width(true)
        .child(&dock)
        .build();
    dock_frame.add_css_class("ls-dock");
    dock_frame.set_halign(gtk::Align::Start);
    dock_frame.set_valign(gtk::Align::Center);
    dock_frame.set_margin_start(14);
    dock_frame.set_margin_top(12);
    dock_frame.set_margin_bottom(BAR_H / 2);
    let mut tools = Vec::new();
    let mut first: Option<gtk::ToggleButton> = None;
    // groups (select · shapes · annotate · redact · crop) are set apart by
    // space, not lines, as GNOME's HIG asks for toolbars
    for (t, _, tip, _) in TOOLS {
        if matches!(t, Tool::Rect | Tool::Text | Tool::Pixelate | Tool::Crop) {
            let gap = gtk::Separator::new(gtk::Orientation::Horizontal);
            gap.add_css_class("spacer");
            dock.append(&gap);
        }
        let b = gtk::ToggleButton::new();
        // 16 px, the size GNOME's symbolic icons are drawn for
        b.set_child(Some(&glyph(*t, 16)));
        b.set_tooltip_text(Some(tip));
        b.add_css_class("ls-tool");
        if let Some(f) = &first {
            b.set_group(Some(f));
        } else {
            first = Some(b.clone());
        }
        dock.append(&b);
        tools.push((*t, b));
    }
    overlay.add_overlay(&dock_frame);

    // ---------------------------------------------------------------- property bar
    let props = hbox(4);
    props.add_css_class("ls-pill");
    props.set_halign(gtk::Align::Center);
    props.set_valign(gtk::Align::End);
    props.set_margin_bottom(16);
    props.set_margin_start(DOCK_W);

    let color_g = hbox(2);
    let mut swatches = Vec::new();
    for i in 0..PALETTE.len() {
        let b = gtk::Button::new();
        b.add_css_class("ls-swatch");
        b.set_widget_name(&format!("ls-swatch-{i}"));
        b.set_valign(gtk::Align::Center);
        b.set_halign(gtk::Align::Center);
        b.set_size_request(22, 22);
        b.set_tooltip_text(Some(PALETTE_NAMES[i]));
        color_g.append(&b);
        swatches.push(b);
    }
    let color = crate::compat::color_button(&to_rgba(cfg.line_color));
    color.set_tooltip_text(Some("Pick any colour…"));
    color.set_valign(gtk::Align::Center);
    round_color_button(&color);
    color.add_css_class("ls-custom");
    color_g.append(&color);

    let fill_g = hbox(2);
    let fill_chk = gtk::ToggleButton::new();
    fill_chk.set_child(Some(&prop_glyph("fill")));
    fill_chk.set_tooltip_text(Some("Fill the shape (text: background box)"));
    fill_chk.set_active(cfg.fill_enabled);
    let fill = crate::compat::color_button(&to_rgba(cfg.fill_color));
    fill.set_tooltip_text(Some("Fill colour"));
    fill.set_valign(gtk::Align::Center);
    round_color_button(&fill);
    fill_g.append(&fill_chk);
    fill_g.append(&fill);

    let width_g = hbox(2);
    let width_cap = cap("Width");
    let width = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 30.0, 1.0);
    width.set_draw_value(false);
    width.set_value(cfg.line_width);
    width.set_valign(gtk::Align::Center);
    width.set_tooltip_text(Some("Line width (also: blur / pixelate strength)"));
    let width_val = gtk::Label::new(Some(&format!("{}", cfg.line_width as i32)));
    width_val.add_css_class("ls-value");
    // live preview: a short stroke as thick as the line, in the current colour
    let width_dot = gtk::DrawingArea::new();
    width_dot.set_content_width(30);
    width_dot.set_content_height(24);
    width_dot.set_valign(gtk::Align::Center);
    width_dot.add_css_class("ls-width-dot");
    {
        let (width, color) = (width.clone(), color.clone());
        width_dot.set_draw_func(move |a, cr, w, h| {
            let c = from_rgba(&color.rgba());
            let lw = width.value().clamp(1.0, h as f64 - 4.0);
            let (x0, x1, y) = (lw / 2.0 + 2.0, w as f64 - lw / 2.0 - 2.0, h as f64 / 2.0);
            cr.set_line_cap(cairo::LineCap::Round);
            // faint outline first so white / black strokes stay visible on the pill
            let fg = crate::compat::fg_color(a);
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.28);
            cr.set_line_width(lw + 2.0);
            cr.move_to(x0, y);
            cr.line_to(x1, y);
            cr.stroke().ok();
            cr.set_source_rgba(c[0], c[1], c[2], 1.0);
            cr.set_line_width(lw);
            cr.move_to(x0, y);
            cr.line_to(x1, y);
            cr.stroke().ok();
        });
    }
    width_g.append(&width_cap);
    width_g.append(&width_dot);
    width_g.append(&width);
    width_g.append(&width_val);

    let font_g = hbox(4);
    let font = gtk::SpinButton::with_range(6.0, 120.0, 1.0);
    font.set_numeric(true);
    font.set_value(cfg.font_size);
    font.set_valign(gtk::Align::Center);
    font.set_tooltip_text(Some("Font size"));
    font_g.append(&cap("Size"));
    font_g.append(&font);

    let shadow_g = hbox(0);
    let shadow = gtk::ToggleButton::new();
    shadow.set_child(Some(&prop_glyph("shadow")));
    shadow.set_tooltip_text(Some("Drop shadow under shapes and text"));
    shadow.set_active(cfg.shadow);
    shadow_g.append(&shadow);

    let hint = gtk::Label::new(None);
    hint.add_css_class("ls-hint");
    let del_btn = gtk::Button::from_icon_name("user-trash-symbolic");
    del_btn.set_tooltip_text(Some("Delete selected (Del)"));
    del_btn.add_css_class("ls-danger");

    let sep1 = gtk::Separator::new(gtk::Orientation::Vertical);
    let sep2 = gtk::Separator::new(gtk::Orientation::Vertical);
    props.append(&color_g);
    props.append(&sep1);
    props.append(&fill_g);
    props.append(&width_g);
    props.append(&font_g);
    props.append(&sep2);
    props.append(&shadow_g);
    props.append(&hint);
    props.append(&del_btn);
    overlay.add_overlay(&props);

    // ---------------------------------------------------------------- zoom pill
    let zoom_pill = hbox(0);
    zoom_pill.add_css_class("ls-pill");
    zoom_pill.set_halign(gtk::Align::End);
    zoom_pill.set_valign(gtk::Align::Start);
    zoom_pill.set_margin_end(16);
    zoom_pill.set_margin_top(14);
    let zoom_out = icon_button("zoom-out-symbolic", "Zoom out (Ctrl+-)");
    let zoom_in = icon_button("zoom-in-symbolic", "Zoom in (Ctrl++)");
    let zoom_label = gtk::Label::new(Some("100%"));
    zoom_label.add_css_class("ls-zoom-label");
    let zoom_fit = gtk::Button::new();
    zoom_fit.set_child(Some(&zoom_label));
    zoom_fit.set_tooltip_text(Some("Fit to window (Ctrl+0) · right-click: 100% (Ctrl+1)"));
    zoom_pill.append(&zoom_out);
    zoom_pill.append(&zoom_fit);
    zoom_pill.append(&zoom_in);
    overlay.add_overlay(&zoom_pill);

    // ---------------------------------------------------------------- crop bar
    let crop_bar = hbox(6);
    crop_bar.add_css_class("ls-pill");
    crop_bar.set_halign(gtk::Align::Center);
    crop_bar.set_valign(gtk::Align::Start);
    crop_bar.set_margin_top(14);
    crop_bar.append(&cap("Crop to selection"));
    let crop_cancel = gtk::Button::with_label("Cancel");
    let crop_apply = gtk::Button::with_label("Apply");
    crop_apply.add_css_class("ls-primary");
    crop_bar.append(&crop_cancel);
    crop_bar.append(&crop_apply);
    crop_bar.set_visible(false);
    overlay.add_overlay(&crop_bar);

    // ---------------------------------------------------------------- toast
    let toast = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(180)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .margin_top(14)
        .can_target(false)
        .build();
    let toast_label = gtk::Label::new(None);
    toast_label.add_css_class("ls-toast");
    toast.set_child(Some(&toast_label));
    overlay.add_overlay(&toast);

    win.set_child(Some(&overlay));

    // ---------------------------------------------------------------- size / zoom
    let (iw, ih) = (img.width() as f64, img.height() as f64);
    let (mw, mh) = crate::overlay::monitors()
        .first()
        .map(|(_, g)| (g.w, g.h))
        .unwrap_or((1366.0, 768.0));
    let (maxw, maxh) = (mw * 0.88, mh * 0.88);
    let chrome = (DOCK_W as f64 + 2.0 * PAD + 8.0, (BAR_H + TOP_H) as f64 + 2.0 * PAD + 56.0);
    let zoom = (1.0f64).min((maxw - chrome.0) / iw).min((maxh - chrome.1) / ih).max(0.05);
    let ww = (iw * zoom + chrome.0).clamp(980.0, maxw.max(980.0));
    let wh = (ih * zoom + chrome.1).clamp(620.0, maxh.max(620.0));
    win.set_default_size(ww as i32, wh as i32);

    let style = Style {
        color: cfg.line_color,
        fill: cfg.fill_color,
        fill_on: cfg.fill_enabled,
        width: cfg.line_width,
        font: cfg.font_size,
        shadow: cfg.shadow,
    };
    let e = Rc::new(Editor {
        ed: RefCell::new(Ed {
            doc: Doc {
                bg: img,
                shapes: pointer
                    .map(|(sp, x, y)| {
                        let (w, h) = (sp.width() as f64, sp.height() as f64);
                        vec![Shape {
                            kind: Tool::Pointer,
                            a: (x, y),
                            b: (x + w, y + h),
                            pts: Vec::new(),
                            color: [0.0; 4],
                            fill: None,
                            width: 1.0,
                            font: 12.0,
                            shadow: false,
                            text: String::new(),
                            num: 0,
                            sprite: Some(sp),
                        }]
                    })
                    .unwrap_or_default(),
            },
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Rect,
            sel: None,
            preview: None,
            move_orig: None,
            move_saved: false,
            drag_start: (0.0, 0.0),
            crop: None,
            zoom,
            dirty: false,
            path: None,
            style,
            allow_close: false,
        }),
        ui: Ui {
            win: win.clone(),
            area: area.clone(),
            scroller: scroller.clone(),
            subtitle,
            zoom_label,
            crop_bar,
            toast,
            toast_label,
            toast_gen: Cell::new(0),
            undo_btn: undo_btn.clone(),
            redo_btn: redo_btn.clone(),
            del_btn: del_btn.clone(),
            tools,
            color: color.clone(),
            swatches: swatches.clone(),
            fill: fill.clone(),
            fill_chk: fill_chk.clone(),
            width: width.clone(),
            width_cap,
            width_val,
            width_dot: width_dot.clone(),
            font: font.clone(),
            shadow: shadow.clone(),
            groups: Groups { color: color_g, fill: fill_g, width: width_g, font: font_g, shadow: shadow_g, hint, sep1, sep2 },
        },
        ctx: ctx.clone(),
        syncing: Cell::new(false),
        pointer: Cell::new(None),
        last_style_ckpt: Cell::new(None),
        view: RefCell::new(None),
    });
    OPEN.with(|o| o.borrow_mut().push(e.clone()));

    // ---------------------------------------------------------------- wiring
    macro_rules! on {
        ($btn:expr, $m:ident) => {{
            let w = Rc::downgrade(&e);
            $btn.connect_clicked(move |_| {
                if let Some(e) = w.upgrade() {
                    e.$m();
                }
            });
        }};
    }
    on!(save_btn, save);
    on!(copy_btn, copy);
    on!(undo_btn, undo);
    on!(redo_btn, redo);
    on!(del_btn, delete_selected);
    on!(crop_apply, apply_crop);
    on!(zoom_fit, zoom_fit);
    {
        let w = Rc::downgrade(&e);
        crop_cancel.connect_clicked(move |_| {
            if let Some(e) = w.upgrade() {
                e.ed.borrow_mut().crop = None;
                e.refresh();
            }
        });
    }
    {
        let tx = ctx.tx.clone();
        open_btn.connect_clicked(move |_| {
            let _ = tx.send_blocking(crate::Cmd::OpenDialog);
        });
    }
    let add_action = |name: &str, f: Box<dyn Fn(&Rc<Editor>)>| {
        let a = gio::SimpleAction::new(name, None);
        let w = Rc::downgrade(&e);
        a.connect_activate(move |_, _| {
            if let Some(e) = w.upgrade() {
                f(&e);
            }
        });
        win.add_action(&a);
    };
    add_action("save-as", Box::new(|e| e.save_as()));
    add_action("print", Box::new(|e| e.print()));
    add_action("capture", Box::new(|e| {
        // started from the header-bar menu: let the popover close first
        let _ = e.ctx.tx.send_blocking(crate::Cmd::cap(crate::Mode::Region).from_menu());
    }));
    add_action("prefs", Box::new(|e| {
        let _ = e.ctx.tx.send_blocking(crate::Cmd::Settings);
    }));
    for (b, f) in [(&zoom_in, 1.25), (&zoom_out, 0.8)] {
        let w = Rc::downgrade(&e);
        b.connect_clicked(move |_| {
            if let Some(e) = w.upgrade() {
                let z = e.ed.borrow().zoom * f;
                e.set_zoom(z);
            }
        });
    }
    {
        let r = gtk::GestureClick::new();
        r.set_button(3);
        let w = Rc::downgrade(&e);
        r.connect_pressed(move |_, _, _, _| {
            if let Some(e) = w.upgrade() {
                e.set_zoom(1.0);
            }
        });
        zoom_fit.add_controller(r);
    }
    for (t, b) in &e.ui.tools {
        let w = Rc::downgrade(&e);
        let t = *t;
        b.connect_toggled(move |b| {
            if b.is_active() {
                if let Some(e) = w.upgrade() {
                    e.set_tool(t);
                }
            }
        });
    }
    for (i, b) in swatches.iter().enumerate() {
        let w = Rc::downgrade(&e);
        b.set_tooltip_text(Some(["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Black", "White"][i]));
        b.connect_clicked(move |_| {
            if let Some(e) = w.upgrade() {
                e.ui.color.set_rgba(&to_rgba(PALETTE[i]));
            }
        });
    }
    {
        let w = Rc::downgrade(&e);
        color.connect_rgba_notify(move |_| style_changed(&w));
        let w = Rc::downgrade(&e);
        fill.connect_rgba_notify(move |_| style_changed(&w));
        let w = Rc::downgrade(&e);
        fill_chk.connect_toggled(move |_| style_changed(&w));
        let w = Rc::downgrade(&e);
        width.connect_value_changed(move |_| style_changed(&w));
        let w = Rc::downgrade(&e);
        font.connect_value_changed(move |_| style_changed(&w));
        let w = Rc::downgrade(&e);
        shadow.connect_toggled(move |_| style_changed(&w));
    }

    {
        let w = Rc::downgrade(&e);
        area.set_draw_func(move |a, cr, _, _| {
            if let Some(e) = w.upgrade() {
                e.draw(a, cr);
            }
        });
    }

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    {
        let w = Rc::downgrade(&e);
        drag.connect_drag_begin(move |g, x, y| {
            if let Some(e) = w.upgrade() {
                e.area_grab_focus();
                e.press(e.to_img(x, y), g.current_event_state());
            }
        });
        let w = Rc::downgrade(&e);
        drag.connect_drag_update(move |g, dx, dy| {
            if let (Some(e), Some((x, y))) = (w.upgrade(), g.start_point()) {
                e.motion(e.to_img(x + dx, y + dy), g.current_event_state());
            }
        });
        let w = Rc::downgrade(&e);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(e) = w.upgrade() {
                e.release();
            }
        });
    }
    area.add_controller(drag);

    let dbl = gtk::GestureClick::new();
    dbl.set_button(1);
    {
        let w = Rc::downgrade(&e);
        dbl.connect_pressed(move |_, n, x, y| {
            if n != 2 {
                return;
            }
            if let Some(e) = w.upgrade() {
                let p = e.to_img(x, y);
                let tool = e.ed.borrow().tool;
                if tool == Tool::Crop && e.ed.borrow().crop.is_some() {
                    e.apply_crop();
                } else if tool == Tool::Select {
                    if let Some(i) = e.hit(p) {
                        if e.ed.borrow().doc.shapes[i].kind == Tool::Text {
                            e.text_popover(p, Some(i));
                        }
                    }
                }
            }
        });
    }
    area.add_controller(dbl);

    let area_motion = gtk::EventControllerMotion::new();
    {
        let w = Rc::downgrade(&e);
        area_motion.connect_motion(move |_, x, y| {
            if let Some(e) = w.upgrade() {
                e.update_cursor(Some(e.to_img(x, y)));
            }
        });
    }
    area.add_controller(area_motion);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    {
        let w = Rc::downgrade(&e);
        scroll.connect_scroll(move |c, _, dy| {
            let Some(e) = w.upgrade() else { return glib::Propagation::Proceed };
            if c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                let z = e.ed.borrow().zoom * if dy < 0.0 { 1.15 } else { 1.0 / 1.15 };
                e.zoom_at(z, e.pointer.get());
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    scroller.add_controller(scroll);
    let hover = gtk::EventControllerMotion::new();
    {
        let w = Rc::downgrade(&e);
        hover.connect_motion(move |_, x, y| {
            if let Some(e) = w.upgrade() {
                e.pointer.set(Some((x, y)));
            }
        });
        let w = Rc::downgrade(&e);
        hover.connect_leave(move |_| {
            if let Some(e) = w.upgrade() {
                e.pointer.set(None);
            }
        });
    }
    scroller.add_controller(hover);

    let keys = gtk::EventControllerKey::new();
    {
        let w = Rc::downgrade(&e);
        keys.connect_key_pressed(move |_, key, _, mods| match w.upgrade() {
            Some(e) => e.key(key, mods),
            None => glib::Propagation::Proceed,
        });
    }
    win.add_controller(keys);

    {
        let w = Rc::downgrade(&e);
        win.connect_close_request(move |_| match w.upgrade() {
            Some(e) => e.close_request(),
            None => glib::Propagation::Proceed,
        });
        let w = Rc::downgrade(&e);
        win.connect_destroy(move |_| {
            if let Some(e) = w.upgrade() {
                OPEN.with(|o| o.borrow_mut().retain(|x| !Rc::ptr_eq(x, &e)));
            }
        });
    }

    e.ui.tools[1].1.set_active(true);
    e.update_swatches();
    e.refresh();
    win.present();
}

fn style_changed(w: &Weak<Editor>) {
    if let Some(e) = w.upgrade() {
        e.style_changed();
    }
}

impl Editor {
    fn area_grab_focus(&self) {
        self.ui.area.grab_focus();
    }

    fn to_img(&self, x: f64, y: f64) -> Pt {
        let z = self.ed.borrow().zoom;
        ((x - PAD) / z, (y - PAD) / z)
    }

    fn hit(&self, p: Pt) -> Option<usize> {
        let ed = self.ed.borrow();
        ed.doc.shapes.iter().rposition(|s| s.hit(p))
    }

    fn refresh(&self) {
        let ed = self.ed.borrow();
        let (w, h) = (ed.doc.bg.width(), ed.doc.bg.height());
        self.ui.area.set_content_width((w as f64 * ed.zoom + 2.0 * PAD).round() as i32);
        self.ui.area.set_content_height((h as f64 * ed.zoom + 2.0 * PAD).round() as i32);
        self.ui.undo_btn.set_sensitive(!ed.undo.is_empty());
        self.ui.redo_btn.set_sensitive(!ed.redo.is_empty());
        self.ui.del_btn.set_visible(ed.sel.is_some());
        self.ui.crop_bar.set_visible(ed.crop.is_some());
        // keep toasts below the crop bar instead of on top of it
        self.ui.toast.set_margin_top(if ed.crop.is_some() { 66 } else { 14 });
        let state = match (&ed.path, ed.dirty) {
            (Some(p), false) => format!(" · saved as {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            (_, true) => " · edited".into(),
            _ => String::new(),
        };
        self.ui.subtitle.set_text(&format!("{w} × {h}{state}"));
        self.ui.zoom_label.set_text(&format!("{:.0}%", ed.zoom * 100.0));
        self.ui.area.queue_draw();
        drop(ed);
        self.update_props();
    }

    fn toast(&self, msg: &str) {
        self.ui.toast_label.set_text(msg);
        self.ui.toast.set_reveal_child(true);
        let generation = self.ui.toast_gen.get() + 1;
        self.ui.toast_gen.set(generation);
        let toast = self.ui.toast.clone();
        let w = OPEN.with(|o| o.borrow().iter().find(|e| e.ui.toast == toast).map(Rc::downgrade));
        glib::timeout_add_local_once(std::time::Duration::from_millis(1800), move || {
            if let Some(e) = w.and_then(|w| w.upgrade()) {
                if e.ui.toast_gen.get() == generation {
                    e.ui.toast.set_reveal_child(false);
                }
            }
        });
    }

    fn set_zoom(&self, z: f64) {
        self.zoom_at(z, None);
    }

    /// Changes the zoom keeping the image point under `anchor` (scroller
    /// coordinates; the middle of the view when `None`) where it is.
    fn zoom_at(&self, z: f64, anchor: Option<(f64, f64)>) {
        let z = z.clamp(0.05, 16.0);
        let sc = &self.ui.scroller;
        let (vw, vh) = (sc.width() as f64, sc.height() as f64);
        let (ax, ay) = anchor.unwrap_or((vw / 2.0, vh / 2.0));
        let img_pt = sc
            .compute_point(&self.ui.area, &gtk::graphene::Point::new(ax as f32, ay as f32))
            .map(|p| self.to_img(p.x() as f64, p.y() as f64));
        let (iw, ih) = {
            let mut ed = self.ed.borrow_mut();
            ed.zoom = z;
            (ed.doc.bg.width() as f64, ed.doc.bg.height() as f64)
        };
        self.refresh();
        let Some((px, py)) = img_pt else { return };
        // the scrolled content once the new size is allocated (area plus its margins)
        let cw = iw * z + 2.0 * PAD + DOCK_W as f64;
        let ch = ih * z + 2.0 * PAD + (BAR_H + TOP_H) as f64;
        for (adj, content, view, pos) in [
            (sc.hadjustment(), cw, vw, DOCK_W as f64 + PAD + px * z - ax),
            (sc.vadjustment(), ch, vh, TOP_H as f64 + PAD + py * z - ay),
        ] {
            if content > view {
                adj.configure(pos.clamp(0.0, content - view), 0.0, content, adj.step_increment(), adj.page_increment(), view);
            }
        }
    }

    fn zoom_fit(&self) {
        let (w, h) = {
            let ed = self.ed.borrow();
            (ed.doc.bg.width() as f64, ed.doc.bg.height() as f64)
        };
        let aw = self.ui.scroller.width() as f64 - 2.0 * PAD - DOCK_W as f64 - 4.0;
        let ah = self.ui.scroller.height() as f64 - 2.0 * PAD - (BAR_H + TOP_H) as f64 - 4.0;
        if aw > 10.0 && ah > 10.0 {
            self.set_zoom((aw / w).min(ah / h).min(1.0));
        }
    }

    /// Shows only the properties that matter for the current tool / selection.
    fn update_props(&self) {
        let ed = self.ed.borrow();
        let kind = match (ed.tool, ed.sel) {
            (Tool::Select, Some(i)) => ed.doc.shapes.get(i).map(|s| s.kind).unwrap_or(Tool::Select),
            (t, _) => t,
        };
        let g = &self.ui.groups;
        let (color, fill, width, font, shadow, hint) = match kind {
            Tool::Rect | Tool::Ellipse => (true, true, true, false, true, ""),
            Tool::Line | Tool::Arrow | Tool::Pen => (true, false, true, false, true, ""),
            Tool::Text => (true, true, false, true, true, ""),
            Tool::Counter => (true, false, false, true, true, ""),
            Tool::Pixelate | Tool::Blur => (false, false, true, false, false, ""),
            Tool::Highlight => (false, false, false, false, false, "Drag over text to highlight it"),
            Tool::Crop => (false, false, false, false, false, "Drag to choose the area, then Enter"),
            Tool::Select => (false, false, false, false, false, "Click a shape to edit · drag to move · Del to delete"),
            Tool::Pointer => (false, false, false, false, false, "Mouse pointer · drag to move · Del to remove"),
        };
        g.color.set_visible(color);
        g.fill.set_visible(fill);
        g.width.set_visible(width);
        g.font.set_visible(font);
        g.shadow.set_visible(shadow);
        g.sep1.set_visible(color && (fill || width || font));
        g.sep2.set_visible(shadow && (fill || width || font));
        g.hint.set_visible(!hint.is_empty());
        g.hint.set_text(hint);
        self.ui.width_cap.set_text(if matches!(kind, Tool::Pixelate | Tool::Blur) { "Strength" } else { "Width" });
    }

    fn update_swatches(&self) {
        let c = from_rgba(&self.ui.color.rgba());
        for (i, b) in self.ui.swatches.iter().enumerate() {
            let p = PALETTE[i];
            let same = (0..3).all(|k| (p[k] - c[k]).abs() < 0.01);
            if same {
                b.add_css_class("selected");
            } else {
                b.remove_css_class("selected");
            }
        }
        self.ui.width_val.set_text(&format!("{}", self.ui.width.value() as i32));
        self.ui.width_dot.queue_draw();
    }

    fn set_tool(&self, t: Tool) {
        {
            let mut ed = self.ed.borrow_mut();
            ed.tool = t;
            if t != Tool::Crop {
                ed.crop = None;
            }
            if t != Tool::Select {
                ed.sel = None;
            }
        }
        for (tt, b) in &self.ui.tools {
            if *tt == t && !b.is_active() {
                b.set_active(true);
            }
        }
        self.update_cursor(None);
        self.update_props();
        self.refresh();
    }

    /// Tool cursor, or a move cursor over something that can be dragged.
    fn update_cursor(&self, p: Option<Pt>) {
        let tool = self.ed.borrow().tool;
        let movable = p.and_then(|p| self.hit(p)).map(|i| {
            let k = self.ed.borrow().doc.shapes[i].kind;
            tool == Tool::Select || (k == Tool::Pointer && tool != Tool::Crop)
        });
        let name = match tool {
            _ if movable == Some(true) => "move",
            Tool::Select => "default",
            Tool::Text => "text",
            _ => "crosshair",
        };
        if self.ui.area.cursor().and_then(|c| c.name()).as_deref() != Some(name) {
            self.ui.area.set_cursor_from_name(Some(name));
        }
    }

    fn style_changed(&self) {
        self.update_swatches();
        if self.syncing.get() {
            return;
        }
        let style = Style {
            color: from_rgba(&self.ui.color.rgba()),
            fill: from_rgba(&self.ui.fill.rgba()),
            fill_on: self.ui.fill_chk.is_active(),
            width: self.ui.width.value(),
            font: self.ui.font.value(),
            shadow: self.ui.shadow.is_active(),
        };
        {
            let mut cfg = self.ctx.cfg.borrow_mut();
            cfg.line_color = style.color;
            cfg.fill_color = style.fill;
            cfg.fill_enabled = style.fill_on;
            cfg.line_width = style.width;
            cfg.font_size = style.font;
            cfg.shadow = style.shadow;
            cfg.save();
        }
        let mut ed = self.ed.borrow_mut();
        ed.style = style.clone();
        if let Some(i) = ed.sel {
            // one undo step per burst of changes (e.g. dragging the width slider)
            let now = std::time::Instant::now();
            let recent = self.last_style_ckpt.get().map(|t| now.duration_since(t).as_millis() < 800).unwrap_or(false);
            if !recent {
                ed.checkpoint();
            }
            self.last_style_ckpt.set(Some(now));
            if let Some(s) = ed.doc.shapes.get_mut(i) {
                if !matches!(s.kind, Tool::Highlight | Tool::Pixelate | Tool::Blur) {
                    s.color = style.color;
                    s.fill = style.fill_on.then_some(style.fill);
                }
                s.width = style.width;
                s.font = style.font;
                s.shadow = style.shadow;
            }
        }
        drop(ed);
        self.refresh();
    }

    fn sync_ui(&self, s: &Shape) {
        self.syncing.set(true);
        if !matches!(s.kind, Tool::Highlight | Tool::Pixelate | Tool::Blur) {
            self.ui.color.set_rgba(&to_rgba(s.color));
            self.ui.fill_chk.set_active(s.fill.is_some());
            if let Some(f) = s.fill {
                self.ui.fill.set_rgba(&to_rgba(f));
            }
        }
        self.ui.width.set_value(s.width);
        self.ui.font.set_value(s.font);
        self.ui.shadow.set_active(s.shadow);
        self.syncing.set(false);
        self.update_swatches();
    }

    fn press(self: &Rc<Self>, p: Pt, _mods: gdk::ModifierType) {
        let tool = self.ed.borrow().tool;
        // the captured pointer can be dragged with any drawing tool
        let on_pointer = tool != Tool::Crop
            && self.hit(p).map(|i| self.ed.borrow().doc.shapes[i].kind == Tool::Pointer).unwrap_or(false);
        let tool = if on_pointer { Tool::Select } else { tool };
        match tool {
            Tool::Select => {
                let hit = self.hit(p);
                let mut ed = self.ed.borrow_mut();
                ed.sel = hit;
                ed.move_orig = hit.map(|i| ed.doc.shapes[i].clone());
                ed.move_saved = false;
                ed.drag_start = p;
                let s = ed.move_orig.clone();
                drop(ed);
                if let Some(s) = s {
                    self.sync_ui(&s);
                }
                self.update_props();
            }
            Tool::Text => {
                let hit = self.hit(p);
                let is_text = hit.map(|i| self.ed.borrow().doc.shapes[i].kind == Tool::Text).unwrap_or(false);
                self.text_popover(p, if is_text { hit } else { None });
            }
            Tool::Counter => {
                let mut ed = self.ed.borrow_mut();
                ed.checkpoint();
                let s = ed.new_shape(Tool::Counter, p);
                ed.doc.shapes.push(s);
                ed.renumber();
            }
            Tool::Crop => {
                let shape = self.ed.borrow().new_shape(Tool::Crop, p);
                let mut ed = self.ed.borrow_mut();
                ed.crop = Some(R { x: p.0, y: p.1, w: 0.0, h: 0.0 });
                ed.preview = Some(shape);
            }
            k => {
                let s = self.ed.borrow().new_shape(k, p);
                self.ed.borrow_mut().preview = Some(s);
            }
        }
        self.refresh();
    }

    fn motion(&self, p: Pt, mods: gdk::ModifierType) {
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let mut ed = self.ed.borrow_mut();
        if ed.move_orig.is_some() {
            if let (Some(i), Some(orig)) = (ed.sel, ed.move_orig.clone()) {
                if !ed.move_saved {
                    ed.checkpoint();
                    ed.move_saved = true;
                }
                let mut s = orig;
                s.translate(p.0 - ed.drag_start.0, p.1 - ed.drag_start.1);
                if let Some(slot) = ed.doc.shapes.get_mut(i) {
                    *slot = s;
                }
            }
        } else if let Some(mut s) = ed.preview.take() {
            let mut b = p;
            if shift {
                let (dx, dy) = (p.0 - s.a.0, p.1 - s.a.1);
                match s.kind {
                    Tool::Line | Tool::Arrow => {
                        let ang = dy.atan2(dx);
                        let snap = (ang / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
                        let len = (dx * dx + dy * dy).sqrt();
                        b = (s.a.0 + len * snap.cos(), s.a.1 + len * snap.sin());
                    }
                    Tool::Pen => {}
                    _ => {
                        let m = dx.abs().max(dy.abs());
                        b = (s.a.0 + m * dx.signum(), s.a.1 + m * dy.signum());
                    }
                }
            }
            s.b = b;
            if s.kind == Tool::Pen {
                s.pts.push(p);
            }
            if s.kind == Tool::Crop {
                let full = R { x: 0.0, y: 0.0, w: ed.doc.bg.width() as f64, h: ed.doc.bg.height() as f64 };
                ed.crop = Some(s.rect().intersect(&full));
            }
            ed.preview = Some(s);
        }
        drop(ed);
        self.refresh();
    }

    fn release(&self) {
        let mut ed = self.ed.borrow_mut();
        ed.move_orig = None;
        if let Some(s) = ed.preview.take() {
            match s.kind {
                Tool::Crop => {
                    if ed.crop.map(|r| r.w < 2.0 || r.h < 2.0).unwrap_or(true) {
                        ed.crop = None;
                    }
                }
                Tool::Pen => {
                    if s.pts.len() > 1 {
                        ed.checkpoint();
                        ed.doc.shapes.push(s);
                    }
                }
                _ => {
                    let r = s.rect();
                    if r.w >= 2.0 || r.h >= 2.0 {
                        ed.checkpoint();
                        ed.doc.shapes.push(s);
                    }
                }
            }
        }
        drop(ed);
        self.refresh();
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &cairo::Context) {
        let t = std::time::Instant::now();
        // hairline around the image in the theme's text colour, faint: keeps a
        // dark screenshot from melting into the dark canvas (and a white one
        // into the light canvas)
        let fg = crate::compat::fg_color(area);
        let edge = [fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.22];
        self.draw_impl(cr, edge);
        crate::trace("editor draw", t);
    }

    /// The background downscaled to `scale` device pixels per image pixel,
    /// computed once per zoom level. GTK records the draw function and
    /// replays it when rendering the frame; painting the full-size image
    /// with cairo's downscaling filter made every frame take ~170 ms for an
    /// 8K screenshot, even while just dragging a shape.
    fn scaled_bg(&self, bg: &ImageSurface, scale: f64) -> Option<ImageSurface> {
        let mut v = self.view.borrow_mut();
        if let Some(c) = v.as_ref() {
            if c.src.to_raw_none() == bg.to_raw_none() && c.scale == scale {
                return Some(c.img.clone());
            }
        }
        *v = None;
        let w = ((bg.width() as f64 * scale).round() as i32).max(1);
        let h = ((bg.height() as f64 * scale).round() as i32).max(1);
        let img = ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
        let cr = cairo::Context::new(&img).ok()?;
        cr.scale(w as f64 / bg.width() as f64, h as f64 / bg.height() as f64);
        cr.set_source_surface(bg, 0.0, 0.0).ok()?;
        cr.source().set_filter(cairo::Filter::Good);
        cr.paint().ok()?;
        drop(cr);
        *v = Some(ScaledBg { src: bg.clone(), scale, img: img.clone() });
        Some(img)
    }

    fn draw_impl(&self, cr: &cairo::Context, edge: [f64; 4]) {
        // device pixels per logical pixel (HiDPI), before our own transforms
        let ds = cr.matrix().xx().abs().max(0.01);
        let ed = self.ed.borrow();
        let (iw, ih) = (ed.doc.bg.width() as f64 * ed.zoom, ed.doc.bg.height() as f64 * ed.zoom);
        // soft drop shadow under the image
        for i in 0..14 {
            let g = i as f64 * 1.6;
            let a = 0.05 * (1.0 - i as f64 / 14.0);
            cr.set_source_rgba(0.0, 0.0, 0.0, a);
            rounded(cr, PAD - g, PAD - g + 6.0, iw + 2.0 * g, ih + 2.0 * g, 4.0 + g);
            cr.fill().ok();
        }
        // 1 device pixel, just outside the image
        let hair = 1.0 / ds;
        cr.set_source_rgba(edge[0], edge[1], edge[2], edge[3]);
        cr.set_line_width(hair);
        cr.rectangle(PAD - hair / 2.0, PAD - hair / 2.0, iw + hair, ih + hair);
        cr.stroke().ok();
        cr.translate(PAD, PAD);
        cr.rectangle(0.0, 0.0, iw, ih);
        cr.clip();
        cr.scale(ed.zoom, ed.zoom);
        if ed.zoom >= 2.0 {
            // crisp pixels when zoomed in
            cr.set_source_surface(&ed.doc.bg, 0.0, 0.0).ok();
            cr.source().set_filter(cairo::Filter::Nearest);
            cr.paint().ok();
            for s in &ed.doc.shapes {
                draw_shape(cr, s, &ed.doc.bg);
            }
        } else {
            let scale = ed.zoom * ds;
            let (bw, bh) = (ed.doc.bg.width() as f64, ed.doc.bg.height() as f64);
            match (scale < 0.999).then(|| self.scaled_bg(&ed.doc.bg, scale)).flatten() {
                Some(small) => {
                    cr.save().ok();
                    cr.scale(bw / small.width() as f64, bh / small.height() as f64);
                    cr.set_source_surface(&small, 0.0, 0.0).ok();
                    cr.paint().ok();
                    cr.restore().ok();
                    for s in &ed.doc.shapes {
                        draw_shape(cr, s, &ed.doc.bg);
                    }
                }
                None => {
                    *self.view.borrow_mut() = None;
                    draw_doc(cr, &ed.doc);
                }
            }
        }
        if let Some(p) = &ed.preview {
            if p.kind != Tool::Crop {
                draw_shape(cr, p, &ed.doc.bg);
            }
        }
        let lw = 1.0 / ed.zoom;
        // the selection outline may reach past the image edge
        cr.reset_clip();
        if let Some(i) = ed.sel {
            if let Some(s) = ed.doc.shapes.get(i) {
                let r = s.bbox();
                let pad = s.width / 2.0 + 3.0 * lw;
                cr.set_line_width(lw);
                cr.set_dash(&[4.0 * lw, 3.0 * lw], 0.0);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.rectangle(r.x - pad, r.y - pad, r.w + 2.0 * pad, r.h + 2.0 * pad);
                cr.stroke_preserve().ok();
                cr.set_dash(&[4.0 * lw, 3.0 * lw], 4.0 * lw);
                cr.set_source_rgb(0.13, 0.65, 0.40);
                cr.stroke().ok();
                cr.set_dash(&[], 0.0);
            }
        }
        if let Some(c) = ed.crop {
            let (w, h) = (ed.doc.bg.width() as f64, ed.doc.bg.height() as f64);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
            cr.rectangle(0.0, 0.0, w, h);
            cr.rectangle(c.x, c.y, c.w, c.h);
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            cr.fill().ok();
            cr.set_fill_rule(cairo::FillRule::Winding);
            cr.set_line_width(lw * 2.0);
            cr.set_source_rgb(0.17, 0.71, 0.45);
            cr.rectangle(c.x, c.y, c.w, c.h);
            cr.stroke().ok();
        }
    }

    fn key(self: &Rc<Self>, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let lower = key.to_lower();
        if ctrl {
            match lower {
                gdk::Key::s if shift => self.save_as(),
                gdk::Key::s => self.save(),
                gdk::Key::c => self.copy(),
                gdk::Key::p => self.print(),
                gdk::Key::z if shift => self.redo(),
                gdk::Key::z => self.undo(),
                gdk::Key::y => self.redo(),
                gdk::Key::o => {
                    let _ = self.ctx.tx.send_blocking(crate::Cmd::OpenDialog);
                }
                gdk::Key::w => self.ui.win.close(),
                gdk::Key::_0 => self.zoom_fit(),
                gdk::Key::_1 => self.set_zoom(1.0),
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => {
                    let z = self.ed.borrow().zoom * 1.25;
                    self.set_zoom(z)
                }
                gdk::Key::minus | gdk::Key::KP_Subtract => {
                    let z = self.ed.borrow().zoom * 0.8;
                    self.set_zoom(z)
                }
                _ => return glib::Propagation::Proceed,
            }
            return glib::Propagation::Stop;
        }
        match key {
            gdk::Key::Delete | gdk::Key::BackSpace => self.delete_selected(),
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if self.ed.borrow().crop.is_some() {
                    self.apply_crop();
                } else {
                    return glib::Propagation::Proceed;
                }
            }
            gdk::Key::Escape => {
                let mut ed = self.ed.borrow_mut();
                ed.crop = None;
                ed.sel = None;
                drop(ed);
                self.refresh();
            }
            _ => {
                if mods.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
                    return glib::Propagation::Proceed;
                }
                let Some(c) = lower.to_unicode() else { return glib::Propagation::Proceed };
                match TOOLS.iter().find(|t| t.3 == c) {
                    Some(t) => self.set_tool(t.0),
                    None => return glib::Propagation::Proceed,
                }
            }
        }
        glib::Propagation::Stop
    }

    fn undo(&self) {
        let mut ed = self.ed.borrow_mut();
        if let Some(d) = ed.undo.pop() {
            let cur = std::mem::replace(&mut ed.doc, d);
            ed.redo.push(cur);
            ed.sel = None;
            ed.dirty = true;
        }
        drop(ed);
        self.refresh();
    }

    fn redo(&self) {
        let mut ed = self.ed.borrow_mut();
        if let Some(d) = ed.redo.pop() {
            let cur = std::mem::replace(&mut ed.doc, d);
            ed.undo.push(cur);
            ed.sel = None;
            ed.dirty = true;
        }
        drop(ed);
        self.refresh();
    }

    fn delete_selected(&self) {
        let mut ed = self.ed.borrow_mut();
        if let Some(i) = ed.sel.take() {
            ed.checkpoint();
            ed.doc.shapes.remove(i);
            ed.renumber();
        }
        drop(ed);
        self.refresh();
    }

    fn apply_crop(&self) {
        let mut ed = self.ed.borrow_mut();
        let Some(c) = ed.crop.take() else { return };
        let (x, y) = (c.x.round() as i32, c.y.round() as i32);
        let (w, h) = (c.w.round() as i32, c.h.round() as i32);
        let bg = if w < 1 || h < 1 { Err(String::new()) } else { imageops::crop(&ed.doc.bg, x, y, w, h) };
        let bg = match bg {
            Ok(bg) => bg,
            Err(e) => {
                drop(ed);
                self.refresh();
                if !e.is_empty() {
                    self.ctx.error(Some(self.ui.win.upcast_ref()), &format!("Could not crop: {e}"));
                }
                return;
            }
        };
        ed.checkpoint();
        ed.doc.bg = bg;
        for s in &mut ed.doc.shapes {
            s.translate(-x as f64, -y as f64);
        }
        ed.sel = None;
        drop(ed);
        self.set_tool(Tool::Select);
        self.refresh();
    }

    fn text_popover(self: &Rc<Self>, p: Pt, edit: Option<usize>) {
        let zoom = self.ed.borrow().zoom;
        let pop = gtk::Popover::new();
        pop.set_parent(&self.ui.area);
        pop.set_pointing_to(Some(&gdk::Rectangle::new((p.0 * zoom + PAD) as i32, (p.1 * zoom + PAD) as i32, 1, 1)));
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let tv = gtk::TextView::new();
        tv.set_wrap_mode(gtk::WrapMode::WordChar);
        let sw = gtk::ScrolledWindow::new();
        sw.set_child(Some(&tv));
        sw.set_size_request(260, 90);
        let ok = gtk::Button::with_label(if edit.is_some() { "Update text" } else { "Add text" });
        ok.add_css_class("ls-primary");
        let hint = gtk::Label::new(Some("Ctrl+Enter to confirm"));
        hint.add_css_class("dim-label");
        bx.append(&sw);
        bx.append(&hint);
        bx.append(&ok);
        pop.set_child(Some(&bx));
        if let Some(i) = edit {
            tv.buffer().set_text(&self.ed.borrow().doc.shapes[i].text);
        }

        let commit = {
            let w = Rc::downgrade(self);
            let tv = tv.clone();
            let pop = pop.clone();
            Rc::new(move || {
                let b = tv.buffer();
                let text = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
                if let Some(e) = w.upgrade() {
                    let mut ed = e.ed.borrow_mut();
                    // the shape list may have changed while the popover was open
                    let edit = edit.filter(|&i| ed.doc.shapes.get(i).map(|s| s.kind) == Some(Tool::Text));
                    match edit {
                        Some(i) if text.trim().is_empty() => {
                            ed.checkpoint();
                            ed.doc.shapes.remove(i);
                            ed.sel = None;
                            ed.renumber();
                        }
                        Some(i) => {
                            ed.checkpoint();
                            ed.doc.shapes[i].text = text;
                        }
                        None if !text.trim().is_empty() => {
                            ed.checkpoint();
                            let mut s = ed.new_shape(Tool::Text, p);
                            s.text = text;
                            ed.doc.shapes.push(s);
                        }
                        None => {}
                    }
                    drop(ed);
                    e.refresh();
                }
                pop.popdown();
            })
        };
        {
            let c = commit.clone();
            ok.connect_clicked(move |_| c());
        }
        let k = gtk::EventControllerKey::new();
        {
            let c = commit.clone();
            k.connect_key_pressed(move |_, key, _, m| {
                if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && m.contains(gdk::ModifierType::CONTROL_MASK) {
                    c();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }
        tv.add_controller(k);
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
        tv.grab_focus();
    }

    /// The annotated image; reports the error (e.g. out of memory) itself.
    fn rendered(&self) -> Option<ImageSurface> {
        let r = render(&self.ed.borrow().doc);
        match r {
            Ok(img) => Some(img),
            Err(e) => {
                self.ctx.error(Some(self.ui.win.upcast_ref()), &format!("Could not render the image: {e}"));
                None
            }
        }
    }

    fn save_to(&self, path: PathBuf) -> bool {
        let Some(img) = self.rendered() else { return false };
        let q = self.ctx.cfg.borrow().jpeg_quality;
        match imageops::save(&img, &path, q) {
            Ok(()) => {
                {
                    let mut ed = self.ed.borrow_mut();
                    ed.dirty = false;
                    ed.path = Some(path.clone());
                }
                self.ctx.notify("Screenshot saved", &path.display().to_string(), Some(&path));
                self.toast(&format!("Saved {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                self.refresh();
                true
            }
            Err(err) => {
                self.ctx.error(Some(self.ui.win.upcast_ref()), &format!("Could not save {}:\n{err}", path.display()));
                false
            }
        }
    }

    fn save(&self) {
        let p = self.ed.borrow().path.clone();
        let p = p.unwrap_or_else(|| self.ctx.cfg.borrow().next_output_path());
        self.save_to(p);
    }

    fn save_as(self: &Rc<Self>) {
        let cfg = self.ctx.cfg.borrow().clone();
        let suggested = self.ed.borrow().path.clone().unwrap_or_else(|| cfg.next_output_path());
        let filters = [
            crate::compat::Filter { name: "PNG image", patterns: &["*.png"], pixbuf_formats: false },
            crate::compat::Filter { name: "JPEG image", patterns: &["*.jpg", "*.jpeg"], pixbuf_formats: false },
        ];
        let w = Rc::downgrade(self);
        crate::compat::save_file(Some(self.ui.win.upcast_ref()), "Save screenshot", &suggested, &filters, move |res| {
            if let (Some(mut p), Some(e)) = (res, w.upgrade()) {
                if p.extension().is_none() {
                    p.set_extension("png");
                }
                e.save_to(p);
            }
        });
    }

    fn copy(&self) {
        let Some(img) = self.rendered() else { return };
        imageops::copy_to_clipboard(&img);
        self.toast("Copied to clipboard");
    }

    fn print(&self) {
        let Some(img) = self.rendered() else { return };
        let op = gtk::PrintOperation::new();
        op.set_n_pages(1);
        op.set_job_name("Linshot screenshot");
        op.connect_draw_page(move |_, pc, _| {
            let cr = pc.cairo_context();
            let (pw, ph) = (pc.width(), pc.height());
            let (iw, ih) = (img.width() as f64, img.height() as f64);
            let s = (pw / iw).min(ph / ih).min(1.0);
            cr.scale(s, s);
            cr.set_source_surface(&img, 0.0, 0.0).ok();
            cr.paint().ok();
        });
        if let Err(e) = op.run(gtk::PrintOperationAction::PrintDialog, Some(&self.ui.win)) {
            self.ctx.error(Some(self.ui.win.upcast_ref()), &format!("Printing failed: {e}"));
        }
    }

    fn close_request(self: &Rc<Self>) -> glib::Propagation {
        let ed = self.ed.borrow();
        if ed.allow_close || !ed.dirty {
            drop(ed);
            // GTK 4 only disposes a closed window (and emits "destroy") once
            // nothing references it, but OPEN holds the editor which holds the
            // window: forget it here or every editor (with its images and undo
            // history) stays in memory for the life of the process
            OPEN.with(|o| o.borrow_mut().retain(|x| !Rc::ptr_eq(x, self)));
            return glib::Propagation::Proceed;
        }
        drop(ed);
        let w = Rc::downgrade(self);
        crate::compat::choose(
            Some(self.ui.win.upcast_ref()),
            "Save changes to this screenshot?",
            "Your annotations will be lost if you don't save them.",
            &["_Save", "_Discard", "_Cancel"],
            0,
            2,
            move |r| {
                let Some(e) = w.upgrade() else { return };
                let close = match r {
                    0 => {
                        let p = e.ed.borrow().path.clone().unwrap_or_else(|| e.ctx.cfg.borrow().next_output_path());
                        e.save_to(p)
                    }
                    1 => true,
                    _ => false,
                };
                if close {
                    e.ed.borrow_mut().allow_close = true;
                    e.ui.win.close();
                }
            },
        );
        glib::Propagation::Stop
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn doc(bg: &ImageSurface) -> Doc {
        Doc { bg: bg.clone(), shapes: Vec::new() }
    }

    #[test]
    fn undo_history_counts_distinct_backgrounds() {
        let a = ImageSurface::create(cairo::Format::ARgb32, 100, 100).unwrap();
        let b = ImageSurface::create(cairo::Format::ARgb32, 50, 50).unwrap();
        // shared with the current document: free
        assert_eq!(history_bytes(&doc(&a), &[doc(&a), doc(&a)]), 0);
        // counted once however many snapshots share it
        assert_eq!(history_bytes(&doc(&a), &[doc(&b), doc(&b), doc(&a)]), 50 * 4 * 50);
    }

    #[test]
    fn obfuscation_cache_reuses_and_is_bounded() {
        let bg = ImageSurface::create(cairo::Format::ARgb32, 64, 64).unwrap();
        let x = obfuscated(&bg, [0, 0, 32, 32, 4, 4]).unwrap();
        let y = obfuscated(&bg, [0, 0, 32, 32, 4, 4]).unwrap();
        assert_eq!(x.to_raw_none(), y.to_raw_none());
        for i in 0..100 {
            obfuscated(&bg, [i % 32, 0, 16, 16, 2, 2]).unwrap();
        }
        assert!(bg.user_data(&OBF_KEY).unwrap().0.borrow().len() <= OBF_CACHE_MAX);
    }
}
