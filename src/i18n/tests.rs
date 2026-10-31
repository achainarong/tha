use super::*;
use std::{collections::HashSet, fs, path::Path};

/// Text that reaches `dynamic` from stored values, so no `t(...)` call names it.
const STORED_VALUES: &[&str] = &[
    "German (QWERTZ)",
    "Standard keys",
    "Adds - / + for the metrics range and F12 to leave a shell",
    "Terminal colors",
    "Ctrl-]",
    "exited",
    "failed exit",
    "stopping",
    "removing",
    "completed",
    "healthy",
    "alias",
    "connecting",
    "connected",
    "unavailable",
    "out of memory",
    "health",
    "signalled",
    "started",
    "stopped",
    "restarted",
    "created",
    "removed",
    "errors",
    "failed",
    "OOM killed",
    "paused",
    "restarting",
    "running",
    "some stopped",
    "starting",
    "stopped/empty",
    "unhealthy",
    "unknown",
    "warning",
    "workload issues",
    "stopped-only",
    "unreferenced",
    "unused",
];

fn rust_files(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// Every translatable key: `t` and `tf!` calls in the source, plus the display tables.
fn source_keys() -> HashSet<String> {
    let call = regex::Regex::new(r#"(?:\bt|\btf!)\(\s*"((?:[^"\\]|\\.)*)""#).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let mut keys = HashSet::new();
    for file in files {
        if file.starts_with(root.join("i18n")) {
            continue;
        }
        let text = fs::read_to_string(&file).unwrap();
        let code = match text.find("#[cfg(test)]\nmod tests") {
            Some(end) if !file.ends_with("model.rs") => &text[..end],
            _ => &text[..],
        };
        for found in call.captures_iter(code) {
            keys.insert(found[1].replace("\\n", "\n").replace("\\\"", "\""));
        }
    }
    let tables = [
        crate::ui::inventory::IMAGE_FILTERS,
        crate::ui::inventory::VOLUME_FILTERS,
        crate::ui::inventory::SORTS,
        crate::ui::HELP_LINES,
        &crate::history::RANGE_NAMES,
        STORED_VALUES,
    ];
    keys.extend(tables.into_iter().flatten().map(|key| key.to_string()));
    keys.extend(crate::ui::COMMANDS.iter().map(|(_, text)| text.to_string()));
    keys.extend(
        crate::shortcuts::ACTIONS
            .iter()
            .map(|info| info.label.to_string()),
    );
    keys
}

fn placeholders(text: &str) -> HashSet<String> {
    regex::Regex::new(r"\{[A-Za-z0-9_]+\}")
        .unwrap()
        .find_iter(text)
        .map(|found| found.as_str().to_owned())
        .collect()
}

#[test]
fn every_string_has_a_german_translation_and_none_is_stale() {
    let keys = source_keys();
    let german: HashSet<String> = de::ENTRIES.iter().map(|(key, _)| key.to_string()).collect();
    let mut missing: Vec<_> = keys.difference(&german).collect();
    missing.sort();
    assert!(missing.is_empty(), "No German text for: {missing:#?}");
    let mut stale: Vec<_> = german.difference(&keys).collect();
    stale.sort();
    assert!(
        stale.is_empty(),
        "German text for removed strings: {stale:#?}"
    );
}

#[test]
fn translations_keep_placeholders_and_line_structure() {
    let mut seen = HashSet::new();
    for (key, text) in de::ENTRIES {
        assert!(seen.insert(*key), "Duplicate key: {key}");
        assert_eq!(placeholders(key), placeholders(text), "{key}");
        assert_eq!(
            key.matches('\n').count(),
            text.matches('\n').count(),
            "{key}"
        );
        assert_eq!(key.starts_with(' '), text.starts_with(' '), "{key}");
        assert_eq!(key.ends_with(' '), text.ends_with(' '), "{key}");
    }
}

/// Conventions taken from translated container tools (Cockpit, Rancher Desktop).
#[test]
fn german_follows_established_terminology() {
    let banned = [
        (
            r"\b[Dd]u\b|\b[Dd]ein(e|er|es|en|em)?\b",
            "address neutrally, never with du",
        ),
        (r"Ctrl", "German keyboards and tools say Strg"),
        (
            r"^Containers$|Containers-|Ansicht Containers",
            "the German plural is Container",
        ),
        (r"\blaufen\]|\d laufen\b| aktiv\]", "counts say laufend"),
        (
            r"\bgesund\b|\bungesund\b",
            "health stays healthy/unhealthy, as the engine prints it",
        ),
    ];
    for (pattern, reason) in banned {
        let pattern = regex::Regex::new(pattern).unwrap();
        for (key, text) in de::ENTRIES {
            assert!(!pattern.is_match(text), "{reason}: {key} => {text}");
        }
    }
}

#[test]
fn german_cheatsheet_lines_fit_the_help_dialog_without_wrapping() {
    // The dialog is 80 cells wide, less its border and padding.
    let width = 74;
    let german: HashMap<_, _> = de::ENTRIES.iter().copied().collect();
    for line in crate::ui::HELP_LINES {
        let text = german[line].trim_end_matches('\n');
        assert!(
            unicode_width::UnicodeWidthStr::width(text) <= width,
            "Too wide: {text}"
        );
    }
}

#[test]
fn german_replaces_text_and_fills_named_placeholders_in_any_order() {
    isolate_thread();
    set_language(Language::English);
    assert_eq!(t("Delete"), "Delete");
    assert_eq!(dynamic("not in any table"), "not in any table");
    set_language(Language::German);
    assert_eq!(t("Delete"), "Löschen");
    assert_eq!(dynamic("running"), "läuft");
    assert_eq!(dynamic("not in any table"), "not in any table");
    assert_eq!(
        crate::tf!("Failed: {error}", error = "boom"),
        "Fehlgeschlagen: boom"
    );
    set_language(Language::English);
}
