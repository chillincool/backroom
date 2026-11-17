//! The chat area's look: CSS from the theme and preferences, and the
//! resolved colours that message markup needs.

use adw::prelude::*;
use backroom_app::prefs::{MONOSPACE, Preferences};
use backroom_app::theme::{Rank, Theme, palette_index};

/// Concrete `#rrggbb` colours for Pango markup. Theme colours left unset
/// come from the desktop (libadwaita's palette and accent colour).
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub background: String,
    /// `None`: the theme leaves it to the desktop; markup doesn't set it.
    pub text: Option<String>,
    pub muted: String,
    pub accent: String,
    pub error: String,
    ranks: [String; 6],
    nicks: Vec<String>,
}

// libadwaita's palette (GNOME HIG): readable on light and dark backgrounds.
const LIGHT_NICKS: &[&str] = &[
    "#c01c28", "#c64600", "#26a269", "#1c71d8", "#613583", "#986a44", "#0b7a75", "#a51d76",
    "#3a6b00", "#813d9c",
];
const DARK_NICKS: &[&str] = &[
    "#f66151", "#ffa348", "#8ff0a4", "#99c1f1", "#dc8add", "#cdab8f", "#62d6c4", "#f5a3d8",
    "#c0e875", "#c061cb",
];

impl Palette {
    /// `probe` gives the desktop's text and accent colours.
    pub fn new(theme: &Theme, dark: bool, probe: &Probe) -> Palette {
        let pick = |set: &Option<String>, fallback: &str| {
            set.clone().unwrap_or_else(|| fallback.to_owned())
        };
        let default_ranks = if dark {
            [
                "#dc8add", "#f5a3d8", "#8ff0a4", "#62d6c4", "#ffa348", "#99c1f1",
            ]
        } else {
            [
                "#813d9c", "#a51d76", "#26a269", "#0b7a75", "#c64600", "#1c71d8",
            ]
        };
        let ranks = [
            Rank::Owner,
            Rank::Admin,
            Rank::Op,
            Rank::Halfop,
            Rank::Voice,
            Rank::Other,
        ]
        .map(|r| r)
        .iter()
        .zip(default_ranks)
        .map(|(rank, fallback)| theme.rank_color(*rank).unwrap_or(fallback).to_owned())
        .collect::<Vec<_>>()
        .try_into()
        .unwrap_or_else(|_| default_ranks.map(str::to_owned));
        let nicks = if theme.nicks.is_empty() {
            (if dark { DARK_NICKS } else { LIGHT_NICKS })
                .iter()
                .map(|c| (*c).to_owned())
                .collect()
        } else {
            theme.nicks.clone()
        };
        Palette {
            dark,
            background: pick(&theme.background, &probe.background),
            text: theme.text.clone(),
            muted: pick(&theme.muted, &probe.muted),
            accent: pick(&theme.accent, &probe.accent),
            error: if dark { "#ff7b63" } else { "#c01c28" }.into(),
            ranks,
            nicks,
        }
    }

    /// Text colour where one must be given (reverse video).
    pub fn text_or_default(&self) -> String {
        self.text
            .clone()
            .unwrap_or_else(|| if self.dark { "#ffffff" } else { "#1e1e1e" }.to_owned())
    }

    /// The colour for a member's highest prefix; `None` (no prefix): the
    /// text colour.
    pub fn rank(&self, prefix: Option<char>) -> Option<&str> {
        Some(match prefix.map(Rank::of) {
            None => return self.text.as_deref(),
            Some(Rank::Owner) => &self.ranks[0],
            Some(Rank::Admin) => &self.ranks[1],
            Some(Rank::Op) => &self.ranks[2],
            Some(Rank::Halfop) => &self.ranks[3],
            Some(Rank::Voice) => &self.ranks[4],
            Some(Rank::Other) => &self.ranks[5],
        })
    }

    pub fn nick(&self, nick: &str) -> &str {
        &self.nicks[palette_index(nick, self.nicks.len())]
    }
}

/// The desktop's colours where markup needs them. The accent colour is
/// read from a label inside the window (styled like the rest of it), so it
/// follows the desktop's accent.
#[derive(Debug, Clone)]
pub struct Probe {
    pub background: String,
    pub muted: String,
    pub accent: String,
}

impl Probe {
    pub fn read(dark: bool, accent_label: &gtk::Label) -> Probe {
        let fallback = if dark { "#78aeed" } else { "#1c71d8" };
        let accent = hex(&accent_label.color());
        // Unstyled (black or white): the label isn't in the window yet.
        let accent = if accent == "#000000" || accent == "#ffffff" {
            fallback.to_owned()
        } else {
            accent
        };
        Probe {
            background: if dark { "#1e1e1e" } else { "#ffffff" }.into(),
            muted: if dark { "#9a9996" } else { "#77767b" }.into(),
            accent,
        }
    }
}

fn hex(c: &gtk::gdk::RGBA) -> String {
    let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        to(c.red()),
        to(c.green()),
        to(c.blue())
    )
}

/// CSS for the chat area, member list and topic.
pub fn css(theme: &Theme, prefs: &Preferences) -> String {
    let or = |set: &Option<String>, fallback: &str| {
        set.as_deref()
            .filter(|c| backroom_app::theme::parse_hex(c).is_some())
            .map_or_else(|| fallback.to_owned(), str::to_owned)
    };
    let bg = or(&theme.background, "@view_bg_color");
    let surface = or(&theme.surface, "@window_bg_color");
    let text = or(&theme.text, "@view_fg_color");
    let muted = or(&theme.muted, "alpha(@view_fg_color, 0.55)");
    let accent = or(&theme.accent, "@accent_color");
    let font = match prefs.font_family.as_str() {
        "" => String::new(),
        MONOSPACE => "font-family: monospace;".into(),
        family => format!(
            "font-family: \"{}\";",
            family.replace(['"', '\\', ';', '}'], "")
        ),
    };
    let size = if prefs.font_size > 0.0 {
        format!("font-size: {}pt;", prefs.font_size)
    } else {
        String::new()
    };
    let pad = prefs.density.padding();
    format!(
        r#"
.chat-view, .chat-view > listview, .chat-view row {{ background-color: {bg}; color: {text}; }}
.chat-view row {{ padding: 0; }}
.chat-view row:hover {{ background-color: alpha({text}, 0.03); }}
.chat-text {{ {font} {size} }}
.chat-view .message {{ padding: {pad}px 12px; }}
.chat-view .message.stacked-head {{ padding-top: 6px; }}
.chat-view .mention {{ background-color: alpha({accent}, 0.14); }}
.chat-view .flash {{ background-color: alpha({accent}, 0.30); }}
.chat-view .muted, .member-list .muted, .topic-bar .muted {{ color: {muted}; }}
.chat-view .time {{ color: alpha({muted}, 0.85); font-feature-settings: "tnum"; font-size: 0.85em; }}
.chat-view .day {{ color: {muted}; font-size: 0.85em; font-weight: 500; padding: 8px 12px; }}
.chat-view .day separator {{ background-color: alpha({muted}, 0.3); }}
.chat-view .unread-line {{ padding: 4px 12px; }}
.chat-view .unread-line separator {{ background-color: @error_color; }}
.chat-view .unread-line .badge {{ background-color: @error_color; color: white; border-radius: 99px; padding: 0 6px; font-size: 0.75em; font-weight: bold; }}
.chat-view .history-header {{ color: {muted}; font-size: 0.85em; padding: 6px; }}
.chat-view label link, .topic-bar label link {{ color: inherit; text-decoration-line: none; }}
.chat-view label.ctrl link:hover, .topic-bar label.ctrl link:hover {{ text-decoration-line: underline; }}
.topic-bar {{ background-color: {surface}; color: {text}; padding: 4px 12px; border-bottom: 1px solid alpha({text}, 0.1); }}
.composer-box {{ background-color: {bg}; border-top: 1px solid alpha({text}, 0.12); }}
.composer, .composer > text {{ background-color: transparent; color: {text}; }}
.typing {{ color: {muted}; font-size: 0.85em; padding: 0 12px; }}
.member-list, .member-list row {{ background-color: {surface}; color: {text}; }}
.member-list row {{ padding: 1px 8px; min-height: 0; }}
paned.pane-split .pane.focused-pane .topic-bar {{ box-shadow: inset 0 2px {accent}; }}
.sidebar-badge {{ background-color: alpha(@window_fg_color, 0.15); border-radius: 99px; padding: 0 6px; font-size: 0.8em; font-weight: bold; min-width: 12px; }}
.sidebar-badge.mention-badge {{ background-color: @accent_bg_color; color: @accent_fg_color; }}
.navigation-sidebar > row.network-row {{ margin-top: 6px; padding-left: 2px; }}
.navigation-sidebar > row.network-row:first-child {{ margin-top: 0; }}
.sidebar-fold {{ min-width: 22px; min-height: 22px; padding: 0; }}
.status-dot {{ min-width: 7px; min-height: 7px; border-radius: 99px; background-color: @warning_color; }}
.status-dot.online {{ background-color: @success_color; }}
.status-dot.offline {{ background-color: alpha(@window_fg_color, 0.3); }}
.unread-name {{ font-weight: bold; }}
.preview-card {{ padding: 8px; }}
"#
    )
}
