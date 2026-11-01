# log-text-filtering Specification

## Purpose
Let users narrow retained and incoming logs with ordinary text while preserving original entries and consistent search, navigation, and export behavior.

## Requirements

### Requirement: Multiple terms require same-entry matches
The system SHALL split a text filter on whitespace and show an entry only when its original message contains every nonempty term as a case-insensitive literal substring, in any order. Terms SHALL NOT be combined across entries.

#### Scenario: Both terms occur in a message
- **WHEN** the filter is `backend1 500` and messages are `backend1 returned 500`, `500 returned by BACKEND1`, `backend1 returned 200`, and `backend2 returned 500`
- **THEN** only the first two entries are visible

#### Scenario: Terms occur in different entries
- **WHEN** one entry contains only `backend1` and another contains only `500`
- **THEN** neither matches `backend1 500`

#### Scenario: Repeated whitespace and literal characters
- **WHEN** the filter is `  backend1   [500]  `
- **THEN** it requires the literal substrings `backend1` and `[500]`, without treating brackets as regex syntax

### Requirement: Message matching is independent of presentation
The system SHALL match original message text rather than source labels or formatting inserted by wrapping or pretty JSON. A matching entry SHALL remain visible as a whole entry, not only its matching visual rows.

#### Scenario: Source name does not occur in the message
- **WHEN** the source is `backend1`, the message is `HTTP 500`, and the filter is `backend1 500`
- **THEN** the entry is hidden

#### Scenario: Terms are on different wrapped rows of one message
- **WHEN** a message contains both terms but wrapping places them on separate screen rows
- **THEN** the full entry matches and remains visible regardless of terminal width

### Requirement: Filtering preserves retained data
The system SHALL filter retained entries and each incoming entry without deleting hidden entries or changing history-fetch limits. Empty or whitespace-only text SHALL disable only the text constraint. Clearing all log filters SHALL also clear advanced constraints.

#### Scenario: Clearing restores retained entries
- **WHEN** a text filter hides retained entries and the user clears it with no advanced constraints active
- **THEN** all still-retained entries are eligible for display without refetching

#### Scenario: New entries are filtered live
- **WHEN** matching and nonmatching entries arrive while following an active text filter
- **THEN** only matching entries appear and existing buffer capacity and eviction limits still apply

### Requirement: Simple and advanced filters compose
The system SHALL provide an ordinary text-filter input separate from advanced structured-filter editing. Text and advanced predicates SHALL combine with AND. Applying or clearing text SHALL preserve advanced predicates, and cancelling an edit SHALL preserve the previously active filter.

#### Scenario: Text and severity both constrain results
- **WHEN** the text filter is `backend1 500` and the advanced minimum severity is error
- **THEN** only entries satisfying both text terms and the severity predicate are visible

#### Scenario: Cancel does not replace the active filter
- **WHEN** the user edits filter text and presses Esc instead of applying
- **THEN** the previously active text and advanced constraints remain unchanged

### Requirement: Navigation search and export agree on eligible entries
The system SHALL base scrolling, wrapping, follow, highlight search, and export eligibility on the combined filter. Existing highlight search SHALL remain independently available without changing filtering. The UI SHALL show active filtering and an explicit no-matches state.

#### Scenario: Highlight search ignores hidden entries
- **WHEN** a search term occurs in both filtered-out and visible entries
- **THEN** match counts and next/previous navigation include only visible entries

#### Scenario: Export contains the matching original entries
- **WHEN** the user exports retained logs with text and advanced filters active
- **THEN** the export contains exactly the eligible original entries and its review identifies the active filter

#### Scenario: A paused anchor becomes hidden
- **WHEN** the user applies a filter that hides the paused anchor entry
- **THEN** the viewport moves to a valid visible position or the no-matches state without clearing retained history or crashing
