# Changelog

Notable changes to `minisearch-wasm`. Versions before 0.7.0 are summarized in
`PORTING.md`. Release notes of the latest published version are in
`RELEASE_NOTES.md`.

## 0.12.0 - unreleased

### Changed

- Document ids and stored fields live on the JavaScript side, kept the way
  MiniSearch keeps them: ids of any type in a `Map` (SameValueZero), stored
  fields as live objects. The engine indexes each document by its short id.
  Object ids, `-0`, `NaN`, Dates, arrays, getters and other values no longer
  move an index to the JavaScript engine; `getStoredFields` returns the live
  object and an edit to it shows up in results, still in Wasm; stored fields
  named like result properties (`score`, `terms`, `match`, …) are assigned over
  the result and decide the order, as upstream.
- `tokenize` and `processTerm` callbacks (constructor, search options, query
  nodes) run on this side, called exactly as MiniSearch calls them; the engine
  indexes and searches the terms they make. Indexes with custom tokenizers or
  term processors, the most common MiniSearch customization, stay in Wasm.
- `boostDocument` runs in Wasm: the engine calls it back while it scores, once
  per posting list, for the same documents and terms in the same order as
  MiniSearch, and on a dirty index in the pass that cleans up. An exception
  ends the search, as upstream. Only a query-tree node with a `boostDocument`
  of its own still transfers the index.
- `search()` builds its result objects in one JavaScript loop from short ids,
  scores and interned terms: about 2.4× faster than 0.11.0, and 3.2× with
  stored fields (0.11.0 was slower than MiniSearch there).
- `addAll` sends documents to the engine in batches, and indexing allocates
  far less (token slices, a faster hash for field lengths, no copy of terms
  that are already lowercase, byte-wise radix edges, appended postings): about
  2× faster than 0.11.0, 3.3× faster than MiniSearch.
- `loadJSON` and `loadJSONAsync` read postings straight into the engine's
  lists instead of a JSON value tree: `loadJSON` is about 2.5× faster than
  0.11.0 and 1.8× faster than MiniSearch's. Ids and stored fields are read with
  `JSON.parse`, as upstream, so object ids and duplicate ids load natively.
  `loadJSONAsync` waits for a timer where MiniSearch's does (after every
  1,000th entry of each map and posting list, and every 1,000th term), not
  after every 1,000 steps: where a timer wait is long (about 15 ms on
  Windows) it took 1.7× MiniSearch's time and now takes the same.
- Native snapshots: binary version 5 marks a frequency of 1 in the low bit of
  the posting delta (the benchmark index is 40% smaller) and leaves out the
  engine's placeholder ids. The facade saves the ids and stored fields next to
  the engine snapshot, as JSON (`MSWID01\n` byte envelope; `format:
  "minisearch-wasm/identity"` for `toNativeJSON`), and loading requires the
  `tokenize`/`processTerm` callbacks an index was built with. Snapshots of
  0.9.0 to 0.11.0 still load.
- Short ids are renumbered after a vacuum only once most of them are free, so
  `toJSON()` shows MiniSearch's short ids in ordinary use.
- `searchJoined` maps short ids to ids on the JavaScript side: about 1.3×
  faster than 0.11.0. `toBytes` and `loadBytes` take 1–2 ms longer on 20,000
  documents than in 0.11.0, the ids now travelling as JSON; they remain 65×
  and 10× faster than MiniSearch's JSON.
- The published Wasm module leaves out the engine's own document and search
  bindings, which the facade no longer calls (Cargo feature `core-api`, built
  into `target/pkg-core` for the engine test suites). Sizes, gzipped: Wasm
  313 KB (0.11.0: 302 KB), ESM facade 33 KB (28 KB); the tarball is 469 KB
  (446 KB). `scripts/size-budget.json` records the new sizes.

### Fixed

- `removeAll()` keeps `dirtCount`, as MiniSearch does.
- `toJSON()` follows MiniSearch's key order, and `averageFieldLength` has
  MiniSearch's length and holes (`null` in JSON) for fields no document had.
  After a mutation history, `JSON.stringify(index)` is MiniSearch's, byte for
  byte.
- A field listed twice in the search option `fields` was scored twice; fields
  are now scored once each, in JavaScript object key order.
- A `processTerm` that throws leaves the field it throws in, and the terms
  before, indexed (or removed), as upstream.
- Subclasses: `addAll` calls an overridden `add`, `discardAll` an overridden
  `discard`, `autoSuggest` an overridden `search`, and `loadJSON`/`loadBytes`
  return an instance of the class they are called on.
- A field named like an `Object.prototype` member (`constructor`, `toString`)
  indexes what MiniSearch indexes: extraction and stringification run in
  JavaScript.
- Options explicitly `undefined` for `filter`, `tokenize`, `processTerm` or
  `boostDocument` replace the constructor's default with nothing, as upstream.

## 0.11.0 - 2026-09-19

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

- Native `filter` callbacks run in traversal order before sorting, preserving
  stateful predicates and score mutations without transferring the index.
- Compact searches with stored fields named like result properties evaluate
  queries and callbacks once; native compact searches with a row filter also
  avoid evaluating per-term callbacks twice.
- Native `loadJSONAsync` reconstructs document maps and postings in batches
  with timer yields, instead of loading synchronously after one initial yield.
- `addAllAsync` uses MiniSearch's scheduler, so short batches are deferred too.
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
