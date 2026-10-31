//! Translations, keyed by the English text. A test checks that every key in the source has a
//! translation, so a new string cannot ship untranslated. Keyboard shortcuts never depend on
//! the language: layout presets live in `crate::keyboard`.
//!
//! Translate only at display time. Values that code compares, stores or searches stay in
//! English, and `dynamic` translates them at the moment they are drawn.
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::HashMap,
    fmt::Display,
    sync::{
        LazyLock,
        atomic::{AtomicU8, Ordering},
    },
};

pub mod de;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "de")]
    German,
}

impl Language {
    pub const ALL: &[Self] = &[Self::English, Self::German];

    /// Each language is listed under its own name.
    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::German => "Deutsch",
        }
    }

    fn id(self) -> u8 {
        match self {
            Self::English => 0,
            Self::German => 1,
        }
    }

    fn from_id(id: u8) -> Self {
        if id == 1 { Self::German } else { Self::English }
    }
}

// Background tasks also produce text, so the language is process-wide.
static CURRENT: AtomicU8 = AtomicU8::new(0);
thread_local! {
    static ISOLATED: Cell<Option<Language>> = const { Cell::new(None) };
}

static GERMAN: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| de::ENTRIES.iter().copied().collect());

pub fn language() -> Language {
    ISOLATED
        .get()
        .unwrap_or_else(|| Language::from_id(CURRENT.load(Ordering::Relaxed)))
}

pub fn set_language(language: Language) {
    if ISOLATED.get().is_some() {
        ISOLATED.set(Some(language));
    } else {
        CURRENT.store(language.id(), Ordering::Relaxed);
    }
}

/// Keeps this thread's language to itself, so parallel tests cannot affect each other.
pub fn isolate_thread() {
    ISOLATED.set(Some(language()));
}

fn lookup(key: &str) -> Option<&'static str> {
    match language() {
        Language::English => None,
        Language::German => GERMAN.get(key).copied(),
    }
}

/// Translates fixed text.
pub fn t(key: &'static str) -> &'static str {
    lookup(key).unwrap_or(key)
}

/// Translates text only known when drawing, such as a stored status. Unknown text is unchanged.
pub fn dynamic(key: &str) -> &str {
    lookup(key).unwrap_or(key)
}

/// Fills `{name}` placeholders in the translated template. Translations may reorder them.
pub fn fill(key: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut text = dynamic(key).to_owned();
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

/// `format!` for translated text: `tf!("Row {row} of {total}", row = row, total = total)`.
#[macro_export]
macro_rules! tf {
    ($key:literal $(, $name:ident = $value:expr)* $(,)?) => {{
        // A block, so the borrowed values never live across an `.await` in the caller.
        let text = $crate::i18n::fill(
            $key,
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*],
        );
        text
    }};
}

#[cfg(test)]
mod tests;
