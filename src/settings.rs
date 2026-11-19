// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

use crate::config::{self, AfterCapture};
use crate::{capture, shortcuts, Ctx};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;

const ACTIONS: [(AfterCapture, &str); 4] = [
    (AfterCapture::Editor, "Open in editor"),
    (AfterCapture::Save, "Save to folder"),
    (AfterCapture::Clipboard, "Copy to clipboard"),
    (AfterCapture::SaveAndClipboard, "Save and copy"),
];

pub fn show(ctx: &Rc<Ctx>) {
    let cfg = ctx.cfg.borrow().clone();
    let win = gtk::Window::builder().title("Linshot preferences").modal(false).resizable(false).build();
    win.set_application(Some(&ctx.app));

    let grid = gtk::Grid::builder()
        .row_spacing(10)
        .column_spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let mut row = 0;
    let mut add = |label: &str, w: &gtk::Widget| {
        let l = gtk::Label::new(Some(label));
        l.set_xalign(1.0);
        grid.attach(&l, 0, row, 1, 1);
        grid.attach(w, 1, row, 1, 1);
        row += 1;
    };

    let dir_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let dir = gtk::Entry::builder().text(&cfg.output_dir).hexpand(true).width_chars(32).build();
    let browse = gtk::Button::from_icon_name("folder-open-symbolic");
    browse.set_tooltip_text(Some("Choose folder"));
    dir_box.append(&dir);
    dir_box.append(&browse);
    add("Save folder", dir_box.upcast_ref());

    let pattern = gtk::Entry::builder()
        .text(&cfg.filename_pattern)
        .tooltip_text("strftime pattern, e.g. %Y-%m-%d_%H-%M-%S")
        .build();
    add("File name", pattern.upcast_ref());

    let format = gtk::DropDown::from_strings(&["PNG", "JPEG"]);
    format.set_selected(if cfg.extension() == "jpg" { 1 } else { 0 });
    add("Format", format.upcast_ref());

    let quality = gtk::SpinButton::with_range(10.0, 100.0, 1.0);
    quality.set_value(cfg.jpeg_quality as f64);
    add("JPEG quality", quality.upcast_ref());

    let after = gtk::DropDown::from_strings(&ACTIONS.map(|a| a.1));
    after.set_selected(ACTIONS.iter().position(|a| a.0 == cfg.after_capture).unwrap_or(0) as u32);
    add("After capture", after.upcast_ref());

    const THEMES: [(&str, &str); 3] = [("system", "Follow system"), ("light", "Light"), ("dark", "Dark")];
    let theme = gtk::DropDown::from_strings(&THEMES.map(|t| t.1));
    theme.set_selected(THEMES.iter().position(|t| t.0 == cfg.theme).unwrap_or(0) as u32);
    add("Appearance", theme.upcast_ref());

    let notif = gtk::CheckButton::with_label("Show notifications");
    notif.set_active(cfg.notifications);
    add("", notif.upcast_ref());
    let cur = gtk::CheckButton::with_label("Capture mouse pointer");
    cur.set_active(cfg.capture_cursor);
    add("", cur.upcast_ref());
    let mag = gtk::CheckButton::with_label("Show magnifier while selecting");
    mag.set_active(cfg.magnifier);
    add("", mag.upcast_ref());
    let hot = gtk::CheckButton::with_label("Use Print key shortcuts (X11, restart to apply)");
    hot.set_active(cfg.hotkeys);
    hot.set_sensitive(capture::is_x11());
    add("", hot.upcast_ref());
    let auto = gtk::CheckButton::with_label("Start Linshot when I log in");
    auto.set_active(config::autostart_path().exists());
    add("", auto.upcast_ref());

    let sc_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let sc_add = gtk::Button::with_label("Set up Print key shortcuts");
    let sc_remove = gtk::Button::with_label("Remove");
    let sc_state = gtk::Label::new(None);
    sc_state.add_css_class("dim-label");
    sc_box.append(&sc_add);
    sc_box.append(&sc_remove);
    sc_box.append(&sc_state);
    let refresh_sc = {
        let (sc_add, sc_remove, sc_state) = (sc_add.clone(), sc_remove.clone(), sc_state.clone());
        move || {
            let ok = shortcuts::supported();
            let on = ok && shortcuts::installed();
            sc_add.set_sensitive(ok);
            sc_remove.set_sensitive(on);
            sc_state.set_text(if !ok {
                "not available on this desktop — bind the commands below manually"
            } else if on {
                "✔ active"
            } else {
                ""
            });
        }
    };
    refresh_sc();
    {
        let ctx = ctx.clone();
        let win = win.clone();
        let refresh = refresh_sc.clone();
        sc_add.connect_clicked(move |_| {
            crate::setup_shortcuts(&ctx, Some(&win));
            refresh();
        });
    }
    {
        let ctx = ctx.clone();
        let win = win.clone();
        let refresh = refresh_sc.clone();
        sc_remove.connect_clicked(move |_| {
            if let Err(e) = crate::remove_shortcuts(&ctx) {
                ctx.error(Some(&win), &e);
            }
            refresh();
        });
    }
    add("Shortcuts", sc_box.upcast_ref());

    let session = if capture::is_wayland() { "Wayland" } else if capture::is_x11() { "X11" } else { "unknown" };
    let info = gtk::Label::new(Some(&format!(
        "Session: {session}.  Shortcut commands for your desktop's keyboard settings:\n\
         linshot region · linshot window · linshot full · linshot last"
    )));
    info.add_css_class("dim-label");
    info.set_xalign(0.0);
    info.set_wrap(true);
    grid.attach(&info, 0, row, 2, 1);
    row += 1;

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&save);
    grid.attach(&buttons, 0, row, 2, 1);
    win.set_child(Some(&grid));

    {
        let dir = dir.clone();
        let win = win.clone();
        browse.connect_clicked(move |_| {
            let dlg = gtk::FileDialog::builder().title("Screenshot folder").modal(true).build();
            dlg.set_initial_folder(Some(&gio::File::for_path(config::shellexpand(&dir.text()))));
            let dir = dir.clone();
            dlg.select_folder(Some(&win), gio::Cancellable::NONE, move |r| {
                if let Ok(Some(p)) = r.map(|f| f.path()) {
                    dir.set_text(&p.to_string_lossy());
                }
            });
        });
    }
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let ctx = ctx.clone();
        let win2 = win.clone();
        save.connect_clicked(move |_| {
            {
                let mut c = ctx.cfg.borrow_mut();
                c.output_dir = dir.text().to_string();
                c.filename_pattern = pattern.text().to_string();
                c.format = if format.selected() == 1 { "jpg".into() } else { "png".into() };
                c.jpeg_quality = quality.value() as u8;
                c.after_capture = ACTIONS[(after.selected() as usize).min(3)].0;
                c.notifications = notif.is_active();
                c.magnifier = mag.is_active();
                c.capture_cursor = cur.is_active();
                c.hotkeys = hot.is_active();
                c.theme = THEMES[(theme.selected() as usize).min(2)].0.into();
                c.save();
            }
            crate::apply_theme(&ctx);
            if let Err(e) = config::set_autostart(auto.is_active()) {
                ctx.error(Some(&win2), &format!("Could not update autostart: {e}"));
            }
            win2.close();
        });
    }
    let _ = glib::Propagation::Proceed;
    win.present();
}
