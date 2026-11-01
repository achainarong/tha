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

/// Split a leading RFC 3339 timestamp, as engines add it, from the rest of a line.
fn split_timestamp(text: &str) -> (Option<OffsetDateTime>, &str) {
    if let Some((prefix, remaining)) = text.split_once(char::is_whitespace)
        && let Ok(value) = OffsetDateTime::parse(prefix, &Rfc3339)
    {
        return (Some(value), remaining.trim_start());
    }
    (None, text)
}

/// A log line without the timestamp the engine added in front of it.
pub fn message(text: &str) -> &str {
    split_timestamp(text).1
}

pub fn line_timestamp(text: &str) -> Option<OffsetDateTime> {
    split_timestamp(text).0
}

/// Indent a JSON object or array, keeping any timestamp prefix. This reformats the source
/// text rather than a parsed value, so key order and exact numbers are preserved.
pub fn pretty_json(text: &str, metadata: &Metadata) -> Option<String> {
    let json = metadata.json.as_ref()?;
    if !json.is_object() && !json.is_array() {
        return None;
    }
    let payload = split_timestamp(text).1;
    let mut output = String::with_capacity(text.len() + text.len() / 2);
    output.push_str(&text[..text.len() - payload.len()]);
    let newline = |output: &mut String, depth: usize| {
        output.push('\n');
        output.extend(std::iter::repeat_n("  ", depth));
    };
    let mut depth = 0usize;
    let mut chars = payload.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                output.push(ch);
                let mut escaped = false;
                for ch in chars.by_ref() {
                    output.push(ch);
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        break;
                    }
                }
            }
            '{' | '[' => {
                output.push(ch);
                while chars.next_if(|next| next.is_whitespace()).is_some() {}
                if let Some(close) = chars.next_if(|next| matches!(next, '}' | ']')) {
                    output.push(close);
                } else {
                    depth += 1;
                    newline(&mut output, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut output, depth);
                output.push(ch);
            }
            ',' => {
                output.push(ch);
                newline(&mut output, depth);
            }
            ':' => output.push_str(": "),
            ch if ch.is_whitespace() => {}
            ch => output.push(ch),
        }
    }
    Some(output)
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub json: Option<Arc<Value>>,
    pub level: Option<Level>,
    pub timestamp: Option<OffsetDateTime>,
}
impl Metadata {
    pub fn parse(text: &str, error: bool) -> Self {
        let (mut timestamp, payload) = split_timestamp(text);
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

    fn text_filter(text: &str) -> Filter {
        let mut filter = Filter::default();
        filter.set_text(text);
        filter
    }

    fn matching<'a>(filter: &Filter, logs: &'a LogBuffer) -> Vec<&'a str> {
        logs.entries
            .iter()
            .filter(|entry| filter.matches(entry))
            .map(|entry| entry.text.as_str())
            .collect()
    }

    #[test]
    fn every_text_term_must_occur_in_the_same_message_in_any_order() {
        let mut logs = LogBuffer::new(10);
        for text in [
            "backend1 returned 500",
            "500 returned by BACKEND1",
            "backend1 returned 200",
            "backend2 returned 500",
        ] {
            logs.push("api", text, false);
        }
        assert_eq!(
            matching(&text_filter("backend1 500"), &logs),
            ["backend1 returned 500", "500 returned by BACKEND1"]
        );
        let mut separate = LogBuffer::new(10);
        separate.push("api", "only backend1", false);
        separate.push("api", "only 500", false);
        assert!(matching(&text_filter("backend1 500"), &separate).is_empty());
    }

    #[test]
    fn a_leading_dash_excludes_a_word_and_a_backslash_makes_it_literal() {
        let keep = |query: &str, text: &str| {
            let mut logs = LogBuffer::new(10);
            logs.push("api", text, false);
            text_filter(query).matches(logs.entries.back().unwrap())
        };
        assert!(!keep("-healthcheck", "GET /healthcheck 200"));
        assert!(keep("-healthcheck", "GET /checkout 200"));
        assert!(keep("500 -healthcheck", "GET /checkout 500"));
        assert!(!keep("500 -healthcheck", "GET /healthcheck 500"));
        assert!(!keep("500 -healthcheck", "GET /checkout 200"));
        assert!(
            !keep("-HEALTHCHECK", "get /healthcheck"),
            "Case does not matter"
        );
        assert!(keep("\\-v", "tool -v printed"));
        assert!(!keep("\\-v", "tool printed"));
        assert!(keep("-", "a - b"), "A lone dash is just a dash");
        assert!(text_filter("-x").text_active());
    }

    #[test]
    fn text_terms_are_literal_and_whitespace_only_text_is_no_constraint() {
        let mut logs = LogBuffer::new(10);
        logs.push("api", "backend1 [500] done", false);
        logs.push("api", "backend1 5 done", false);
        logs.push("api", "\"quoted\" GRÜẞE", false);
        assert_eq!(
            matching(&text_filter("  backend1   [500]  "), &logs),
            ["backend1 [500] done"]
        );
        assert!(matching(&text_filter("back.nd1"), &logs).is_empty());
        assert_eq!(matching(&text_filter("\"quoted\""), &logs).len(), 1);
        assert_eq!(matching(&text_filter("grüße"), &logs).len(), 1);
        let empty = text_filter(" \t ");
        assert!(!empty.active());
        assert_eq!(matching(&empty, &logs).len(), 3);
    }

    #[test]
    fn text_matches_the_message_rather_than_source_timestamp_or_presentation() {
        let mut logs = LogBuffer::new(10);
        logs.push("backend1", "HTTP 500", false);
        assert!(matching(&text_filter("backend1 500"), &logs).is_empty());
        assert_eq!(matching(&text_filter("500"), &logs).len(), 1);

        let mut stamped = LogBuffer::new(10);
        stamped.push("api", "2026-10-29T10:00:00.500Z request done", false);
        assert!(matching(&text_filter("500"), &stamped).is_empty());
        assert!(matching(&text_filter("2026"), &stamped).is_empty());
        assert_eq!(matching(&text_filter("done"), &stamped).len(), 1);

        // Indentation added for display cannot split or create a match.
        let mut pretty = LogBuffer::new(10);
        pretty.set_pretty_json(true);
        pretty.push("api", r#"{"a":"backend1","b":500}"#, false);
        assert!(pretty.entries[0].pretty.is_some());
        assert_eq!(
            matching(&text_filter("\"a\":\"backend1\""), &pretty).len(),
            1
        );
        assert_eq!(matching(&text_filter("\"b\":500"), &pretty).len(), 1);
    }

    #[test]
    fn text_and_advanced_predicates_combine_and_replace_only_their_own_part() {
        let mut logs = LogBuffer::new(10);
        logs.push("api", "ERROR backend1 500", false);
        logs.push("api", "INFO backend1 500", false);
        logs.push("api", "ERROR backend2 500", false);
        let mut filter = Filter::parse(r#"{"min_level":"error"}"#).unwrap();
        filter.set_text("backend1 500");
        assert_eq!(matching(&filter, &logs), ["ERROR backend1 500"]);
        assert!(filter.text_active() && filter.advanced_active());

        filter.set_text("");
        assert!(!filter.text_active() && filter.advanced_active());
        assert_eq!(matching(&filter, &logs).len(), 2);

        filter.set_text("backend1");
        filter.set_advanced(Filter::parse("{}").unwrap());
        assert!(filter.text_active() && !filter.advanced_active());
        assert_eq!(matching(&filter, &logs).len(), 2);
        // JSON documents never turn into text terms, and terms are not part of the JSON grammar.
        assert!(Filter::parse(r#"{"terms":["a"]}"#).is_err());
        assert!(Filter::parse("backend1").is_err());
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
    /// Lowercased literal terms; an entry must contain all of them in its message.
    terms: Vec<String>,
    excluded: Vec<String>,
}
impl Filter {
    /// Parses the advanced JSON document. Text terms are set separately with `set_text`,
    /// because log text can itself look like JSON and must never be guessed at.
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
            terms: Vec::new(),
            excluded: Vec::new(),
        })
    }
    pub fn active(&self) -> bool {
        self != &Self::default()
    }
    /// Replaces the text terms and leaves the advanced predicates alone.
    ///
    /// `-word` hides messages containing it, and `\\-word` searches for a literal `-word`.
    pub fn set_text(&mut self, text: &str) {
        self.terms.clear();
        self.excluded.clear();
        for word in text.split_whitespace() {
            let word = word.to_lowercase();
            match word.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => self.excluded.push(rest.to_owned()),
                _ => self.terms.push(
                    word.strip_prefix("\\-")
                        .map_or_else(|| word.clone(), |rest| format!("-{rest}")),
                ),
            }
        }
    }
    /// Replaces the advanced predicates and leaves the text terms alone.
    pub fn set_advanced(&mut self, advanced: Self) {
        let terms = std::mem::take(&mut self.terms);
        let excluded = std::mem::take(&mut self.excluded);
        *self = Self {
            terms,
            excluded,
            ..advanced
        };
    }
    pub fn text_active(&self) -> bool {
        !self.terms.is_empty() || !self.excluded.is_empty()
    }
    pub fn advanced_active(&self) -> bool {
        self.source.is_some()
            || self.level.is_some()
            || self.min_level.is_some()
            || !self.fields.is_empty()
            || self.since.is_some()
            || self.until.is_some()
    }
    /// Every term must occur in this one message. The source label, the timestamp the engine
    /// adds and any pretty-printing are not part of it: `500` must not match `10:00:00.500Z`.
    fn matches_text(&self, entry: &super::Entry) -> bool {
        if !self.text_active() {
            return true;
        }
        let message = split_timestamp(&entry.text).1.to_lowercase();
        self.terms.iter().all(|term| message.contains(term))
            && !self.excluded.iter().any(|term| message.contains(term))
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
            // Last, since it is the only predicate that allocates.
            && self.matches_text(entry)
    }
}
