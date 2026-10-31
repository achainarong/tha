//! Keyboard layout presets. Each follows how that layout is normally used instead of copying
//! US key positions, so a preset only adds keys that are awkward to reach there.
use crate::shortcuts::ActionId;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardPreset {
    #[default]
    Us,
    German,
}

impl KeyboardPreset {
    pub const ALL: &[Self] = &[Self::Us, Self::German];

    pub fn label(self) -> &'static str {
        match self {
            Self::Us => "US",
            Self::German => "German (QWERTZ)",
        }
    }

    /// Navigation keys added on top of the catalog defaults.
    pub fn keys(self) -> &'static [(char, ActionId)] {
        match self {
            Self::Us => &[],
            // `[` and `]` need AltGr on QWERTZ.
            Self::German => &[('-', ActionId::PreviousRange), ('+', ActionId::NextRange)],
        }
    }

    /// `Ctrl-]` needs AltGr on QWERTZ and is unreliable there.
    fn exec_escape(self) -> Option<KeyCode> {
        match self {
            Self::Us => None,
            Self::German => Some(KeyCode::F(12)),
        }
    }

    pub fn leaves_exec(self, key: KeyEvent) -> bool {
        // Legacy terminals encode Ctrl-] as 0x1d, which Crossterm reports as Ctrl-5.
        (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5')))
            || self.exec_escape() == Some(key.code)
    }

    pub fn exec_escape_hint(self) -> &'static str {
        match self {
            Self::Us => "Ctrl-]",
            Self::German => "F12",
        }
    }

    /// What the preset adds, for the settings pane and the cheatsheet.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Us => "Standard keys",
            Self::German => "Adds - / + for the metrics range and F12 to leave a shell",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_german_leaves_the_shell_with_f12_and_every_preset_keeps_ctrl_bracket() {
        let f12 = KeyEvent::from(KeyCode::F(12));
        let bracket = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert!(!KeyboardPreset::Us.leaves_exec(f12));
        assert!(KeyboardPreset::German.leaves_exec(f12));
        for preset in KeyboardPreset::ALL {
            assert!(preset.leaves_exec(bracket));
            assert!(!preset.leaves_exec(KeyEvent::from(KeyCode::Char(']'))));
        }
    }
}
