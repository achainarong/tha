//! Presentation ordering only. Engine IDs and backend worker indices never move.
use std::cmp::Ordering;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextSort {
    #[default]
    ConnectedFirst,
    Name,
    Cpu,
    Memory,
}

impl ContextSort {
    pub const ALL: [Self; 4] = [Self::ConnectedFirst, Self::Name, Self::Cpu, Self::Memory];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ConnectedFirst => "Connected first",
            Self::Name => "Name",
            Self::Cpu => "CPU usage (snapshot)",
            Self::Memory => "Memory usage (snapshot)",
        }
    }
}

pub struct Context<'a> {
    pub engine: usize,
    pub name: &'a str,
    pub endpoint_key: String,
    pub connected: bool,
    /// Only complete, fresh measurements qualify for usage ranking.
    pub cpu: Option<f64>,
    pub memory: Option<u64>,
}

pub fn order(contexts: &[Context<'_>], pins: &[String], mode: ContextSort) -> Vec<usize> {
    let pin = |context: &Context<'_>| {
        pins.iter()
            .position(|name| name == context.name)
            .unwrap_or(usize::MAX)
    };
    let by_name = |a: &Context<'_>, b: &Context<'_>| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(b.name))
            .then_with(|| a.endpoint_key.cmp(&b.endpoint_key))
            .then_with(|| a.engine.cmp(&b.engine))
    };
    let mut sorted: Vec<_> = contexts.iter().collect();
    sorted.sort_by(|a, b| {
        pin(a)
            .cmp(&pin(b))
            .then_with(|| match mode {
                ContextSort::ConnectedFirst => b.connected.cmp(&a.connected),
                ContextSort::Name => Ordering::Equal,
                ContextSort::Cpu => descending_cpu(
                    a.cpu
                        .filter(|value| a.connected && value.is_finite() && *value >= 0.0),
                    b.cpu
                        .filter(|value| b.connected && value.is_finite() && *value >= 0.0),
                )
                .then_with(|| b.connected.cmp(&a.connected)),
                ContextSort::Memory => descending(
                    a.memory.filter(|_| a.connected),
                    b.memory.filter(|_| b.connected),
                )
                .then_with(|| b.connected.cmp(&a.connected)),
            })
            .then_with(|| by_name(a, b))
    });
    sorted.into_iter().map(|context| context.engine).collect()
}

fn descending<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

fn descending_cpu(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.total_cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(
        engine: usize,
        name: &str,
        connected: bool,
        cpu: Option<f64>,
        memory: Option<u64>,
    ) -> Context<'_> {
        Context {
            engine,
            name,
            connected,
            cpu,
            memory,
            endpoint_key: format!("socket-{engine}"),
        }
    }

    #[test]
    fn connected_first_is_alphabetical_and_name_mode_is_pure_name_order() {
        let contexts = [
            context(0, "OrbStack", true, None, None),
            context(1, "alpha", false, None, None),
            context(2, "Colima", true, None, None),
        ];
        assert_eq!(
            order(&contexts, &[], ContextSort::ConnectedFirst),
            [2, 0, 1]
        );
        assert_eq!(order(&contexts, &[], ContextSort::Name), [1, 2, 0]);
    }

    #[test]
    fn pins_keep_their_configured_order_even_when_unavailable() {
        let contexts = [
            context(0, "OrbStack", true, None, None),
            context(1, "Docker", false, None, None),
            context(2, "Colima", true, None, None),
        ];
        let pins = vec!["Docker".into(), "OrbStack".into(), "not-discovered".into()];
        assert_eq!(
            order(&contexts, &pins, ContextSort::ConnectedFirst),
            [1, 0, 2]
        );
    }

    #[test]
    fn metrics_are_numeric_descending_with_unknown_and_invalid_values_last() {
        let contexts = [
            context(0, "small", true, Some(9.0), Some(512 * 1024 * 1024)),
            context(1, "large", true, Some(120.0), Some(2 * 1024 * 1024 * 1024)),
            context(2, "idle", true, Some(0.0), Some(0)),
            context(3, "warming", true, None, None),
            context(4, "offline", false, Some(999.0), Some(u64::MAX)),
            context(5, "invalid", true, Some(f64::NAN), None),
        ];
        assert_eq!(order(&contexts, &[], ContextSort::Cpu), [1, 0, 2, 5, 3, 4]);
        assert_eq!(
            order(&contexts, &[], ContextSort::Memory),
            [1, 0, 2, 5, 3, 4]
        );
        let pins = vec!["small".into()];
        assert_eq!(order(&contexts, &pins, ContextSort::Cpu)[0], 0);
    }

    #[test]
    fn equal_labels_and_values_have_deterministic_tie_breakers() {
        let contexts = [
            context(3, "same", true, Some(1.0), Some(10)),
            context(1, "same", true, Some(1.0), Some(10)),
        ];
        assert_eq!(order(&contexts, &[], ContextSort::Cpu), [1, 3]);
    }
}
