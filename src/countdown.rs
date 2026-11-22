// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Countdown for delayed captures: a small floating bubble with the seconds
//! left and a Cancel button (Esc cancels too), mirrored on the tray icon.
//! The bubble hides itself shortly before the grab so it is never captured.
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long before the grab the bubble disappears (window managers and
/// compositors may animate it out).
const HIDE_BEFORE_MS: u64 = 350;

pub struct Bubble {
    win: gtk::Window,
    num: gtk::Label,
}

impl Bubble {
    fn new(app: &gtk::Application, cancel: Rc<Cell<bool>>) -> Bubble {
        let win = gtk::Window::builder()
            .application(app)
            .title("Linshot countdown")
            .decorated(false)
            .resizable(false)
            .build();
        win.add_css_class("ls-countdown-win");

        let card = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        card.add_css_class("ls-countdown");
        let num = gtk::Label::new(None);
        num.add_css_class("ls-countdown-num");
        num.set_width_chars(2);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some("Capturing in"));
        title.add_css_class("ls-countdown-title");
        title.set_xalign(0.0);
        let hint = gtk::Label::new(Some("Esc to cancel"));
        hint.add_css_class("ls-countdown-hint");
        hint.set_xalign(0.0);
        text.append(&title);
        text.append(&hint);
        let cancel_btn = gtk::Button::with_label("Cancel");
        cancel_btn.add_css_class("ls-soft");
        cancel_btn.set_valign(gtk::Align::Center);
        card.append(&num);
        card.append(&text);
        card.append(&cancel_btn);
        win.set_child(Some(&card));

        {
            let cancel = cancel.clone();
            cancel_btn.connect_clicked(move |_| cancel.set(true));
        }
        let keys = gtk::EventControllerKey::new();
        {
            let cancel = cancel.clone();
            keys.connect_key_pressed(move |_, k, _, _| {
                if k == gdk::Key::Escape {
                    cancel.set(true);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        win.add_controller(keys);
        // closed by the window manager: same as Cancel
        win.connect_close_request(move |_| {
            cancel.set(true);
            glib::Propagation::Proceed
        });
        Bubble { win, num }
    }

    fn set(&self, n: u32) {
        self.num.set_text(&n.to_string());
    }
}

/// Counts down `secs` seconds, showing the bubble and calling `tick` with the
/// seconds left (None at the end). Returns false if the user cancelled.
pub async fn run(app: &gtk::Application, secs: u32, cancel: Rc<Cell<bool>>, tick: impl Fn(Option<u32>)) -> bool {
    let total = Duration::from_secs(secs as u64);
    let hide_at = total.saturating_sub(Duration::from_millis(HIDE_BEFORE_MS));
    let bubble = Bubble::new(app, cancel.clone());
    bubble.set(secs);
    bubble.win.present();
    let start = Instant::now();
    let mut shown = true;
    let mut last = None;
    let ok = loop {
        if cancel.get() {
            break false;
        }
        let elapsed = start.elapsed();
        if elapsed >= total {
            break true;
        }
        let left = ((total - elapsed).as_millis() as u64).div_ceil(1000) as u32;
        if last != Some(left) {
            last = Some(left);
            bubble.set(left);
            tick(Some(left));
        }
        if shown && elapsed >= hide_at {
            bubble.win.set_visible(false);
            shown = false;
        }
        glib::timeout_future(Duration::from_millis(50)).await;
    };
    bubble.win.destroy();
    tick(None);
    ok
}
