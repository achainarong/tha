//! Live settings behind the settings pane. Choices are saved in the preferences state file,
//! never config.toml, so comments in the hand-edited file survive.
use crate::{app::App, i18n::Language, keyboard::KeyboardPreset, theme::Theme};
use serde::{Deserialize, Serialize};

/// `None` means "never changed in the pane", so config.toml still decides.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SavedSettings {
    pub theme: Option<Theme>,
    pub keyboard: Option<KeyboardPreset>,
    pub mouse: Option<bool>,
    pub language: Option<Language>,
    pub remote_connect: Option<crate::remote::ConnectMode>,
    pub editor: Option<crate::editor::Editor>,
}

/// What the settings fall back to: config.toml, or the built-in defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BaseSettings {
    pub theme: Theme,
    pub keyboard: KeyboardPreset,
    pub mouse: bool,
    pub log_wrap: bool,
    pub language: Language,
    pub remote_connect: crate::remote::ConnectMode,
    pub editor: crate::editor::Editor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingRow {
    Theme,
    Language,
    Keyboard,
    Editor,
    Mouse,
    WrapLogs,
    RemoteConnect,
    Servers,
    Reset,
}

impl SettingRow {
    pub const ALL: &[Self] = &[
        Self::Theme,
        Self::Language,
        Self::Keyboard,
        Self::Editor,
        Self::Mouse,
        Self::WrapLogs,
        Self::RemoteConnect,
        Self::Servers,
        Self::Reset,
    ];
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsState {
    pub selected: usize,
    pub picker: Option<OptionPicker>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionList {
    Theme,
    Language,
}

impl OptionList {
    pub fn labels(self) -> Vec<&'static str> {
        match self {
            Self::Theme => Theme::ALL.iter().map(|theme| theme.label()).collect(),
            Self::Language => Language::ALL.iter().map(|item| item.label()).collect(),
        }
    }
}

/// Browsing previews each option; nothing is saved until the choice is confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionPicker {
    pub list: OptionList,
    pub selected: usize,
    original: usize,
}

fn cycled<T: Copy + PartialEq>(all: &[T], current: T, forward: bool) -> T {
    let index = all.iter().position(|item| *item == current).unwrap_or(0);
    all[if forward {
        (index + 1) % all.len()
    } else {
        (index + all.len() - 1) % all.len()
    }]
}

impl App {
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Tree titles are built from translated text, so they are rebuilt with the language.
    pub fn set_language(&mut self, language: Language) {
        self.language = language;
        crate::i18n::set_language(language);
        self.refresh_tree();
    }

    pub fn set_keyboard(&mut self, preset: KeyboardPreset) {
        self.keyboard = preset;
        // custom_bindings were validated when the configuration loaded.
        self.keymap = crate::keymap::Keymap::parse(&self.custom_bindings)
            .unwrap_or_default()
            .with_preset(preset);
    }

    pub fn apply_base_settings(&mut self, base: BaseSettings) {
        self.base_settings = base;
        self.set_language(base.language);
        self.mouse_enabled = base.mouse;
        self.log_view.wrap = base.log_wrap;
        self.set_theme(base.theme);
        self.set_keyboard(base.keyboard);
        self.remote_connect = base.remote_connect;
        self.editor = base.editor;
    }

    pub fn apply_saved_settings(&mut self, saved: SavedSettings) {
        self.saved_settings = saved;
        let base = self.base_settings;
        self.set_language(saved.language.unwrap_or(base.language));
        self.mouse_enabled = saved.mouse.unwrap_or(base.mouse);
        self.set_theme(saved.theme.unwrap_or(base.theme));
        self.set_keyboard(saved.keyboard.unwrap_or(base.keyboard));
        self.remote_connect = saved.remote_connect.unwrap_or(base.remote_connect);
        self.editor = saved.editor.unwrap_or(base.editor);
    }

    pub fn move_setting(&mut self, amount: isize) {
        if let Some(settings) = &mut self.settings {
            settings.selected = settings
                .selected
                .saturating_add_signed(amount)
                .min(SettingRow::ALL.len() - 1);
        }
    }

    /// Reset only runs when `confirm` is set, so moving left or right cannot trigger it.
    pub fn change_setting(&mut self, forward: bool, confirm: bool) {
        let Some(settings) = self.settings else {
            return;
        };
        match SettingRow::ALL[settings.selected] {
            SettingRow::Theme => {
                self.open_option_picker(OptionList::Theme);
                return;
            }
            SettingRow::Language => {
                self.open_option_picker(OptionList::Language);
                return;
            }
            SettingRow::Keyboard => {
                let preset = cycled(KeyboardPreset::ALL, self.keyboard, forward);
                self.saved_settings.keyboard = Some(preset);
                self.set_keyboard(preset);
            }
            SettingRow::Editor => {
                let editor = cycled(crate::editor::Editor::ALL, self.editor, forward);
                self.editor = editor;
                self.saved_settings.editor = Some(editor);
            }
            SettingRow::Mouse => {
                self.mouse_enabled = !self.mouse_enabled;
                self.saved_settings.mouse = Some(self.mouse_enabled);
            }
            SettingRow::WrapLogs => self.log_view.toggle_wrap(&self.logs),
            // Only the next start reads this; it never opens or closes a connection now.
            SettingRow::RemoteConnect => {
                let mode = cycled(
                    &crate::remote::ConnectMode::ALL,
                    self.remote_connect,
                    forward,
                );
                self.remote_connect = mode;
                self.saved_settings.remote_connect = Some(mode);
            }
            SettingRow::Servers if confirm => {
                self.settings = None;
                self.open_servers();
                return;
            }
            SettingRow::Servers => return,
            SettingRow::Reset if confirm => {
                self.apply_saved_settings(SavedSettings::default());
                if self.log_view.wrap != self.base_settings.log_wrap {
                    self.log_view.toggle_wrap(&self.logs);
                }
            }
            SettingRow::Reset => return,
        }
        self.save_settings();
    }

    fn apply_option(&mut self, list: OptionList, index: usize) {
        match list {
            OptionList::Theme => self.set_theme(Theme::ALL[index]),
            OptionList::Language => self.set_language(Language::ALL[index]),
        }
    }

    fn open_option_picker(&mut self, list: OptionList) {
        let current = match list {
            OptionList::Theme => Theme::ALL.iter().position(|item| *item == self.theme),
            OptionList::Language => Language::ALL.iter().position(|item| *item == self.language),
        }
        .unwrap_or(0);
        if let Some(settings) = &mut self.settings {
            settings.picker = Some(OptionPicker {
                list,
                selected: current,
                original: current,
            });
        }
    }

    /// Previews the option at `index` without saving it.
    pub fn preview_option(&mut self, index: usize) {
        let Some(picker) = self
            .settings
            .as_mut()
            .and_then(|settings| settings.picker.as_mut())
        else {
            return;
        };
        let index = index.min(picker.list.labels().len() - 1);
        picker.selected = index;
        let list = picker.list;
        self.apply_option(list, index);
    }

    pub fn move_option(&mut self, amount: isize) {
        if let Some(picker) = self.settings.and_then(|settings| settings.picker) {
            self.preview_option(picker.selected.saturating_add_signed(amount));
        }
    }

    pub fn confirm_option(&mut self) {
        let Some(picker) = self.settings.as_mut().and_then(|state| state.picker.take()) else {
            return;
        };
        match picker.list {
            OptionList::Theme => self.saved_settings.theme = Some(self.theme),
            OptionList::Language => self.saved_settings.language = Some(self.language),
        }
        self.save_settings();
    }

    pub fn cancel_option(&mut self) {
        if let Some(picker) = self.settings.as_mut().and_then(|state| state.picker.take()) {
            self.apply_option(picker.list, picker.original);
        }
    }

    fn save_settings(&mut self) {
        self.settings_save_error = self.preferences_path.as_ref().and_then(|path| {
            crate::preferences::save(path, &self.preferences())
                .err()
                .map(|error| format!("{error:#}"))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Keymap;
    use crossterm::event::{KeyCode, KeyEvent};

    fn app() -> App {
        crate::i18n::isolate_thread();
        let mut app = App::new(crate::demo::connections(), 100, false, true);
        app.settings = Some(SettingsState::default());
        app
    }

    fn select(app: &mut App, row: SettingRow) {
        app.settings = Some(SettingsState {
            selected: SettingRow::ALL.iter().position(|r| *r == row).unwrap(),
            picker: None,
        });
    }

    #[test]
    fn saved_settings_win_over_config_and_none_falls_back() {
        let mut app = app();
        app.apply_base_settings(BaseSettings {
            theme: Theme::Nord,
            keyboard: KeyboardPreset::German,
            mouse: false,
            log_wrap: true,
            language: Language::English,
            remote_connect: crate::remote::ConnectMode::Startup,
            editor: Default::default(),
        });
        assert_eq!(app.theme, Theme::Nord);
        assert_eq!(app.keyboard, KeyboardPreset::German);
        assert!(!app.mouse_enabled);

        app.apply_saved_settings(SavedSettings {
            theme: Some(Theme::GruvboxDark),
            mouse: Some(true),
            ..Default::default()
        });
        assert_eq!(app.theme, Theme::GruvboxDark);
        assert_eq!(app.keyboard, KeyboardPreset::German, "unset keeps config");
        assert!(app.mouse_enabled);
    }

    #[test]
    fn rows_cycle_save_and_reset_only_on_confirm() {
        let mut app = app();
        let base = BaseSettings {
            mouse: true,
            log_wrap: true,
            ..Default::default()
        };
        app.apply_base_settings(base);

        select(&mut app, SettingRow::Theme);
        app.change_setting(true, true);
        app.preview_option(1);
        app.confirm_option();
        assert_eq!(app.saved_settings.theme, Some(Theme::CatppuccinMocha));

        select(&mut app, SettingRow::Keyboard);
        app.change_setting(true, false);
        assert_eq!(app.keyboard, KeyboardPreset::German);

        select(&mut app, SettingRow::Mouse);
        app.change_setting(true, false);
        assert!(!app.mouse_enabled);

        select(&mut app, SettingRow::WrapLogs);
        app.change_setting(true, false);
        assert!(!app.log_view.wrap);

        select(&mut app, SettingRow::Reset);
        app.change_setting(true, false);
        assert_eq!(app.theme, Theme::CatppuccinMocha, "left/right cannot reset");
        app.change_setting(true, true);
        assert_eq!(app.saved_settings, SavedSettings::default());
        assert_eq!(app.theme, Theme::Terminal);
        assert_eq!(app.keyboard, KeyboardPreset::Us);
        assert!(app.mouse_enabled);
        assert!(app.log_view.wrap);
    }

    #[test]
    fn option_lists_preview_without_saving_until_confirmed() {
        let directory = std::env::temp_dir().join(format!("tha-themes-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("preferences.toml");
        let mut app = app();
        app.preferences_path = Some(path.clone());
        select(&mut app, SettingRow::Theme);

        // Left and right open the list too; neither changes the theme by itself.
        app.change_setting(false, false);
        let picker = app.settings.unwrap().picker.unwrap();
        assert_eq!(picker.list, OptionList::Theme);
        assert_eq!(Theme::ALL[picker.selected], Theme::Terminal);
        assert_eq!(app.theme, Theme::Terminal);

        app.move_option(1);
        assert_eq!(app.theme, Theme::CatppuccinMocha);
        assert_eq!(app.saved_settings.theme, None, "a preview is not a choice");
        assert!(!path.exists(), "a preview writes nothing");

        app.cancel_option();
        assert_eq!(app.theme, Theme::Terminal, "cancel restores the old theme");
        assert_eq!(app.settings.unwrap().picker, None);
        assert_eq!(app.saved_settings.theme, None);

        app.change_setting(true, true);
        app.move_option(isize::MAX);
        assert_eq!(app.theme, *Theme::ALL.last().unwrap());
        app.move_option(isize::MIN);
        assert_eq!(app.theme, Theme::Terminal, "the list stops at each end");
        app.preview_option(2);
        app.confirm_option();
        assert_eq!(app.saved_settings.theme, Some(Theme::GruvboxDark));
        assert_eq!(app.settings.unwrap().picker, None);
        assert_eq!(
            crate::preferences::load(&path)
                .unwrap()
                .unwrap()
                .settings
                .theme,
            Some(Theme::GruvboxDark)
        );

        select(&mut app, SettingRow::Language);
        app.change_setting(true, true);
        let picker = app.settings.unwrap().picker.unwrap();
        assert_eq!(picker.list, OptionList::Language);
        assert_eq!(picker.list.labels(), ["English", "Deutsch"]);
        app.move_option(1);
        assert_eq!(app.language, Language::German, "the preview applies live");
        assert_eq!(app.saved_settings.language, None);
        app.cancel_option();
        assert_eq!(app.language, Language::English);
        app.change_setting(true, true);
        app.move_option(1);
        app.confirm_option();
        assert_eq!(app.saved_settings.language, Some(Language::German));
        assert_eq!(
            app.keyboard,
            KeyboardPreset::Us,
            "language leaves keys alone"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_saved_monochrome_theme_from_an_older_build_still_loads() {
        let saved: SavedSettings = toml::from_str("theme = 'monochrome'").unwrap();
        assert_eq!(saved.theme, Some(Theme::Terminal));
    }

    #[test]
    fn monochrome_outlasts_every_theme() {
        let mut app = app();
        app.monochrome = true;
        app.set_theme(Theme::Nord);
        assert!(app.monochrome);
        assert_eq!(app.theme, Theme::Nord);
    }

    #[test]
    fn german_keys_arrive_live_user_bindings_win_and_us_returns_cleanly() {
        let minus = KeyEvent::from(KeyCode::Char('-'));
        let mut app = app();
        app.custom_bindings = [("+".to_string(), "help".to_string())].into();
        app.set_keyboard(KeyboardPreset::Us);
        assert_eq!(app.keymap.action(minus), None);

        app.set_keyboard(KeyboardPreset::German);
        assert_eq!(
            app.keymap.action(minus),
            Some(crate::shortcuts::ActionId::PreviousRange)
        );
        assert_eq!(
            app.keymap.action(KeyEvent::from(KeyCode::Char('+'))),
            Some(crate::shortcuts::ActionId::Help),
            "a user binding beats the preset"
        );

        app.set_keyboard(KeyboardPreset::Us);
        assert_eq!(app.keymap.action(minus), None);
        assert!(Keymap::parse(&app.custom_bindings).is_ok());
    }

    #[test]
    fn saved_choices_survive_a_preferences_round_trip() {
        let directory = std::env::temp_dir().join(format!("tha-settings-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("preferences.toml");
        let mut app = app();
        app.preferences_path = Some(path.clone());
        select(&mut app, SettingRow::Keyboard);
        app.change_setting(true, false);
        assert_eq!(app.settings_save_error, None);

        let loaded = crate::preferences::load(&path).unwrap().unwrap();
        assert_eq!(loaded.settings.keyboard, Some(KeyboardPreset::German));
        assert_eq!(loaded.settings.theme, None, "untouched rows stay unset");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
