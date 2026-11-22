// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Full-screen region / window selector shown over a frozen screenshot.
use cairo::ImageSurface;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl R {
    pub fn from_points(a: (f64, f64), b: (f64, f64)) -> R {
        R { x: a.0.min(b.0), y: a.1.min(b.1), w: (a.0 - b.0).abs(), h: (a.1 - b.1).abs() }
    }
    pub fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x && p.1 >= self.y && p.0 < self.x + self.w && p.1 < self.y + self.h
    }
    pub fn union(&self, o: &R) -> R {
        if self.w <= 0.0 {
            return *o;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        R { x, y, w: (self.x + self.w).max(o.x + o.w) - x, h: (self.y + self.h).max(o.y + o.h) - y }
    }
    pub fn intersect(&self, o: &R) -> R {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let x2 = (self.x + self.w).min(o.x + o.w);
        let y2 = (self.y + self.h).min(o.y + o.h);
        R { x, y, w: (x2 - x).max(0.0), h: (y2 - y).max(0.0) }
    }
}

/// Monitor geometries in logical (GTK) coordinates.
pub fn monitors() -> Vec<(gdk::Monitor, R)> {
    let mut out = Vec::new();
    if let Some(d) = gdk::Display::default() {
        let list = d.monitors();
        for i in 0..list.n_items() {
            if let Some(m) = list.item(i).and_downcast::<gdk::Monitor>() {
                let g = m.geometry();
                out.push((m, R { x: g.x() as f64, y: g.y() as f64, w: g.width() as f64, h: g.height() as f64 }));
            }
        }
    }
    out
}

struct State {
    img: ImageSurface,
    /// `img` with the dimming already applied (see `dimmed`)
    dim: Option<ImageSurface>,
    virt: R,
    sx: f64,
    sy: f64,
    windows: Vec<R>,
    window_mode: bool,
    magnifier: bool,
    cursor: (f64, f64),
    has_cursor: bool,
    start: Option<(f64, f64)>,
    cur: (f64, f64),
    done: bool,
}

impl State {
    fn selection(&self) -> Option<R> {
        let s = self.start?;
        let r = R::from_points(s, self.cur);
        (r.w >= 3.0 || r.h >= 3.0).then_some(r)
    }
    fn hover_window(&self) -> Option<R> {
        self.windows.iter().find(|w| w.contains(self.cursor)).map(|w| w.intersect(&self.virt))
    }
    /// Window mode only means something when there are windows to pick.
    fn picking_windows(&self) -> bool {
        self.window_mode && !self.windows.is_empty()
    }
    /// What a release / Enter would capture right now (and what is drawn).
    fn active(&self) -> Option<R> {
        if self.picking_windows() {
            return if self.has_cursor { self.hover_window() } else { None };
        }
        if let Some(s) = self.selection() {
            return Some(s);
        }
        // no real drag (hovering, or a plain click): the window under the pointer
        if !self.windows.is_empty() && self.has_cursor {
            return self.hover_window();
        }
        None
    }
    fn to_px(&self, r: &R) -> (i32, i32, i32, i32) {
        let x = ((r.x - self.virt.x) * self.sx).round() as i32;
        let y = ((r.y - self.virt.y) * self.sy).round() as i32;
        let x2 = ((r.x + r.w - self.virt.x) * self.sx).round() as i32;
        let y2 = ((r.y + r.h - self.virt.y) * self.sy).round() as i32;
        let (iw, ih) = (self.img.width(), self.img.height());
        let (x, y) = (x.clamp(0, iw), y.clamp(0, ih));
        let (x2, y2) = (x2.clamp(0, iw), y2.clamp(0, ih));
        (x, y, x2 - x, y2 - y)
    }
}

pub type Done = Box<dyn FnOnce(Option<(i32, i32, i32, i32)>)>;

/// Shows the selector. `windows` are window rectangles in image pixels.
pub fn select(
    app: &gtk::Application,
    img: ImageSurface,
    windows_px: Vec<(i32, i32, i32, i32)>,
    window_mode: bool,
    magnifier: bool,
    done: Done,
) {
    let mons = monitors();
    let virt = mons.iter().fold(R::default(), |a, (_, g)| a.union(g));
    let virt = if virt.w <= 0.0 {
        R { x: 0.0, y: 0.0, w: img.width() as f64, h: img.height() as f64 }
    } else {
        virt
    };
    let sx = img.width() as f64 / virt.w;
    let sy = img.height() as f64 / virt.h;
    let windows = windows_px
        .iter()
        .map(|&(x, y, w, h)| R { x: virt.x + x as f64 / sx, y: virt.y + y as f64 / sy, w: w as f64 / sx, h: h as f64 / sy })
        .collect();
    let dim = dimmed(&img);
    let st = Rc::new(RefCell::new(State {
        img,
        dim,
        virt,
        sx,
        sy,
        windows,
        window_mode,
        magnifier,
        cursor: (0.0, 0.0),
        has_cursor: false,
        start: None,
        cur: (0.0, 0.0),
        done: false,
    }));
    let done = Rc::new(RefCell::new(Some(done)));
    let wins: Rc<RefCell<Vec<gtk::Window>>> = Rc::new(RefCell::new(Vec::new()));
    let areas: Rc<RefCell<Vec<gtk::DrawingArea>>> = Rc::new(RefCell::new(Vec::new()));

    let redraw = {
        let areas = areas.clone();
        move || areas.borrow().iter().for_each(|a| a.queue_draw())
    };
    // monitor-list watch, disconnected when the overlay is done
    let watch: Rc<RefCell<Option<(gtk::gio::ListModel, glib::SignalHandlerId)>>> = Rc::new(RefCell::new(None));
    let finish = {
        let st = st.clone();
        let wins = wins.clone();
        let areas = areas.clone();
        let done = done.clone();
        let watch = watch.clone();
        Rc::new(move |result: Option<R>| {
            let px = {
                let mut s = st.borrow_mut();
                if s.done {
                    return;
                }
                s.done = true;
                result.map(|r| s.to_px(&r)).filter(|r| r.2 > 0 && r.3 > 0)
            };
            let to_close = wins.borrow().clone();
            for w in to_close {
                w.close();
            }
            let done = done.clone();
            let (wins, areas, watch) = (wins.clone(), areas.clone(), watch.clone());
            // let the compositor remove the overlay before anything else happens
            glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                if let Some(f) = done.borrow_mut().take() {
                    f(px);
                }
                // the widgets' handlers hold these lists: clearing them breaks the
                // reference cycle so the windows and the screenshot are freed
                wins.borrow_mut().clear();
                areas.borrow_mut().clear();
                if let Some((list, id)) = watch.borrow_mut().take() {
                    list.disconnect(id);
                }
            });
        })
    };

    if mons.is_empty() {
        let full = st.borrow().to_px(&virt);
        if let Some(f) = done.borrow_mut().take() {
            f(Some(full));
        }
        return;
    }
    if let Some(d) = gdk::Display::default() {
        // a monitor added or removed while selecting: the frozen screenshot no
        // longer matches the screen layout (and a window may have lost its
        // monitor), so cancel rather than crop the wrong area
        let list = d.monitors();
        let finish = finish.clone();
        let id = list.connect_items_changed(move |_, _, _, _| finish(None));
        *watch.borrow_mut() = Some((list, id));
    }

    for (mon, g) in mons.iter().cloned() {
        let win = gtk::Window::builder().application(app).decorated(false).title("Linshot selection").build();
        win.add_css_class("linshot-overlay");
        let area = gtk::DrawingArea::new();
        area.set_cursor_from_name(Some("crosshair"));
        area.set_focusable(true);
        win.set_child(Some(&area));

        {
            let st = st.clone();
            area.set_draw_func(move |_, cr, w, h| draw(&st.borrow(), cr, g, w as f64, h as f64));
        }
        // pointer controllers sit on the window, not the drawing area: when the
        // overlay maps under a pointer that has not moved, some window managers
        // (xfwm4) deliver the first press before the pointer has "entered" the
        // area, and only the window sees it
        let motion = gtk::EventControllerMotion::new();
        let track = {
            let st = st.clone();
            let redraw = redraw.clone();
            move |x: f64, y: f64| {
                let mut s = st.borrow_mut();
                s.cursor = (g.x + x, g.y + y);
                s.has_cursor = true;
                drop(s);
                redraw();
            }
        };
        {
            // the overlay opens under a pointer that has not moved yet: take
            // its position from the enter event, or window picking and the
            // hover highlight stay off until the first motion
            let track = track.clone();
            motion.connect_enter(move |_, x, y| track(x, y));
        }
        motion.connect_motion(move |_, x, y| track(x, y));
        win.add_controller(motion);

        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        {
            let st = st.clone();
            drag.connect_drag_begin(move |_, x, y| {
                let mut s = st.borrow_mut();
                let p = (g.x + x, g.y + y);
                s.start = Some(p);
                s.cur = p;
                s.cursor = p;
                s.has_cursor = true;
            });
        }
        {
            let st = st.clone();
            let redraw = redraw.clone();
            drag.connect_drag_update(move |d, dx, dy| {
                let mut s = st.borrow_mut();
                if let Some((x, y)) = d.start_point() {
                    s.cur = (g.x + x + dx, g.y + y + dy);
                    s.cursor = s.cur;
                }
                drop(s);
                redraw();
            });
        }
        {
            let st = st.clone();
            let finish = finish.clone();
            let redraw = redraw.clone();
            drag.connect_drag_end(move |_, _, _| {
                // capture exactly what is highlighted (Space may have
                // switched modes in the middle of the drag)
                let a = st.borrow().active();
                match a {
                    Some(r) => finish(Some(r)),
                    None => {
                        st.borrow_mut().start = None;
                        redraw();
                    }
                }
            });
        }
        win.add_controller(drag);

        let rclick = gtk::GestureClick::new();
        rclick.set_button(3);
        {
            let finish = finish.clone();
            rclick.connect_pressed(move |_, _, _, _| finish(None));
        }
        win.add_controller(rclick);

        let keys = gtk::EventControllerKey::new();
        {
            let st = st.clone();
            let finish = finish.clone();
            let redraw = redraw.clone();
            keys.connect_key_pressed(move |_, key, _, _| {
                match key {
                    gdk::Key::Escape => finish(None),
                    gdk::Key::space => {
                        let mut s = st.borrow_mut();
                        s.window_mode = !s.window_mode;
                        drop(s);
                        redraw();
                    }
                    gdk::Key::Return | gdk::Key::KP_Enter => {
                        let a = st.borrow().active();
                        if a.is_some() {
                            finish(a);
                        }
                    }
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            });
        }
        win.add_controller(keys);

        {
            // closed by the window manager (Alt+F4, ...): same as cancelling
            let st = st.clone();
            let finish = finish.clone();
            win.connect_close_request(move |_| {
                if st.borrow().done {
                    glib::Propagation::Proceed
                } else {
                    finish(None); // closes all overlay windows, this one included
                    glib::Propagation::Stop
                }
            });
        }

        win.fullscreen_on_monitor(&mon);
        win.present();
        area.grab_focus();
        areas.borrow_mut().push(area);
        wins.borrow_mut().push(win);
    }
}

const DIM: f64 = 0.45;

/// The screenshot darkened like the area outside the selection. Painting
/// this once per frame (plus the undimmed selection) instead of the image
/// and then a translucent full-screen fill saves one full-screen blend on
/// every pointer move, which matters on large or several monitors.
fn dimmed(img: &ImageSurface) -> Option<ImageSurface> {
    let out = ImageSurface::create(cairo::Format::ARgb32, img.width(), img.height()).ok()?;
    let cr = cairo::Context::new(&out).ok()?;
    cr.set_source_surface(img, 0.0, 0.0).ok()?;
    cr.paint().ok()?;
    cr.set_source_rgba(0.0, 0.0, 0.0, DIM);
    cr.paint().ok()?;
    drop(cr);
    Some(out)
}

fn draw(s: &State, cr: &cairo::Context, g: R, w: f64, h: f64) {
    let active = s.active().map(|r| R { x: r.x - g.x, y: r.y - g.y, ..r });
    let paint_img = |img: &ImageSurface| {
        cr.save().ok();
        cr.translate(s.virt.x - g.x, s.virt.y - g.y);
        cr.scale(1.0 / s.sx, 1.0 / s.sy);
        cr.set_source_surface(img, 0.0, 0.0).ok();
        cr.paint().ok();
        cr.restore().ok();
    };
    if let Some(dim) = &s.dim {
        // frozen screenshot, dimmed outside the selection
        paint_img(dim);
        if let Some(r) = active {
            cr.save().ok();
            cr.rectangle(r.x, r.y, r.w, r.h);
            cr.clip();
            paint_img(&s.img);
            cr.restore().ok();
        }
    } else {
        paint_img(&s.img);
        cr.set_source_rgba(0.0, 0.0, 0.0, DIM);
        cr.rectangle(0.0, 0.0, w, h);
        if let Some(r) = active {
            cr.rectangle(r.x, r.y, r.w, r.h);
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
        }
        cr.fill().ok();
        cr.set_fill_rule(cairo::FillRule::Winding);
    }

    let (cx, cy) = (s.cursor.0 - g.x, s.cursor.1 - g.y);
    let on_this = s.has_cursor && cx >= 0.0 && cy >= 0.0 && cx < w && cy < h;

    // crosshair
    if on_this && s.start.is_none() && !s.picking_windows() {
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
        cr.set_line_width(1.0);
        cr.move_to(0.0, cy.floor() + 0.5);
        cr.line_to(w, cy.floor() + 0.5);
        cr.move_to(cx.floor() + 0.5, 0.0);
        cr.line_to(cx.floor() + 0.5, h);
        cr.stroke().ok();
    }

    if let Some(r) = active {
        cr.set_source_rgb(0.17, 0.71, 0.45);
        cr.set_line_width(2.0);
        cr.rectangle(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0);
        cr.stroke().ok();
        let px = s.to_px(&R { x: r.x + g.x, y: r.y + g.y, ..r });
        // above the selection, or just inside it when it touches the top edge
        let ly = if r.y - 26.0 >= 2.0 { r.y - 26.0 } else { (r.y + 6.0).max(2.0) };
        label(cr, &format!("{} × {}", px.2, px.3), r.x.max(0.0) + if ly > r.y { 6.0 } else { 0.0 }, ly, w);
    }

    // help text: bottom left, moved to the top when it would cover the
    // selection or the pointer
    let help = if s.picking_windows() {
        "Window mode · click a window · Space: region mode · Esc: cancel"
    } else if s.windows.is_empty() {
        "Drag to select · Esc: cancel"
    } else {
        "Drag to select · click a window to capture it · Space: window mode · Esc: cancel"
    };
    if on_this {
        let hw = label_width(cr, help);
        let covers = |y: f64| {
            let hr = R { x: 12.0 - 8.0, y: y - 8.0, w: hw + 16.0, h: 24.0 + 16.0 };
            active.map(|r| r.intersect(&hr).w > 0.0 && r.intersect(&hr).h > 0.0).unwrap_or(false)
                || hr.contains((cx, cy))
        };
        let y = if covers(h - 36.0) && !covers(12.0) { 12.0 } else { h - 36.0 };
        label(cr, help, 12.0, y, w);
    }

    if on_this && s.magnifier {
        magnifier(s, cr, cx, cy, w, h);
    }
}

/// Does the one-time work of the first selector ahead of time: GTK creates
/// its renderer (GL setup) when the first window is realized, and cairo loads
/// the label font on first use. Done once in the background after startup,
/// this takes several hundred ms off the first Print press.
pub fn warm_up(app: &gtk::Application) {
    let t = std::time::Instant::now();
    if let Ok(img) = ImageSurface::create(cairo::Format::ARgb32, 64, 32) {
        if let Ok(cr) = cairo::Context::new(&img) {
            label(&cr, "0 × 0", 0.0, 0.0, 64.0);
        }
    }
    // realized (renderer and surface created) but never shown
    let win = gtk::Window::builder().application(app).decorated(false).title("Linshot").build();
    win.add_css_class("linshot-overlay");
    win.set_child(Some(&gtk::DrawingArea::new()));
    WidgetExt::realize(&win);
    win.destroy();
    crate::trace("warm up", t);
}

fn label_width(cr: &cairo::Context, text: &str) -> f64 {
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(13.0);
    cr.text_extents(text).map(|e| e.x_advance()).unwrap_or(text.len() as f64 * 8.0) + 16.0
}

fn label(cr: &cairo::Context, text: &str, x: f64, y: f64, maxw: f64) {
    let bw = label_width(cr, text);
    let x = x.min(maxw - bw - 2.0).max(2.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.75);
    rounded(cr, x, y, bw, 24.0, 5.0);
    cr.fill().ok();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.move_to(x + 8.0, y + 16.5);
    cr.show_text(text).ok();
}

pub fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

fn magnifier(s: &State, cr: &cairo::Context, cx: f64, cy: f64, w: f64, h: f64) {
    const SIZE: f64 = 128.0;
    const PIX: f64 = 15.0;
    let mut bx = cx + 24.0;
    let mut by = cy + 24.0;
    if bx + SIZE > w {
        bx = cx - 24.0 - SIZE;
    }
    if by + SIZE + 28.0 > h {
        by = cy - 24.0 - SIZE - 28.0;
    }
    let ix = ((s.cursor.0 - s.virt.x) * s.sx).floor();
    let iy = ((s.cursor.1 - s.virt.y) * s.sy).floor();
    let k = SIZE / PIX;

    cr.save().ok();
    cr.rectangle(bx, by, SIZE, SIZE);
    cr.clip();
    cr.set_source_rgb(0.1, 0.1, 0.1);
    cr.paint().ok();
    cr.translate(bx, by);
    cr.scale(k, k);
    cr.set_source_surface(&s.img, -(ix - (PIX / 2.0).floor()), -(iy - (PIX / 2.0).floor())).ok();
    cr.source().set_filter(cairo::Filter::Nearest);
    cr.paint().ok();
    cr.restore().ok();

    // center pixel box
    let c = (PIX / 2.0).floor() * k;
    cr.set_source_rgba(0.17, 0.71, 0.45, 0.9);
    cr.set_line_width(1.5);
    cr.rectangle(bx + c, by + c, k, k);
    cr.stroke().ok();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(2.0);
    cr.rectangle(bx, by, SIZE, SIZE);
    cr.stroke().ok();

    // pixel colour + coordinates
    let mut rgb = String::new();
    if ix >= 0.0 && iy >= 0.0 && (ix as i32) < s.img.width() && (iy as i32) < s.img.height() {
        let stride = s.img.stride() as usize;
        let off = iy as usize * stride + ix as usize * 4;
        let _ = s.img.with_data(|d| {
            if off + 4 <= d.len() {
                let px = u32::from_ne_bytes(d[off..off + 4].try_into().unwrap());
                rgb = format!("  #{:06X}", px & 0xff_ffff);
            }
        });
    }
    label(cr, &format!("{}, {}{}", ix as i64, iy as i64, rgb), bx, by + SIZE + 4.0, w);
}
