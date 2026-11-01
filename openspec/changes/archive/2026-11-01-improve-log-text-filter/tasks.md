# Tasks

## 1. Confirm scope and implement the predicate

- [x] 1.1 Resolve message-only versus source-inclusive matching with the user, update all affected artifacts, and add the source `backend1` / payload `HTTP 500` expected outcome to the spec before implementation.
- [x] 1.2 Add simple text state and AND matching to the effective filter; verify unit tests cover one/multiple terms, reverse order, absent terms, separate entries, whitespace, case, literal punctuation, and the agreed source behavior.
- [x] 1.3 Preserve structured JSON parsing and predicate composition; verify old filter tests and text-plus-source/level/field/time tests pass and document the new predicate semantics alongside the code.

## 2. Deliver the usable filtering flow

- [x] 2.1 Add the simple filter field and an explicit Advanced route using existing modal/keymap conventions; verify UI/input tests for Enter apply, Esc cancel, empty input, independent advanced state, and help text.
- [x] 2.2 Integrate visible rows, search counts, wrapping, paused anchors, follow, eviction, and filter summaries; verify integration tests for live arrivals, no matches, resizing, pretty JSON, and clearing without changing the retained buffer.
- [x] 2.3 Update localized strings, README, and keyboard help for text versus highlight search and clearing behavior; verify documented shortcuts in demo mode and existing translation checks.

## 3. Export and performance integration

- [x] 3.1 Make export use the combined effective filter and show its summary during review; verify a private export contains exactly matching original entries and existing no-overwrite protections pass.
- [x] 3.2 Extend relevant performance coverage for active filters over a full retained buffer and incoming entries; verify idle redraws do not introduce repeated full-buffer matching and no unbounded allocations are added.
- [x] 3.3 Run `make check` and `openspec validate improve-log-text-filter --strict --no-interactive`; record results and manually verify `backend1 500` excludes single-term entries in demo fixtures.

## Verification record (2026-10-31)

- Decision gate resolved by the user: message text only. A source named `backend1` with the message `HTTP 500` does not match `backend1 500`.
- The engine's timestamp prefix is also excluded from matching, so `500` does not match `10:00:00.500Z`.
- `make check` and `openspec validate --changes --strict --no-interactive` pass.
- Benchmark with 5,000 retained 4,000-byte entries and no matches: first filter pass about 2.1 ms, idle sync about 0.07 µs, one new entry about 13 µs. Rejected entries used to be judged again on every frame.
- `backend1 500` was checked by rendering the demo app to a test backend, not in a live terminal.

## Workflow follow-up

- Review implementation and resolve the decision gate before marking the change complete.
- Archive only after implementation, checks, and user review.
