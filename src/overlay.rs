// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Full-screen region / window selector shown over a frozen screenshot.
use cairo::ImageSurface;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
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

/// Edge bits of a resize.
const LEFT: u8 = 1;
const TOP: u8 = 2;
const RIGHT: u8 = 4;
const BOTTOM: u8 = 8;
/// How close (logical px) to an edge a press grabs it.
const HANDLE: f64 = 9.0;
/// The smallest selection adjusting can leave (so Enter always captures something).
const MIN_SEL: f64 = 4.0;
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    /// outside the selection: starts a new one
    New,
    Move,
    Resize(u8),
    /// a button of the confirm bar: 0 cancel, 1 capture, 2 the bar itself
    Bar(u8),
}

/// What a press at `p` does to the selection `r`: grab an edge or corner
/// (also from just outside it), move it from inside, or start over.
fn hit_test(r: &R, p: (f64, f64)) -> Hit {
    // small selections keep an interior to move them by
    let t = HANDLE.min(r.w.min(r.h) / 3.0).max(3.0);
    let near = |v: f64, edge: f64| (v - edge).abs() <= t;
    if !(p.0 >= r.x - t && p.0 <= r.x + r.w + t && p.1 >= r.y - t && p.1 <= r.y + r.h + t) {
        return Hit::New;
    }
    let mut e = 0;
    if near(p.0, r.x) {
        e |= LEFT;
    } else if near(p.0, r.x + r.w) {
        e |= RIGHT;
    }
    if near(p.1, r.y) {
        e |= TOP;
    } else if near(p.1, r.y + r.h) {
        e |= BOTTOM;
    }
    if e != 0 {
        Hit::Resize(e)
    } else if r.contains(p) {
        Hit::Move
    } else {
        Hit::New
    }
}

fn cursor_for(h: Hit) -> &'static str {
    match h {
        Hit::New => "crosshair",
        Hit::Move => "move",
        Hit::Bar(_) => "pointer",
        Hit::Resize(e) => match (e & (LEFT | RIGHT), e & (TOP | BOTTOM)) {
            (0, _) => "ns-resize",
            (_, 0) => "ew-resize",
            (x, y) if (x == LEFT) == (y == TOP) => "nwse-resize",
            _ => "nesw-resize",
        },
    }
}

/// `orig` with the given edges moved by `d`, inside `bounds`. Dragging an
/// edge past the opposite one flips the rectangle.
fn resized(orig: &R, edges: u8, d: (f64, f64), bounds: &R) -> R {
    let (mut x0, mut y0, mut x1, mut y1) = (orig.x, orig.y, orig.x + orig.w, orig.y + orig.h);
    if edges & LEFT != 0 {
        x0 += d.0;
    }
    if edges & RIGHT != 0 {
        x1 += d.0;
    }
    if edges & TOP != 0 {
        y0 += d.1;
    }
    if edges & BOTTOM != 0 {
        y1 += d.1;
    }
    let cx = |v: f64| v.clamp(bounds.x, bounds.x + bounds.w);
    let cy = |v: f64| v.clamp(bounds.y, bounds.y + bounds.h);
    let mut r = R::from_points((cx(x0), cy(y0)), (cx(x1), cy(y1)));
    if r.w < MIN_SEL {
        r.w = MIN_SEL;
        r.x = r.x.min(bounds.x + bounds.w - MIN_SEL);
    }
    if r.h < MIN_SEL {
        r.h = MIN_SEL;
        r.y = r.y.min(bounds.y + bounds.h - MIN_SEL);
    }
    r
}

/// `orig` moved by `d`, kept inside `bounds`.
fn moved(orig: &R, d: (f64, f64), bounds: &R) -> R {
    let x = (orig.x + d.0).clamp(bounds.x, (bounds.x + bounds.w - orig.w).max(bounds.x));
    let y = (orig.y + d.1).clamp(bounds.y, (bounds.y + bounds.h - orig.h).max(bounds.y));
    R { x, y, ..*orig }
}

const BTN_W: f64 = 96.0;
const BTN_H: f64 = 30.0;
const BAR_GAP: f64 = 6.0;
const BAR_PAD: f64 = 4.0;

/// The confirm bar beside the selection `sel`: its outline and the Cancel and
/// Capture buttons. Below the selection, right-aligned; above it, or inside
/// its corner, when the screen has no room below.
fn bar_rects(sel: &R, bounds: &R) -> (R, [R; 2]) {
    let w = 2.0 * BTN_W + BAR_GAP + 2.0 * BAR_PAD;
    let h = BTN_H + 2.0 * BAR_PAD;
    let (x0, y0) = (bounds.x + 4.0, bounds.y + 4.0);
    let (x1, y1) = (bounds.x + bounds.w - 4.0, bounds.y + bounds.h - 4.0);
    let below = sel.y + sel.h + 8.0;
    let y = if below + h <= y1 {
        below
    } else if sel.y - 8.0 - h >= y0 {
        sel.y - 8.0 - h
    } else {
        (sel.y + sel.h - h - 12.0).max(y0)
    };
    let x = (sel.x + sel.w - w).clamp(x0, (x1 - w).max(x0));
    let btn = |i: f64| R { x: x + BAR_PAD + i * (BTN_W + BAR_GAP), y: y + BAR_PAD, w: BTN_W, h: BTN_H };
    (R { x, y, w, h }, [btn(0.0), btn(1.0)])
}

/// Which part of the confirm bar is under `p`: 0 Cancel, 1 Capture, 2 the rest.
fn bar_hit(sel: &R, bounds: &R, p: (f64, f64)) -> Option<u8> {
    let (bar, btns) = bar_rects(sel, bounds);
    if !bar.contains(p) {
        return None;
    }
    Some(btns.iter().position(|b| b.contains(p)).map_or(2, |i| i as u8))
}

/// What a press at `p` does while adjusting: the confirm bar, else the selection.
fn press_hit(a: &Adjust, bounds: &R, p: (f64, f64)) -> Hit {
    match bar_hit(&a.sel, bounds, p) {
        Some(i) => Hit::Bar(i),
        None => hit_test(&a.sel, p),
    }
}

/// How far an arrow key nudges the selection, in logical px: one image pixel
/// (ten with Shift), so it is exact on HiDPI screens too.
fn nudge_delta(key: gdk::Key, shift: bool, (sx, sy): (f64, f64)) -> Option<(f64, f64)> {
    let n = if shift { 10.0 } else { 1.0 };
    let (dx, dy) = match key {
        gdk::Key::Left | gdk::Key::KP_Left => (-1.0, 0.0),
        gdk::Key::Right | gdk::Key::KP_Right => (1.0, 0.0),
        gdk::Key::Up | gdk::Key::KP_Up => (0.0, -1.0),
        gdk::Key::Down | gdk::Key::KP_Down => (0.0, 1.0),
        _ => return None,
    };
    Some((dx * n / sx, dy * n / sy))
}

/// The selection being adjusted after a drag.
struct Adjust {
    sel: R,
    /// what the current drag does, with the selection as it was at its start
    grab: Option<(Hit, R)>,
    last_press: Option<(std::time::Instant, (f64, f64))>,
    /// the current press is the second of a double click
    double: bool,
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
    /// a drag makes a selection that can be adjusted (off: capture on release)
    adjustable: bool,
    /// set once a drag has made a selection: it can then be adjusted
    adjust: Option<Adjust>,
    /// the current drag began outside an adjustable selection
    restart: bool,
    /// Ctrl+C: copy the area to the clipboard and skip the editor
    copy_only: bool,
    cursor_name: &'static str,
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
        if let Some(a) = &self.adjust {
            return Some(a.sel);
        }
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

/// Called once with the chosen area in image pixels (None: cancelled) and
/// whether to only copy it to the clipboard (Ctrl+C) instead of the usual action.
pub type Done = Box<dyn FnOnce(Option<(i32, i32, i32, i32)>, bool)>;

/// Shows the selector. `windows` are window rectangles in image pixels.
pub fn select(
    app: &gtk::Application,
    img: ImageSurface,
    windows_px: Vec<(i32, i32, i32, i32)>,
    window_mode: bool,
    magnifier: bool,
    adjust: bool,
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
        adjustable: adjust,
        adjust: None,
        restart: false,
        copy_only: false,
        cursor_name: "crosshair",
        done: false,
    }));
    let done = Rc::new(RefCell::new(Some(done)));
    let wins: Rc<RefCell<Vec<gtk::Window>>> = Rc::new(RefCell::new(Vec::new()));
    let areas: Rc<RefCell<Vec<gtk::DrawingArea>>> = Rc::new(RefCell::new(Vec::new()));

    let redraw = {
        let areas = areas.clone();
        move || areas.borrow().iter().for_each(|a| a.queue_draw())
    };
    // cursor shape: resize / move arrows over an adjustable selection
    let set_cursor = {
        let (st, areas) = (st.clone(), areas.clone());
        move || {
            let mut s = st.borrow_mut();
            let name = match &s.adjust {
                Some(a) if a.grab.is_some() => return,
                Some(a) => cursor_for(press_hit(a, &s.virt, s.cursor)),
                None => "crosshair",
            };
            if s.cursor_name == name {
                return;
            }
            s.cursor_name = name;
            drop(s);
            areas.borrow().iter().for_each(|a| a.set_cursor_from_name(Some(name)));
        }
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
            let (px, copy) = {
                let mut s = st.borrow_mut();
                if s.done {
                    return;
                }
                s.done = true;
                (result.map(|r| s.to_px(&r)).filter(|r| r.2 > 0 && r.3 > 0), s.copy_only)
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
                    f(px, copy);
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
            f(Some(full), false);
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

    // no X11 window manager: fullscreen requests are ignored, so size and
    // place the windows ourselves (each needs its own title to find it)
    let unmanaged = crate::capture::is_x11() && !crate::capture::x11_has_wm();
    for (i, (mon, g)) in mons.iter().cloned().enumerate() {
        let title = if unmanaged { format!("Linshot selection {}", i + 1) } else { "Linshot selection".to_string() };
        let win = gtk::Window::builder()
            .application(app)
            .decorated(false)
            .title(&title)
            .default_width(g.w as i32)
            .default_height(g.h as i32)
            .build();
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
            let set_cursor = set_cursor.clone();
            move |x: f64, y: f64| {
                let mut s = st.borrow_mut();
                s.cursor = (g.x + x, g.y + y);
                s.has_cursor = true;
                drop(s);
                set_cursor();
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
                s.cursor = p;
                s.has_cursor = true;
                let bounds = s.virt;
                match s.adjust.as_ref().map(|a| press_hit(a, &bounds, p)) {
                    // the first press of a drag: a new selection
                    None => {
                        s.start = Some(p);
                        s.cur = p;
                    }
                    // pressed outside the one being adjusted: start over
                    Some(Hit::New) => {
                        s.adjust = None;
                        s.restart = true;
                        s.start = Some(p);
                        s.cur = p;
                    }
                    Some(h) => {
                        if let Some(a) = s.adjust.as_mut() {
                            let double = a
                                .last_press
                                .is_some_and(|(t, q)| t.elapsed() < DOUBLE_CLICK && (q.0 - p.0).abs() <= 5.0 && (q.1 - p.1).abs() <= 5.0);
                            a.last_press = Some((std::time::Instant::now(), p));
                            a.double = double && !matches!(h, Hit::Bar(_));
                            a.grab = Some((h, a.sel));
                        }
                    }
                }
            });
        }
        {
            let st = st.clone();
            let redraw = redraw.clone();
            drag.connect_drag_update(move |d, dx, dy| {
                let mut s = st.borrow_mut();
                if let Some((x, y)) = d.start_point() {
                    let p = (g.x + x + dx, g.y + y + dy);
                    s.cursor = p;
                    let bounds = s.virt;
                    match s.adjust.as_mut() {
                        Some(a) => {
                            if let Some((hit, orig)) = a.grab {
                                a.sel = match hit {
                                    Hit::Move => moved(&orig, (dx, dy), &bounds),
                                    Hit::Resize(e) => resized(&orig, e, (dx, dy), &bounds),
                                    Hit::New | Hit::Bar(_) => a.sel,
                                };
                            }
                        }
                        None => s.cur = p,
                    }
                }
                drop(s);
                redraw();
            });
        }
        {
            let st = st.clone();
            let finish = finish.clone();
            let redraw = redraw.clone();
            let set_cursor = set_cursor.clone();
            drag.connect_drag_end(move |_, _, _| {
                let mut s = st.borrow_mut();
                // end of moving or resizing the selection: a double click captures it
                let (bounds, cursor) = (s.virt, s.cursor);
                if let Some(a) = s.adjust.as_mut() {
                    let grab = a.grab.take();
                    let (double, sel) = (std::mem::take(&mut a.double), a.sel);
                    drop(s);
                    // a button counts when the press and the release are both on it
                    let button = match grab {
                        Some((Hit::Bar(i), _)) if bar_hit(&sel, &bounds, cursor) == Some(i) => Some(i),
                        _ => None,
                    };
                    match button {
                        Some(0) => finish(None),
                        Some(1) => finish(Some(sel)),
                        _ if double => finish(Some(sel)),
                        _ => {
                            set_cursor();
                            redraw();
                        }
                    }
                    return;
                }
                let restart = std::mem::take(&mut s.restart);
                // a real drag (not a click on a window) leaves the selection
                // open for adjusting
                let dragged = s.selection().is_some() && !s.picking_windows();
                // capture exactly what is highlighted (Space may have
                // switched modes in the middle of the drag)
                let a = s.active();
                match a {
                    Some(r) if dragged && s.adjustable => {
                        s.start = None;
                        s.adjust = Some(Adjust { sel: r, grab: None, last_press: None, double: false });
                        drop(s);
                        set_cursor();
                        redraw();
                    }
                    // a click outside the selection only drops it
                    Some(_) | None if restart => {
                        s.start = None;
                        drop(s);
                        redraw();
                    }
                    Some(r) => {
                        drop(s);
                        finish(Some(r));
                    }
                    None => {
                        s.start = None;
                        drop(s);
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
            keys.connect_key_pressed(move |_, key, _, state| {
                match key {
                    gdk::Key::Escape => finish(None),
                    // arrows nudge the selection by one pixel (Shift: ten)
                    k if st.borrow().adjust.is_some() && nudge_delta(k, false, (1.0, 1.0)).is_some() => {
                        let mut s = st.borrow_mut();
                        let scale = (s.sx, s.sy);
                        let bounds = s.virt;
                        if let (Some(d), Some(a)) = (nudge_delta(k, state.contains(gdk::ModifierType::SHIFT_MASK), scale), s.adjust.as_mut()) {
                            a.sel = moved(&a.sel, d, &bounds);
                        }
                        drop(s);
                        redraw();
                    }
                    // the selection is made: nothing to switch
                    gdk::Key::space if st.borrow().adjust.is_some() => return glib::Propagation::Proceed,
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
                    // copy what is highlighted, no editor
                    gdk::Key::c | gdk::Key::C if state.contains(gdk::ModifierType::CONTROL_MASK) => {
                        let a = st.borrow().active();
                        if a.is_some() {
                            st.borrow_mut().copy_only = true;
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
        if unmanaged {
            let scale = mon.scale_factor() as f64;
            let (x, y, w, h) = ((g.x * scale) as i32, (g.y * scale) as i32, (g.w * scale) as u32, (g.h * scale) as u32);
            // the window exists on the server once GDK has flushed: retry briefly
            let tries = Rc::new(Cell::new(0));
            glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
                tries.set(tries.get() + 1);
                if crate::capture::x11_place_own_window(&title, x, y, w, h) || tries.get() >= 25 {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        }
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
    if on_this && s.start.is_none() && s.adjust.is_none() && !s.picking_windows() {
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
        if let Some(a) = &s.adjust {
            handles(cr, &r);
            confirm_bar(cr, &a.sel, s, g);
        }
    }

    // help text: bottom left, moved to the top when it would cover the
    // selection or the pointer
    let help = if s.adjust.is_some() {
        "Drag to adjust · arrows nudge (Shift: 10 px) · Enter: capture · Ctrl+C: copy · Esc: cancel"
    } else if s.picking_windows() {
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

    // not over the confirm bar: the magnifier would cover its buttons
    let over_bar = s.adjust.as_ref().is_some_and(|a| bar_hit(&a.sel, &s.virt, s.cursor).is_some());
    if on_this && s.magnifier && !over_bar {
        magnifier(s, cr, cx, cy, w, h);
    }
}

/// The Cancel / Capture buttons beside the selection.
fn confirm_bar(cr: &cairo::Context, sel: &R, s: &State, g: R) {
    let (bar, btns) = bar_rects(sel, &s.virt);
    let off = |r: &R| R { x: r.x - g.x, y: r.y - g.y, ..*r };
    let bar = off(&bar);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.8);
    rounded(cr, bar.x, bar.y, bar.w, bar.h, 9.0);
    cr.fill().ok();
    let hover = bar_hit(sel, &s.virt, s.cursor).filter(|&i| i < 2);
    for (i, b) in btns.iter().enumerate() {
        let b = off(b);
        let hot = hover == Some(i as u8);
        if i == 1 {
            // Capture: the confirming action
            cr.set_source_rgb(if hot { 0.22 } else { 0.17 }, if hot { 0.80 } else { 0.71 }, if hot { 0.52 } else { 0.45 });
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, if hot { 0.28 } else { 0.14 });
        }
        rounded(cr, b.x, b.y, b.w, b.h, 6.0);
        cr.fill().ok();
        // icon, then text
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        let (ix, iy) = (b.x + 14.0, b.y + b.h / 2.0);
        if i == 1 {
            cr.move_to(ix - 5.0, iy + 0.5);
            cr.line_to(ix - 1.5, iy + 4.0);
            cr.line_to(ix + 5.5, iy - 4.0);
        } else {
            cr.move_to(ix - 4.0, iy - 4.0);
            cr.line_to(ix + 4.0, iy + 4.0);
            cr.move_to(ix + 4.0, iy - 4.0);
            cr.line_to(ix - 4.0, iy + 4.0);
        }
        cr.stroke().ok();
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        cr.set_font_size(13.0);
        cr.move_to(b.x + 28.0, b.y + b.h / 2.0 + 4.5);
        cr.show_text(if i == 1 { "Capture" } else { "Cancel" }).ok();
    }
}

/// Square grips on the corners and, when there is room, edge midpoints.
fn handles(cr: &cairo::Context, r: &R) {
    let (x1, y1) = (r.x + r.w, r.y + r.h);
    let (mx, my) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let mut pts = vec![(r.x, r.y), (x1, r.y), (x1, y1), (r.x, y1)];
    if r.w >= 48.0 {
        pts.extend([(mx, r.y), (mx, y1)]);
    }
    if r.h >= 48.0 {
        pts.extend([(r.x, my), (x1, my)]);
    }
    cr.set_line_width(1.5);
    for (x, y) in pts {
        cr.rectangle(x - 4.0, y - 4.0, 8.0, 8.0);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.fill_preserve().ok();
        cr.set_source_rgb(0.17, 0.71, 0.45);
        cr.stroke().ok();
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

#[cfg(test)]
mod tests {
    use super::*;

    const SEL: R = R { x: 100.0, y: 100.0, w: 200.0, h: 100.0 };
    const BOUNDS: R = R { x: 0.0, y: 0.0, w: 1000.0, h: 800.0 };

    #[test]
    fn presses_grab_edges_corners_inside_or_outside() {
        assert_eq!(hit_test(&SEL, (200.0, 150.0)), Hit::Move);
        assert_eq!(hit_test(&SEL, (100.0, 150.0)), Hit::Resize(LEFT));
        assert_eq!(hit_test(&SEL, (300.0, 150.0)), Hit::Resize(RIGHT));
        assert_eq!(hit_test(&SEL, (200.0, 100.0)), Hit::Resize(TOP));
        assert_eq!(hit_test(&SEL, (200.0, 200.0)), Hit::Resize(BOTTOM));
        assert_eq!(hit_test(&SEL, (100.0, 100.0)), Hit::Resize(LEFT | TOP));
        assert_eq!(hit_test(&SEL, (300.0, 200.0)), Hit::Resize(RIGHT | BOTTOM));
        assert_eq!(hit_test(&SEL, (300.0, 100.0)), Hit::Resize(RIGHT | TOP));
        assert_eq!(hit_test(&SEL, (100.0, 200.0)), Hit::Resize(LEFT | BOTTOM));
        // the grip reaches a little outside the edge, but not far
        assert_eq!(hit_test(&SEL, (94.0, 150.0)), Hit::Resize(LEFT));
        assert_eq!(hit_test(&SEL, (60.0, 150.0)), Hit::New);
        assert_eq!(hit_test(&SEL, (500.0, 500.0)), Hit::New);
        // a tiny selection still has an inside to move it by
        let tiny = R { x: 100.0, y: 100.0, w: 30.0, h: 30.0 };
        assert_eq!(hit_test(&tiny, (115.0, 115.0)), Hit::Move);
    }

    #[test]
    fn confirm_bar_sits_below_above_or_inside() {
        let (bar, [cancel, capture]) = bar_rects(&SEL, &BOUNDS);
        // below, right-aligned with the selection, Capture at the corner
        assert!(bar.y > SEL.y + SEL.h);
        assert_eq!(bar.x + bar.w, SEL.x + SEL.w);
        assert!(capture.x > cancel.x && capture.x + capture.w <= bar.x + bar.w);
        // no room below: above
        let low = R { y: 700.0, ..SEL };
        assert!(bar_rects(&low, &BOUNDS).0.y + bar.h < low.y);
        // whole screen: inside the selection's bottom-right
        let full = R { x: 0.0, y: 0.0, w: 1000.0, h: 800.0 };
        let inside = bar_rects(&full, &BOUNDS).0;
        assert!(inside.y + inside.h <= 800.0 && inside.x + inside.w <= 1000.0 && inside.y >= 0.0);
        // a narrow selection near the left edge keeps the bar on screen
        let thin = R { x: 2.0, y: 100.0, w: 20.0, h: 50.0 };
        assert!(bar_rects(&thin, &BOUNDS).0.x >= 0.0);
    }

    #[test]
    fn bar_presses_pick_a_button() {
        let (bar, [cancel, capture]) = bar_rects(&SEL, &BOUNDS);
        let mid = |r: &R| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        assert_eq!(bar_hit(&SEL, &BOUNDS, mid(&cancel)), Some(0));
        assert_eq!(bar_hit(&SEL, &BOUNDS, mid(&capture)), Some(1));
        // the padding is part of the bar but does nothing
        assert_eq!(bar_hit(&SEL, &BOUNDS, (bar.x + 1.0, bar.y + 1.0)), Some(2));
        assert_eq!(bar_hit(&SEL, &BOUNDS, (SEL.x + 5.0, SEL.y + 5.0)), None);
        let a = Adjust { sel: SEL, grab: None, last_press: None, double: false };
        assert_eq!(press_hit(&a, &BOUNDS, mid(&capture)), Hit::Bar(1));
        assert_eq!(press_hit(&a, &BOUNDS, (200.0, 150.0)), Hit::Move);
    }

    #[test]
    fn arrow_keys_nudge_by_image_pixels() {
        assert_eq!(nudge_delta(gdk::Key::Left, false, (1.0, 1.0)), Some((-1.0, 0.0)));
        assert_eq!(nudge_delta(gdk::Key::Down, true, (1.0, 1.0)), Some((0.0, 10.0)));
        // 2x HiDPI: an image pixel is half a logical pixel
        assert_eq!(nudge_delta(gdk::Key::Right, false, (2.0, 2.0)), Some((0.5, 0.0)));
        assert_eq!(nudge_delta(gdk::Key::a, false, (1.0, 1.0)), None);
        // and nudging keeps the selection on screen
        let r = moved(&SEL, nudge_delta(gdk::Key::Up, true, (1.0, 1.0)).unwrap(), &R { x: 0.0, y: 95.0, w: 1000.0, h: 800.0 });
        assert_eq!(r.y, 95.0);
    }

    #[test]
    fn cursors_match_the_grab() {
        assert_eq!(cursor_for(Hit::New), "crosshair");
        assert_eq!(cursor_for(Hit::Move), "move");
        assert_eq!(cursor_for(Hit::Resize(LEFT)), "ew-resize");
        assert_eq!(cursor_for(Hit::Resize(BOTTOM)), "ns-resize");
        assert_eq!(cursor_for(Hit::Resize(LEFT | TOP)), "nwse-resize");
        assert_eq!(cursor_for(Hit::Resize(RIGHT | BOTTOM)), "nwse-resize");
        assert_eq!(cursor_for(Hit::Resize(RIGHT | TOP)), "nesw-resize");
        assert_eq!(cursor_for(Hit::Resize(LEFT | BOTTOM)), "nesw-resize");
    }

    #[test]
    fn resizing_moves_only_the_grabbed_edges() {
        let r = resized(&SEL, RIGHT | BOTTOM, (50.0, 20.0), &BOUNDS);
        assert_eq!(r, R { x: 100.0, y: 100.0, w: 250.0, h: 120.0 });
        let r = resized(&SEL, LEFT, (-40.0, 99.0), &BOUNDS);
        assert_eq!(r, R { x: 60.0, y: 100.0, w: 240.0, h: 100.0 });
        // dragging an edge past the opposite one flips the rectangle
        let r = resized(&SEL, LEFT, (250.0, 0.0), &BOUNDS);
        assert_eq!(r, R { x: 300.0, y: 100.0, w: 50.0, h: 100.0 });
        // stays on screen
        let r = resized(&SEL, RIGHT | BOTTOM, (5000.0, 5000.0), &BOUNDS);
        assert_eq!((r.x + r.w, r.y + r.h), (1000.0, 800.0));
        // never collapses
        let r = resized(&SEL, LEFT, (200.0, 0.0), &BOUNDS);
        assert!(r.w >= MIN_SEL);
    }

    #[test]
    fn moving_keeps_the_size_and_stays_on_screen() {
        assert_eq!(moved(&SEL, (30.0, -20.0), &BOUNDS), R { x: 130.0, y: 80.0, ..SEL });
        let r = moved(&SEL, (-500.0, -500.0), &BOUNDS);
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 200.0, 100.0));
        let r = moved(&SEL, (5000.0, 5000.0), &BOUNDS);
        assert_eq!((r.x + r.w, r.y + r.h), (1000.0, 800.0));
    }
}
