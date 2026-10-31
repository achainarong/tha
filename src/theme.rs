//! Foreground palettes. Backgrounds are never painted, so transparent terminals keep working.
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    // Monochrome was briefly a theme; a file naming it should still load.
    #[serde(alias = "monochrome")]
    Terminal,
    CatppuccinMocha,
    GruvboxDark,
    TokyoNight,
    Nord,
}

/// cyan, green, yellow, red, blue, magenta, dark gray
type Palette = [(u8, u8, u8); 7];

impl Theme {
    pub const ALL: &[Self] = &[
        Self::Terminal,
        Self::CatppuccinMocha,
        Self::GruvboxDark,
        Self::TokyoNight,
        Self::Nord,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal colors",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::TokyoNight => "Tokyo Night",
            Self::Nord => "Nord",
        }
    }

    fn palette(self) -> Option<Palette> {
        Some(match self {
            Self::Terminal => return None,
            Self::CatppuccinMocha => [
                (0x89, 0xdc, 0xeb),
                (0xa6, 0xe3, 0xa1),
                (0xf9, 0xe2, 0xaf),
                (0xf3, 0x8b, 0xa8),
                (0x89, 0xb4, 0xfa),
                (0xcb, 0xa6, 0xf7),
                (0x6c, 0x70, 0x86),
            ],
            Self::GruvboxDark => [
                (0x8e, 0xc0, 0x7c),
                (0xb8, 0xbb, 0x26),
                (0xfa, 0xbd, 0x2f),
                (0xfb, 0x49, 0x34),
                (0x83, 0xa5, 0x98),
                (0xd3, 0x86, 0x9b),
                (0x92, 0x83, 0x74),
            ],
            Self::TokyoNight => [
                (0x7d, 0xcf, 0xff),
                (0x9e, 0xce, 0x6a),
                (0xe0, 0xaf, 0x68),
                (0xf7, 0x76, 0x8e),
                (0x7a, 0xa2, 0xf7),
                (0xbb, 0x9a, 0xf7),
                (0x56, 0x5f, 0x89),
            ],
            Self::Nord => [
                (0x88, 0xc0, 0xd0),
                (0xa3, 0xbe, 0x8c),
                (0xeb, 0xcb, 0x8b),
                (0xbf, 0x61, 0x6a),
                (0x81, 0xa1, 0xc1),
                (0xb4, 0x8e, 0xad),
                (0x4c, 0x56, 0x6a),
            ],
        })
    }

    pub fn needs_truecolor(self) -> bool {
        self.palette().is_some()
    }

    /// The UI asks for named colors; the terminal's own palette answers unless a theme does.
    pub fn color(self, color: Color) -> Color {
        let Some(palette) = self.palette() else {
            return color;
        };
        let index = match color {
            Color::Cyan => 0,
            Color::Green => 1,
            Color::Yellow => 2,
            Color::Red => 3,
            Color::Blue => 4,
            Color::Magenta => 5,
            Color::DarkGray => 6,
            other => return other,
        };
        let (red, green, blue) = palette[index];
        Color::Rgb(red, green, blue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_colors_pass_through_and_palettes_replace_every_named_color() {
        let named = [
            Color::Cyan,
            Color::Green,
            Color::Yellow,
            Color::Red,
            Color::Blue,
            Color::Magenta,
            Color::DarkGray,
        ];
        for color in named {
            assert_eq!(Theme::Terminal.color(color), color);
            for theme in Theme::ALL.iter().filter(|theme| theme.needs_truecolor()) {
                assert!(matches!(theme.color(color), Color::Rgb(..)), "{theme:?}");
            }
        }
        assert_eq!(Theme::Nord.color(Color::Reset), Color::Reset);
    }
}
