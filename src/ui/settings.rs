//! The settings overlay. Rows change live; the logic lives in `crate::settings`.
use super::*;
use crate::settings::{OptionList, OptionPicker, SettingRow};

fn truecolor_reported() -> bool {
    std::env::var("COLORTERM").is_ok_and(|value| matches!(value.as_str(), "truecolor" | "24bit"))
}

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(state) = app.settings else {
        return;
    };
    let on_off = |value: bool| if value { t("On") } else { t("Off") };
    let area = centered(area, 72, 18);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(t(" Settings "))
        .title_bottom(t(" j/k Select · h/l or Enter Change · Esc Close "));
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (index, row) in SettingRow::ALL.iter().enumerate() {
        let (name, value): (&str, Option<&str>) = match row {
            SettingRow::Theme => (t("Theme"), Some(t(app.theme.label()))),
            SettingRow::Language => (t("Language"), Some(app.language.label())),
            SettingRow::Keyboard => (t("Keyboard layout"), Some(t(app.keyboard.label()))),
            SettingRow::Editor => (t("Editor"), Some(app.editor.label())),
            SettingRow::Mouse => (t("Mouse"), Some(on_off(app.mouse_enabled))),
            SettingRow::WrapLogs => (t("Wrap logs"), Some(on_off(app.log_view.wrap))),
            SettingRow::RemoteConnect => {
                (t("Connect to servers"), Some(app.remote_connect.label()))
            }
            SettingRow::Servers => (t("Servers"), None),
            SettingRow::Reset => (t("Reset to defaults"), None),
        };
        let text = match (row, value) {
            (SettingRow::Theme | SettingRow::Language, Some(value)) => {
                format!("{:<22}{value} ▾", name)
            }
            (_, Some(value)) => format!("{:<22}◂ {value} ▸", name),
            (_, None) => name.to_owned(),
        };
        lines.push(if index == state.selected {
            Line::styled(text, selected())
        } else {
            Line::from(text)
        });
        if inner.height as usize > index && state.picker.is_none() {
            app.mouse_regions.push((
                Rect::new(inner.x, inner.y + index as u16, inner.width, 1),
                MouseTarget::SettingsRow(index),
            ));
        }
    }
    lines.push(Line::from(""));

    match SettingRow::ALL[state.selected] {
        SettingRow::Theme => {
            lines.push(swatches(app));
            lines.extend(theme_note(app));
        }
        SettingRow::Language => lines.push(Line::styled(
            t("Separate from the keyboard layout, so any language works with any layout."),
            muted(),
        )),
        SettingRow::Keyboard => lines.push(Line::styled(t(app.keyboard.summary()), muted())),
        SettingRow::Editor => lines.push(Line::styled(
            t("Opens Compose files with :compose-edit. It must be installed on this computer."),
            muted(),
        )),
        SettingRow::Mouse => lines.push(Line::styled(
            t("Hold Shift to select terminal text while the mouse is on."),
            muted(),
        )),
        SettingRow::WrapLogs => lines.push(Line::styled(
            t("The same setting as w in the log pane."),
            muted(),
        )),
        SettingRow::RemoteConnect => lines.push(Line::styled(
            t("Used when tha opens. The background service never connects to servers."),
            muted(),
        )),
        SettingRow::Servers => lines.push(Line::styled(
            t("Enter adds, edits or removes servers."),
            muted(),
        )),
        SettingRow::Reset => lines.push(Line::styled(
            t("Enter returns these settings to config.toml or the defaults."),
            muted(),
        )),
    }
    if let Some(error) = &app.settings_save_error {
        lines.push(Line::styled(
            format!("{}: {}", t("Could not save settings"), safe_text(error)),
            accent(app, Color::Red),
        ));
    } else if app.preferences_path.is_none() {
        lines.push(Line::styled(
            t("Changes last for this session: demo mode or saving is turned off."),
            muted(),
        ));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    if let Some(picker) = state.picker {
        draw_option_picker(frame, app, area, picker);
    }
}

fn swatches(app: &App) -> Line<'static> {
    Line::from(
        [
            Color::Cyan,
            Color::Green,
            Color::Yellow,
            Color::Red,
            Color::Blue,
            Color::Magenta,
        ]
        .into_iter()
        .map(|color| Span::styled("● ", accent(app, color)))
        .collect::<Vec<_>>(),
    )
}

fn theme_note(app: &App) -> Option<Line<'static>> {
    let text = if app.monochrome {
        t("Colors are off: NO_COLOR, --monochrome or :monochrome.")
    } else if app.theme.needs_truecolor() && !truecolor_reported() {
        t("Your terminal does not report truecolor, so this theme may look wrong.")
    } else {
        return None;
    };
    Some(Line::styled(text, muted()))
}

/// Sits over the settings rows, so the app behind it shows the previewed option.
fn draw_option_picker(frame: &mut Frame, app: &mut App, area: Rect, picker: OptionPicker) {
    let labels: Vec<_> = picker.list.labels().into_iter().map(t).collect();
    let area = centered(area, 52, labels.len() as u16 + 7);
    frame.render_widget(Clear, area);
    let title = match picker.list {
        OptionList::Theme => t("Theme"),
        OptionList::Language => t("Language"),
    };
    let block = Block::bordered()
        .title(format!(" {} ", title))
        .title_bottom(t(" j/k Preview · Enter Apply · Esc Cancel "));
    let inner = padded(block.inner(area));
    frame.render_widget(block, area);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (index, option) in labels.iter().enumerate() {
        lines.push(if index == picker.selected {
            Line::styled(format!("▸ {option}"), selected())
        } else {
            Line::from(format!("  {option}"))
        });
        if (index as u16) < inner.height {
            app.mouse_regions.push((
                Rect::new(inner.x, inner.y + index as u16, inner.width, 1),
                MouseTarget::OptionRow(index),
            ));
        }
    }
    if picker.list == OptionList::Theme {
        lines.push(Line::from(""));
        lines.push(swatches(app));
        lines.extend(theme_note(app));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
