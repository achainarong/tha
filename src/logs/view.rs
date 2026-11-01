//! Display-line navigation with byte anchors, independent of terminal size.
//! Hard wrapping keeps every payload character, including whitespace and long tokens.
use super::{Entry, LogBuffer};
use regex::{Regex, RegexBuilder};
use std::{collections::VecDeque, ops::Range};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Source,
    Payload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub sequence: u64,
    pub field: Field,
    pub range: Range<usize>,
}

impl SearchMatch {
    fn key(&self) -> (u64, Field, usize) {
        (self.sequence, self.field, self.range.start)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    sequence: u64,
    byte: usize,
}

struct EntryLayout {
    sequence: u64,
    ranges: Vec<Range<usize>>,
}

struct SearchCount {
    sequence: u64,
    count: usize,
}

pub struct VisualRow {
    pub sequence: u64,
    pub range: Range<usize>,
    pub continuation: bool,
}

pub struct LogView {
    pub filter: super::filter::Filter,
    pub wrap: bool,
    pub horizontal: usize,
    pub height: usize,
    width: usize,
    anchor: Option<Position>,
    query: String,
    query_is_regex: bool,
    /// Search syntax for the next query; literal by default.
    pub regex_search: bool,
    regex: Option<Regex>,
    pub active: Option<SearchMatch>,
    pub match_count: usize,
    pub match_index: Option<usize>,
    layouts: VecDeque<EntryLayout>,
    /// Sum of retained layout rows, updated with append/eviction and invalidation.
    visual_rows: usize,
    /// Newest entry the filter has judged. Rejected entries have no layout to remember them,
    /// and judging them again on every idle frame would rescan the buffer.
    layout_scanned: Option<u64>,
    layout_mode: Option<(usize, bool, bool, super::filter::Filter)>,
    search_stamp: Option<(u64, u64, usize)>,
    search_filter: Option<super::filter::Filter>,
    search_counts: VecDeque<SearchCount>,
    search_last: Option<u64>,
    #[cfg(test)]
    search_scans: usize,
    #[cfg(test)]
    layout_scans: usize,
}

impl LogView {
    pub fn new(wrap: bool) -> Self {
        Self {
            filter: Default::default(),
            wrap,
            horizontal: 0,
            height: 1,
            width: 1,
            anchor: None,
            query: String::new(),
            query_is_regex: false,
            regex_search: false,
            regex: None,
            active: None,
            match_count: 0,
            match_index: None,
            layouts: VecDeque::new(),
            visual_rows: 0,
            layout_scanned: None,
            layout_mode: None,
            search_stamp: None,
            search_filter: None,
            search_counts: VecDeque::new(),
            search_last: None,
            #[cfg(test)]
            search_scans: 0,
            #[cfg(test)]
            layout_scans: 0,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn query_is_regex(&self) -> bool {
        self.query_is_regex
    }
    pub fn regex(&self) -> Option<&Regex> {
        self.regex.as_ref()
    }

    /// Retained entries passing the filter, as of the last `sync`.
    pub fn matching_entries(&self) -> usize {
        self.layouts.len()
    }

    pub fn reset_content(&mut self) {
        self.layouts.clear();
        self.visual_rows = 0;
        self.layout_scanned = None;
        self.reset_search_counts();
        self.anchor = None;
        self.active = None;
        self.match_count = 0;
        self.match_index = None;
        self.horizontal = 0;
    }

    /// Report an invalid pattern before it replaces the current search.
    pub fn check_query(&self, query: &str) -> Result<(), regex::Error> {
        self.compile(query).map(|_| ())
    }

    fn compile(&self, query: &str) -> Result<Regex, regex::Error> {
        let pattern = if self.regex_search {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        RegexBuilder::new(&pattern).case_insensitive(true).build()
    }

    pub fn set_query(&mut self, query: String, buffer: &mut LogBuffer) {
        self.regex = if query.is_empty() {
            None
        } else {
            self.compile(&query).ok()
        };
        self.query_is_regex = self.regex_search && self.regex.is_some();
        self.query = query;
        self.active = None;
        self.reset_search_counts();
        self.sync(buffer, self.width, self.height);
        if self.regex.is_some() {
            self.next_match(buffer, false);
        }
    }

    fn matches<'a>(&'a self, buffer: &'a LogBuffer) -> impl Iterator<Item = SearchMatch> + 'a {
        buffer
            .entries
            .iter()
            .filter(|entry| self.filter.matches(entry))
            .flat_map(move |entry| {
                self.regex
                    .iter()
                    .flat_map(move |regex| entry_matches(entry, regex))
            })
    }

    fn reset_search_counts(&mut self) {
        self.search_counts.clear();
        self.search_last = None;
        self.search_stamp = None;
        self.match_count = 0;
    }

    fn sync_search(&mut self, buffer: &LogBuffer, first: u64) {
        if self.search_filter.as_ref() != Some(&self.filter) {
            self.reset_search_counts();
            self.search_filter = Some(self.filter.clone());
        }
        while self
            .search_counts
            .front()
            .is_some_and(|cached| cached.sequence < first)
        {
            self.match_count -= self.search_counts.pop_front().unwrap().count;
        }
        if let Some(regex) = &self.regex {
            let start = self.search_last.map_or(0, |last| {
                buffer
                    .entries
                    .partition_point(|entry| entry.sequence <= last)
            });
            for entry in buffer.entries.range(start..) {
                if !self.filter.matches(entry) {
                    continue;
                }
                #[cfg(test)]
                {
                    self.search_scans += 1;
                }
                let count = entry_matches(entry, regex).count();
                // Store counts, not occurrences: memory is bounded by retained entries,
                // even when a short query matches every character in a long payload.
                if count > 0 {
                    self.search_counts.push_back(SearchCount {
                        sequence: entry.sequence,
                        count,
                    });
                    self.match_count += count;
                }
            }
        }
        self.search_last = buffer.entries.back().map(|entry| entry.sequence);
        self.match_index = self.active.as_ref().and_then(|active| {
            let entry = entry(buffer, active.sequence)?;
            if !self.filter.matches(entry) {
                return None;
            }
            let preceding: usize = self
                .search_counts
                .iter()
                .take_while(|cached| cached.sequence < active.sequence)
                .map(|cached| cached.count)
                .sum();
            entry_matches(entry, self.regex.as_ref()?)
                .position(|found| &found == active)
                .map(|index| preceding + index + 1)
        });
        if self.match_index.is_none() {
            self.active = None;
        }
    }

    pub fn sync(&mut self, buffer: &LogBuffer, width: usize, height: usize) {
        self.width = width.max(1);
        self.height = height.max(1);
        let mode = (
            self.width,
            self.wrap,
            buffer.pretty_json,
            self.filter.clone(),
        );
        if self.layout_mode.as_ref() != Some(&mode) {
            self.layouts.clear();
            self.visual_rows = 0;
            self.layout_scanned = None;
            self.search_stamp = None;
            self.layout_mode = Some(mode);
        }
        let first = buffer.entries.front().map(|e| e.sequence).unwrap_or(0);
        while let Some(layout) = self.layouts.front() {
            if layout.sequence >= first {
                break;
            }
            self.visual_rows -= layout.ranges.len();
            self.layouts.pop_front();
        }
        let start = self.layout_scanned.map_or(0, |scanned| {
            buffer
                .entries
                .partition_point(|entry| entry.sequence <= scanned)
        });
        for entry in buffer.entries.range(start..) {
            #[cfg(test)]
            {
                self.layout_scans += 1;
            }
            if self.filter.matches(entry) {
                let ranges = row_ranges(entry.display(), self.width, self.wrap);
                self.visual_rows += ranges.len();
                self.layouts.push_back(EntryLayout {
                    sequence: entry.sequence,
                    ranges,
                });
            }
        }
        if let Some(entry) = buffer.entries.back() {
            self.layout_scanned = Some(entry.sequence);
        }
        let stamp = buffer
            .entries
            .back()
            .map(|entry| (first, entry.sequence, buffer.entries.len()));
        if self.search_stamp != stamp {
            self.sync_search(buffer, first);
            self.search_stamp = stamp;
        }
    }

    fn total_rows(&self) -> usize {
        self.visual_rows
    }

    pub fn scroll_position(&self, buffer: &LogBuffer) -> (usize, usize) {
        (self.start(buffer), self.total_rows())
    }

    /// Thumb start and length, matching Ratatui's rounded scrollbar geometry.
    pub fn scrollbar_thumb(&self, buffer: &LogBuffer, track: usize) -> (usize, usize) {
        if track == 0 {
            return (0, 0);
        }
        let range = self.total_rows().saturating_sub(self.height);
        let extent = range + self.height;
        let length = ((self.height * track + extent / 2) / extent).clamp(1, track);
        let start =
            ((self.start(buffer).min(range) * track + extent / 2) / extent).min(track - length);
        (start, length)
    }

    fn start(&self, buffer: &LogBuffer) -> usize {
        if buffer.follow {
            return self.total_rows().saturating_sub(self.height);
        }
        let Some(anchor) = self.anchor else {
            return 0;
        };
        let mut index = 0;
        for layout in &self.layouts {
            if layout.sequence >= anchor.sequence {
                if layout.sequence == anchor.sequence {
                    index += layout
                        .ranges
                        .iter()
                        .rposition(|range| range.start <= anchor.byte)
                        .unwrap_or(0);
                }
                return index;
            }
            index += layout.ranges.len();
        }
        self.total_rows().saturating_sub(self.height)
    }

    fn position_at(&self, mut index: usize) -> Option<Position> {
        for layout in &self.layouts {
            if index < layout.ranges.len() {
                return Some(Position {
                    sequence: layout.sequence,
                    byte: layout.ranges[index].start,
                });
            }
            index -= layout.ranges.len();
        }
        None
    }

    pub fn visible(&self, buffer: &LogBuffer) -> Vec<VisualRow> {
        let rows = self.layouts.iter().flat_map(|layout| {
            layout
                .ranges
                .iter()
                .enumerate()
                .map(move |(index, range)| VisualRow {
                    sequence: layout.sequence,
                    range: range.clone(),
                    continuation: index > 0,
                })
        });
        if buffer.follow {
            // Follow needs only the tail, regardless of retained wrapped history.
            let mut visible: Vec<_> = rows.rev().take(self.height).collect();
            visible.reverse();
            visible
        } else {
            rows.skip(self.start(buffer)).take(self.height).collect()
        }
    }

    pub fn scroll(&mut self, buffer: &mut LogBuffer, amount: isize) {
        self.sync(buffer, self.width, self.height);
        let index = self
            .start(buffer)
            .saturating_add_signed(amount)
            .min(self.total_rows().saturating_sub(1));
        self.anchor = self.position_at(index);
        buffer.follow = false;
    }

    /// Jumps to a fraction of the scrollable range, as a scrollbar drag does.
    pub fn scroll_to_fraction(
        &mut self,
        buffer: &mut LogBuffer,
        numerator: usize,
        denominator: usize,
    ) {
        self.sync(buffer, self.width, self.height);
        let range = self.total_rows().saturating_sub(self.height);
        self.anchor = self.position_at(range * numerator / denominator.max(1));
        buffer.follow = false;
    }

    pub fn first(&mut self, buffer: &mut LogBuffer) {
        self.sync(buffer, self.width, self.height);
        self.anchor = self.position_at(0);
        buffer.follow = false;
    }

    pub fn toggle_wrap(&mut self, buffer: &LogBuffer) {
        self.sync(buffer, self.width, self.height);
        if !buffer.follow {
            let mut position = self.position_at(self.start(buffer));
            if let Some(anchor) = &mut position
                && let Some(entry) = buffer
                    .entries
                    .iter()
                    .find(|entry| entry.sequence == anchor.sequence)
            {
                let text = entry.display();
                let line = line_start(text, anchor.byte);
                if self.wrap {
                    self.horizontal = text[line..anchor.byte].width();
                } else {
                    let end = text[line..].find('\n').map_or(text.len(), |at| line + at);
                    anchor.byte = line + byte_at_column(&text[line..end], self.horizontal);
                }
            }
            self.anchor = position;
        }
        self.wrap = !self.wrap;
        self.sync(buffer, self.width, self.height);
    }

    /// Keep the top entry in place across modes. Byte offsets differ between the two
    /// texts, so the position is the start of that entry and a current match is dropped.
    pub fn toggle_pretty_json(&mut self, buffer: &mut LogBuffer) {
        self.sync(buffer, self.width, self.height);
        let top = self.position_at(self.start(buffer));
        buffer.set_pretty_json(!buffer.pretty_json);
        self.anchor = top.map(|position| Position {
            sequence: position.sequence,
            byte: 0,
        });
        self.horizontal = 0;
        self.active = None;
        self.match_index = None;
        self.reset_search_counts();
        self.sync(buffer, self.width, self.height);
    }

    pub fn next_match(&mut self, buffer: &mut LogBuffer, backwards: bool) {
        let active = self.active.as_ref().map(SearchMatch::key);
        let next = if backwards {
            self.matches(buffer)
                .filter(|found| active.is_none_or(|key| found.key() < key))
                .last()
                .or_else(|| self.matches(buffer).last())
        } else {
            self.matches(buffer)
                .find(|found| active.is_none_or(|key| found.key() > key))
                .or_else(|| self.matches(buffer).next())
        };
        if let Some(found) = next {
            self.anchor = Some(Position {
                sequence: found.sequence,
                byte: if found.field == Field::Payload {
                    found.range.start
                } else {
                    0
                },
            });
            if !self.wrap
                && let Some(entry) = buffer
                    .entries
                    .iter()
                    .find(|entry| entry.sequence == found.sequence)
            {
                self.horizontal = if found.field == Field::Payload {
                    let text = entry.display();
                    text[line_start(text, found.range.start)..found.range.start].width()
                } else {
                    0
                };
            }
            self.active = Some(found);
            self.search_stamp = None;
            buffer.follow = false;
            self.sync(buffer, self.width, self.height);
        }
    }
}

fn entry_matches<'a>(entry: &'a Entry, regex: &'a Regex) -> impl Iterator<Item = SearchMatch> + 'a {
    [
        (Field::Source, entry.source.as_str()),
        (Field::Payload, entry.display()),
    ]
    .into_iter()
    .flat_map(move |(field, text)| {
        // A pattern such as `a*` also matches nothing, which cannot be shown or navigated.
        regex
            .find_iter(text)
            .filter(|found| !found.is_empty())
            .map(move |found| SearchMatch {
                sequence: entry.sequence,
                field,
                range: found.range(),
            })
    })
}

pub fn entry(buffer: &LogBuffer, sequence: u64) -> Option<&Entry> {
    let first = buffer.entries.front()?.sequence;
    buffer.entries.get(sequence.checked_sub(first)? as usize)
}

pub fn byte_at_column(text: &str, column: usize) -> usize {
    let mut width = 0;
    for (index, grapheme) in text.grapheme_indices(true) {
        if width >= column {
            return index;
        }
        width += grapheme.width();
    }
    text.len()
}

fn line_start(text: &str, byte: usize) -> usize {
    text[..byte].rfind('\n').map_or(0, |at| at + 1)
}

/// Rows of one entry. Newlines exist only in pretty-printed JSON and never appear in a row.
fn row_ranges(text: &str, width: usize, wrap: bool) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        let end = start + line.len();
        if wrap {
            rows.extend(
                wrap_ranges(line, width)
                    .into_iter()
                    .map(|range| start + range.start..start + range.end),
            );
        } else {
            rows.push(start..end);
        }
        start = end + 1;
    }
    rows
}

pub fn wrap_ranges(text: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    // Printable ASCII has one byte, cell and grapheme per character. Controls
    // (including CRLF), combining marks and wide characters use the Unicode path.
    if !text.is_empty() && text.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        return (0..text.len())
            .step_by(width)
            .map(|start| start..start.saturating_add(width).min(text.len()))
            .collect();
    }
    let mut rows = Vec::new();
    let mut start = 0;
    let mut columns = 0;
    for (index, grapheme) in text.grapheme_indices(true) {
        let size = grapheme.width();
        if columns + size > width && index > start {
            rows.push(start..index);
            start = index;
            columns = 0;
        }
        columns += size;
    }
    rows.push(start..text.len());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_search_matches_full_scan(view: &LogView, buffer: &LogBuffer) {
        let matches: Vec<_> = view.matches(buffer).collect();
        assert_eq!(view.match_count, matches.len());
        assert_eq!(
            view.match_index,
            view.active
                .as_ref()
                .and_then(|active| matches.iter().position(|found| found == active))
                .map(|index| index + 1)
        );
        assert!(view.search_counts.len() <= buffer.entries.len());
    }

    fn text_filter(text: &str) -> crate::logs::filter::Filter {
        let mut filter = crate::logs::filter::Filter::default();
        filter.set_text(text);
        filter
    }

    #[test]
    fn idle_frames_do_not_rejudge_entries_the_filter_rejected() {
        let mut buffer = LogBuffer::new(100);
        for _ in 0..50 {
            buffer.push("api", "nothing to see", false);
        }
        let mut view = LogView::new(true);
        view.filter = text_filter("backend1 500");
        view.sync(&buffer, 80, 10);
        assert_eq!(view.layout_scans, 50);
        assert_eq!(view.matching_entries(), 0);
        for _ in 0..3 {
            view.sync(&buffer, 80, 10);
        }
        assert_eq!(view.layout_scans, 50, "Idle frames must not rescan");

        buffer.push("api", "backend1 500", false);
        buffer.push("api", "backend1 only", false);
        view.sync(&buffer, 80, 10);
        assert_eq!(view.layout_scans, 52, "Only new entries are judged");
        assert_eq!(view.matching_entries(), 1);

        view.filter = text_filter("nothing");
        view.sync(&buffer, 80, 10);
        assert_eq!(
            view.layout_scans, 104,
            "A new filter judges everything once"
        );
        assert_eq!(view.matching_entries(), 50);

        view.reset_content();
        view.sync(&LogBuffer::new(1), 80, 10);
        let mut fresh = LogBuffer::new(4);
        fresh.push("api", "nothing again", false);
        view.sync(&fresh, 80, 10);
        assert_eq!(
            view.matching_entries(),
            1,
            "A reset starts from the beginning"
        );
    }

    #[test]
    fn text_filter_keeps_wrapped_entries_whole_and_follows_live_arrivals() {
        let mut buffer = LogBuffer::new(4);
        buffer.push("api", "backend1 aaaaaaaaaaaaaaaaaaaaaaaaaaa 500", false);
        buffer.push("api", "backend1 only", false);
        let mut view = LogView::new(true);
        view.filter = text_filter("backend1 500");
        // The terms land on different rows at this width, and the entry stays whole.
        view.sync(&buffer, 10, 20);
        assert!(view.total_rows() > 1);
        let rows = view.visible(&buffer);
        assert!(rows.iter().all(|row| row.sequence == 0));
        assert_eq!(rows.last().unwrap().range.end, buffer.entries[0].text.len());

        buffer.push("api", "backend1 returned 500", false);
        buffer.push("api", "backend2 returned 500", false);
        view.sync(&buffer, 80, 20);
        assert_eq!(view.matching_entries(), 2);
        assert_eq!(view.visible(&buffer).last().unwrap().sequence, 2);
        // Eviction still applies, and hidden entries count against capacity.
        buffer.push("api", "other", false);
        buffer.push("api", "other", false);
        view.sync(&buffer, 80, 20);
        assert_eq!(view.matching_entries(), 1);
        assert_eq!(buffer.entries.len(), 4);
    }

    #[test]
    fn hiding_a_paused_anchor_moves_to_a_valid_position_and_search_skips_hidden_entries() {
        let mut buffer = LogBuffer::new(10);
        for text in ["needle first", "needle backend1 500", "needle last"] {
            buffer.push("api", text, false);
        }
        let mut view = LogView::new(true);
        view.sync(&buffer, 80, 1);
        view.first(&mut buffer);
        assert!(!buffer.follow);
        assert_eq!(view.visible(&buffer)[0].sequence, 0);

        view.set_query("needle".into(), &mut buffer);
        view.filter = text_filter("backend1 500");
        view.sync(&buffer, 80, 1);
        assert_eq!(view.match_count, 1, "Hidden occurrences are not counted");
        assert_eq!(view.visible(&buffer)[0].sequence, 1);

        view.filter = text_filter("absent");
        view.sync(&buffer, 80, 1);
        assert!(view.visible(&buffer).is_empty());
        assert_eq!(view.match_count, 0);
        assert_eq!(buffer.entries.len(), 3);

        view.filter = Default::default();
        view.sync(&buffer, 80, 1);
        assert_eq!(view.matching_entries(), 3);
        assert_eq!(view.match_count, 3);
    }

    #[test]
    fn search_counts_scan_only_new_entries_and_survive_navigation_resize_and_eviction() {
        let mut buffer = LogBuffer::new(3);
        for text in ["needle needle", "no match", "ÉRROR needle"] {
            buffer.push("needle-api", text, false);
        }
        let mut view = LogView::new(true);
        view.set_query("needle".into(), &mut buffer);
        assert_eq!(view.search_scans, 3);
        for backwards in [false, false, true, true] {
            view.next_match(&mut buffer, backwards);
            assert_search_matches_full_scan(&view, &buffer);
        }
        view.sync(&buffer, 3, 2);
        view.toggle_wrap(&buffer);
        assert_eq!(
            view.search_scans, 3,
            "Resizing/navigation must reuse counts"
        );
        for text in ["needle", "no match", "needle needle", "ÉRROR"] {
            let scans = view.search_scans;
            buffer.push("api", text, false);
            view.sync(&buffer, 7, 2);
            assert_eq!(view.search_scans, scans + 1);
            assert_search_matches_full_scan(&view, &buffer);
        }
        // Several complete buffers may be evicted between frames.
        for _ in 0..10 {
            buffer.push("api", "needle", false);
        }
        let scans = view.search_scans;
        view.sync(&buffer, 7, 2);
        assert_eq!(view.search_scans, scans + 3);
        assert_search_matches_full_scan(&view, &buffer);
        assert!(view.active.is_none());
    }

    #[test]
    fn search_counts_invalidate_for_query_filter_and_content_not_layout() {
        let mut buffer = LogBuffer::new(4);
        buffer.push("API", "ERROR needle needle", false);
        buffer.push("worker", "INFO needle", false);
        buffer.push("API", "ÉRROR érror", false);
        let mut view = LogView::new(true);
        view.set_query("needle".into(), &mut buffer);
        for filter in [r#"{"source":"api"}"#, r#"{"level":"error"}"#, "{}"] {
            view.filter = super::super::filter::Filter::parse(filter).unwrap();
            view.sync(&buffer, 10, 3);
            assert_search_matches_full_scan(&view, &buffer);
            buffer.push("worker", "ERROR needle", false);
            view.sync(&buffer, 10, 3);
            assert_search_matches_full_scan(&view, &buffer);
        }
        view.set_query("érror".into(), &mut buffer);
        assert_search_matches_full_scan(&view, &buffer);
        view.set_query(String::new(), &mut buffer);
        assert_eq!(view.match_count, 0);
        assert!(view.search_counts.is_empty());
        view.set_query("needle".into(), &mut buffer);
        buffer = LogBuffer::new(4);
        view.reset_content();
        view.sync(&buffer, 10, 3);
        assert_search_matches_full_scan(&view, &buffer);
        buffer.push("api", "needle needle", false);
        view.sync(&buffer, 10, 3);
        assert_eq!(view.match_count, 2);
        assert_search_matches_full_scan(&view, &buffer);
    }

    #[test]
    fn search_rank_tracks_retained_matches_and_dense_queries_keep_one_count_per_entry() {
        let mut buffer = LogBuffer::new(3);
        buffer.push("a", "a", false);
        buffer.push("a", "aaa", false);
        buffer.push("b", "a", false);
        let mut view = LogView::new(true);
        view.set_query("a".into(), &mut buffer);
        for _ in 0..3 {
            view.next_match(&mut buffer, false);
        }
        assert_eq!(view.active.as_ref().unwrap().sequence, 1);
        assert_eq!(view.match_index, Some(4));
        buffer.push("b", "none", false);
        view.sync(&buffer, 10, 3);
        assert_eq!(view.match_index, Some(2), "Evicted matches shift the rank");
        assert_search_matches_full_scan(&view, &buffer);
        for _ in 0..3 {
            buffer.push("a", &"a".repeat(16_000), false);
        }
        view.sync(&buffer, 10, 3);
        assert_eq!(view.match_count, 48_003);
        assert_eq!(view.search_counts.len(), 3);
        assert_search_matches_full_scan(&view, &buffer);
    }

    #[test]
    fn printable_ascii_fast_path_matches_grapheme_wrapping() {
        for text in [
            "",
            " ",
            "abcdef  ghij",
            "a\tb",
            "a\r\nb",
            "ไทย 界 e\u{301} 👩‍💻",
        ] {
            for width in [0, 1, 2, 7, usize::MAX] {
                let mut expected = Vec::new();
                let mut start = 0;
                let mut columns = 0;
                for (index, grapheme) in text.grapheme_indices(true) {
                    let size = grapheme.width();
                    if columns + size > width.max(1) && index > start {
                        expected.push(start..index);
                        start = index;
                        columns = 0;
                    }
                    columns += size;
                }
                expected.push(start..text.len());
                assert_eq!(wrap_ranges(text, width), expected, "{text:?}, {width}");
            }
        }
    }

    #[test]
    fn wrapping_keeps_unicode_and_whitespace_without_splitting_graphemes() {
        let text = "  ไทย 界 e\u{301} 👩‍💻 longtoken";
        let ranges = wrap_ranges(text, 5);
        assert_eq!(
            ranges
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<String>(),
            text
        );
        assert!(ranges.iter().all(|range| text[range.clone()].width() <= 5));
        let empty = wrap_ranges("", 3);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0], 0..0);
    }

    #[test]
    fn scroll_moves_display_lines_and_toggle_preserves_top_payload_position() {
        let mut buffer = LogBuffer::new(20);
        buffer.push("api", "abcdefghijklmnop", false);
        let mut view = LogView::new(true);
        view.sync(&buffer, 4, 2);
        view.first(&mut buffer);
        view.scroll(&mut buffer, 2);
        assert_eq!(view.visible(&buffer)[0].range, 8..12);
        view.toggle_wrap(&buffer);
        assert!(!view.wrap);
        assert_eq!(view.horizontal, 8);
        view.toggle_wrap(&buffer);
        assert_eq!(view.visible(&buffer)[0].range, 8..12);
        view.sync(&buffer, 3, 2);
        assert_eq!(view.visible(&buffer)[0].range, 6..9);
    }

    #[test]
    fn search_keeps_context_and_navigates_each_unicode_case_insensitive_occurrence() {
        let mut buffer = LogBuffer::new(20);
        buffer.push("api", "before", false);
        buffer.push("api", "ÉRROR érror", false);
        buffer.push("api", "after", false);
        let mut view = LogView::new(true);
        view.sync(&buffer, 5, 6);
        view.set_query("érror".into(), &mut buffer);
        assert_eq!(view.match_count, 2);
        assert_eq!(view.match_index, Some(1));
        view.next_match(&mut buffer, false);
        assert_eq!(view.match_index, Some(2));
        view.next_match(&mut buffer, false);
        assert_eq!(view.match_index, Some(1));
        view.next_match(&mut buffer, true);
        assert_eq!(view.match_index, Some(2));
        view.first(&mut buffer);
        assert!(
            view.visible(&buffer)
                .iter()
                .any(|row| entry(&buffer, row.sequence).unwrap().text == "before")
        );
        view.set_query("not present".into(), &mut buffer);
        assert_eq!(view.match_count, 0);
        assert!(!view.visible(&buffer).is_empty());
    }

    #[test]
    fn follow_shows_end_and_paused_eviction_clamps_to_retained_history() {
        let mut buffer = LogBuffer::new(2);
        buffer.push("api", "12345678", false);
        let mut view = LogView::new(true);
        view.sync(&buffer, 2, 2);
        assert_eq!(view.visible(&buffer)[0].range, 4..6);
        view.first(&mut buffer);
        buffer.push("api", "two", false);
        buffer.push("api", "three", false);
        view.sync(&buffer, 2, 2);
        assert_eq!(view.visible(&buffer)[0].sequence, 1);
    }

    #[test]
    fn follow_viewport_preserves_partial_entries_unicode_and_continuation_flags() {
        let mut buffer = LogBuffer::new(3);
        buffer.push("api", "abcdefgh", false);
        buffer.push("api", "界界ab", false);
        let mut view = LogView::new(true);
        view.sync(&buffer, 4, 3);
        let rows: Vec<_> = view
            .visible(&buffer)
            .into_iter()
            .map(|row| (row.sequence, row.range, row.continuation))
            .collect();
        assert_eq!(
            rows,
            vec![(0, 4..8, true), (1, 0..6, false), (1, 6..8, true)]
        );

        view.first(&mut buffer);
        view.scroll(&mut buffer, 1);
        buffer.push("api", "ijklmnop", false);
        buffer.push("api", "qrstuvwx", false);
        view.sync(&buffer, 4, 3);
        assert_eq!(view.scroll_position(&buffer), (0, 6));
        buffer.follow = true;
        let rows: Vec<_> = view
            .visible(&buffer)
            .into_iter()
            .map(|row| (row.sequence, row.range, row.continuation))
            .collect();
        assert_eq!(
            rows,
            vec![(2, 4..8, true), (3, 0..4, false), (3, 4..8, true)]
        );
        assert_eq!(view.scroll_position(&buffer), (3, 6));

        view.sync(&buffer, 8, 10);
        assert_eq!(view.scroll_position(&buffer), (0, 3));
        assert!(view.visible(&buffer).iter().all(|row| !row.continuation));
        view.filter = text_filter("absent");
        view.sync(&buffer, 8, 10);
        assert_eq!(view.scroll_position(&buffer), (0, 0));
        assert!(view.visible(&buffer).is_empty());
    }

    #[test]
    fn follow_viewport_tracks_pretty_json_wrap_changes_and_content_reset() {
        let mut buffer = LogBuffer::new(2);
        buffer.push("api", r#"{"a":1}"#, false);
        let mut view = LogView::new(true);
        view.sync(&buffer, 80, 2);
        view.toggle_pretty_json(&mut buffer);
        assert_eq!(view.scroll_position(&buffer), (1, 3));
        let rows: Vec<_> = view
            .visible(&buffer)
            .into_iter()
            .map(|row| (row.range, row.continuation))
            .collect();
        assert_eq!(rows, vec![(2..10, true), (11..12, true)]);
        view.toggle_wrap(&buffer);
        assert_eq!(view.scroll_position(&buffer), (1, 3));
        view.toggle_pretty_json(&mut buffer);
        assert_eq!(view.scroll_position(&buffer), (0, 1));

        view.reset_content();
        buffer = LogBuffer::new(2);
        view.sync(&buffer, 80, 2);
        assert_eq!(view.scroll_position(&buffer), (0, 0));
        buffer.push("api", "", false);
        view.sync(&buffer, 80, 2);
        assert_eq!(view.scroll_position(&buffer), (0, 1));
        let rows = view.visible(&buffer);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].range, 0..0);
        assert!(!rows[0].continuation);
    }

    #[test]
    fn scrollbar_position_tracks_wrapped_lines_follow_and_paused_navigation() {
        let mut buffer = LogBuffer::new(10);
        buffer.push("api", "abcdefghijklmnop", false);
        let mut view = LogView::new(true);
        view.sync(&buffer, 4, 2);
        assert_eq!(view.scroll_position(&buffer), (2, 4));
        view.first(&mut buffer);
        assert_eq!(view.scroll_position(&buffer), (0, 4));
        view.scroll(&mut buffer, 1);
        assert_eq!(view.scroll_position(&buffer), (1, 4));
        view.toggle_wrap(&buffer);
        assert_eq!(view.scroll_position(&buffer), (0, 1));
        buffer.follow = true;
        view.toggle_wrap(&buffer);
        assert_eq!(view.scroll_position(&buffer), (2, 4));
    }

    fn row_text(view: &LogView, buffer: &LogBuffer) -> Vec<String> {
        view.visible(buffer)
            .into_iter()
            .map(|row| entry(buffer, row.sequence).unwrap().display()[row.range].to_owned())
            .collect()
    }

    #[test]
    fn regex_search_is_opt_in_case_insensitive_and_ignores_empty_matches() {
        let mut buffer = LogBuffer::new(4);
        buffer.push("api", "status=500 failed", false);
        buffer.push("api", "status=200 a.c", false);
        let mut view = LogView::new(true);
        view.sync(&buffer, 80, 4);
        view.set_query("a.c".into(), &mut buffer);
        assert_eq!(view.match_count, 1, "Literal by default");
        assert!(!view.query_is_regex());
        view.regex_search = true;
        view.set_query(r"STATUS=5\d\d".into(), &mut buffer);
        assert_eq!(view.match_count, 1);
        assert!(view.query_is_regex());
        view.set_query("z*".into(), &mut buffer);
        assert_eq!(view.match_count, 0);
        assert!(view.active.is_none());
        assert!(view.check_query("(unclosed").is_err());
        assert_eq!(view.query(), "z*", "Checking does not replace the search");
        view.regex_search = false;
        assert!(view.check_query("(unclosed").is_ok());
    }

    #[test]
    fn pretty_json_uses_one_row_per_line_and_search_targets_what_is_shown() {
        let mut buffer = LogBuffer::new(4);
        buffer.push("api", "before", false);
        buffer.push("api", r#"{"level":"warn","message":"retry"}"#, false);
        let mut view = LogView::new(false);
        view.sync(&buffer, 40, 10);
        view.set_query("level".into(), &mut buffer);
        assert_eq!(view.match_index, Some(1));
        view.toggle_pretty_json(&mut buffer);
        assert!(buffer.pretty_json);
        assert!(view.active.is_none(), "Old byte offsets are not reused");
        assert_eq!(view.match_count, 1);
        view.first(&mut buffer);
        assert_eq!(
            row_text(&view, &buffer),
            [
                "before",
                "{",
                "  \"level\": \"warn\",",
                "  \"message\": \"retry\"",
                "}"
            ]
        );
        assert!(view.visible(&buffer)[2].continuation);
        view.set_query("message".into(), &mut buffer);
        assert_eq!(view.visible(&buffer)[0].range.start, 21);
        assert_eq!(
            view.horizontal, 3,
            "Columns count from the start of the line"
        );
        view.toggle_wrap(&buffer);
        assert_eq!(row_text(&view, &buffer)[0], "  \"message\": \"retry\"");
        view.toggle_wrap(&buffer);
        assert_eq!(row_text(&view, &buffer)[0], "  \"message\": \"retry\"");
        view.toggle_pretty_json(&mut buffer);
        assert_eq!(
            row_text(&view, &buffer)[0],
            r#"{"level":"warn","message":"retry"}"#,
            "Turning it off keeps the same entry at the top"
        );
    }

    #[test]
    fn pretty_json_wraps_each_line_and_keeps_every_character() {
        let mut buffer = LogBuffer::new(2);
        buffer.push("api", r#"{"message":"abcdefghij"}"#, false);
        buffer.set_pretty_json(true);
        let mut view = LogView::new(true);
        view.sync(&buffer, 8, 10);
        let rows = row_text(&view, &buffer);
        assert!(
            rows.iter()
                .all(|row| row.width() <= 8 && !row.contains('\n'))
        );
        assert_eq!(rows.concat(), buffer.entries[0].display().replace('\n', ""));
    }
}
