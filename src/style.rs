// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Aditya Telange

//! App stylesheet. Layout rules live in BASE; colours come from a light or
//! dark token sheet that is swapped when the appearance changes.
use gtk::gdk;

const BASE: &str = r#"
/* ---------- selection overlay ---------- */
.linshot-overlay { background: black; }

/* ---------- launcher / preferences ---------- */
window.ls-launcher, window.ls-prefs { background: @ls_header; color: @ls_text; }
.linshot-big {
  padding: 14px 10px 12px; border-radius: 14px; min-width: 104px;
  background: @ls_surface; color: @ls_text;
  border: 1px solid @ls_border; box-shadow: 0 1px 3px rgba(0,0,0,0.10);
  transition: border-color 120ms ease, background 120ms ease;
}
.linshot-big:hover { border-color: @ls_accent; background: @ls_accent_soft; }
.linshot-big:active { background: shade(@ls_accent_soft, 0.9); }
.linshot-big:hover .ls-mode-icon { color: @ls_accent; }
.ls-mode-name { font-weight: 600; }
.ls-mode-key { color: @ls_muted; font-size: 0.82em; font-feature-settings: "tnum"; }
.ls-launcher-cap { color: @ls_muted; }
.ls-launcher-hint, .ls-prefs-note { color: @ls_muted; font-size: 0.92em; }
.ls-prefs-label { color: @ls_muted; }
.ls-prefs-section { font-weight: 700; font-size: 0.85em; color: @ls_muted; margin-top: 6px; }
.ls-launcher button.flat:hover { background: rgba(239,68,68,0.15); color: #ef4444; }

/* ---------- editor ---------- */
window.ls-editor { background: @ls_canvas; }
window.ls-editor headerbar {
  background: @ls_header;
  color: @ls_text;
  box-shadow: none;
  border-bottom: 1px solid @ls_border;
  min-height: 48px;
  padding: 0 6px;
}
window.ls-editor headerbar button { border-radius: 9px; }
.ls-title { font-weight: 700; }
.ls-subtitle { font-size: 0.82em; color: @ls_muted; font-feature-settings: "tnum"; }

.ls-canvas, .ls-canvas > viewport { background: @ls_canvas; }

.ls-dock, .ls-pill {
  background: @ls_surface;
  color: @ls_text;
  border: 1px solid @ls_border;
  box-shadow: 0 10px 30px rgba(0,0,0,0.25), 0 1px 3px rgba(0,0,0,0.18);
}
.ls-dock { border-radius: 18px; }
.ls-dock > viewport { background: none; }
.ls-dock-tools { padding: 6px; }
.ls-pill { border-radius: 999px; padding: 5px 8px; }

.ls-tool {
  min-width: 40px; min-height: 40px; padding: 0;
  border-radius: 12px; border: none; box-shadow: none;
  background: none; color: @ls_text;
  transition: background 120ms ease;
}
.ls-tool:hover { background: @ls_hover; }
.ls-tool:checked { background: @ls_accent; color: #ffffff; }
.ls-tool:checked:hover { background: @ls_accent; }

.ls-dock-tools separator { margin: 5px 8px; min-height: 1px; background: @ls_border; }
.ls-pill separator { margin: 6px 4px; min-width: 1px; background: @ls_border; }

.ls-pill button {
  border-radius: 999px; min-height: 30px; min-width: 30px;
  padding: 0 8px; border: none; box-shadow: none;
  background: none; color: @ls_text;
}
.ls-pill button:hover { background: @ls_hover; }
/* GtkToggleButton's CSS node is button.toggle */
.ls-pill button.toggle:checked { background: @ls_accent_soft; color: @ls_accent; }
.ls-pill button.toggle:checked:hover { background: @ls_accent_soft; }
.ls-pill button.ls-danger:hover { background: rgba(239,68,68,0.15); color: #ef4444; }
.ls-pill .ls-cap { color: @ls_muted; font-size: 0.85em; margin: 0 2px 0 4px; }
.ls-pill scale { min-width: 120px; padding: 0 4px; }
.ls-pill scale trough { min-height: 4px; border-radius: 99px; background: @ls_track; }
.ls-pill scale highlight { background: @ls_accent; border-radius: 99px; }
.ls-pill scale slider { margin: -6px; min-width: 16px; min-height: 16px; background: #ffffff; border-radius: 99px;
  box-shadow: 0 0 0 1px rgba(0,0,0,0.14), 0 1px 3px rgba(0,0,0,0.35); border: none; }
.ls-pill .ls-value { font-feature-settings: "tnum"; min-width: 22px; color: @ls_muted; font-size: 0.85em; }
.ls-pill spinbutton { min-height: 30px; border-radius: 999px; background: @ls_hover; border: none; box-shadow: none; }
.ls-pill spinbutton text { min-width: 28px; }
.ls-pill spinbutton button { min-width: 24px; }
.ls-pill button.ls-swatch {
  min-width: 18px; min-height: 18px; padding: 0; margin: 0 3px;
  border-radius: 999px; border: 2px solid transparent;
  box-shadow: inset 0 0 0 1px rgba(127,127,127,0.45);
}
/* ring with a surface-coloured gap: visible on every swatch colour in both themes */
.ls-pill button.ls-swatch.selected {
  border-color: @ls_surface;
  box-shadow: 0 0 0 2px @ls_text;
}
.ls-pill button.ls-swatch:hover { box-shadow: inset 0 0 0 1px rgba(127,127,127,0.45), 0 0 0 2px @ls_hover; }
.ls-pill button.ls-swatch.selected:hover { box-shadow: 0 0 0 2px @ls_text; }
/* colour dialog buttons: round swatch inside a ring (rainbow = custom colour) */
.ls-pill colorbutton > button {
  padding: 3px; margin: 0 2px; min-width: 0; min-height: 0; border-radius: 999px;
  background: @ls_track;
}
.ls-pill colorbutton.ls-custom > button {
  background-image: conic-gradient(#ef4444, #facc15, #22c55e, #06b6d4, #3b82f6, #a855f7, #ef4444);
}
.ls-pill colorbutton > button:hover { box-shadow: 0 0 0 2px @ls_hover; }
.ls-pill colorbutton colorswatch {
  border-radius: 999px; box-shadow: 0 0 0 1.5px @ls_surface;
}
.ls-pill colorbutton colorswatch > overlay { border-radius: 999px; border: none; }

#ls-swatch-0 { background: #ef4444; }
#ls-swatch-1 { background: #f97316; }
#ls-swatch-2 { background: #facc15; }
#ls-swatch-3 { background: #22c55e; }
#ls-swatch-4 { background: #3b82f6; }
#ls-swatch-5 { background: #a855f7; }
#ls-swatch-6 { background: #111827; }
#ls-swatch-7 { background: #ffffff; }

.ls-primary {
  background: @ls_accent; color: #ffffff; font-weight: 600;
  border-radius: 9px; padding: 4px 14px; border: none; box-shadow: none;
}
.ls-primary:hover { background: shade(@ls_accent, 1.08); }
.ls-pill button.ls-primary { background: @ls_accent; color: #ffffff; padding: 0 14px; }
.ls-pill button.ls-primary:hover { background: shade(@ls_accent, 1.08); }
.ls-primary:active { background: shade(@ls_accent, 0.92); }
.ls-soft { background: @ls_hover; border-radius: 9px; padding: 4px 12px; border: none; box-shadow: none; font-weight: 600; }

.ls-zoom-label { font-feature-settings: "tnum"; min-width: 54px; font-weight: 600; }
.ls-hint { color: @ls_muted; padding: 0 10px; }
.ls-toast {
  background: @ls_toast; color: #ffffff; font-weight: 600;
  border-radius: 999px; padding: 9px 18px;
  box-shadow: 0 8px 24px rgba(0,0,0,0.3);
}
"#;

const LIGHT: &str = r#"
@define-color ls_canvas #e5e6eb;
@define-color ls_header #f8f8fa;
@define-color ls_surface rgba(255,255,255,0.97);
@define-color ls_border rgba(0,0,0,0.08);
@define-color ls_text #1c1d21;
@define-color ls_muted #6b6e78;
@define-color ls_hover rgba(0,0,0,0.06);
@define-color ls_track rgba(0,0,0,0.14);
@define-color ls_accent #1f9d5f;
@define-color ls_accent_soft rgba(31,157,95,0.14);
@define-color ls_toast rgba(28,29,33,0.94);
"#;

const DARK: &str = r#"
@define-color ls_canvas #131417;
@define-color ls_header #1d1e22;
@define-color ls_surface rgba(38,40,45,0.96);
@define-color ls_border rgba(255,255,255,0.08);
@define-color ls_text #ececf1;
@define-color ls_muted #9b9ea8;
@define-color ls_hover rgba(255,255,255,0.08);
@define-color ls_track rgba(255,255,255,0.16);
@define-color ls_accent #22a565;
@define-color ls_accent_soft rgba(34,165,101,0.2);
@define-color ls_toast rgba(14,15,17,0.94);
"#;

thread_local! {
    static TOKENS: gtk::CssProvider = gtk::CssProvider::new();
}

pub fn init() {
    let Some(d) = gdk::Display::default() else { return };
    TOKENS.with(|t| {
        crate::compat::load_css(t, LIGHT);
        gtk::style_context_add_provider_for_display(&d, t, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    });
    let base = gtk::CssProvider::new();
    crate::compat::load_css(&base, BASE);
    gtk::style_context_add_provider_for_display(&d, &base, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
}

pub fn set_dark(dark: bool) {
    TOKENS.with(|t| crate::compat::load_css(t, if dark { DARK } else { LIGHT }));
}
