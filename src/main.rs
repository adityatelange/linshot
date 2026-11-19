// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

mod capture;
mod compat;
mod config;
mod editor;
mod hotkeys;
mod imageops;
mod overlay;
mod png;
mod session;
mod settings;
mod shortcuts;
mod style;
mod theme;
mod wlr;
mod wm;
mod tray;

use cairo::ImageSurface;
use config::{AfterCapture, Config};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const APP_ID: &str = "io.github.linshot.Linshot";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Prints how long a step took when LINSHOT_TRACE is set (performance debugging).
pub fn trace(what: &str, since: std::time::Instant) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("LINSHOT_TRACE").is_some()) {
        eprintln!("linshot: [trace] {what}: {:.1} ms", since.elapsed().as_secs_f64() * 1000.0);
    }
}

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
    system_dark: Cell<bool>,
    control: RefCell<Option<gtk::ApplicationWindow>>,
    watchers: RefCell<Vec<gio::FileMonitor>>,
    tray: RefCell<Option<tray::TrayHandle>>,
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
        compat::alert(parent, "Linshot", msg);
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
    session::init();
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
    install_panic_log();

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(startup);
    app.connect_command_line(|_, cl| {
        let args: Vec<String> = cl.arguments().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
        let cwd = cl.cwd().unwrap_or_else(|| PathBuf::from("."));
        match parse(&args, &cwd) {
            Ok(cmds) => CTX.with(|c| {
                let Some(ctx) = c.get() else { return 0 };
                let mut status = 0;
                for cmd in cmds {
                    if matches!(cmd, Cmd::Capture(_)) && ctx.busy.get() {
                        // tell the caller (e.g. a script using -o) instead of failing silently
                        printerr(cl, "linshot: another capture is in progress, request ignored\n");
                        status = 1;
                        continue;
                    }
                    let _ = ctx.tx.send_blocking(cmd);
                }
                status
            }),
            Err(e) => {
                eprintln!("linshot: {e}");
                2
            }
        }
    });
    app.run_with_args(&args)
}

/// Linshot usually runs in the background (autostart, tray) where stderr is
/// lost. A panic in a GTK callback aborts the process, so record it in
/// ~/.local/state/linshot/crash.log (newest last, capped) before that happens.
/// Panics in helper threads only end that thread but are logged the same way.
fn install_panic_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let Some(dir) = dirs::state_dir().or_else(dirs::cache_dir).map(|d| d.join("linshot")) else { return };
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("crash.log");
        // keep the log small: start over once it passes 256 KiB
        let append = std::fs::metadata(&path).map(|m| m.len() < 256 * 1024).unwrap_or(false);
        let file = std::fs::OpenOptions::new().create(true).append(append).write(true).truncate(!append).open(&path);
        if let Ok(mut f) = file {
            use std::io::Write;
            let thread = std::thread::current();
            let _ = writeln!(
                f,
                "--- {} linshot {VERSION}, thread '{}'\n{info}\n{}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                thread.name().unwrap_or("?"),
                std::backtrace::Backtrace::force_capture()
            );
        }
    }));
}

/// Prints on the stderr of the process that sent this command line.
fn printerr(cl: &gio::ApplicationCommandLine, msg: &str) {
    use glib::translate::ToGlibPtr;
    let Ok(msg) = std::ffi::CString::new(msg) else { return };
    // SAFETY: valid command line object; "%s" with one NUL-terminated string
    // (printerr_literal needs GLib 2.80)
    unsafe { gio::ffi::g_application_command_line_printerr(cl.to_glib_none().0, c"%s".as_ptr(), msg.as_ptr()) }
}

fn startup(app: &gtk::Application) {
    let (tx, rx) = async_channel::unbounded::<Cmd>();
    let ctx = Rc::new(Ctx {
        app: app.clone(),
        cfg: RefCell::new(Config::load()),
        tx: tx.clone(),
        busy: Cell::new(false),
        system_dark: Cell::new(false),
        control: RefCell::new(None),
        watchers: RefCell::new(Vec::new()),
        tray: RefCell::new(None),
    });
    CTX.with(|c| {
        let _ = c.set(ctx.clone());
    });

    gtk::Window::set_default_icon_name("linshot");
    style::init();

    let open_file = gio::SimpleAction::new("open-file", Some(glib::VariantTy::STRING));
    open_file.connect_activate(|_, v| {
        if let Some(p) = v.and_then(|v| v.get::<String>()) {
            let _ = gio::AppInfo::launch_default_for_uri(&gio::File::for_path(p).uri(), gio::AppLaunchContext::NONE);
        }
    });
    app.add_action(&open_file);

    *ctx.tray.borrow_mut() = Some(tray::spawn(tx.clone()));
    apply_theme(&ctx);
    let (ttx, trx) = async_channel::unbounded::<bool>();
    theme::spawn_watcher(ttx);
    {
        let ctx = ctx.clone();
        glib::spawn_future_local(async move {
            while let Ok(dark) = trx.recv().await {
                ctx.system_dark.set(dark);
                apply_theme(&ctx);
                let panel_dark = dark || tray::panel_always_dark();
                if let Some(h) = ctx.tray.borrow().as_ref() {
                    h.set_dark_panel(panel_dark);
                }
            }
        });
    }

    if capture::is_x11() && ctx.cfg.borrow().hotkeys && !shortcuts::installed() {
        hotkeys::spawn(tx.clone());
    }
    std::mem::forget(app.hold());

    restore_shortcuts(&ctx);
    if !ctx.cfg.borrow().shortcuts_prompted
        && ctx.cfg.borrow().desktop_shortcuts.is_none()
        && shortcuts::supported()
        && !shortcuts::installed()
    {
        let ctx = ctx.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || offer_shortcuts(&ctx));
    }

    glib::spawn_future_local(async move {
        while let Ok(cmd) = rx.recv().await {
            handle(&ctx, cmd);
        }
    });
}

/// Registers Print-key shortcuts in the desktop's keyboard settings.
/// Re-applies the user's desktop shortcuts if something (a session that
/// regenerates its config at login, for example) removed them, and keeps
/// watching for that while Linshot runs.
fn restore_shortcuts(ctx: &Rc<Ctx>) {
    if !shortcuts::supported() {
        return;
    }
    let want = ctx.cfg.borrow().desktop_shortcuts;
    match want {
        Some(true) => {
            if !shortcuts::installed() {
                match shortcuts::install() {
                    Ok(()) => eprintln!("linshot: keyboard shortcuts were missing, restored them"),
                    Err(e) => eprintln!("linshot: could not restore keyboard shortcuts: {e}"),
                }
            }
        }
        None if shortcuts::installed() => {
            let mut c = ctx.cfg.borrow_mut();
            c.desktop_shortcuts = Some(true);
            c.save();
        }
        _ => return,
    }
    watch_shortcuts(ctx);
}

fn watch_shortcuts(ctx: &Rc<Ctx>) {
    if !ctx.watchers.borrow().is_empty() {
        return;
    }
    let Some(path) = shortcuts::watch_path() else { return };
    let Ok(mon) = gio::File::for_path(&path).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
        return;
    };
    let pending = Rc::new(Cell::new(false));
    let ctx2 = ctx.clone();
    mon.connect_changed(move |_, _, _, _| {
        if pending.replace(true) {
            return;
        }
        let ctx = ctx2.clone();
        let pending = pending.clone();
        // let whoever rewrote the file finish first
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            pending.set(false);
            if ctx.cfg.borrow().desktop_shortcuts == Some(true) && !shortcuts::installed() {
                if shortcuts::install().is_ok() {
                    eprintln!("linshot: desktop config was rewritten, restored keyboard shortcuts");
                }
            }
        });
    });
    ctx.watchers.borrow_mut().push(mon);
}

pub fn remove_shortcuts(ctx: &Ctx) -> Result<(), String> {
    {
        let mut c = ctx.cfg.borrow_mut();
        c.desktop_shortcuts = Some(false);
        c.save();
    }
    shortcuts::uninstall()
}

pub fn setup_shortcuts(ctx: &Rc<Ctx>, parent: Option<&gtk::Window>) {
    match shortcuts::install() {
        Ok(()) => {
            {
                let mut c = ctx.cfg.borrow_mut();
                c.hotkeys = false; // the desktop handles Print now
                c.desktop_shortcuts = Some(true);
                c.save();
            }
            // run at login so lost shortcuts get restored
            if let Err(e) = config::set_autostart(true) {
                eprintln!("linshot: could not enable autostart: {e}");
            }
            watch_shortcuts(ctx);
            ctx.notify(
                "Keyboard shortcuts added",
                "Print: region · Alt+Print: window · Ctrl+Print: full screen · Shift+Print: last region",
                None,
            );
        }
        Err(e) => ctx.error(parent, &format!("Could not add keyboard shortcuts: {e}")),
    }
}

fn offer_shortcuts(ctx: &Rc<Ctx>) {
    {
        let mut c = ctx.cfg.borrow_mut();
        c.shortcuts_prompted = true;
        c.save();
    }
    let detail = format!(
        "Linshot will add these shortcuts to {}'s keyboard settings and take Print away from the built-in screenshot tool:\n\n\
         Print — capture region\nAlt+Print — capture window\nCtrl+Print — capture full screen\nShift+Print — capture last region\n\n\
         You can undo this in Linshot's Preferences.",
        shortcuts::desktop_name()
    );
    let ctx2 = ctx.clone();
    let parent = ctx.app.active_window();
    compat::choose(
        parent.as_ref(),
        "Use the Print key for Linshot?",
        &detail,
        &["Set up shortcuts", "Not now"],
        0,
        1,
        move |r| {
            if r == 0 {
                setup_shortcuts(&ctx2, None);
            }
        },
    );
}

/// Applies the configured appearance: follow the desktop, or force light/dark.
pub fn apply_theme(ctx: &Ctx) {
    let dark = match ctx.cfg.borrow().theme.as_str() {
        "dark" => true,
        "light" => false,
        _ => ctx.system_dark.get(),
    };
    if let Some(s) = gtk::Settings::default() {
        s.set_gtk_application_prefer_dark_theme(dark);
        // themes that are dark by name (e.g. Mint-Y-Dark) when following the system
        let named_dark = s.gtk_theme_name().map(|n| n.to_lowercase().contains("dark")).unwrap_or(false);
        style::set_dark(dark || (ctx.cfg.borrow().theme == "system" && named_dark));
    } else {
        style::set_dark(dark);
    }
}

fn handle(ctx: &Rc<Ctx>, cmd: Cmd) {
    match cmd {
        Cmd::Capture(req) => capture(ctx, req),
        Cmd::OpenFile(p) => match imageops::load_file(&p) {
            Ok(img) => editor::open(ctx, img, p.file_name().map(|n| n.to_string_lossy().into_owned()), None),
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
    let ctx2 = ctx.clone();
    let parent = ctx.app.active_window();
    let filters = [compat::Filter { name: "Images", patterns: &[], pixbuf_formats: true }];
    compat::open_file(parent.as_ref(), "Open image", &filters, move |p| {
        if let Some(p) = p {
            let _ = ctx2.tx.send_blocking(Cmd::OpenFile(p));
        }
    });
}

/// Marks a capture as running; dropping it (on any path, including the
/// selection overlay going away without an answer) allows the next one.
struct Busy(Rc<Ctx>);

impl Busy {
    fn acquire(ctx: &Rc<Ctx>) -> Option<Busy> {
        (!ctx.busy.replace(true)).then(|| Busy(ctx.clone()))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.set(false);
    }
}

fn capture(ctx: &Rc<Ctx>, req: Req) {
    let Some(busy) = Busy::acquire(ctx) else {
        eprintln!("linshot: a capture is already in progress, ignoring {:?}", req.mode);
        return;
    };
    if let Some(w) = ctx.control.borrow().as_ref() {
        w.set_visible(false);
    }
    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        let wait = if req.delay > 0 { req.delay as u64 * 1000 } else { 300 };
        glib::timeout_future(std::time::Duration::from_millis(wait)).await;

        let wayland = capture::is_wayland();
        let x11 = capture::is_x11();
        let want_picker = wayland && matches!(req.mode, Mode::Window | Mode::Active);
        let cursor = ctx.cfg.borrow().capture_cursor;
        let want_windows = x11 && matches!(req.mode, Mode::Region | Mode::Window | Mode::Last);
        let want_active = x11 && req.mode == Mode::Active;
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            // Wayland compositors with an IPC window list (Wayfire, Sway, Hyprland)
            let wl = if wayland { wm::windows() } else { None };
            let interactive = want_picker && wl.is_none();
            let mut active = if want_active { capture::x11_active_window().map(|r| Area::Px(r)) } else { None };
            // no interactive portal (e.g. wlroots): grab everything and let the user select
            let t = std::time::Instant::now();
            let (g, interactive) = match capture::grab(interactive, cursor) {
                Err(e) if interactive && !e.contains("cancelled") => (capture::grab(false, cursor), false),
                r => (r, interactive),
            };
            trace("grab", t);
            let mut wins: Vec<Area> = if want_windows {
                capture::x11_windows().into_iter().map(Area::Px).collect()
            } else {
                Vec::new()
            };
            if let Some(w) = wl {
                wins = w.list.into_iter().map(Area::Logical).collect();
                if active.is_none() {
                    active = w.active.map(Area::Logical);
                }
            }
            let _ = tx.send_blocking((g, wins, active, interactive));
        });
        let Ok((g, wins, active, interactive)) = rx.recv().await else {
            ctx.error(None, "Screenshot failed: the capture thread stopped unexpectedly");
            return;
        };
        let t = std::time::Instant::now();
        let (img, ptr): (ImageSurface, Ptr) = match g.and_then(|(g, c)| imageops::surface_from_grab(g).map(|s| (s, c))) {
            Ok((i, c)) => (i, c.and_then(|c| {
                ImageSurface::create_for_data(c.data, cairo::Format::ARgb32, c.w, c.h, c.w * 4).ok().map(|s| (s, c.x, c.y))
            })),
            Err(e) => {
                drop(busy);
                if !e.contains("cancelled") {
                    ctx.error(None, &format!("Screenshot failed: {e}"));
                }
                return;
            }
        };
        trace("surface from grab", t);
        let fits = |r: [i32; 4]| r[2] > 0 && r[3] > 0 && r[0] + r[2] <= img.width() && r[1] + r[3] <= img.height();
        let to_px = |a: &Area| a.to_px(img.width(), img.height());
        let wins: Vec<(i32, i32, i32, i32)> = wins.iter().map(to_px).collect();
        let active = active.as_ref().map(to_px);
        // no active-window info: choose the window in our overlay instead
        let mode = if req.mode == Mode::Active && active.is_none() && !interactive { Mode::Window } else { req.mode };
        let last = ctx.cfg.borrow().last_region.filter(|r| fits(*r));
        match (mode, active, last) {
            (Mode::Full, ..) => {
                drop(busy);
                deliver(&ctx, Ok(img), &req, ptr_at(&ptr, 0, 0))
            }
            _ if interactive => {
                drop(busy);
                deliver(&ctx, Ok(img), &req, ptr_at(&ptr, 0, 0))
            }
            (Mode::Active, Some((x, y, w, h)), _) => {
                drop(busy);
                let (x2, y2) = ((x + w).min(img.width()), (y + h).min(img.height()));
                let (x, y) = (x.clamp(0, img.width() - 1), y.clamp(0, img.height() - 1));
                deliver(&ctx, imageops::crop(&img, x, y, (x2 - x).max(1), (y2 - y).max(1)), &req, ptr_at(&ptr, x, y))
            }
            (Mode::Last, _, Some(r)) => {
                drop(busy);
                deliver(&ctx, imageops::crop(&img, r[0], r[1], r[2], r[3]), &req, ptr_at(&ptr, r[0], r[1]))
            }
            _ => {
                let ctx2 = ctx.clone();
                let full = img.clone();
                let mag = ctx.cfg.borrow().magnifier;
                overlay::select(
                    &ctx.app,
                    img,
                    wins,
                    mode == Mode::Window,
                    mag,
                    Box::new(move |r| {
                        drop(busy);
                        if let Some((x, y, w, h)) = r {
                            {
                                let mut c = ctx2.cfg.borrow_mut();
                                c.last_region = Some([x, y, w, h]);
                                c.save();
                            }
                            deliver(&ctx2, imageops::crop(&full, x, y, w, h), &req, ptr_at(&ptr, x, y))
                        }
                    }),
                );
            }
        }
    });
}

/// A window rectangle either in image pixels (X11) or global logical
/// coordinates (Wayland compositor IPC).
enum Area {
    Px((i32, i32, i32, i32)),
    Logical(wm::Rect),
}

impl Area {
    fn to_px(&self, img_w: i32, img_h: i32) -> (i32, i32, i32, i32) {
        match *self {
            Area::Px(r) => r,
            Area::Logical((x, y, w, h)) => {
                let virt = overlay::monitors().iter().fold(overlay::R::default(), |a, (_, g)| a.union(g));
                if virt.w <= 0.0 {
                    return (x as i32, y as i32, w as i32, h as i32);
                }
                let sx = img_w as f64 / virt.w;
                let sy = img_h as f64 / virt.h;
                (
                    ((x - virt.x) * sx).round() as i32,
                    ((y - virt.y) * sy).round() as i32,
                    (w * sx).round() as i32,
                    (h * sy).round() as i32,
                )
            }
        }
    }
}

/// Captured pointer sprite and its position in the full screenshot.
type Ptr = Option<(ImageSurface, i32, i32)>;

/// The pointer relative to a crop starting at (x, y).
fn ptr_at(p: &Ptr, x: i32, y: i32) -> Option<(ImageSurface, f64, f64)> {
    p.as_ref().map(|(s, px, py)| (s.clone(), (px - x) as f64, (py - y) as f64))
}

fn deliver(ctx: &Rc<Ctx>, img: Result<ImageSurface, String>, req: &Req, ptr: Option<(ImageSurface, f64, f64)>) {
    let img = match img {
        Ok(i) => i,
        Err(e) => return ctx.error(None, &format!("Screenshot failed: {e}")),
    };
    // keep the pointer only if it is inside the captured area
    let ptr = ptr.filter(|(s, x, y)| {
        *x + (s.width() as f64) > 0.0 && *y + (s.height() as f64) > 0.0 && *x < img.width() as f64 && *y < img.height() as f64
    });
    let action = req.action.unwrap_or(ctx.cfg.borrow().after_capture);
    if action == AfterCapture::Editor && req.output.is_none() {
        editor::open(ctx, img, None, ptr);
        return;
    }
    // not going to the editor: paint the pointer in
    if let Some((s, x, y)) = &ptr {
        if let Ok(cr) = cairo::Context::new(&img) {
            cr.set_source_surface(s, *x, *y).ok();
            cr.paint().ok();
        }
    }
    let q = ctx.cfg.borrow().jpeg_quality;
    let save = |path: PathBuf| match {
        let t = std::time::Instant::now();
        let r = imageops::save(&img, &path, q);
        trace("save", t);
        r
    } {
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
    match action {
        AfterCapture::Editor => {}
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

/// Theme-coloured capture-mode glyphs for the launcher (icon themes differ
/// too much; Adwaita's "selection-mode" is a check mark, for instance).
fn mode_glyph(mode: Mode) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_content_width(30);
    a.set_content_height(30);
    a.set_can_target(false);
    a.set_draw_func(move |w, cr, width, _| {
        let c = crate::compat::fg_color(w);
        let (r, g, b) = (c.red() as f64, c.green() as f64, c.blue() as f64);
        cr.scale(width as f64 / 24.0, width as f64 / 24.0);
        cr.set_source_rgb(r, g, b);
        cr.set_line_width(1.8);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        match mode {
            Mode::Region | Mode::Last => {
                // dashed selection with corner handles
                cr.set_dash(&[2.2, 2.6], 0.0);
                cr.rectangle(4.0, 5.0, 16.0, 13.0);
                cr.stroke().ok();
                cr.set_dash(&[], 0.0);
                for (x, y) in [(4.0, 5.0), (20.0, 5.0), (4.0, 18.0), (20.0, 18.0)] {
                    cr.rectangle(x - 1.6, y - 1.6, 3.2, 3.2);
                    cr.fill().ok();
                }
                if mode == Mode::Last {
                    // "again" arrow inside the selection
                    cr.arc(12.0, 11.5, 3.6, 0.6, std::f64::consts::TAU - 0.2);
                    cr.stroke().ok();
                    cr.move_to(15.9, 8.2);
                    cr.line_to(15.6, 10.9);
                    cr.line_to(13.0, 10.4);
                    cr.stroke().ok();
                }
            }
            Mode::Window | Mode::Active => {
                overlay::rounded(cr, 3.0, 4.5, 18.0, 15.0, 2.5);
                cr.stroke().ok();
                cr.move_to(3.0, 8.8);
                cr.line_to(21.0, 8.8);
                cr.stroke().ok();
                for x in [6.0, 8.6] {
                    cr.arc(x, 6.7, 0.8, 0.0, std::f64::consts::TAU);
                    cr.fill().ok();
                }
            }
            Mode::Full => {
                overlay::rounded(cr, 2.5, 4.0, 19.0, 13.0, 2.0);
                cr.stroke().ok();
                cr.move_to(9.0, 20.5);
                cr.line_to(15.0, 20.5);
                cr.move_to(12.0, 17.0);
                cr.line_to(12.0, 20.5);
                cr.stroke().ok();
            }
        }
    });
    a
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
    win.add_css_class("ls-launcher");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.set_margin_top(18);
    root.set_margin_bottom(18);
    root.set_margin_start(18);
    root.set_margin_end(18);
    let grid = gtk::Grid::builder().row_spacing(10).column_spacing(10).column_homogeneous(true).build();
    let delay = gtk::SpinButton::with_range(0.0, 60.0, 1.0);
    delay.set_numeric(true);
    delay.set_valign(gtk::Align::Center);
    let x11 = capture::is_x11();
    let entries: [(&str, &str, Mode); 4] = [
        ("Region", if x11 { "Print" } else { "" }, Mode::Region),
        ("Window", if x11 { "Alt+Print" } else { "" }, Mode::Window),
        ("Full screen", if x11 { "Ctrl+Print" } else { "" }, Mode::Full),
        ("Last region", if x11 { "Shift+Print" } else { "" }, Mode::Last),
    ];
    for (i, (label, key, mode)) in entries.into_iter().enumerate() {
        let b = gtk::Button::new();
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let icon = mode_glyph(mode);
        icon.add_css_class("ls-mode-icon");
        icon.set_halign(gtk::Align::Center);
        icon.set_margin_bottom(6);
        bx.append(&icon);
        let l = gtk::Label::new(Some(label));
        l.add_css_class("ls-mode-name");
        bx.append(&l);
        if !key.is_empty() {
            let k = gtk::Label::new(Some(key));
            k.add_css_class("ls-mode-key");
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

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let dl = gtk::Label::new(Some("Delay"));
    dl.add_css_class("ls-launcher-cap");
    row.append(&dl);
    row.append(&delay);
    let secs = gtk::Label::new(Some("s"));
    secs.add_css_class("ls-launcher-cap");
    row.append(&secs);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    for (label, cmd) in [("Open image…", Cmd::OpenDialog), ("Folder", Cmd::OpenFolder), ("Preferences", Cmd::Settings), ("Quit", Cmd::Quit)] {
        let b = gtk::Button::with_label(label);
        b.add_css_class(if matches!(cmd, Cmd::Quit) { "flat" } else { "ls-soft" });
        b.set_valign(gtk::Align::Center);
        let tx = ctx.tx.clone();
        b.connect_clicked(move |_| {
            let _ = tx.send_blocking(cmd.clone());
        });
        row.append(&b);
    }
    root.append(&row);

    // without a panel that shows tray icons, say how to get back here
    let (icon, reopen) = if tray::host_available() {
        (" (tray icon)", "")
    } else {
        ("", "  Start Linshot from the application menu to reopen this window.")
    };
    let hint = gtk::Label::new(Some(&if x11 {
        format!("Linshot keeps running in the background{icon} so the Print-key shortcuts and the clipboard keep working.{reopen}")
    } else {
        format!("Wayland: add keyboard shortcuts in your desktop settings, e.g.  Print → linshot region,  Alt+Print → linshot window,  Ctrl+Print → linshot full.  Linshot keeps running in the background{icon}.{reopen}")
    }));
    hint.set_wrap(true);
    hint.set_max_width_chars(60);
    hint.set_xalign(0.0);
    hint.add_css_class("ls-launcher-hint");
    root.append(&hint);
    win.set_child(Some(&root));
    win.connect_close_request(|w| {
        w.set_visible(false);
        glib::Propagation::Stop
    });
    win.present();
    *ctx.control.borrow_mut() = Some(win);
}
