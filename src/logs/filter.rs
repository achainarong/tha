//! Explicit retained-log filtering. Missing timestamps/levels are never guessed.
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Trace,
    Debug,
    Info,
    #[serde(alias = "warning")]
    Warn,
    Error,
    #[serde(alias = "critical")]
    Fatal,
}
impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Fatal => "fatal",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        serde_json::from_value(Value::String(
            value.trim_matches(['[', ']', ':']).to_ascii_lowercase(),
        ))
        .ok()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub json: Option<Arc<Value>>,
    pub level: Option<Level>,
    pub timestamp: Option<OffsetDateTime>,
}
impl Metadata {
    pub fn parse(text: &str, error: bool) -> Self {
        let mut payload = text;
        let mut timestamp = None;
        if let Some((prefix, remaining)) = text.split_once(char::is_whitespace)
            && let Ok(value) = OffsetDateTime::parse(prefix, &Rfc3339)
        {
            timestamp = Some(value);
            payload = remaining.trim_start();
        }
        let json: Option<Value> = serde_json::from_str(payload).ok();
        let mut level;
        if let Some(json) = &json {
            level = json
                .get("level")
                .or_else(|| json.get("severity"))
                .or_else(|| json.get("log.level"))
                .or_else(|| json.pointer("/log/level"))
                .and_then(Value::as_str)
                .and_then(Level::parse);
            if timestamp.is_none() {
                timestamp = json
                    .get("timestamp")
                    .or_else(|| json.get("time"))
                    .or_else(|| json.get("ts"))
                    .and_then(Value::as_str)
                    .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok());
            }
        } else {
            level = payload.split_whitespace().next().and_then(Level::parse);
        }
        if level.is_none() && error {
            level = Some(Level::Error);
        }
        Self {
            json: json.map(Arc::new),
            level,
            timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::{LogBuffer, view::LogView};

    #[test]
    fn structured_filters_are_typed_case_insensitive_for_sources_and_explicit_about_time() {
        let mut logs = LogBuffer::new(10);
        logs.push(
            "Api",
            r#"2026-10-29T10:00:00Z {"level":"ERROR","http":{"status":500},"message":"bad"}"#,
            false,
        );
        logs.push("Api", r#"{"severity":"warn","timestamp":"2026-10-29T12:00:00+02:00","http":{"status":"500"}}"#, false);
        logs.push("worker", "INFO there is no error here", false);
        logs.push("worker", "message without a timestamp", false);
        let filter = Filter::parse(r#"{"source":"api","min_level":"warn","fields":{"/http/status":500},"since":"2026-10-29T10:00:00Z","until":"2026-10-29T10:00:00Z"}"#).unwrap();
        assert!(filter.matches(&logs.entries[0]));
        assert!(!filter.matches(&logs.entries[1]));
        let at =
            Filter::parse(r#"{"since":"2026-10-29T10:00:00Z","until":"2026-10-29T10:00:00Z"}"#)
                .unwrap();
        assert!(at.matches(&logs.entries[0]));
        assert!(at.matches(&logs.entries[1]));
        assert!(!at.matches(&logs.entries[3]));
        assert!(
            !Filter::parse(r#"{"level":"error"}"#)
                .unwrap()
                .matches(&logs.entries[2])
        );
        assert!(!Filter::parse("{}").unwrap().active());
    }

    #[test]
    fn invalid_filters_fail_instead_of_silently_guessing() {
        for input in [
            r#"{"level":"banana"}"#,
            r#"{"unknown":true}"#,
            r#"{"fields":{"http.status":500}}"#,
            r#"{"fields":{"/bad~x":1}}"#,
            r#"{"since":"yesterday"}"#,
            r#"{"since":"2026-10-30T00:00:00Z","until":"2026-10-29T00:00:00Z"}"#,
        ] {
            assert!(Filter::parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn filtering_preserves_original_entries_and_search_scroll_and_wrap_use_only_visible_rows() {
        let mut logs = LogBuffer::new(10);
        for text in [
            "INFO before needle",
            "ERROR visible needle payload",
            "INFO after needle",
            "ERROR second needle payload",
        ] {
            logs.push("api", text, false);
        }
        let mut view = LogView::new(true);
        view.filter = Filter::parse(r#"{"level":"error"}"#).unwrap();
        view.sync(&logs, 8, 2);
        assert_eq!(logs.entries.len(), 4);
        assert!(
            view.visible(&logs)
                .iter()
                .all(|row| row.sequence == 1 || row.sequence == 3)
        );
        view.set_query("needle".into(), &mut logs);
        assert_eq!(view.match_count, 2);
        let anchor = view.active.clone();
        view.toggle_wrap(&logs);
        assert_eq!(view.active, anchor);
        view.first(&mut logs);
        assert_eq!(view.visible(&logs)[0].sequence, 1);
        view.filter = Filter::default();
        view.sync(&logs, 80, 10);
        assert_eq!(view.match_count, 4);
        view.first(&mut logs);
        assert_eq!(view.visible(&logs)[0].sequence, 0);
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Spec {
    source: Option<String>,
    level: Option<Level>,
    min_level: Option<Level>,
    fields: BTreeMap<String, Value>,
    since: Option<String>,
    until: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Filter {
    source: Option<String>,
    level: Option<Level>,
    min_level: Option<Level>,
    fields: BTreeMap<String, Value>,
    since: Option<OffsetDateTime>,
    until: Option<OffsetDateTime>,
}
impl Filter {
    pub fn parse(text: &str) -> Result<Self> {
        let spec: Spec = serde_json::from_str(text)?;
        if spec.fields.len() > 16 {
            bail!("At most 16 JSON field predicates are allowed");
        }
        for pointer in spec.fields.keys() {
            if !pointer.starts_with('/') || pointer.len() > 512 {
                bail!("Field names must be JSON pointers such as /http/status (maximum 512 bytes)");
            }
            let mut bytes = pointer.bytes();
            while let Some(byte) = bytes.next() {
                if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
                    bail!("JSON pointer escapes must use ~0 or ~1");
                }
            }
        }
        let since = spec
            .since
            .map(|text| OffsetDateTime::parse(&text, &Rfc3339))
            .transpose()?;
        let until = spec
            .until
            .map(|text| OffsetDateTime::parse(&text, &Rfc3339))
            .transpose()?;
        if since.zip(until).is_some_and(|(since, until)| since > until) {
            bail!("since must not be later than until");
        }
        Ok(Self {
            source: spec
                .source
                .filter(|source| !source.is_empty())
                .map(|source| source.to_lowercase()),
            level: spec.level,
            min_level: spec.min_level,
            fields: spec.fields,
            since,
            until,
        })
    }
    pub fn active(&self) -> bool {
        self != &Self::default()
    }
    pub fn matches(&self, entry: &super::Entry) -> bool {
        self.source
            .as_ref()
            .is_none_or(|source| entry.source.to_lowercase().contains(source))
            && self
                .level
                .is_none_or(|level| entry.metadata.level == Some(level))
            && self
                .min_level
                .is_none_or(|minimum| entry.metadata.level.is_some_and(|level| level >= minimum))
            && self
                .since
                .is_none_or(|since| entry.metadata.timestamp.is_some_and(|at| at >= since))
            && self
                .until
                .is_none_or(|until| entry.metadata.timestamp.is_some_and(|at| at <= until))
            && self.fields.iter().all(|(pointer, value)| {
                entry
                    .metadata
                    .json
                    .as_ref()
                    .is_some_and(|json| json.pointer(pointer) == Some(value))
            })
    }
}
