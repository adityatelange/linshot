// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Registers Linshot's Print-key shortcuts with the desktop (Wayfire,
//! Cinnamon, GNOME, XFCE).
//! Needed on Wayland, where apps cannot grab global keys, and on X11 desktops
//! that already own the Print key. Also frees Print from the desktop's own
//! screenshot shortcuts.
use gtk::gio;
use gtk::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Desktop {
    Wayfire,
    Cinnamon,
    Gnome,
    Xfce,
    Other,
}

fn wayfire_running() -> bool {
    if std::env::var_os("WAYFIRE_CONFIG_FILE").is_some() || std::env::var_os("WAYFIRE_SOCKET").is_some() {
        return true;
    }
    std::fs::read_dir("/proc")
        .map(|rd| {
            rd.flatten().any(|e| {
                std::fs::read_to_string(e.path().join("comm")).map(|c| c.trim() == "wayfire").unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

pub fn desktop() -> Desktop {
    let d = crate::session::desktop();
    if d.contains("wayfire") || (crate::capture::is_wayland() && wayfire_running()) {
        Desktop::Wayfire
    } else if d.contains("cinnamon") {
        Desktop::Cinnamon
    } else if d.contains("gnome") || d.contains("unity") || d.contains("ubuntu") {
        Desktop::Gnome
    } else if d.contains("xfce") {
        Desktop::Xfce
    } else {
        Desktop::Other
    }
}

pub fn desktop_name() -> &'static str {
    match desktop() {
        Desktop::Wayfire => "Wayfire",
        Desktop::Cinnamon => "Cinnamon",
        Desktop::Gnome => "GNOME",
        Desktop::Xfce => "Xfce",
        Desktop::Other => "your desktop",
    }
}

/// How to bind the commands by hand on desktops Linshot cannot configure.
pub fn manual_hint() -> Option<&'static str> {
    if desktop() != Desktop::Other {
        return None;
    }
    let d = crate::session::desktop();
    let has = |n: &str| d.contains(n);
    Some(if has("sway") {
        "Sway, in ~/.config/sway/config:\n  bindsym Print exec linshot region\n  bindsym Alt+Print exec linshot window\n  bindsym Ctrl+Print exec linshot full"
    } else if has("hyprland") {
        "Hyprland, in ~/.config/hypr/hyprland.conf:\n  bind = , Print, exec, linshot region\n  bind = ALT, Print, exec, linshot window\n  bind = CTRL, Print, exec, linshot full"
    } else if has("niri") {
        "niri, in the binds { } section of ~/.config/niri/config.kdl:\n  Print { spawn \"linshot\" \"region\"; }\n  Alt+Print { spawn \"linshot\" \"window\"; }\n  Ctrl+Print { spawn \"linshot\" \"full\"; }"
    } else if has("river") {
        "river, in ~/.config/river/init:\n  riverctl map normal None Print spawn 'linshot region'\n  riverctl map normal Alt Print spawn 'linshot window'\n  riverctl map normal Control Print spawn 'linshot full'"
    } else if has("labwc") {
        "labwc, inside <keyboard> in ~/.config/labwc/rc.xml:\n  <keybind key=\"Print\"><action name=\"Execute\" command=\"linshot region\"/></keybind>\n  <keybind key=\"A-Print\"><action name=\"Execute\" command=\"linshot window\"/></keybind>"
    } else if has("kde") || has("plasma") {
        "KDE Plasma: System Settings → Keyboard → Shortcuts → Add New → Command or Script, or add the Linshot application there and give its Capture Region / Window / Full Screen actions a key. Take Print away from Spectacle first."
    } else if has("lxqt") {
        "LXQt: Preferences → Shortcut Keys → Add, with the commands above."
    } else if has("mate") {
        "MATE: Keyboard Shortcuts → Add, with the commands above; clear Print from “Take a screenshot” first."
    } else if has("cosmic") {
        "COSMIC: Settings → Keyboard → Keyboard Shortcuts → Custom Shortcuts, with the commands above."
    } else {
        return None;
    })
}

pub fn supported() -> bool {
    match desktop() {
        Desktop::Wayfire => true,
        Desktop::Cinnamon => settings(CIN_KB, None).is_some(),
        Desktop::Gnome => settings(GNOME_MK, None).is_some(),
        Desktop::Xfce => crate::capture::has_cmd("xfconf-query"),
        Desktop::Other => false,
    }
}

/// (name, linshot sub-command, accelerator)
const BINDINGS: [(&str, &str, &str); 4] = [
    ("Linshot: capture region", "region", "Print"),
    ("Linshot: capture window", "window", "<Alt>Print"),
    ("Linshot: capture full screen", "full", "<Primary>Print"),
    ("Linshot: capture last region", "last", "<Shift>Print"),
];

const CIN_KB: &str = "org.cinnamon.desktop.keybindings";
const CIN_CUSTOM: &str = "org.cinnamon.desktop.keybindings.custom-keybinding";
const CIN_MEDIA: &str = "org.cinnamon.desktop.keybindings.media-keys";
const CIN_PATH: &str = "/org/cinnamon/desktop/keybindings/custom-keybindings/";
const GNOME_MK: &str = "org.gnome.settings-daemon.plugins.media-keys";
const GNOME_CUSTOM: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const GNOME_SHELL_KB: &str = "org.gnome.shell.keybindings";
const GNOME_PATH: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/";

/// Desktop screenshot shortcuts that compete for the Print key.
const CONFLICTS: &[(&str, &[&str])] = &[
    (CIN_MEDIA, &["screenshot", "screenshot-clip", "window-screenshot", "window-screenshot-clip", "area-screenshot", "area-screenshot-clip"]),
    (GNOME_SHELL_KB, &["show-screenshot-ui", "screenshot", "screenshot-window"]),
    (GNOME_MK, &["screenshot", "screenshot-clip", "window-screenshot", "window-screenshot-clip", "area-screenshot", "area-screenshot-clip"]),
];

fn settings(schema: &str, path: Option<&str>) -> Option<gio::Settings> {
    let src = gio::SettingsSchemaSource::default()?;
    src.lookup(schema, true)?;
    Some(match path {
        Some(p) => gio::Settings::with_path(schema, p),
        None => gio::Settings::new(schema),
    })
}

fn has_key(s: &gio::Settings, key: &str) -> bool {
    s.settings_schema().map(|sc| sc.has_key(key)).unwrap_or(false)
}

fn norm(accel: &str) -> String {
    accel.to_lowercase().replace("<control>", "<primary>").replace("<ctrl>", "<primary>")
}

pub(crate) fn command(sub: &str) -> String {
    let exe = std::env::current_exe()
        .ok()
        .filter(|p| p.starts_with("/usr") || p.starts_with("/opt"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "linshot".into());
    format!("{exe} {sub}")
}

fn is_ours(cmd: &str) -> bool {
    cmd.split_whitespace().next().map(|b| b.ends_with("linshot")).unwrap_or(false)
}

/// Removes Print-key accelerators from the desktop's own screenshot shortcuts.
fn free_print_key() {
    let ours: Vec<String> = BINDINGS.iter().map(|b| norm(b.2)).collect();
    for (schema, keys) in CONFLICTS {
        let Some(s) = settings(schema, None) else { continue };
        for key in keys.iter().filter(|k| has_key(&s, k)) {
            let v = s.value(key);
            if v.type_() == glib_strv() {
                let cur: Vec<String> = s.strv(key).iter().map(|x| x.to_string()).collect();
                let kept: Vec<&str> = cur.iter().filter(|a| !ours.contains(&norm(a))).map(|a| a.as_str()).collect();
                if kept.len() != cur.len() {
                    let _ = s.set_strv(key, kept.as_slice());
                }
            } else if v.type_() == gtk::glib::VariantTy::STRING {
                if ours.contains(&norm(&s.string(key))) {
                    let _ = s.set_string(key, "");
                }
            }
        }
    }
}

fn glib_strv() -> &'static gtk::glib::VariantTy {
    gtk::glib::VariantTy::STRING_ARRAY
}

/// Restores the desktop's default screenshot shortcuts.
fn restore_print_key() {
    for (schema, keys) in CONFLICTS {
        let Some(s) = settings(schema, None) else { continue };
        for key in keys.iter().filter(|k| has_key(&s, k)) {
            s.reset(key);
        }
    }
}

pub fn install() -> Result<(), String> {
    match desktop() {
        Desktop::Wayfire => return wayfire::install(),
        Desktop::Xfce => return xfce::install(&mut xfce::Xfconf),
        Desktop::Cinnamon => install_cinnamon(),
        Desktop::Gnome => install_gnome(),
        Desktop::Other => Err(UNSUPPORTED.into()),
    }?;
    free_print_key();
    gio::Settings::sync();
    Ok(())
}

const UNSUPPORTED: &str = "automatic setup supports Wayfire, Cinnamon, GNOME and Xfce";

pub fn uninstall() -> Result<(), String> {
    match desktop() {
        Desktop::Wayfire => return wayfire::uninstall(),
        Desktop::Cinnamon => remove_cinnamon(),
        Desktop::Gnome => remove_gnome(),
        Desktop::Xfce => return xfce::uninstall(&mut xfce::Xfconf),
        Desktop::Other => return Err(UNSUPPORTED.into()),
    }
    restore_print_key();
    gio::Settings::sync();
    Ok(())
}

/// File to watch for external rewrites of our shortcuts (Wayfire only).
pub fn watch_path() -> Option<std::path::PathBuf> {
    (desktop() == Desktop::Wayfire).then(|| {
        let p = wayfire::config_path();
        std::fs::canonicalize(&p).unwrap_or(p)
    })
}

pub fn installed() -> bool {
    match desktop() {
        Desktop::Wayfire => wayfire::installed(),
        Desktop::Cinnamon => cinnamon_entries().map(|(_, ours)| !ours.is_empty()).unwrap_or(false),
        Desktop::Gnome => gnome_entries().map(|(_, ours)| !ours.is_empty()).unwrap_or(false),
        Desktop::Xfce => xfce::installed(&mut xfce::Xfconf),
        Desktop::Other => false,
    }
}

// ---------------------------------------------------------------- Cinnamon

/// (custom-list, names of entries whose command is linshot)
fn cinnamon_entries() -> Option<(Vec<String>, Vec<String>)> {
    let kb = settings(CIN_KB, None)?;
    let list: Vec<String> = kb.strv("custom-list").iter().map(|s| s.to_string()).collect();
    let ours = list
        .iter()
        .filter(|n| {
            settings(CIN_CUSTOM, Some(&format!("{CIN_PATH}{n}/")))
                .map(|s| is_ours(&s.string("command")))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    Some((list, ours))
}

fn remove_cinnamon() {
    let Some((list, ours)) = cinnamon_entries() else { return };
    for n in &ours {
        if let Some(s) = settings(CIN_CUSTOM, Some(&format!("{CIN_PATH}{n}/"))) {
            for k in ["name", "command", "binding"] {
                s.reset(k);
            }
        }
    }
    let kept: Vec<&str> = list.iter().filter(|n| !ours.contains(n)).map(|s| s.as_str()).collect();
    if let Some(kb) = settings(CIN_KB, None) {
        let _ = kb.set_strv("custom-list", kept.as_slice());
    }
}

fn install_cinnamon() -> Result<(), String> {
    remove_cinnamon();
    let kb = settings(CIN_KB, None).ok_or("Cinnamon keybinding settings not found")?;
    let mut list: Vec<String> = kb.strv("custom-list").iter().map(|s| s.to_string()).collect();
    let mut n = 0;
    for (name, sub, accel) in BINDINGS {
        while list.contains(&format!("custom{n}")) {
            n += 1;
        }
        let id = format!("custom{n}");
        let s = settings(CIN_CUSTOM, Some(&format!("{CIN_PATH}{id}/"))).ok_or("custom keybinding schema missing")?;
        s.set_string("name", name).map_err(|e| e.to_string())?;
        s.set_string("command", &command(sub)).map_err(|e| e.to_string())?;
        s.set_strv("binding", [accel].as_slice()).map_err(|e| e.to_string())?;
        list.push(id);
    }
    let refs: Vec<&str> = list.iter().map(|s| s.as_str()).collect();
    kb.set_strv("custom-list", refs.as_slice()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- GNOME

fn gnome_entries() -> Option<(Vec<String>, Vec<String>)> {
    let mk = settings(GNOME_MK, None)?;
    if !has_key(&mk, "custom-keybindings") {
        return None;
    }
    let list: Vec<String> = mk.strv("custom-keybindings").iter().map(|s| s.to_string()).collect();
    let ours = list
        .iter()
        .filter(|p| settings(GNOME_CUSTOM, Some(p)).map(|s| is_ours(&s.string("command"))).unwrap_or(false))
        .cloned()
        .collect();
    Some((list, ours))
}

fn remove_gnome() {
    let Some((list, ours)) = gnome_entries() else { return };
    for p in &ours {
        if let Some(s) = settings(GNOME_CUSTOM, Some(p)) {
            for k in ["name", "command", "binding"] {
                s.reset(k);
            }
        }
    }
    let kept: Vec<&str> = list.iter().filter(|p| !ours.contains(p)).map(|s| s.as_str()).collect();
    if let Some(mk) = settings(GNOME_MK, None) {
        let _ = mk.set_strv("custom-keybindings", kept.as_slice());
    }
}

fn install_gnome() -> Result<(), String> {
    remove_gnome();
    let mk = settings(GNOME_MK, None).ok_or("GNOME media-keys settings not found")?;
    let mut list: Vec<String> = mk.strv("custom-keybindings").iter().map(|s| s.to_string()).collect();
    for (i, (name, sub, accel)) in BINDINGS.iter().enumerate() {
        let path = format!("{GNOME_PATH}linshot{i}/");
        let s = settings(GNOME_CUSTOM, Some(&path)).ok_or("custom keybinding schema missing")?;
        s.set_string("name", name).map_err(|e| e.to_string())?;
        s.set_string("command", &command(sub)).map_err(|e| e.to_string())?;
        s.set_string("binding", accel).map_err(|e| e.to_string())?;
        if !list.contains(&path) {
            list.push(path);
        }
    }
    let refs: Vec<&str> = list.iter().map(|s| s.as_str()).collect();
    mk.set_strv("custom-keybindings", refs.as_slice()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- Xfce

/// Custom application shortcuts in xfconf's xfce4-keyboard-shortcuts channel,
/// changed with xfconf-query (xfsettingsd picks them up immediately). Our
/// entries replace the custom ones for the same keys (by default
/// xfce4-screenshooter); removing them restores Xfce's defaults.
pub mod xfce {
    pub trait Store {
        fn get(&mut self, prop: &str) -> Option<String>;
        fn set(&mut self, prop: &str, ty: &str, value: &str) -> Result<(), String>;
        fn reset(&mut self, prop: &str) -> Result<(), String>;
    }

    const CHANNEL: &str = "xfce4-keyboard-shortcuts";
    const KEYS: [(&str, &str); 4] = [("Print", "region"), ("<Alt>Print", "window"), ("<Primary>Print", "full"), ("<Shift>Print", "last")];

    fn custom(key: &str) -> String {
        format!("/commands/custom/{key}")
    }

    pub fn installed(st: &mut impl Store) -> bool {
        st.get(&custom("Print")).map(|c| super::is_ours(&c)).unwrap_or(false)
    }

    pub fn install(st: &mut impl Store) -> Result<(), String> {
        for (key, sub) in KEYS {
            st.set(&custom(key), "string", &super::command(sub))?;
        }
        // custom shortcuts only take effect with override set
        st.set("/commands/custom/override", "bool", "true")
    }

    pub fn uninstall(st: &mut impl Store) -> Result<(), String> {
        for (key, _) in KEYS {
            let p = custom(key);
            if !st.get(&p).map(|c| super::is_ours(&c)).unwrap_or(false) {
                continue;
            }
            match st.get(&format!("/commands/default/{key}")) {
                Some(d) => st.set(&p, "string", &d)?,
                None => st.reset(&p)?,
            }
        }
        Ok(())
    }

    pub struct Xfconf;

    fn run(args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("xfconf-query")
            .args(["-c", CHANNEL])
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("xfconf-query: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
        } else {
            Err(format!("xfconf-query: {}", String::from_utf8_lossy(&out.stderr).trim()))
        }
    }

    impl Store for Xfconf {
        fn get(&mut self, prop: &str) -> Option<String> {
            run(&["-p", prop]).ok()
        }
        fn set(&mut self, prop: &str, ty: &str, value: &str) -> Result<(), String> {
            run(&["-p", prop, "-n", "-t", ty, "-s", value]).map(|_| ())
        }
        fn reset(&mut self, prop: &str) -> Result<(), String> {
            run(&["-p", prop, "-r"]).map(|_| ())
        }
    }
}

// ---------------------------------------------------------------- Wayfire

/// Edits the [command] section of wayfire.ini; Wayfire reloads it automatically.
pub mod wayfire {
    use std::path::PathBuf;

    const MARK: &str = "# linshot-disabled: ";
    const KEYS: [(&str, &str, &str); 4] = [
        ("linshot_region", "region", "KEY_SYSRQ"),
        ("linshot_window", "window", "<alt> KEY_SYSRQ"),
        ("linshot_full", "full", "<ctrl> KEY_SYSRQ"),
        ("linshot_last", "last", "<shift> KEY_SYSRQ"),
    ];
    const AUTOSTART_KEY: &str = "linshot_autostart";

    pub fn config_path() -> PathBuf {
        if let Some(p) = std::env::var_os("WAYFIRE_CONFIG_FILE") {
            return PathBuf::from(p);
        }
        let cfg = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let nested = cfg.join("wayfire").join("wayfire.ini");
        if nested.exists() {
            nested
        } else {
            cfg.join("wayfire.ini")
        }
    }

    fn norm(v: &str) -> String {
        v.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
    }

    fn is_section(l: &str) -> Option<String> {
        let t = l.trim();
        (t.starts_with('[') && t.ends_with(']')).then(|| t[1..t.len() - 1].trim().to_string())
    }

    fn key_of(l: &str) -> Option<&str> {
        let t = l.trim_start();
        if t.starts_with('#') {
            return None;
        }
        t.split_once('=').map(|(k, _)| k.trim())
    }

    fn strip(lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .filter(|l| !key_of(l).map(|k| k == AUTOSTART_KEY || KEYS.iter().any(|x| k.ends_with(x.0))).unwrap_or(false))
            .map(|l| l.strip_prefix(MARK).map(str::to_string).unwrap_or(l))
            .collect()
    }

    pub fn installed() -> bool {
        std::fs::read_to_string(config_path())
            .map(|s| s.lines().any(|l| key_of(l) == Some("command_linshot_region")))
            .unwrap_or(false)
    }

    pub fn install() -> Result<(), String> {
        let path = config_path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut lines = strip(text.lines().map(str::to_string).collect());
        let ours: Vec<String> = KEYS.iter().map(|k| norm(k.2)).collect();

        // disable other [command] bindings that use the same keys
        let mut section = String::new();
        for l in lines.iter_mut() {
            if let Some(s) = is_section(l) {
                section = s;
                continue;
            }
            if section == "command" {
                if let Some((k, v)) = l.split_once('=') {
                    let k = k.trim();
                    if !k.starts_with('#') && k.contains("binding_") && ours.contains(&norm(v)) {
                        *l = format!("{MARK}{l}");
                    }
                }
            }
        }

        // window detection needs Wayfire's IPC plugins; only touch an explicit plugin list
        let mut section = String::new();
        for l in lines.iter_mut() {
            if let Some(s) = is_section(l) {
                section = s;
                continue;
            }
            if section == "core" && key_of(l) == Some("plugins") {
                for p in ["ipc", "ipc-rules", "autostart"] {
                    if !l.split(['=', ' ', '\t']).any(|w| w == p) {
                        l.push(' ');
                        l.push_str(p);
                    }
                }
            }
        }

        let exe = super::command("").trim().to_string();
        let mut block = Vec::new();
        for (name, sub, key) in KEYS {
            block.push(format!("binding_{name} = {key}"));
            block.push(format!("command_{name} = {exe} {sub}"));
        }
        insert(&mut lines, "command", block);
        // start Linshot with Wayfire so it can restore these if the config is regenerated
        insert(&mut lines, "autostart", vec![format!("{AUTOSTART_KEY} = {exe} --tray")]);
        write(&path, &lines)
    }

    fn insert(lines: &mut Vec<String>, section: &str, block: Vec<String>) {
        match lines.iter().position(|l| is_section(l).as_deref() == Some(section)) {
            Some(i) => {
                for (j, b) in block.into_iter().enumerate() {
                    lines.insert(i + 1 + j, b);
                }
            }
            None => {
                if lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                    lines.push(String::new());
                }
                lines.push(format!("[{section}]"));
                lines.extend(block);
            }
        }
    }

    pub fn uninstall() -> Result<(), String> {
        let path = config_path();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        write(&path, &strip(text.lines().map(str::to_string).collect()))
    }

    fn write(path: &std::path::Path, lines: &[String]) -> Result<(), String> {
        // keep symlinked configs (dotfile managers) intact
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let path = resolved.as_path();
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let mut out = lines.join("\n");
        out.push('\n');
        // write via a temp file so a running Wayfire never reads a half-written config
        let tmp = path.with_extension("ini.linshot-tmp");
        std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    #[derive(Default)]
    struct Mem(std::collections::BTreeMap<String, String>);

    impl super::xfce::Store for Mem {
        fn get(&mut self, p: &str) -> Option<String> {
            self.0.get(p).cloned()
        }
        fn set(&mut self, p: &str, _: &str, v: &str) -> Result<(), String> {
            self.0.insert(p.into(), v.into());
            Ok(())
        }
        fn reset(&mut self, p: &str) -> Result<(), String> {
            self.0.remove(p);
            Ok(())
        }
    }

    #[test]
    fn xfce_install_and_uninstall() {
        use super::xfce;
        // Xfce 4.18 defaults: screenshooter on Print, Alt+Print and Shift+Print
        let mut st = Mem::default();
        for (k, v) in [("Print", "xfce4-screenshooter"), ("<Alt>Print", "xfce4-screenshooter -w"), ("<Shift>Print", "xfce4-screenshooter -r")] {
            st.0.insert(format!("/commands/default/{k}"), v.into());
            st.0.insert(format!("/commands/custom/{k}"), v.into());
        }
        st.0.insert("/commands/custom/<Super>e".into(), "thunar".into());
        let before = st.0.clone();

        assert!(!xfce::installed(&mut st));
        xfce::install(&mut st).unwrap();
        assert!(xfce::installed(&mut st));
        assert!(st.0["/commands/custom/<Primary>Print"].ends_with("linshot full"));
        assert!(st.0["/commands/custom/<Alt>Print"].ends_with("linshot window"));
        assert_eq!(st.0["/commands/custom/override"], "true");
        assert_eq!(st.0["/commands/custom/<Super>e"], "thunar");

        xfce::uninstall(&mut st).unwrap();
        assert!(!xfce::installed(&mut st));
        st.0.remove("/commands/custom/override");
        assert_eq!(st.0, before);
    }

    #[test]
    fn wayfire_install_and_uninstall() {
        let dir = std::env::temp_dir().join(format!("linshot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ini = dir.join("wayfire.ini");
        let original = "[core]\nplugins = command\n\n[command]\nbinding_screenshot = KEY_SYSRQ\ncommand_screenshot = grim\nbinding_terminal = <super> KEY_ENTER\ncommand_terminal = foot\n\n[decoration]\ntitle_height = 30\n";
        std::fs::write(&ini, original).unwrap();
        // SAFETY: this is the only test that touches WAYFIRE_CONFIG_FILE, and no
        // other test reads it, so the concurrent test threads cannot race on it.
        unsafe { std::env::set_var("WAYFIRE_CONFIG_FILE", &ini) };

        super::wayfire::install().unwrap();
        let s = std::fs::read_to_string(&ini).unwrap();
        println!("{s}");
        assert!(super::wayfire::installed());
        assert!(s.contains("# linshot-disabled: binding_screenshot = KEY_SYSRQ"));
        assert!(s.contains("binding_linshot_window = <alt> KEY_SYSRQ"));
        assert!(s.contains("binding_terminal = <super> KEY_ENTER"));
        assert!(s.contains("plugins = command ipc ipc-rules autostart"));
        assert!(s.contains("[autostart]\nlinshot_autostart = "));
        // idempotent
        super::wayfire::install().unwrap();
        assert_eq!(std::fs::read_to_string(&ini).unwrap().matches("command_linshot_region").count(), 1);

        super::wayfire::uninstall().unwrap();
        // plugins stay enabled; everything else is restored
        let after = std::fs::read_to_string(&ini).unwrap();
        assert_eq!(after, format!("{}\n[autostart]\n", original.replace("plugins = command", "plugins = command ipc ipc-rules autostart")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
