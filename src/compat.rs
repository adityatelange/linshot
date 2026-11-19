// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Dialog and styling helpers built only on GTK 4.6 API, so one binary runs
//! on Ubuntu 22.04 (GTK 4.6) and Debian 12 (GTK 4.8) as well as on current
//! releases. The newer replacements (FileDialog, AlertDialog, ColorDialog —
//! GTK 4.10 — and CssProvider::load_from_string — 4.12) are not used.
//!
//! The older APIs are deprecated from GTK 4.10 on but still fully supported in
//! GTK 4; the `allow` keeps the build warning-free if newer feature flags are
//! ever enabled.
#![allow(deprecated)]

use gtk::prelude::*;
use gtk::{gdk, gio};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Shows a modal message with an OK button.
pub fn alert(parent: Option<&gtk::Window>, message: &str, detail: &str) {
    choose(parent, message, detail, &["_OK"], 0, 0, |_| {});
}

/// Asks a question with the given buttons; `done` gets the index of the
/// pressed button, or `cancel` when the dialog is closed another way.
pub fn choose(
    parent: Option<&gtk::Window>,
    message: &str,
    detail: &str,
    buttons: &[&str],
    default: usize,
    cancel: usize,
    done: impl FnOnce(usize) + 'static,
) {
    let mut b = gtk::MessageDialog::builder()
        .modal(true)
        .message_type(gtk::MessageType::Other)
        .buttons(gtk::ButtonsType::None)
        .text(message)
        .secondary_text(detail);
    if let Some(p) = parent {
        b = b.transient_for(p);
    }
    let dlg = b.build();
    for (i, label) in buttons.iter().enumerate() {
        let btn = dlg.add_button(label, gtk::ResponseType::Other(i as u16));
        if i == default && buttons.len() > 1 {
            btn.add_css_class("suggested-action");
        }
    }
    dlg.set_default_response(gtk::ResponseType::Other(default as u16));
    let done = Cell::new(Some(done));
    dlg.connect_response(move |d, r| {
        let idx = match r {
            gtk::ResponseType::Other(i) => i as usize,
            _ => cancel,
        };
        d.destroy();
        if let Some(f) = done.take() {
            f(idx);
        }
    });
    dlg.present();
}

pub struct Filter<'a> {
    pub name: &'a str,
    pub patterns: &'a [&'a str],
    /// also accept every format gdk-pixbuf can load
    pub pixbuf_formats: bool,
}

/// Runs a native (portal-aware) file chooser; `done` gets the chosen path.
fn run_chooser(dlg: gtk::FileChooserNative, done: impl FnOnce(Option<PathBuf>) + 'static) {
    // the dialog must outlive this call; the handler drops the last reference
    let keep = Rc::new(RefCell::new(Some(dlg.clone())));
    let done = Cell::new(Some(done));
    dlg.connect_response(move |d, r| {
        let path = if r == gtk::ResponseType::Accept { d.file().and_then(|f| f.path()) } else { None };
        if let Some(f) = done.take() {
            f(path);
        }
        keep.borrow_mut().take();
    });
    dlg.show();
}

fn chooser(parent: Option<&gtk::Window>, title: &str, action: gtk::FileChooserAction, accept: &str) -> gtk::FileChooserNative {
    let dlg = gtk::FileChooserNative::new(Some(title), parent, action, Some(accept), Some("_Cancel"));
    dlg.set_modal(true);
    dlg
}

fn add_filters(dlg: &gtk::FileChooserNative, filters: &[Filter]) {
    for f in filters {
        let ff = gtk::FileFilter::new();
        ff.set_name(Some(f.name));
        for p in f.patterns {
            ff.add_pattern(p);
        }
        if f.pixbuf_formats {
            ff.add_pixbuf_formats();
        }
        dlg.add_filter(&ff);
    }
}

pub fn open_file(parent: Option<&gtk::Window>, title: &str, filters: &[Filter], done: impl FnOnce(Option<PathBuf>) + 'static) {
    let dlg = chooser(parent, title, gtk::FileChooserAction::Open, "_Open");
    add_filters(&dlg, filters);
    run_chooser(dlg, done);
}

pub fn save_file(
    parent: Option<&gtk::Window>,
    title: &str,
    suggested: &Path,
    filters: &[Filter],
    done: impl FnOnce(Option<PathBuf>) + 'static,
) {
    let dlg = chooser(parent, title, gtk::FileChooserAction::Save, "_Save");
    add_filters(&dlg, filters);
    if let Some(d) = suggested.parent().filter(|d| d.is_dir()) {
        let _ = dlg.set_current_folder(Some(&gio::File::for_path(d)));
    }
    if let Some(n) = suggested.file_name() {
        dlg.set_current_name(&n.to_string_lossy());
    }
    run_chooser(dlg, done);
}

pub fn select_folder(parent: Option<&gtk::Window>, title: &str, initial: &Path, done: impl FnOnce(Option<PathBuf>) + 'static) {
    let dlg = chooser(parent, title, gtk::FileChooserAction::SelectFolder, "_Select");
    if initial.is_dir() {
        let _ = dlg.set_current_folder(Some(&gio::File::for_path(initial)));
    }
    run_chooser(dlg, done);
}

/// A colour button with alpha support.
pub fn color_button(rgba: &gdk::RGBA) -> gtk::ColorButton {
    let b = gtk::ColorButton::with_rgba(rgba);
    b.set_use_alpha(true);
    b
}

/// The widget's current CSS foreground colour (Widget::color() is GTK 4.10).
pub fn fg_color(w: &impl IsA<gtk::Widget>) -> gdk::RGBA {
    w.as_ref().style_context().color()
}

/// CssProvider::load_from_string is GTK 4.12; load_from_data exists since 4.0.
pub fn load_css(p: &gtk::CssProvider, css: &str) {
    p.load_from_data(css);
}
