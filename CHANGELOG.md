# Changelog

Notable changes to `minisearch-wasm`. Versions before 0.7.0 are summarized in
`PORTING.md`. Release notes of the latest published version are in
`RELEASE_NOTES.md`.

## Unreleased

### Changed

- A Wasm index stays in Wasm through everyday use. `discard`/`replace`
  followed by queries, vacuuming, `getStoredFields`, `loadJSON`/`loadJSONAsync`
  and the callbacks `filter`, `prefix`, `fuzzy`, `boostTerm`, `extractField`,
  `stringifyField` and `logger` no longer move it to the JavaScript engine.
  Only `tokenize`, `processTerm`, `boostDocument`, values with no native form
  and an edited `getStoredFields` object still do.
- The native engine reproduces MiniSearch's dirty-index queries exactly: the
  running document frequency of the first query, the lazy removal of stale
  postings (one frequency step per query) and the term deletions that follow.
- Vacuum runs natively, scheduled by a port of MiniSearch's
  `conditionalVacuum`/`performVacuuming` (same active and queued Promises). A
  search or mutation during a vacuum no longer wedges maintenance, which
  MiniSearch 7.2.0's own vacuum does (lucaong/minisearch#306).
- Scores are MiniSearch's bit for bit: the inverse document frequency uses
  fdlibm's `log`, the algorithm behind V8's `Math.log`.
- `compact()` drops postings left by a document that changed before removal
  instead of refusing to run; in JavaScript mode as well.
- Removing documents no longer shifts posting lists (tombstones, swept when
  they outnumber live entries): removing the oldest 50,000 of 100,000
  documents went from 48.7 s to 0.6 s.
- `npm run build` writes the generated glue to `target/wasm-glue`;
  `scripts/finalize-pkg.mjs` assembles `pkg/` from scratch and is idempotent.

### Fixed

- The package ships the repository's README and `LICENSE.txt` (0.10.0 shipped
  the 0.9.0 README).
- Snapshots: negative field averages and postings outside the dirt count load
  again; radix depth is 128 for the binary and the JSON reader alike; internal
  id churn, deeper trees, duplicate field names and nonfinite options are
  refused when saving (or constructing) instead of when loading.
- A rejected snapshot could reserve far more than the 256 MiB decode budget
  (1 MiB of input grew Wasm memory to 1.5 GiB).
- An absurd `fuzzy` distance, or a query term too long for a fuzzy matrix,
  aborted the module and left it unusable for every index.
- Core exports with string parameters trapped on other values; `autoSuggest`,
  `searchJoined*`, `searchRaw` and `autoSuggestJoined` accept query trees and
  the wildcard through the facade. `loadBytes` accepts an `ArrayBuffer`.
- Vacuum emptied terms in sorted order, not MiniSearch's tree order, which could
  leave the radix tree's keys, and with them tie order, different.
- Compact results mis-scored a repeated query term whose occurrences expand
  differently (per-term `prefix`/`fuzzy`, `autoSuggest` with `OR`).
- The declarative `filter` treated `4.0` and `4` as different numbers.
- A `boost` of `0` multiplied scores by zero; MiniSearch reads it as no boost.
- After a transfer, documents without stored values had no stored-fields
  object: `boostDocument` received `undefined`, `getStoredFields` returned it.
- The core's queued auto-vacuum ignored its conditions and options, and its
  first run started a tick late.
- Inputs that the native engine would alter now select the JavaScript engine:
  lone surrogates, `-0`, repeated or non-array `fields`, stored fields named
  like result properties, `Infinity` in search options. Options explicitly set
  to `undefined` behave as in MiniSearch. `discard(1n)` no longer discards `1`.

### Added

- `npm test` (native, native differential, Wasm suites, types, package,
  MiniSearch's own test suite, separators, size budget), `test:upstream`,
  `test:differential`, `check:size`, `build:pkg`, a CI workflow,
  `rust-toolchain.toml`.
- Named types for TypeScript CommonJS consumers, declarations that need neither
  the DOM library nor an installed MiniSearch, a Node entry that survives being
  bundled, a browser-safe `require` branch, a minified global script.

## 0.10.0 - 2026-09-18

Public MiniSearch 7.2.0 compatibility facade: callbacks, object ids, live
stored-field references, query trees, wildcard, defaults, generic TypeScript
types; Node ESM/CommonJS, browser-global, module Worker and `SearchableMap`
entries; CSP-safe static helpers; compatibility snapshot envelope for
JavaScript-mode indexes; ID compaction after clean maintenance.

## 0.9.0 - 2026-09-16

Exact field-length accounting, radix traversal order preserved in binary
snapshots (format version 4), validated snapshot readers, lossless compact ids
with `idTableVersion`; real `Error` objects with MiniSearch's messages,
declarative per-term `prefix`/`fuzzy`/`boostTerm` and stored-field `filter`,
JS-style value stringification, Unicode 17 separator table, `getDefault`,
`loadJSONAsync`, `logger`, MiniSearch-format `toJSON`/`loadJSON`, typed
declarations, Node loading out of the box, a faster `search()` path.

## 0.8.0 - 2026-07-03

Flat posting lists, a memoized prefix/fuzzy expansion cache, and the
all-numeric `searchRaw` boundary.

## 0.7.0 - 2026-07-03

Maintenance parity: `discard`, `replace`, `vacuum`, auto-vacuum, `removeAll`,
`discardAll` and `addAllAsync`.
