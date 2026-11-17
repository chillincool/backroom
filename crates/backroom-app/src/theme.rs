//! Colours for the chat area, member list and topic. The JSON form is the
//! same as the macOS app's, so theme files work on both.

use serde::{Deserialize, Serialize};

/// Each colour is a `#rrggbb` string; `None` means the platform's colour
/// for that role, so the default themes look native.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub id: String,
    pub name: String,
    /// Which appearance the theme is for.
    pub is_dark: bool,
    /// Chat background.
    #[serde(default)]
    pub background: Option<String>,
    /// Topic banner, day separators, composer.
    #[serde(default)]
    pub surface: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    /// Timestamps, joins and parts, notices.
    #[serde(default)]
    pub muted: Option<String>,
    /// Your own nick.
    #[serde(default)]
    pub accent: Option<String>,
    /// Nick colours by channel rank.
    #[serde(default)]
    pub ranks: RankColors,
    /// Nick colours when colouring by nick; empty for the platform palette.
    #[serde(default)]
    pub nicks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RankColors {
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub admin: Option<String>,
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub halfop: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
}

/// Which rank a membership prefix stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Owner,
    Admin,
    Op,
    Halfop,
    Voice,
    /// Ranks some networks add, like IRCnet's `!`.
    Other,
}

impl Rank {
    pub fn of(prefix: char) -> Rank {
        match prefix {
            '~' => Rank::Owner,
            '&' => Rank::Admin,
            '@' => Rank::Op,
            '%' => Rank::Halfop,
            '+' => Rank::Voice,
            _ => Rank::Other,
        }
    }
}

pub const BUILT_IN_PREFIX: &str = "builtin.";

impl Theme {
    pub fn is_built_in(&self) -> bool {
        self.id.starts_with(BUILT_IN_PREFIX)
    }

    /// A theme file: one theme or an array of them.
    pub fn decode_all(json: &[u8]) -> Result<Vec<Theme>, serde_json::Error> {
        serde_json::from_slice::<Vec<Theme>>(json)
            .or_else(|_| serde_json::from_slice::<Theme>(json).map(|t| vec![t]))
    }

    /// The theme's colour for a rank; `None` for the platform default.
    pub fn rank_color(&self, rank: Rank) -> Option<&str> {
        let r = &self.ranks;
        match rank {
            Rank::Owner => r.owner.as_deref(),
            Rank::Admin | Rank::Other => r.admin.as_deref(),
            Rank::Op => r.op.as_deref(),
            Rank::Halfop => r.halfop.as_deref(),
            Rank::Voice => r.voice.as_deref(),
        }
    }

    fn plain(id: &str, name: &str, is_dark: bool) -> Theme {
        Theme {
            id: id.into(),
            name: name.into(),
            is_dark,
            background: None,
            surface: None,
            text: None,
            muted: None,
            accent: None,
            ranks: RankColors::default(),
            nicks: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn colored(
        id: &str,
        name: &str,
        is_dark: bool,
        [background, surface, text, muted, accent]: [&str; 5],
        [owner, admin, op, halfop, voice]: [&str; 5],
        nicks: &[&str],
    ) -> Theme {
        let s = |v: &str| Some(v.to_owned());
        Theme {
            background: s(background),
            surface: s(surface),
            text: s(text),
            muted: s(muted),
            accent: s(accent),
            ranks: RankColors {
                owner: s(owner),
                admin: s(admin),
                op: s(op),
                halfop: s(halfop),
                voice: s(voice),
            },
            nicks: nicks.iter().map(|n| (*n).to_owned()).collect(),
            ..Theme::plain(id, name, is_dark)
        }
    }

    pub fn default_light() -> Theme {
        Theme::plain("builtin.default-light", "Default", false)
    }

    pub fn default_dark() -> Theme {
        Theme::plain("builtin.default-dark", "Default", true)
    }

    pub fn built_ins() -> Vec<Theme> {
        vec![
            Theme::default_light(),
            Theme::default_dark(),
            Theme::solarized(false),
            Theme::solarized(true),
            Theme::gruvbox(false),
            Theme::gruvbox(true),
            Theme::dracula(),
            Theme::nord_light(),
            Theme::nord(),
            Theme::high_contrast(false),
            Theme::high_contrast(true),
        ]
    }

    // https://ethanschoonover.com/solarized/
    fn solarized(dark: bool) -> Theme {
        let (yellow, orange, red, magenta) = ("#b58900", "#cb4b16", "#dc322f", "#d33682");
        let (violet, blue, cyan, green) = ("#6c71c4", "#268bd2", "#2aa198", "#859900");
        let base = if dark {
            ["#002b36", "#073642", "#839496", "#586e75", blue]
        } else {
            ["#fdf6e3", "#eee8d5", "#657b83", "#93a1a1", blue]
        };
        let id = if dark {
            "builtin.solarized-dark"
        } else {
            "builtin.solarized-light"
        };
        Theme::colored(
            id,
            "Solarized",
            dark,
            base,
            [violet, magenta, green, cyan, orange],
            &[yellow, orange, red, magenta, violet, blue, cyan, green],
        )
    }

    // https://github.com/morhetz/gruvbox
    fn gruvbox(dark: bool) -> Theme {
        if dark {
            Theme::colored(
                "builtin.gruvbox-dark",
                "Gruvbox",
                true,
                ["#282828", "#3c3836", "#ebdbb2", "#928374", "#83a598"],
                ["#d3869b", "#fb4934", "#b8bb26", "#8ec07c", "#fe8019"],
                &[
                    "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#fe8019",
                ],
            )
        } else {
            Theme::colored(
                "builtin.gruvbox-light",
                "Gruvbox",
                false,
                ["#fbf1c7", "#ebdbb2", "#3c3836", "#928374", "#076678"],
                ["#8f3f71", "#9d0006", "#79740e", "#427b58", "#af3a03"],
                &[
                    "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#af3a03",
                ],
            )
        }
    }

    // https://draculatheme.com/contribute (dark only)
    pub fn dracula() -> Theme {
        Theme::colored(
            "builtin.dracula",
            "Dracula",
            true,
            ["#282a36", "#44475a", "#f8f8f2", "#6272a4", "#bd93f9"],
            ["#ff79c6", "#ff5555", "#50fa7b", "#8be9fd", "#ffb86c"],
            &[
                "#8be9fd", "#50fa7b", "#ffb86c", "#ff79c6", "#bd93f9", "#ff5555", "#f1fa8c",
            ],
        )
    }

    // https://www.nordtheme.com/docs/colors-and-palettes
    fn nord() -> Theme {
        Theme::colored(
            "builtin.nord",
            "Nord",
            true,
            ["#2e3440", "#3b4252", "#d8dee9", "#7b88a1", "#88c0d0"],
            ["#b48ead", "#bf616a", "#a3be8c", "#8fbcbb", "#d08770"],
            &[
                "#bf616a", "#d08770", "#ebcb8b", "#a3be8c", "#8fbcbb", "#88c0d0", "#81a1c1",
                "#b48ead",
            ],
        )
    }

    fn nord_light() -> Theme {
        Theme::colored(
            "builtin.nord-light",
            "Nord",
            false,
            ["#eceff4", "#e5e9f0", "#2e3440", "#4c566a", "#5e81ac"],
            ["#8a5f87", "#bf616a", "#5f7e45", "#3f7f7d", "#b5643e"],
            &[
                "#bf616a", "#b5643e", "#8a6d1e", "#5f7e45", "#3f7f7d", "#5e81ac", "#8a5f87",
            ],
        )
    }

    fn high_contrast(dark: bool) -> Theme {
        if dark {
            Theme::colored(
                "builtin.high-contrast-dark",
                "High Contrast",
                true,
                ["#000000", "#262626", "#ffffff", "#bfbfbf", "#4da3ff"],
                ["#d699ff", "#ff7ab8", "#5cff5c", "#4dffff", "#ffc04d"],
                &[
                    "#ff6b6b", "#ffc04d", "#5cff5c", "#4dffff", "#4da3ff", "#d699ff", "#ff7ab8",
                ],
            )
        } else {
            Theme::colored(
                "builtin.high-contrast-light",
                "High Contrast",
                false,
                ["#ffffff", "#e6e6e6", "#000000", "#4d4d4d", "#0040dd"],
                ["#7a00cc", "#b00060", "#006b00", "#00676b", "#8a4b00"],
                &[
                    "#b30000", "#8a4b00", "#006b00", "#00676b", "#0040dd", "#7a00cc", "#b00060",
                ],
            )
        }
    }
}

/// Parses `#rrggbb` (or `rrggbb`) into `0xRRGGBB`.
pub fn parse_hex(hex: &str) -> Option<u32> {
    let hex = hex.trim();
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Stable across launches and case-insensitive: which palette entry a nick
/// gets (djb2, as on macOS).
pub fn palette_index(nick: &str, count: usize) -> usize {
    let mut hash: u32 = 5381;
    for byte in nick.to_lowercase().bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(byte));
    }
    (hash % (count.max(1) as u32)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_macos_theme_files() {
        let json = br##"{"id":"custom.x","name":"Mine","isDark":true,"background":"#101010",
            "ranks":{"op":"#00ff00"},"nicks":["#ff0000"]}"##;
        let themes = Theme::decode_all(json).unwrap();
        assert_eq!(themes[0].rank_color(Rank::Op), Some("#00ff00"));
        assert_eq!(themes[0].text, None);
        let list = serde_json::to_vec(&Theme::built_ins()).unwrap();
        assert_eq!(Theme::decode_all(&list).unwrap().len(), 11);
    }

    #[test]
    fn hex_and_palette() {
        assert_eq!(parse_hex("#0a0B0c"), Some(0x0a0b0c));
        assert_eq!(parse_hex("zzz"), None);
        assert_eq!(palette_index("Alice", 7), palette_index("alice", 7));
        assert!(palette_index("bob", 7) < 7);
    }
}
