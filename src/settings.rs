// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Preferences window: grouped cards of rows (title, optional subtitle, a
//! control on the right), switches for on/off settings, in the style of
//! GNOME's preference pages but with plain GTK 4.6 widgets.
use crate::config::{self, AfterCapture};
use crate::{Ctx, capture, shortcuts};
use gtk::prelude::*;
use std::rc::Rc;

const ACTIONS: [(AfterCapture, &str); 4] = [
    (AfterCapture::Editor, "Open in editor"),
    (AfterCapture::Save, "Save to folder"),
    (AfterCapture::Clipboard, "Copy to clipboard"),
    (AfterCapture::SaveAndClipboard, "Save and copy"),
];
const THEMES: [(&str, &str); 3] = [("system", "Follow system"), ("light", "Light"), ("dark", "Dark")];

/// A titled card of rows.
struct Group {
    root: gtk::Box,
    list: gtk::ListBox,
}

impl Group {
    fn new(title: &str) -> Group {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let t = gtk::Label::new(Some(title));
        t.add_css_class("ls-group-title");
        t.set_xalign(0.0);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("ls-group");
        root.append(&t);
        root.append(&list);
        Group { root, list }
    }

    /// Adds a row; returns it so the caller can hide or desensitize it.
    fn row(&self, title: &str, subtitle: Option<&str>, control: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.add_css_class("ls-row");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let t = gtk::Label::new(Some(title));
        t.set_xalign(0.0);
        t.add_css_class("ls-row-title");
        text.append(&t);
        if let Some(s) = subtitle {
            let l = gtk::Label::new(Some(s));
            l.set_xalign(0.0);
            l.set_wrap(true);
            l.set_max_width_chars(40);
            l.add_css_class("ls-row-sub");
            text.append(&l);
        }
        control.set_valign(gtk::Align::Center);
        b.append(&text);
        b.append(control);
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.set_child(Some(&b));
        self.list.append(&row);
        row
    }

    fn switch(&self, title: &str, subtitle: Option<&str>, on: bool) -> (gtk::Switch, gtk::ListBoxRow) {
        let s = gtk::Switch::new();
        s.set_active(on);
        let row = self.row(title, subtitle, &s);
        (s, row)
    }
}

fn dropdown(items: &[&str], selected: usize) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_selected(selected as u32);
    d
}

pub fn show(ctx: &Rc<Ctx>) {
    let cfg = ctx.cfg.borrow().clone();
    let win = gtk::Window::builder()
        .title("Preferences")
        .modal(false)
        .default_width(560)
        .default_height(680)
        .build();
    win.set_application(Some(&ctx.app));
    win.add_css_class("ls-prefs");

    let header = gtk::HeaderBar::new();
    header.set_show_title_buttons(false);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("ls-primary");
    header.pack_start(&cancel);
    header.pack_end(&save);
    win.set_titlebar(Some(&header));

    let page = gtk::Box::new(gtk::Orientation::Vertical, 22);
    page.add_css_class("ls-prefs-page");

    // ------------------------------------------------------------ capture
    let g = Group::new("Capture");
    let (cur, _) = g.switch("Capture mouse pointer", Some("It can still be moved or removed in the editor"), cfg.capture_cursor);
    let (mag, _) = g.switch("Magnifier while selecting", Some("Zoomed view and colour under the pointer"), cfg.magnifier);
    let (adj, _) = g.switch(
        "Adjust selection before capturing",
        Some("After dragging, resize or move the area, then confirm. Off: capture as soon as you let go"),
        cfg.adjust_selection,
    );
    page.append(&g.root);

    // ------------------------------------------------------------ after capture
    let g = Group::new("After capture");
    let after = dropdown(&ACTIONS.map(|a| a.1), ACTIONS.iter().position(|a| a.0 == cfg.after_capture).unwrap_or(0));
    g.row("Then", Some("What happens with a new screenshot"), &after);
    let (notif, _) = g.switch("Notifications", Some("When a screenshot is saved or copied"), cfg.notifications);
    page.append(&g.root);

    // ------------------------------------------------------------ saving
    let g = Group::new("Saving");
    let dir_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let dir = gtk::Entry::builder().text(&cfg.output_dir).width_chars(18).build();
    let browse = gtk::Button::from_icon_name("folder-open-symbolic");
    browse.set_tooltip_text(Some("Choose folder"));
    dir_box.append(&dir);
    dir_box.append(&browse);
    g.row("Folder", None, &dir_box);
    let pattern = gtk::Entry::builder().text(&cfg.filename_pattern).width_chars(18).build();
    g.row("File name", Some("strftime pattern, e.g. %Y-%m-%d_%H-%M-%S"), &pattern);
    let format = dropdown(&["PNG", "JPEG"], if cfg.extension() == "jpg" { 1 } else { 0 });
    g.row("Format", None, &format);
    let quality = gtk::SpinButton::with_range(10.0, 100.0, 1.0);
    quality.set_value(cfg.jpeg_quality as f64);
    let quality_row = g.row("JPEG quality", None, &quality);
    quality_row.set_sensitive(format.selected() == 1);
    {
        let quality_row = quality_row.clone();
        format.connect_selected_notify(move |f| quality_row.set_sensitive(f.selected() == 1));
    }
    page.append(&g.root);

    // ------------------------------------------------------------ shortcuts
    let g = Group::new("Keyboard shortcuts");
    let sc_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let sc_add = gtk::Button::with_label("Set up");
    let sc_remove = gtk::Button::with_label("Remove");
    sc_add.add_css_class("ls-soft");
    sc_remove.add_css_class("ls-soft");
    sc_box.append(&sc_add);
    sc_box.append(&sc_remove);
    let sc_row = g.row("Print key shortcuts", None, &sc_box);
    let sc_sub = gtk::Label::new(None);
    sc_sub.set_xalign(0.0);
    sc_sub.set_wrap(true);
    sc_sub.set_max_width_chars(40);
    sc_sub.add_css_class("ls-row-sub");
    // the row's text column: append the status line under its title
    if let Some(text) = sc_row.child().and_then(|b| b.first_child()).and_then(|t| t.downcast::<gtk::Box>().ok()) {
        text.append(&sc_sub);
    }
    let refresh_sc = {
        let (sc_add, sc_remove, sc_sub) = (sc_add.clone(), sc_remove.clone(), sc_sub.clone());
        move || {
            let ok = shortcuts::supported();
            let on = ok && shortcuts::installed();
            sc_add.set_visible(ok && !on);
            sc_remove.set_visible(on);
            sc_sub.set_text(if !ok {
                "Not available on this desktop: bind the commands below in your keyboard settings"
            } else if on {
                "Active: Print, Alt+Print, Shift+Print, Ctrl+Print"
            } else {
                "Registers Print, Alt+Print, Shift+Print and Ctrl+Print with your desktop"
            });
        }
    };
    refresh_sc();
    {
        let (ctx, win, refresh) = (ctx.clone(), win.clone(), refresh_sc.clone());
        sc_add.connect_clicked(move |_| {
            crate::setup_shortcuts(&ctx, Some(&win));
            refresh();
        });
    }
    {
        let (ctx, win, refresh) = (ctx.clone(), win.clone(), refresh_sc.clone());
        sc_remove.connect_clicked(move |_| {
            if let Err(e) = crate::remove_shortcuts(&ctx) {
                ctx.error(Some(&win), &e);
            }
            refresh();
        });
    }
    let (hot, hot_row) = g.switch("Grab the Print key directly", Some("X11 only, when the desktop has no shortcut settings; applies after a restart"), cfg.hotkeys);
    hot_row.set_sensitive(capture::is_x11());

    let session = if capture::is_wayland() { "Wayland" } else if capture::is_x11() { "X11" } else { "unknown" };
    let mut text = format!(
        "Session: {session}. Commands for your desktop's keyboard settings:\n\
         linshot region · linshot window · linshot full · linshot last"
    );
    if let Some(h) = shortcuts::manual_hint() {
        text.push_str("\n\n");
        text.push_str(h);
    }
    let info = gtk::Label::new(Some(&text));
    info.set_selectable(true);
    info.set_wrap(true);
    info.set_xalign(0.0);
    info.add_css_class("ls-prefs-note");
    info.set_margin_start(4);
    // inside the group, so it sits close under the card (a negative margin
    // here makes GTK warn about the label's size)
    g.root.append(&info);
    page.append(&g.root);

    // ------------------------------------------------------------ general
    let g = Group::new("General");
    let theme = dropdown(&THEMES.map(|t| t.1), THEMES.iter().position(|t| t.0 == cfg.theme).unwrap_or(0));
    g.row("Appearance", None, &theme);
    let (auto, _) = g.switch("Start at login", Some("Keeps the tray icon and shortcuts ready"), config::autostart_path().exists());
    page.append(&g.root);

    // ------------------------------------------------------------ about
    let g = Group::new("About");
    let value = |text: &str| {
        let l = gtk::Label::new(Some(text));
        l.add_css_class("ls-value");
        l.set_selectable(true);
        l
    };
    let link = |label: &str, uri: &str| {
        let b = gtk::LinkButton::with_label(uri, label);
        b.add_css_class("ls-link");
        b
    };
    g.row("Version", None, &value(crate::VERSION));
    g.row("Author", None, &value(crate::about::AUTHOR));
    g.row("License", Some("Free software: you may share and change it"), &link(crate::about::LICENSE, crate::about::LICENSE_URL));
    g.row("Source code", Some("Report problems and read the code"), &link(crate::about::WEBSITE_LABEL, crate::about::WEBSITE));
    page.append(&g.root);

    let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
    clamp.set_halign(gtk::Align::Center);
    clamp.set_size_request(480, -1);
    clamp.append(&page);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();
    win.set_child(Some(&scroller));

    {
        let (dir, win) = (dir.clone(), win.clone());
        browse.connect_clicked(move |_| {
            let initial = std::path::PathBuf::from(config::shellexpand(&dir.text()));
            let dir = dir.clone();
            crate::compat::select_folder(Some(win.upcast_ref()), "Screenshot folder", &initial, move |p| {
                if let Some(p) = p {
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
            let pat = pattern.text();
            if config::format_pattern(&pat, chrono::Local::now()).is_none_or(|n| n.trim().is_empty()) {
                ctx.error(Some(&win2), &format!("\u{201c}{pat}\u{201d} is not a valid file name pattern. Use strftime codes such as %Y-%m-%d_%H-%M-%S."));
                pattern.grab_focus();
                return;
            }
            {
                let mut c = ctx.cfg.borrow_mut();
                c.output_dir = dir.text().to_string();
                c.filename_pattern = pat.to_string();
                c.format = if format.selected() == 1 { "jpg".into() } else { "png".into() };
                c.jpeg_quality = quality.value() as u8;
                c.after_capture = ACTIONS[(after.selected() as usize).min(3)].0;
                c.notifications = notif.is_active();
                c.magnifier = mag.is_active();
                c.adjust_selection = adj.is_active();
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
    // don't open with the whole folder path selected in the first entry
    GtkWindowExt::set_focus(&win, Some(&save));
    win.present();
}
