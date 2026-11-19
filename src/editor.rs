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
    (Tool::Pixelate, "▦", "Pixelate (O)", 'o'),
    (Tool::Blur, "◍", "Blur (B)", 'b'),
    (Tool::Counter, "①", "Step counter (I)", 'i'),
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
    let Ok(small) = ImageSurface::create(cairo::Format::ARgb32, sw, sh) else { return };
    {
        let c = cairo::Context::new(&small).unwrap();
        c.scale(sw as f64 / rw, sh as f64 / rh);
        c.set_source_surface(bg, -rx, -ry).ok();
        c.source().set_filter(cairo::Filter::Good);
        c.paint().ok();
    }
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

fn render(d: &Doc) -> ImageSurface {
    let out = ImageSurface::create(cairo::Format::ARgb32, d.bg.width(), d.bg.height()).unwrap();
    {
        let cr = cairo::Context::new(&out).unwrap();
        draw_doc(&cr, d);
    }
    out
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
        }
    }
}

struct Ui {
    win: gtk::ApplicationWindow,
    area: gtk::DrawingArea,
    scroller: gtk::ScrolledWindow,
    status: gtk::Label,
    crop_btn: gtk::Button,
    undo_btn: gtk::Button,
    redo_btn: gtk::Button,
    del_btn: gtk::Button,
    tools: Vec<(Tool, gtk::ToggleButton)>,
    color: gtk::ColorDialogButton,
    fill: gtk::ColorDialogButton,
    fill_chk: gtk::CheckButton,
    width: gtk::SpinButton,
    font: gtk::SpinButton,
    shadow: gtk::CheckButton,
}

pub struct Editor {
    ed: RefCell<Ed>,
    ui: Ui,
    ctx: Rc<Ctx>,
    syncing: Cell<bool>,
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
    b
}

pub fn open(ctx: &Rc<Ctx>, img: ImageSurface, title: Option<String>) {
    let cfg = ctx.cfg.borrow().clone();
    let win = gtk::ApplicationWindow::builder()
        .application(&ctx.app)
        .title(title.unwrap_or_else(|| "Linshot".into()))
        .icon_name("linshot")
        .build();

    // ---- header bar
    let header = gtk::HeaderBar::new();
    let save_btn = icon_button("document-save-symbolic", "Save (Ctrl+S)");
    let save_as_btn = icon_button("document-save-as-symbolic", "Save as… (Ctrl+Shift+S)");
    let copy_btn = icon_button("edit-copy-symbolic", "Copy image to clipboard (Ctrl+C)");
    let print_btn = icon_button("document-print-symbolic", "Print (Ctrl+P)");
    let undo_btn = icon_button("edit-undo-symbolic", "Undo (Ctrl+Z)");
    let redo_btn = icon_button("edit-redo-symbolic", "Redo (Ctrl+Y)");
    let del_btn = icon_button("edit-delete-symbolic", "Delete selected (Del)");
    let zoom_out = icon_button("zoom-out-symbolic", "Zoom out (Ctrl+-)");
    let zoom_fit = icon_button("zoom-fit-best-symbolic", "Best fit (Ctrl+0)");
    let zoom_100 = icon_button("zoom-original-symbolic", "Actual size (Ctrl+1)");
    let zoom_in = icon_button("zoom-in-symbolic", "Zoom in (Ctrl++)");
    let open_btn = icon_button("document-open-symbolic", "Open image… (Ctrl+O)");
    for b in [&open_btn, &save_btn, &save_as_btn, &copy_btn, &print_btn] {
        header.pack_start(b);
    }
    let zoom_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    zoom_box.add_css_class("linked");
    for b in [&zoom_out, &zoom_fit, &zoom_100, &zoom_in] {
        zoom_box.append(b);
    }
    header.pack_end(&zoom_box);
    header.pack_end(&del_btn);
    header.pack_end(&redo_btn);
    header.pack_end(&undo_btn);
    win.set_titlebar(Some(&header));

    // ---- property bar
    let props = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    props.set_margin_start(8);
    props.set_margin_end(8);
    props.set_margin_top(4);
    props.set_margin_bottom(4);
    let dialog = gtk::ColorDialog::builder().with_alpha(true).build();
    let color = gtk::ColorDialogButton::new(Some(dialog.clone()));
    color.set_rgba(&to_rgba(cfg.line_color));
    color.set_tooltip_text(Some("Line / text colour"));
    let fill = gtk::ColorDialogButton::new(Some(dialog));
    fill.set_rgba(&to_rgba(cfg.fill_color));
    fill.set_tooltip_text(Some("Fill colour"));
    let fill_chk = gtk::CheckButton::with_label("Fill");
    fill_chk.set_active(cfg.fill_enabled);
    let width = gtk::SpinButton::with_range(1.0, 30.0, 1.0);
    width.set_value(cfg.line_width);
    width.set_tooltip_text(Some("Line width (also obfuscation strength)"));
    let font = gtk::SpinButton::with_range(6.0, 120.0, 1.0);
    font.set_value(cfg.font_size);
    font.set_tooltip_text(Some("Font size (text and counters)"));
    let shadow = gtk::CheckButton::with_label("Shadow");
    shadow.set_active(cfg.shadow);
    let crop_btn = gtk::Button::with_label("✔ Apply crop");
    crop_btn.set_tooltip_text(Some("Apply crop (Enter) · Esc cancels"));
    crop_btn.add_css_class("suggested-action");
    crop_btn.set_visible(false);
    props.append(&gtk::Label::new(Some("Colour")));
    props.append(&color);
    props.append(&fill_chk);
    props.append(&fill);
    props.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    props.append(&gtk::Label::new(Some("Width")));
    props.append(&width);
    props.append(&gtk::Label::new(Some("Font")));
    props.append(&font);
    props.append(&shadow);
    props.append(&crop_btn);

    // ---- tools
    let toolbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    toolbox.set_margin_start(4);
    toolbox.set_margin_end(4);
    toolbox.set_margin_top(4);
    let mut tools = Vec::new();
    let mut first: Option<gtk::ToggleButton> = None;
    for (t, glyph, tip, _) in TOOLS {
        let b = gtk::ToggleButton::with_label(glyph);
        b.set_tooltip_text(Some(tip));
        b.add_css_class("flat");
        b.set_size_request(36, 36);
        if let Some(f) = &first {
            b.set_group(Some(f));
        } else {
            first = Some(b.clone());
        }
        toolbox.append(&b);
        tools.push((*t, b));
    }

    // ---- canvas
    let area = gtk::DrawingArea::new();
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_focusable(true);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&area));
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);
    scroller.add_css_class("linshot-canvas");

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_margin_start(8);
    status.set_margin_bottom(3);
    status.set_margin_top(3);
    status.add_css_class("dim-label");

    let mid = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mid.append(&toolbox);
    mid.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    mid.append(&scroller);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&props);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&mid);
    root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    root.append(&status);
    win.set_child(Some(&root));

    // initial size / zoom
    let (iw, ih) = (img.width() as f64, img.height() as f64);
    let (mw, mh) = crate::overlay::monitors()
        .first()
        .map(|(_, g)| (g.w, g.h))
        .unwrap_or((1366.0, 768.0));
    let (maxw, maxh) = (mw * 0.85, mh * 0.85);
    let chrome = (60.0, 130.0);
    let zoom = (1.0f64).min((maxw - chrome.0) / iw).min((maxh - chrome.1) / ih).max(0.05);
    let ww = (iw * zoom + chrome.0).clamp(960.0, maxw.max(960.0));
    let wh = (ih * zoom + chrome.1).clamp(560.0, maxh.max(560.0));
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
            doc: Doc { bg: img, shapes: Vec::new() },
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
            status,
            crop_btn: crop_btn.clone(),
            undo_btn: undo_btn.clone(),
            redo_btn: redo_btn.clone(),
            del_btn: del_btn.clone(),
            tools,
            color: color.clone(),
            fill: fill.clone(),
            fill_chk: fill_chk.clone(),
            width: width.clone(),
            font: font.clone(),
            shadow: shadow.clone(),
        },
        ctx: ctx.clone(),
        syncing: Cell::new(false),
    });
    OPEN.with(|o| o.borrow_mut().push(e.clone()));

    // ---- wiring (closures hold weak references to avoid cycles)
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
    on!(save_as_btn, save_as);
    on!(copy_btn, copy);
    on!(print_btn, print);
    on!(undo_btn, undo);
    on!(redo_btn, redo);
    on!(del_btn, delete_selected);
    on!(crop_btn, apply_crop);
    on!(zoom_fit, zoom_fit);
    {
        let tx = ctx.tx.clone();
        open_btn.connect_clicked(move |_| {
            let _ = tx.send_blocking(crate::Cmd::OpenDialog);
        });
    }
    for (b, f) in [(&zoom_in, 1.25), (&zoom_out, 0.8), (&zoom_100, 0.0)] {
        let w = Rc::downgrade(&e);
        b.connect_clicked(move |_| {
            if let Some(e) = w.upgrade() {
                let z = if f == 0.0 { 1.0 } else { e.ed.borrow().zoom * f };
                e.set_zoom(z);
            }
        });
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
        area.set_draw_func(move |_, cr, _, _| {
            if let Some(e) = w.upgrade() {
                e.draw(cr);
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
                    let hit = e.hit(p);
                    if let Some(i) = hit {
                        if e.ed.borrow().doc.shapes[i].kind == Tool::Text {
                            e.text_popover(p, Some(i));
                        }
                    }
                }
            }
        });
    }
    area.add_controller(dbl);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    {
        let w = Rc::downgrade(&e);
        scroll.connect_scroll(move |c, _, dy| {
            let Some(e) = w.upgrade() else { return glib::Propagation::Proceed };
            if c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                let z = e.ed.borrow().zoom * if dy < 0.0 { 1.15 } else { 1.0 / 1.15 };
                e.set_zoom(z);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    scroller.add_controller(scroll);

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
        (x / z, y / z)
    }

    fn hit(&self, p: Pt) -> Option<usize> {
        let ed = self.ed.borrow();
        ed.doc.shapes.iter().rposition(|s| s.hit(p))
    }

    fn refresh(&self) {
        let ed = self.ed.borrow();
        let (w, h) = (ed.doc.bg.width(), ed.doc.bg.height());
        self.ui.area.set_content_width((w as f64 * ed.zoom).round() as i32);
        self.ui.area.set_content_height((h as f64 * ed.zoom).round() as i32);
        self.ui.undo_btn.set_sensitive(!ed.undo.is_empty());
        self.ui.redo_btn.set_sensitive(!ed.redo.is_empty());
        self.ui.del_btn.set_sensitive(ed.sel.is_some());
        self.ui.crop_btn.set_visible(ed.crop.is_some());
        let saved = match (&ed.path, ed.dirty) {
            (Some(p), false) => format!(" · saved to {}", p.display()),
            (_, true) => " · modified".into(),
            _ => String::new(),
        };
        self.ui.status.set_text(&format!("{w} × {h} px · {:.0}%{saved}", ed.zoom * 100.0));
        self.ui.area.queue_draw();
    }

    fn set_zoom(&self, z: f64) {
        self.ed.borrow_mut().zoom = z.clamp(0.05, 16.0);
        self.refresh();
    }

    fn zoom_fit(&self) {
        let (w, h) = {
            let ed = self.ed.borrow();
            (ed.doc.bg.width() as f64, ed.doc.bg.height() as f64)
        };
        let aw = self.ui.scroller.width() as f64 - 4.0;
        let ah = self.ui.scroller.height() as f64 - 4.0;
        if aw > 10.0 && ah > 10.0 {
            self.set_zoom((aw / w).min(ah / h).min(1.0));
        }
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
        self.ui.area.set_cursor_from_name(Some(match t {
            Tool::Select => "default",
            Tool::Text => "text",
            _ => "crosshair",
        }));
        self.refresh();
    }

    fn style_changed(&self) {
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
            ed.checkpoint();
            let s = &mut ed.doc.shapes[i];
            if !matches!(s.kind, Tool::Highlight | Tool::Pixelate | Tool::Blur) {
                s.color = style.color;
                s.fill = style.fill_on.then_some(style.fill);
            }
            s.width = style.width;
            s.font = style.font;
            s.shadow = style.shadow;
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
    }

    fn press(self: &Rc<Self>, p: Pt, _mods: gdk::ModifierType) {
        let tool = self.ed.borrow().tool;
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
                self.ed.borrow_mut().crop = Some(R { x: p.0, y: p.1, w: 0.0, h: 0.0 });
                self.ed.borrow_mut().preview = Some(self.ed.borrow().new_shape(Tool::Crop, p));
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
        if ed.tool == Tool::Select {
            if let (Some(i), Some(orig)) = (ed.sel, ed.move_orig.clone()) {
                if !ed.move_saved {
                    ed.checkpoint();
                    ed.move_saved = true;
                }
                let mut s = orig;
                s.translate(p.0 - ed.drag_start.0, p.1 - ed.drag_start.1);
                ed.doc.shapes[i] = s;
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

    fn draw(&self, cr: &cairo::Context) {
        let ed = self.ed.borrow();
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
            draw_doc(cr, &ed.doc);
        }
        if let Some(p) = &ed.preview {
            if p.kind != Tool::Crop {
                draw_shape(cr, p, &ed.doc.bg);
            }
        }
        let lw = 1.0 / ed.zoom;
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
                cr.set_source_rgb(0.1, 0.4, 0.9);
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
        if w < 1 || h < 1 {
            return;
        }
        ed.checkpoint();
        ed.doc.bg = imageops::crop(&ed.doc.bg, x, y, w, h);
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
        pop.set_pointing_to(Some(&gdk::Rectangle::new((p.0 * zoom) as i32, (p.1 * zoom) as i32, 1, 1)));
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let tv = gtk::TextView::new();
        tv.set_wrap_mode(gtk::WrapMode::WordChar);
        let sw = gtk::ScrolledWindow::new();
        sw.set_child(Some(&tv));
        sw.set_size_request(260, 90);
        let ok = gtk::Button::with_label(if edit.is_some() { "Update text" } else { "Add text" });
        ok.add_css_class("suggested-action");
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

    fn rendered(&self) -> ImageSurface {
        render(&self.ed.borrow().doc)
    }

    fn save_to(&self, path: PathBuf) -> bool {
        let img = self.rendered();
        let q = self.ctx.cfg.borrow().jpeg_quality;
        match imageops::save(&img, &path, q) {
            Ok(()) => {
                {
                    let mut ed = self.ed.borrow_mut();
                    ed.dirty = false;
                    ed.path = Some(path.clone());
                }
                self.ctx.notify("Screenshot saved", &path.display().to_string(), Some(&path));
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
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        for (name, pats) in [("PNG image", ["*.png", ""]), ("JPEG image", ["*.jpg", "*.jpeg"])] {
            let f = gtk::FileFilter::new();
            f.set_name(Some(name));
            for p in pats.iter().filter(|p| !p.is_empty()) {
                f.add_pattern(p);
            }
            filters.append(&f);
        }
        let dlg = gtk::FileDialog::builder()
            .title("Save screenshot")
            .initial_name(suggested.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            .filters(&filters)
            .modal(true)
            .build();
        if let Some(d) = suggested.parent() {
            dlg.set_initial_folder(Some(&gio::File::for_path(d)));
        }
        let w = Rc::downgrade(self);
        dlg.save(Some(&self.ui.win), gio::Cancellable::NONE, move |res| {
            if let (Ok(f), Some(e)) = (res, w.upgrade()) {
                if let Some(mut p) = f.path() {
                    if p.extension().is_none() {
                        p.set_extension("png");
                    }
                    e.save_to(p);
                }
            }
        });
    }

    fn copy(&self) {
        imageops::copy_to_clipboard(&self.rendered());
        self.ctx.notify("Copied to clipboard", "The annotated screenshot is on the clipboard.", None);
        self.ui.status.set_text("Copied to clipboard");
    }

    fn print(&self) {
        let img = self.rendered();
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
            return glib::Propagation::Proceed;
        }
        drop(ed);
        let dlg = gtk::AlertDialog::builder()
            .modal(true)
            .message("Save changes to this screenshot?")
            .detail("Your annotations will be lost if you don't save them.")
            .buttons(["Save", "Discard", "Cancel"])
            .default_button(0)
            .cancel_button(2)
            .build();
        let w = Rc::downgrade(self);
        dlg.choose(Some(&self.ui.win), gio::Cancellable::NONE, move |r| {
            let Some(e) = w.upgrade() else { return };
            let close = match r {
                Ok(0) => {
                    let p = e.ed.borrow().path.clone().unwrap_or_else(|| e.ctx.cfg.borrow().next_output_path());
                    e.save_to(p)
                }
                Ok(1) => true,
                _ => false,
            };
            if close {
                e.ed.borrow_mut().allow_close = true;
                e.ui.win.close();
            }
        });
        glib::Propagation::Stop
    }
}

