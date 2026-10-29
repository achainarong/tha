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
    regex: Option<Regex>,
    pub active: Option<SearchMatch>,
    pub match_count: usize,
    pub match_index: Option<usize>,
    layouts: VecDeque<EntryLayout>,
    layout_mode: Option<(usize, bool, super::filter::Filter)>,
    search_stamp: Option<(u64, u64, usize)>,
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
            regex: None,
            active: None,
            match_count: 0,
            match_index: None,
            layouts: VecDeque::new(),
            layout_mode: None,
            search_stamp: None,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn regex(&self) -> Option<&Regex> {
        self.regex.as_ref()
    }

    pub fn reset_content(&mut self) {
        self.layouts.clear();
        self.search_stamp = None;
        self.anchor = None;
        self.active = None;
        self.match_count = 0;
        self.match_index = None;
        self.horizontal = 0;
    }

    pub fn set_query(&mut self, query: String, buffer: &mut LogBuffer) {
        self.query = query;
        self.regex = if self.query.is_empty() {
            None
        } else {
            RegexBuilder::new(&regex::escape(&self.query))
                .case_insensitive(true)
                .build()
                .ok()
        };
        self.active = None;
        self.search_stamp = None;
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
                [
                    (Field::Source, entry.source.as_str()),
                    (Field::Payload, entry.text.as_str()),
                ]
                .into_iter()
                .flat_map(move |(field, text)| {
                    self.regex.iter().flat_map(move |regex| {
                        regex.find_iter(text).map(move |found| SearchMatch {
                            sequence: entry.sequence,
                            field,
                            range: found.range(),
                        })
                    })
                })
            })
    }

    pub fn sync(&mut self, buffer: &LogBuffer, width: usize, height: usize) {
        self.width = width.max(1);
        self.height = height.max(1);
        if self.layout_mode != Some((self.width, self.wrap, self.filter.clone())) {
            self.layouts.clear();
            self.search_stamp = None;
            self.layout_mode = Some((self.width, self.wrap, self.filter.clone()));
        }
        let first = buffer.entries.front().map(|e| e.sequence).unwrap_or(0);
        while self
            .layouts
            .front()
            .is_some_and(|layout| layout.sequence < first)
        {
            self.layouts.pop_front();
        }
        let last = self.layouts.back().map(|layout| layout.sequence);
        for entry in buffer.entries.iter().filter(|entry| {
            last.is_none_or(|sequence| entry.sequence > sequence) && self.filter.matches(entry)
        }) {
            self.layouts.push_back(EntryLayout {
                sequence: entry.sequence,
                ranges: if self.wrap {
                    wrap_ranges(&entry.text, self.width)
                } else {
                    std::iter::once(0..entry.text.len()).collect()
                },
            });
        }
        let stamp = buffer
            .entries
            .back()
            .map(|entry| (first, entry.sequence, buffer.entries.len()));
        if self.search_stamp != stamp {
            let mut count = 0;
            let mut index = None;
            for found in self.matches(buffer) {
                count += 1;
                if self.active.as_ref() == Some(&found) {
                    index = Some(count);
                }
            }
            self.match_count = count;
            self.match_index = index;
            if index.is_none() {
                self.active = None;
            }
            self.search_stamp = stamp;
        }
    }

    fn total_rows(&self) -> usize {
        self.layouts.iter().map(|layout| layout.ranges.len()).sum()
    }

    pub fn scroll_position(&self, buffer: &LogBuffer) -> (usize, usize) {
        (self.start(buffer), self.total_rows())
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
        let start = self.start(buffer);
        self.layouts
            .iter()
            .flat_map(|layout| {
                layout
                    .ranges
                    .iter()
                    .enumerate()
                    .map(move |(index, range)| VisualRow {
                        sequence: layout.sequence,
                        range: range.clone(),
                        continuation: index > 0,
                    })
            })
            .skip(start)
            .take(self.height)
            .collect()
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
                if self.wrap {
                    self.horizontal = entry.text[..anchor.byte].width();
                } else {
                    anchor.byte = byte_at_column(&entry.text, self.horizontal);
                }
            }
            self.anchor = position;
        }
        self.wrap = !self.wrap;
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
                    entry.text[..found.range.start].width()
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

pub fn wrap_ranges(text: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
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
}
