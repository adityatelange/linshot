// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! Global hotkeys on X11 (XGrabKey). Wayland compositors do not allow this;
//! users bind `linshot region` etc. in their desktop's keyboard settings instead.
use crate::{Cmd, Mode};

const XK_PRINT: u32 = 0xff61;

pub fn spawn(tx: async_channel::Sender<Cmd>) {
    std::thread::spawn(move || {
        if let Err(e) = run(tx) {
            eprintln!("linshot: global hotkeys unavailable: {e}");
        }
    });
}

fn run(tx: async_channel::Sender<Cmd>) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::protocol::Event;

    let (conn, sn) = x11rb::connect(None)?;
    let setup = conn.setup();
    let root = setup.roots[sn].root;
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let map = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = map.keysyms_per_keycode as usize;
    let keycode = map
        .keysyms
        .chunks(per.max(1))
        .position(|syms| syms.contains(&XK_PRINT))
        .map(|i| min + i as u8)
        .ok_or("no Print key on keyboard")?;

    let bindings = [
        (ModMask::from(0u16), Mode::Region),
        (ModMask::M1, Mode::Window),
        (ModMask::CONTROL, Mode::Full),
        (ModMask::SHIFT, Mode::Last),
    ];
    let extras = [
        ModMask::from(0u16),
        ModMask::LOCK,
        ModMask::M2,
        ModMask::LOCK | ModMask::M2,
    ];
    let mut failed = Vec::new();
    for (m, cmd) in &bindings {
        for e in extras {
            if conn
                .grab_key(true, root, *m | e, keycode, GrabMode::ASYNC, GrabMode::ASYNC)?
                .check()
                .is_err()
            {
                failed.push(format!("{cmd:?}"));
                break;
            }
        }
    }
    conn.flush()?;
    if !failed.is_empty() {
        eprintln!(
            "linshot: some Print-key shortcuts are taken by another program ({}).",
            failed.join(", ")
        );
    }
    let relevant = u16::from(KeyButMask::SHIFT | KeyButMask::CONTROL | KeyButMask::MOD1);
    loop {
        if let Event::KeyPress(ev) = conn.wait_for_event()? {
            if ev.detail != keycode {
                continue;
            }
            let st = u16::from(ev.state) & relevant;
            let cmd = if st == u16::from(KeyButMask::MOD1) {
                Mode::Window
            } else if st == u16::from(KeyButMask::CONTROL) {
                Mode::Full
            } else if st == u16::from(KeyButMask::SHIFT) {
                Mode::Last
            } else if st == 0 {
                Mode::Region
            } else {
                continue;
            };
            // ungrab keyboard focus is implicit; tell the GTK thread
            let _ = tx.send_blocking(Cmd::cap(cmd));
        }
    }
}
