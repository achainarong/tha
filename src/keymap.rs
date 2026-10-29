//! Navigation-only aliases, parsed by crokey rather than a custom key grammar.
use anyhow::{Context, Result, bail};
use crokey::KeyCombination;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Keymap(Vec<(KeyCombination, KeyEvent)>);
impl Keymap {
    pub fn parse(bindings: &BTreeMap<String, String>) -> Result<Self> {
        let mut mapped = Vec::new();
        for (key, action) in bindings {
            let combination = key
                .parse::<KeyCombination>()
                .with_context(|| format!("Invalid navigation key {key:?}"))?
                .normalized();
            if !combination.is_ansi_compatible() {
                bail!("Navigation binding {key:?} must be a single key, not a simultaneous chord");
            }
            let event: KeyEvent = combination.into();
            if matches!(
                event.code,
                KeyCode::Esc
                    | KeyCode::Enter
                    | KeyCode::Tab
                    | KeyCode::BackTab
                    | KeyCode::Char(':' | '/')
            ) || (event.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(event.code, KeyCode::Char('c' | ']' | '5')))
            {
                bail!("Navigation key {key:?} is reserved for input, cancel, or safety routing");
            }
            if mapped.iter().any(|(existing, _)| existing == &combination) {
                bail!("Duplicate normalized navigation key {key:?}");
            }
            let code = match action.as_str() {
                "down" => KeyCode::Down,
                "up" => KeyCode::Up,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "page-down" => KeyCode::PageDown,
                "page-up" => KeyCode::PageUp,
                "first" => KeyCode::Home,
                "last" => KeyCode::End,
                "overview" => KeyCode::Char('1'),
                "logs" => KeyCode::Char('2'),
                "networks" => KeyCode::Char('3'),
                "exec" => KeyCode::Char('4'),
                "layers" => KeyCode::Char('5'),
                "open-exec" => KeyCode::Char('e'),
                "actions" => KeyCode::Char('a'),
                "performance" => KeyCode::Char('p'),
                "sort-contexts" => KeyCode::Char('o'),
                "wrap" => KeyCode::Char('w'),
                "follow" => KeyCode::Char('f'),
                "help" => KeyCode::F(1),
                "filter-logs" => KeyCode::F(3),
                "exec-sessions" => KeyCode::F(6),
                _ => bail!("Unknown navigation action {action:?} for {key:?}"),
            };
            mapped.push((combination, KeyEvent::new(code, KeyModifiers::NONE)));
        }
        Ok(Self(mapped))
    }
    pub fn translate(&self, event: KeyEvent) -> KeyEvent {
        self.0
            .iter()
            .find(|(key, _)| *key == KeyCombination::from(event))
            .map(|(_, mapped)| *mapped)
            .unwrap_or(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_normalizes_and_rejects_ambiguous_or_reserved_bindings() {
        let bindings = BTreeMap::from([("alt-j".into(), "down".into())]);
        let map = Keymap::parse(&bindings).unwrap();
        assert_eq!(
            map.translate(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT))
                .code,
            KeyCode::Down
        );
        for key in ["ctrl-c", "ctrl-]", "enter", "esc", ":", "/"] {
            assert!(
                Keymap::parse(&BTreeMap::from([(key.into(), "down".into())])).is_err(),
                "{key}"
            );
        }
        assert!(
            Keymap::parse(&BTreeMap::from([(
                "x".into(),
                "delete-without-confirmation".into()
            )]))
            .is_err()
        );
    }
}
