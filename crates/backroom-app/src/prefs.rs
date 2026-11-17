//! Interface preferences. Nothing secret.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageLayout {
    /// Time, nick and text in aligned columns (irssi, WeeChat).
    #[default]
    Columns,
    /// Time, then nick and text run together.
    Inline,
    /// The nick above a group of messages.
    Stacked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimestampStyle {
    Hidden,
    #[default]
    Minutes,
    Seconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresenceStyle {
    /// One line per join or part.
    Show,
    /// Each run of joins and parts on one line.
    #[default]
    Fold,
    Hide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NickColoring {
    /// By channel rank: ops, voiced, …
    #[default]
    Rank,
    /// Each nick its own colour.
    Nick,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Compact,
    #[default]
    Regular,
    Comfortable,
}

impl Density {
    /// Vertical padding around each message line, in pixels.
    pub fn padding(self) -> u32 {
        match self {
            Density::Compact => 0,
            Density::Regular => 1,
            Density::Comfortable => 3,
        }
    }
}

pub const FONT_SIZES: std::ops::RangeInclusive<f64> = 8.0..=24.0;
pub const NICK_COLUMN_WIDTHS: std::ops::RangeInclusive<u32> = 6..=24;
/// `font_family` value for the system's monospace font.
pub const MONOSPACE: &str = "monospace";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub appearance: AppearanceMode,
    pub light_theme: String,
    pub dark_theme: String,
    pub custom_themes: Vec<Theme>,
    /// "" for the interface font, `MONOSPACE` for the monospace font,
    /// otherwise a font family name.
    pub font_family: String,
    /// Points; 0 follows the interface font's size.
    pub font_size: f64,
    pub density: Density,
    pub layout: MessageLayout,
    /// Show the nick on every message, not just the first of a run.
    pub repeat_nick: bool,
    /// Width of the nick column, in characters.
    pub nick_column_width: u32,
    pub timestamps: TimestampStyle,
    pub presence: PresenceStyle,
    pub nick_coloring: NickColoring,
    /// Show mIRC colours and formatting; off strips them.
    pub show_formatting: bool,
    /// Words that count as mentions, besides our nick.
    pub highlight_words: Vec<String>,
    /// Nicks whose messages are hidden.
    pub ignored: Vec<String>,
    /// Desktop notifications for mentions and private messages.
    pub notifications: bool,
    pub notification_sound: bool,
    /// Also notify for every channel message, not only mentions.
    pub notify_all_messages: bool,
    /// Let others see when we're typing.
    pub send_typing: bool,
    /// A preview when the mouse rests on a link (fetching the link reveals
    /// our address to its site, so only hovered links are fetched).
    pub link_previews: bool,
    /// Without a bouncer: also write plain-text logs, one file per
    /// conversation per day.
    pub text_logs: bool,
    /// Where the text logs go; "" for the default folder.
    pub text_log_folder: String,
    /// Keep running when the window is closed (until Quit).
    pub run_in_background: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            appearance: AppearanceMode::System,
            light_theme: Theme::default_light().id,
            dark_theme: Theme::default_dark().id,
            custom_themes: Vec::new(),
            font_family: String::new(),
            font_size: 0.0,
            density: Density::Regular,
            layout: MessageLayout::Columns,
            repeat_nick: false,
            nick_column_width: 12,
            timestamps: TimestampStyle::Minutes,
            presence: PresenceStyle::Fold,
            nick_coloring: NickColoring::Rank,
            show_formatting: true,
            highlight_words: Vec::new(),
            ignored: Vec::new(),
            notifications: true,
            notification_sound: true,
            notify_all_messages: false,
            send_typing: true,
            link_previews: true,
            text_logs: false,
            text_log_folder: String::new(),
            run_in_background: false,
        }
    }
}

impl Preferences {
    pub fn load(path: &Path) -> Preferences {
        let mut prefs: Preferences = crate::storage::read_json(path).unwrap_or_default();
        if prefs.font_size != 0.0 && !FONT_SIZES.contains(&prefs.font_size) {
            prefs.font_size = 0.0;
        }
        if !NICK_COLUMN_WIDTHS.contains(&prefs.nick_column_width) {
            prefs.nick_column_width = 12;
        }
        prefs
    }

    pub fn save(&self, path: &Path) {
        crate::storage::write_json(path, self);
    }

    pub fn ignore(&mut self, nick: &str) {
        if !self.ignored.iter().any(|n| n.eq_ignore_ascii_case(nick)) {
            self.ignored.push(nick.to_owned());
        }
    }

    pub fn unignore(&mut self, nick: &str) {
        self.ignored.retain(|n| !n.eq_ignore_ascii_case(nick));
    }

    /// Where the core writes text logs; `None` when they're off.
    pub fn active_text_log_path(&self, default: &Path) -> Option<String> {
        self.text_logs
            .then(|| self.text_log_dir(default).display().to_string())
    }

    pub fn text_log_dir(&self, default: &Path) -> PathBuf {
        if self.text_log_folder.is_empty() {
            default.to_owned()
        } else {
            PathBuf::from(&self.text_log_folder)
        }
    }

    // Themes

    pub fn all_themes(&self) -> Vec<Theme> {
        Theme::built_ins()
            .into_iter()
            .chain(self.custom_themes.iter().cloned())
            .collect()
    }

    pub fn themes(&self, dark: bool) -> Vec<Theme> {
        self.all_themes()
            .into_iter()
            .filter(|t| t.is_dark == dark)
            .collect()
    }

    /// The chosen theme for an appearance, falling back to the default if
    /// it was deleted or is for the other appearance.
    pub fn theme(&self, dark: bool) -> Theme {
        let id = if dark {
            &self.dark_theme
        } else {
            &self.light_theme
        };
        self.all_themes()
            .into_iter()
            .find(|t| &t.id == id && t.is_dark == dark)
            .unwrap_or_else(|| {
                if dark {
                    Theme::default_dark()
                } else {
                    Theme::default_light()
                }
            })
    }

    /// Adds imported themes as new custom themes. Returns how many.
    pub fn import_themes(&mut self, json: &[u8]) -> Result<usize, serde_json::Error> {
        let themes = Theme::decode_all(json)?;
        let n = themes.len();
        for mut theme in themes {
            theme.id = format!("custom.{}", uuid::Uuid::new_v4());
            self.custom_themes.push(theme);
        }
        Ok(n)
    }

    pub fn delete_theme(&mut self, id: &str) {
        self.custom_themes.retain(|t| t.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, r#"{"layout":"inline","nick_column_width":99}"#).unwrap();
        let prefs = Preferences::load(&path);
        assert_eq!(prefs.layout, MessageLayout::Inline);
        assert_eq!(prefs.nick_column_width, 12);
        assert!(prefs.notifications);
    }

    #[test]
    fn ignore_list_is_case_insensitive() {
        let mut p = Preferences::default();
        p.ignore("Bob");
        p.ignore("bob");
        assert_eq!(p.ignored, ["Bob"]);
        p.unignore("BOB");
        assert!(p.ignored.is_empty());
    }

    #[test]
    fn imported_themes_get_new_ids() {
        let mut p = Preferences::default();
        let json = serde_json::to_vec(&Theme::dracula()).unwrap();
        assert_eq!(p.import_themes(&json).unwrap(), 1);
        assert!(p.custom_themes[0].id.starts_with("custom."));
        p.dark_theme = p.custom_themes[0].id.clone();
        assert_eq!(p.theme(true).name, "Dracula");
        assert_eq!(p.theme(false).id, Theme::default_light().id);
    }
}
