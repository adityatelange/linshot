// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

mod capture;
mod config;
mod editor;
mod hotkeys;
mod imageops;
mod overlay;
mod settings;
mod tray;

use cairo::ImageSurface;
use config::{AfterCapture, Config};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const APP_ID: &str = "io.github.linshot.Linshot";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Region,
    Window,
    Active,
    Full,
    Last,
}

#[derive(Clone, Debug)]
pub struct Req {
    pub mode: Mode,
    pub delay: u32,
    pub output: Option<PathBuf>,
    pub action: Option<AfterCapture>,
}

#[derive(Clone, Debug)]
pub enum Cmd {
    Capture(Req),
    OpenDialog,
    OpenFile(PathBuf),
    OpenFolder,
    Settings,
    Show,
    Quit,
}

impl Cmd {
    pub fn cap(mode: Mode) -> Cmd {
        Cmd::Capture(Req { mode, delay: 0, output: None, action: None })
    }
    pub fn delayed(mode: Mode, delay: u32) -> Cmd {
        Cmd::Capture(Req { mode, delay, output: None, action: None })
    }
}

pub struct Ctx {
    pub app: gtk::Application,
    pub cfg: RefCell<Config>,
    pub tx: async_channel::Sender<Cmd>,
    busy: Cell<bool>,
    control: RefCell<Option<gtk::ApplicationWindow>>,
}

impl Ctx {
    pub fn notify(&self, title: &str, body: &str, path: Option<&Path>) {
        if !self.cfg.borrow().notifications {
            return;
        }
        let n = gio::Notification::new(title);
        n.set_body(Some(body));
        if let Some(p) = path {
            n.set_default_action_and_target_value("app.open-file", Some(&p.to_string_lossy().to_variant()));
        }
        self.app.send_notification(Some("linshot"), &n);
    }

    pub fn error(&self, parent: Option<&gtk::Window>, msg: &str) {
        eprintln!("linshot: {msg}");
        gtk::AlertDialog::builder().modal(true).message("Linshot").detail(msg).build().show(parent);
    }
}

thread_local! {
    static CTX: OnceCell<Rc<Ctx>> = const { OnceCell::new() };
}

const HELP: &str = "\
Linshot — screenshot and annotation tool

Usage: linshot [COMMAND] [OPTIONS]

Commands:
  region          Select a region (click a window to grab it) [Print]
  window          Pick a window                               [Alt+Print]
  active          Capture the active window
  full            Capture all screens                         [Ctrl+Print]
  last            Capture the previously selected region      [Shift+Print]
  open FILE...    Open images in the editor
  settings        Show preferences
  quit            Stop the running instance
  (none)          Show the Linshot launcher window

Options:
  -d, --delay SECONDS   Wait before capturing
  -o, --output FILE     Save straight to FILE (png/jpg), no editor
  -c, --clipboard       Copy to clipboard instead of opening the editor
  -s, --save            Save to the screenshots folder instead of opening the editor
  -e, --editor          Open the editor (default)
      --tray            Start in the background (used for autostart)
  -h, --help            Show this help
  -V, --version         Show version

Linshot runs as a single background instance with a tray icon. On X11 it
grabs the Print key itself; on Wayland bind the commands above to keys in
your desktop's keyboard settings, e.g.  Print → linshot region
";

fn parse(args: &[String], cwd: &Path) -> Result<Vec<Cmd>, String> {
    let mut mode = None;
    let mut files = Vec::new();
    let mut other = None;
    let mut delay = 0;
    let mut output = None;
    let mut action = None;
    let mut tray_only = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "region" => mode = Some(Mode::Region),
            "window" => mode = Some(Mode::Window),
            "active" => mode = Some(Mode::Active),
            "full" | "fullscreen" => mode = Some(Mode::Full),
            "last" | "last-region" => mode = Some(Mode::Last),
            "settings" | "preferences" => other = Some(Cmd::Settings),
            "quit" => other = Some(Cmd::Quit),
            "open" => {
                for f in it.by_ref() {
                    files.push(cwd.join(f));
                }
                if files.is_empty() {
                    other = Some(Cmd::OpenDialog);
                }
            }
            "-d" | "--delay" => {
                delay = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--delay needs a number of seconds")?
            }
            "-o" | "--output" => output = Some(cwd.join(it.next().ok_or("--output needs a file name")?)),
            "-c" | "--clipboard" => action = Some(AfterCapture::Clipboard),
            "-s" | "--save" => action = Some(AfterCapture::Save),
            "-e" | "--editor" => action = Some(AfterCapture::Editor),
            "--tray" | "--background" => tray_only = true,
            x if x.starts_with("--gapplication") => {}
            x => return Err(format!("unknown argument '{x}' (see linshot --help)")),
        }
    }
    let mut cmds: Vec<Cmd> = files.into_iter().map(Cmd::OpenFile).collect();
    if let Some(m) = mode {
        cmds.push(Cmd::Capture(Req { mode: m, delay, output, action }));
    }
    if let Some(o) = other {
        cmds.push(o);
    }
    if cmds.is_empty() && !tray_only {
        cmds.push(Cmd::Show);
    }
    Ok(cmds)
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("linshot {VERSION}");
        return glib::ExitCode::SUCCESS;
    }
    if let Err(e) = parse(&args[1..], Path::new(".")) {
        eprintln!("linshot: {e}");
        return glib::ExitCode::from(2);
    }

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(startup);
    app.connect_command_line(|_, cl| {
        let args: Vec<String> = cl.arguments().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
        let cwd = cl.cwd().unwrap_or_else(|| PathBuf::from("."));
        match parse(&args, &cwd) {
            Ok(cmds) => {
                CTX.with(|c| {
                    if let Some(ctx) = c.get() {
                        for cmd in cmds {
                            let _ = ctx.tx.send_blocking(cmd);
                        }
                    }
                });
                0
            }
            Err(e) => {
                eprintln!("linshot: {e}");
                2
            }
        }
    });
    app.run_with_args(&args)
}

fn startup(app: &gtk::Application) {
    let (tx, rx) = async_channel::unbounded::<Cmd>();
    let ctx = Rc::new(Ctx {
        app: app.clone(),
        cfg: RefCell::new(Config::load()),
        tx: tx.clone(),
        busy: Cell::new(false),
        control: RefCell::new(None),
    });
    CTX.with(|c| {
        let _ = c.set(ctx.clone());
    });

    gtk::Window::set_default_icon_name("linshot");
    let css = gtk::CssProvider::new();
    css.load_from_string(
        ".linshot-overlay { background: black; }
         .linshot-canvas { background-color: #3c3f41; }
         .linshot-big { padding: 10px 14px; }",
    );
    if let Some(d) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&d, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let open_file = gio::SimpleAction::new("open-file", Some(glib::VariantTy::STRING));
    open_file.connect_activate(|_, v| {
        if let Some(p) = v.and_then(|v| v.get::<String>()) {
            let _ = gio::AppInfo::launch_default_for_uri(&gio::File::for_path(p).uri(), gio::AppLaunchContext::NONE);
        }
    });
    app.add_action(&open_file);

    tray::spawn(tx.clone());
    if capture::is_x11() && ctx.cfg.borrow().hotkeys {
        hotkeys::spawn(tx.clone());
    }
    std::mem::forget(app.hold());

    glib::spawn_future_local(async move {
        while let Ok(cmd) = rx.recv().await {
            handle(&ctx, cmd);
        }
    });
}

fn handle(ctx: &Rc<Ctx>, cmd: Cmd) {
    match cmd {
        Cmd::Capture(req) => capture(ctx, req),
        Cmd::OpenFile(p) => match imageops::load_file(&p) {
            Ok(img) => editor::open(ctx, img, p.file_name().map(|n| n.to_string_lossy().into_owned())),
            Err(e) => ctx.error(None, &e),
        },
        Cmd::OpenDialog => open_dialog(ctx),
        Cmd::OpenFolder => {
            let dir = PathBuf::from(config::shellexpand(&ctx.cfg.borrow().output_dir));
            let _ = std::fs::create_dir_all(&dir);
            let _ = gio::AppInfo::launch_default_for_uri(&gio::File::for_path(dir).uri(), gio::AppLaunchContext::NONE);
        }
        Cmd::Settings => settings::show(ctx),
        Cmd::Show => show_control(ctx),
        Cmd::Quit => ctx.app.quit(),
    }
}

fn open_dialog(ctx: &Rc<Ctx>) {
    let f = gtk::FileFilter::new();
    f.set_name(Some("Images"));
    f.add_pixbuf_formats();
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&f);
    let dlg = gtk::FileDialog::builder().title("Open image").filters(&filters).modal(true).build();
    let ctx2 = ctx.clone();
    let parent = ctx.app.active_window();
    dlg.open(parent.as_ref(), gio::Cancellable::NONE, move |r| {
        if let Ok(Some(p)) = r.map(|f| f.path()) {
            let _ = ctx2.tx.send_blocking(Cmd::OpenFile(p));
        }
    });
}

fn capture(ctx: &Rc<Ctx>, req: Req) {
    if ctx.busy.replace(true) {
        return;
    }
    if let Some(w) = ctx.control.borrow().as_ref() {
        w.set_visible(false);
    }
    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        let wait = if req.delay > 0 { req.delay as u64 * 1000 } else { 300 };
        glib::timeout_future(std::time::Duration::from_millis(wait)).await;

        let wayland = capture::is_wayland();
        let x11 = capture::is_x11();
        let interactive = wayland && matches!(req.mode, Mode::Window | Mode::Active);
        let want_windows = x11 && matches!(req.mode, Mode::Region | Mode::Window | Mode::Last);
        let want_active = x11 && req.mode == Mode::Active;
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let active = if want_active { capture::x11_active_window() } else { None };
            let g = capture::grab(interactive);
            let wins = if want_windows { capture::x11_windows() } else { Vec::new() };
            let _ = tx.send_blocking((g, wins, active));
        });
        let Ok((g, wins, active)) = rx.recv().await else {
            ctx.busy.set(false);
            return;
        };
        let img = match g.and_then(imageops::surface_from_grab) {
            Ok(i) => i,
            Err(e) => {
                ctx.busy.set(false);
                if !e.contains("cancelled") {
                    ctx.error(None, &format!("Screenshot failed: {e}"));
                }
                return;
            }
        };
        let fits = |r: [i32; 4]| r[2] > 0 && r[3] > 0 && r[0] + r[2] <= img.width() && r[1] + r[3] <= img.height();
        match req.mode {
            Mode::Full => deliver(&ctx, img, &req),
            _ if interactive => deliver(&ctx, img, &req),
            Mode::Active => match active {
                Some((x, y, w, h)) => {
                    let (x2, y2) = ((x + w).min(img.width()), (y + h).min(img.height()));
                    let (x, y) = (x.max(0), y.max(0));
                    deliver(&ctx, imageops::crop(&img, x, y, x2 - x, y2 - y), &req)
                }
                None => deliver(&ctx, img, &req),
            },
            Mode::Last if ctx.cfg.borrow().last_region.map(fits).unwrap_or(false) => {
                let r = ctx.cfg.borrow().last_region.unwrap();
                deliver(&ctx, imageops::crop(&img, r[0], r[1], r[2], r[3]), &req)
            }
            _ => {
                let ctx2 = ctx.clone();
                let full = img.clone();
                let mag = ctx.cfg.borrow().magnifier;
                overlay::select(
                    &ctx.app,
                    img,
                    wins,
                    req.mode == Mode::Window,
                    mag,
                    Box::new(move |r| match r {
                        Some((x, y, w, h)) => {
                            {
                                let mut c = ctx2.cfg.borrow_mut();
                                c.last_region = Some([x, y, w, h]);
                                c.save();
                            }
                            deliver(&ctx2, imageops::crop(&full, x, y, w, h), &req)
                        }
                        None => ctx2.busy.set(false),
                    }),
                );
            }
        }
    });
}

fn deliver(ctx: &Rc<Ctx>, img: ImageSurface, req: &Req) {
    ctx.busy.set(false);
    let q = ctx.cfg.borrow().jpeg_quality;
    let save = |path: PathBuf| match imageops::save(&img, &path, q) {
        Ok(()) => {
            ctx.notify("Screenshot saved", &path.display().to_string(), Some(&path));
            true
        }
        Err(e) => {
            ctx.error(None, &format!("Could not save {}: {e}", path.display()));
            false
        }
    };
    let copy = || {
        imageops::copy_to_clipboard(&img);
        ctx.notify("Screenshot copied", "The screenshot is on the clipboard.", None);
    };
    if let Some(out) = &req.output {
        save(out.clone());
        if req.action == Some(AfterCapture::Clipboard) {
            copy();
        }
        return;
    }
    let action = req.action.unwrap_or(ctx.cfg.borrow().after_capture);
    match action {
        AfterCapture::Editor => editor::open(ctx, img, None),
        AfterCapture::Save => {
            save(ctx.cfg.borrow().next_output_path());
        }
        AfterCapture::Clipboard => copy(),
        AfterCapture::SaveAndClipboard => {
            save(ctx.cfg.borrow().next_output_path());
            imageops::copy_to_clipboard(&img);
        }
    }
}

fn show_control(ctx: &Rc<Ctx>) {
    if let Some(w) = ctx.control.borrow().as_ref() {
        w.present();
        return;
    }
    let win = gtk::ApplicationWindow::builder()
        .application(&ctx.app)
        .title("Linshot")
        .resizable(false)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    for m in [&root] {
        m.set_margin_top(16);
        m.set_margin_bottom(16);
        m.set_margin_start(16);
        m.set_margin_end(16);
    }
    let grid = gtk::Grid::builder().row_spacing(8).column_spacing(8).column_homogeneous(true).build();
    let delay = gtk::SpinButton::with_range(0.0, 60.0, 1.0);
    let x11 = capture::is_x11();
    let entries: [(&str, &str, &str, Mode); 4] = [
        ("Region", "selection-mode-symbolic", if x11 { "Print" } else { "" }, Mode::Region),
        ("Window", "window-new-symbolic", if x11 { "Alt+Print" } else { "" }, Mode::Window),
        ("Full screen", "view-fullscreen-symbolic", if x11 { "Ctrl+Print" } else { "" }, Mode::Full),
        ("Last region", "view-refresh-symbolic", if x11 { "Shift+Print" } else { "" }, Mode::Last),
    ];
    for (i, (label, icon, key, mode)) in entries.into_iter().enumerate() {
        let b = gtk::Button::new();
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 4);
        bx.append(&gtk::Image::builder().icon_name(icon).pixel_size(24).build());
        bx.append(&gtk::Label::new(Some(label)));
        if !key.is_empty() {
            let k = gtk::Label::new(Some(key));
            k.add_css_class("dim-label");
            k.add_css_class("caption");
            bx.append(&k);
        }
        b.set_child(Some(&bx));
        b.add_css_class("linshot-big");
        let tx = ctx.tx.clone();
        let delay = delay.clone();
        b.connect_clicked(move |_| {
            let _ = tx.send_blocking(Cmd::delayed(mode, delay.value() as u32));
        });
        grid.attach(&b, i as i32, 0, 1, 1);
    }
    root.append(&grid);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&gtk::Label::new(Some("Delay (s)")));
    row.append(&delay);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    for (label, cmd) in [("Open image…", Cmd::OpenDialog), ("Folder", Cmd::OpenFolder), ("Preferences", Cmd::Settings), ("Quit", Cmd::Quit)] {
        let b = gtk::Button::with_label(label);
        let tx = ctx.tx.clone();
        b.connect_clicked(move |_| {
            let _ = tx.send_blocking(cmd.clone());
        });
        row.append(&b);
    }
    root.append(&row);

    let hint = gtk::Label::new(Some(if x11 {
        "Linshot keeps running in the background (tray icon) so the Print-key shortcuts and the clipboard keep working."
    } else {
        "Wayland: add keyboard shortcuts in your desktop settings, e.g.  Print → linshot region,  Alt+Print → linshot window,  Ctrl+Print → linshot full.  Linshot keeps running in the background."
    }));
    hint.set_wrap(true);
    hint.set_max_width_chars(60);
    hint.set_xalign(0.0);
    hint.add_css_class("dim-label");
    root.append(&hint);
    win.set_child(Some(&root));
    win.connect_close_request(|w| {
        w.set_visible(false);
        glib::Propagation::Stop
    });
    win.present();
    *ctx.control.borrow_mut() = Some(win);
}
