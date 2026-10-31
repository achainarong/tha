# Proposal

## Why

Users need to narrow noisy live logs by typing ordinary text, rather than writing JSON or merely highlighting matches. The explicit user example is `backend1 500`, which must require both terms on the same log message, not either term.

## What Changes

- Add an accessible plain-text retained-log filter with AND semantics across whitespace-separated literal terms.
- Hide nonmatching entries without deleting retained logs; apply the same predicate to incoming entries and exports.
- Compose text filtering with existing structured predicates and preserve independent highlight search.
- Explain active filtering, empty results, clearing, and retained-history limits.
- Case-insensitive matching against original message text, not source names or pretty-printed display fragments. Message-only matching was confirmed by the user.
- Exclude OR/NOT syntax, quoting, regex filter syntax, persistence, and multiline grouping from this change.

## Capabilities

### New Capabilities

- `log-text-filtering`: Literal AND filtering of retained and incoming log entries with consistent view and export behavior.

### Modified Capabilities

None. Existing log behavior has no OpenSpec baseline yet; preserve the compatibility requirements included in the new contract.

## Impact

Touches log predicates, view/cache invalidation, input dialogs, export selection, help/localization, fixtures, and tests. No runtime API, collector, dependency, or database change is expected.
