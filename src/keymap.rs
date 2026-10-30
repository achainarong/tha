//! Navigation-only aliases, parsed by crokey rather than a custom key grammar.
use crate::shortcuts::ActionId;
use anyhow::{Context, Result, bail};
use crokey::KeyCombination;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Keymap(Vec<(KeyCombination, ActionId)>);
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
            let action = ActionId::configurable(action)
                .with_context(|| format!("Unknown navigation action {action:?} for {key:?}"))?;
            mapped.push((combination, action));
        }
        Ok(Self(mapped))
    }
    pub fn action(&self, event: KeyEvent) -> Option<ActionId> {
        self.0
            .iter()
            .find(|(key, _)| *key == KeyCombination::from(event).normalized())
            .map(|(_, action)| *action)
    }

    pub fn bindings(&self) -> impl Iterator<Item = (KeyCombination, ActionId)> + '_ {
        self.0.iter().copied()
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
            map.action(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT)),
            Some(ActionId::Down)
        );
        for key in ["ctrl-c", "ctrl-]", "enter", "esc", ":", "/"] {
            assert!(
                Keymap::parse(&BTreeMap::from([(key.into(), "down".into())])).is_err(),
                "{key}"
            );
        }
        let map = Keymap::parse(&BTreeMap::from([
            ("m".into(), "metrics".into()),
            ("l".into(), "layers".into()),
        ]))
        .unwrap();
        assert_eq!(
            map.action(KeyEvent::from(KeyCode::Char('m'))),
            Some(ActionId::Metrics)
        );
        assert_eq!(
            map.action(KeyEvent::from(KeyCode::Char('l'))),
            Some(ActionId::Layers)
        );
        assert!(
            Keymap::parse(&BTreeMap::from([(
                "x".into(),
                "delete-without-confirmation".into()
            )]))
            .is_err()
        );
    }

    #[test]
    fn all_existing_names_and_views_resolve_to_typed_actions() {
        for name in [
            "down",
            "up",
            "left",
            "right",
            "page-down",
            "page-up",
            "first",
            "last",
            "overview",
            "logs",
            "networks",
            "exec",
            "metrics",
            "layers",
            "security",
            "open-exec",
            "actions",
            "performance",
            "sort-contexts",
            "wrap",
            "follow",
            "help",
            "filter-logs",
            "exec-sessions",
            "views",
        ] {
            let map = Keymap::parse(&BTreeMap::from([("alt-j".into(), name.into())])).unwrap();
            assert_eq!(
                map.action(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT)),
                ActionId::configurable(name)
            );
        }
        assert!(
            Keymap::parse(&BTreeMap::from([
                ("alt-j".into(), "down".into()),
                ("ALT-j".into(), "up".into())
            ]))
            .is_err()
        );
        for key in [
            "ctrl-5",
            "ctrl-]",
            "ctrl-c",
            "tab",
            "shift-tab",
            "esc",
            "enter",
            ":",
            "/",
        ] {
            assert!(
                Keymap::parse(&BTreeMap::from([(key.into(), "views".into())])).is_err(),
                "{key}"
            );
        }
        assert_eq!(
            Keymap::default().action(KeyEvent::from(KeyCode::Char('v'))),
            None
        );
    }
}
