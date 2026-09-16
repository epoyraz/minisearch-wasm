# Porting Tracker

The original MiniSearch Jest tests live in `reference-tests/` and are the
behavior contract for this port. Each Rust test added under `tests/` should link
back to one or more behaviors from those files. The 0.9.0 sections come first;
the sections after them are the porting history in chronological order, each
describing its own release, and a later section supersedes an earlier one where
they disagree. Open work is tracked in `IMPROVEMENTS.md`.

## 0.9.0 correctness fixes

- Field statistics preserve boundary empty tokens, field presence, and exact
  `u32` lengths; null/missing fields are skipped on add/remove/discard.
- Version 4 binary snapshots encode radix nodes with ordered children and
  explicit leaf positions. Prefix/fuzzy match order and suggestions survive
  saves and reloads, including after mutations.
- JSON snapshots are versioned. Both readers validate state before returning an
  engine; binary reads check bounds, overflow, allocation budgets, depth,
  UTF-8, duplicate IDs/fields, and complete input consumption.
- Compact IDs are JSON array strings, decoded using `JSON.parse`, with typed
  values and escaped delimiters. ID tables use null holes and an instance-local
  `idTableVersion`, also included in raw results.

See README's upgrade section for the required index rebuild and ID-decoding
changes. Regression coverage is in `tests/high_priority_regressions.rs` and
`differential/high_priority_regressions.mjs`; the latter compares the rebuilt
WASM package with JS MiniSearch on sparse fields, long fields and maintenance.

Current gate: `cargo fmt -- --check`, `cargo clippy --locked --all-targets --
-D warnings`, `cargo test --locked` (66 tests), `npm run build`, then
`npm run test:wasm` (smoke, high-priority regressions, JS parity and API parity
through the built package), `npm run test:types` (strict TypeScript 7 fixtures)
and `npm run check:separators`.

## 0.9.0 performance work

Measured with `differential/bench_wasm.mjs` on 20,000 synthetic documents
against the 0.8.0 build (medians of 9 rounds, same machine): `searchJoined`
124 -> 113 ms, `autoSuggest` 48 -> 44 ms, `toBytes` 6.9 -> 4.2 ms, `loadBytes`
15.9 -> 17.1 ms, binary snapshot 3% smaller. Ranking output is unchanged; the
smoke and regression suites compare every result format against JS MiniSearch.

- The expansion cache stores an `Rc` handle to each derived term's postings,
  so replaying a cached prefix/fuzzy list needs no radix-tree lookups. Every
  mutation path clears the cache before touching the tree, so `Rc::make_mut`
  mutates in place.
- Per-term postings are a `Vec` sorted by field id instead of a `HashMap`:
  one allocation per term and a short scan per lookup. The JSON layout is
  unchanged.
- A dense `alive` table replaces the per-posting `HashMap` liveness probe on
  dirty indexes, in search and in vacuum.
- The fused accumulator remembers the last derived term per document, so the
  matched-term membership scan runs once per (document, term) rather than
  once per posting.
- The binary reader validates inline while decoding (no second pass), decodes
  varints off the slice with one bounds check each, and stores external IDs
  and stored fields as tagged binary values instead of JSON text. `to_bytes`
  validates only in debug builds, and debug builds cross-check the binary
  reader against the validator so the test suite catches any drift.
- `differential/bench_wasm.mjs` frees the instances it builds and loads. The
  leak had inflated `loadBytes` from about 16 ms to about 257 ms and made the
  0.8.0 build look slower than JS `loadJSON`.

## 0.9.0 MiniSearch parity

Behaviors aligned with JS MiniSearch at no query-time cost. All are verified
against the JS engine by `differential/compat_parity.mjs` (exact result order,
`terms`, `match`, suggestions and scores across fresh, discarded, removed and
vacuumed indexes; 102,060 checks) and natively by `tests/minisearch_compat.rs`.

- **Tie order.** Results with equal scores come out in MiniSearch's `Map`
  order: `OR` and `AND_NOT` keep first-touch order, `AND` follows the last
  term's order, and the final sort is stable by score. The fused path reads
  the order it already tracks; the compatibility path uses an
  insertion-ordered result map. The former document-id tiebreak is gone.
- **`terms` and `match` order.** Both follow JS `Object.keys(match)`:
  array-index-like terms (`2024`, `7`) first in ascending order, then the rest
  in first-match order, in every result format including suggestions.
- **Number formatting.** Field values and ids stringify like JS
  `String(value)`: `10.0` indexes as `10`, `1e21` as `1e+21`, and `1` and
  `1.0` are the same id. serde_json's `float_roundtrip` feature makes decimals
  in JSON text parse with the same rounding as `JSON.parse`.
- **UTF-16 lengths.** Prefix/fuzzy term lengths and fuzzy edit distances count
  UTF-16 code units like `String.length`, so astral characters count as two.
  Unchanged for BMP text.
- **API.** `has(id)`, `getStoredFields(id)` and `replace(document)` are on the
  Wasm class.
- **Interop.** `loadMiniSearchJSON(json, options)` loads the JavaScript
  library's `toJSON()` output (serialization versions 1 and 2) and
  `toMiniSearchJSON()` produces it. Like JS `loadJSON`, loading re-inserts
  terms in serialized order, so a reloaded index orders ties and matched terms
  like a JS instance loaded from the same JSON rather than like the live one;
  scores and result sets are identical either way.

Benchmarked after these changes on the same corpus: nothing measured slower
than before them; `searchJoined` and `autoSuggest` medians moved within
run-to-run variance.

## 0.9.0 API parity (second review, items 1-7 and 10)

Verified against the JS engine by `differential/api_parity.mjs` (127 checks),
natively by `tests/minisearch_options_compat.rs`, by the strict TypeScript
fixtures in `types/`, and by a scratch `npm install` of the packed package in
Node.

- **Errors.** Every failure is a JS `Error` with MiniSearch's message (the
  option-parsing errors were translated: `MiniSearch: option "fields" must be
  provided`, `Invalid combination operator: X`; `removeAll` distinguishes a
  falsy argument from a non-iterable one like JS).
- **Callback options.** Function-valued `filter`, `boostDocument`, `boostTerm`,
  `prefix`, `fuzzy`, `tokenize`, `processTerm`, `extractField` and
  `stringifyField` throw an `Error` naming the option and the alternative, in
  the constructor, per call, in query-tree nodes and in `autoSuggest`.
  Declarative forms: per-term arrays for `prefix` / `fuzzy` / `boostTerm`
  (`PrefixSetting::PerTerm`, `FuzzySetting::PerTerm`, `boost_term`) and a
  `filter` object of stored-field or id values, applied on every path.
- **Field values.** Documents cross the boundary through a schema-driven
  conversion: indexed object values (a `Date`, an object with `toString()`, an
  array containing objects) become `String(value)` on the JS side; primitives,
  ids and stored-only values stay JSON (a stored `Date` becomes its
  `JSON.stringify` form); function-valued properties are skipped.
- **Unicode.** `src/separators.rs` is generated by `scripts/gen-separators.mjs`
  from the running Node's `\p{Z}`/`\p{P}` (Unicode 17.0 for Node 24) and
  replaces the `unicode-general-category` crate (Unicode 15).
- **API.** `getDefault`, `loadJSONAsync`, `logger` (called with
  `version_conflict` warnings, defaulting to `console[level]`), and the JSON
  rename: `toJSON`/`toJSONString`/`loadJSON` are MiniSearch's format,
  `toNativeJSON`/`toNativeJSONString`/`loadNativeJSON` the engine's.
- **Types.** wasm-bindgen `unchecked_param_type`/`unchecked_return_type`
  annotations plus a `typescript_custom_section` describe options, queries and
  results; the seven methods with optional arguments are declared through
  interface merging because the annotation drops the `?`.
- **Node.** `scripts/finalize-pkg.mjs` writes `minisearch_wasm_node.js` and a
  conditional `exports` map (`node` → the entry that reads the `.wasm` bytes,
  `default` → the web loader), so a bare `init()` works everywhere.
- **Compat `search()`.** The engine returns a `CompatTransfer` (ids as JSON,
  scores, interned term/field tables with offsets, stored fields as JSON) and
  one cached JavaScript function builds the MiniSearch-shaped objects. On the
  20k synthetic corpus: 304 ms vs 502 ms in JS (0.58× → ~1.7×), 236 ms with
  `includeMatch: false`.

## Verified Initial Slice

Command:

```powershell
cargo fmt -- --check
cargo test
cargo build --target wasm32-unknown-unknown
wasm-pack build --target web
```

Status: passing, 8 tests, native Rust build, Wasm target build, and web package
build.

Covered:

- `SearchableMap` set/get/has/delete
- `SearchableMap` prefix lookup
- `SearchableMap` fuzzy lookup
- `MiniSearch` add/addAll
- stored fields in search results
- field boosts
- selected search fields
- prefix search
- fuzzy search
- OR, AND, and AND_NOT result combination
- remove, discard, and replace

## Performance + truthfulness pass

The engine is now faster than JS MiniSearch on the real `keyword-search` corpus
(~21k docs) while returning identical rankings. Verified by
`keyword-search/scripts/bench-search-engines.mjs` (`set-identical 30/30`,
max score delta ≈ 1e-14, 0 real ranking bugs). Key changes:

- **Thin boundary `searchJoined`** — query string + `orMode` flag in; result set
  out as a `Float64Array` of scores plus newline-joined `ids`/`terms` strings.
  No options object deserialized per call, no per-hit JS object built.
- **Dropped the `serde_json::Value` double-conversion** in every Wasm result
  path (was serializing each result tree twice).
- **Scoring bug fixed (correctness):** prefix/fuzzy term weights used UTF-8 byte
  length; MiniSearch uses `String.length` (code units). Now uses char count, so
  scores match for multi-byte terms (umlauts/accents). This was the source of a
  ~0.8 max score divergence the benchmark caught.
- **Hot-path:** skip the per-posting liveness check on a clean index
  (`dirtCount == 0`); two-phase `add()` avoids cloning the field list per
  document; `HashMap`/`HashSet` hot maps; `Float64Array` result scores.
- **Build profile:** `lto = true`, `codegen-units = 1`, `opt-level = 3`.

Benchmark snapshot (median, vs JS MiniSearch): search app-workload ~1.5×,
`loadBytes` ~8×, `toBytes` ~12×, `addAllJSON` ~1.3×. Full MiniSearch-compatible
`search()` stays ~0.5× and is kept only for compatibility — see README "Design".

## autoSuggest + exact tree-order parity

`autoSuggest(query, options?)` is ported (issue #2): terms combine with `AND`
and only the last term is prefix-expanded by default; ranked results are
grouped by matched-terms phrase with scores averaged per phrase. Defaults are
configurable via the constructor's `autoSuggestOptions` (persisted in
snapshots) and overridable per call. `autoSuggestJoined(query)` is the
boundary-frugal variant (`{ count, suggestions, scores }`, one string + one
`Float64Array`). The JS `filter` callback option is intentionally not ported
(no JS callbacks in the engine). Rust conformance tests cover the whole
reference `autoSuggest` describe block except the `filter` case.

Because a suggestion's phrase is the document's matched terms **in match
order**, this work also made the radix tree replicate JS MiniSearch's key
order and traversal exactly:

- node key order = insertion order; an edge split re-inserts the shared prefix
  at the END (JS `Map.set` + `Map.delete`), delete-side merges likewise; the
  leaf occupies a real position in the key order (`leaf_pos`).
- prefix/entries traversal consumes keys from the END (JS `TreeIterator`);
  fuzzy traversal walks keys FORWARD (JS `fuzzySearch`).

This makes per-document matched-term order identical to JS and tightens score
parity from ~1e-14 to last-ulp `Math.log`-vs-`ln` differences (~5e-16
relative). Verified by a 3k-doc differential suite (fresh + after
remove/discard; search × 6 option combos + autoSuggest × 3 + query trees × 2):
750 labels, ~400k result rows, all matching. On dirty indexes JS is compared
at its post-lazy-cleanup fixpoint (2nd run of each query), since this port
deliberately never mutates the index during search.

## Intentional API Direction

This port is all Rust at the engine level. It does not preserve JavaScript
callback hooks such as `extractField`, `tokenize`, `processTerm`, `filter`,
`boostTerm`, or `boostDocument`. Those behaviors should become Rust-side config,
query DSL features, or explicit preprocessing steps.

The Wasm search surface is `search` (MiniSearch-compatible objects),
`searchJoined` / `searchJoinedOpts` (compact columnar results) and `searchRaw`
(fully numeric; see the 0.8.0 section). The earlier `searchCompact` /
`searchPacked` variants were removed once `searchJoined` superseded them.

## Query-Expression Trees and Wildcard

`search` accepts the full JS `Query` type: a string, the wildcard, or a
combination node `{ combineWith?, queries: [...], ...optionOverrides }` with
arbitrary nesting. The Rust side mirrors `executeQuery` exactly: node options
are *partial* (`PartialSearchOptions`) and cascade via the JS
`{...inherited, ...node}` spread; the constructor's `searchOptions` merge in
only at string leaves — so, as in JS, a combination node without its own
`combineWith` combines subqueries with `OR` even when the constructor default
is `AND`. The wildcard is `MiniSearchWasm.wildcard`, a registered symbol
(`Symbol.for`), matching every live document with score 1; a top-level
wildcard returns document-insertion order unsorted, like JS. The `filter` /
`boostDocument` / `boostTerm` callbacks are not ported (no-callbacks rule).

Per-call `search` options are now also truly partial: only keys present on the
options object override the constructor's `searchOptions`, matching JS (an
options object like `{ fuzzy: 0.2 }` no longer resets `combineWith`/`prefix`
to library defaults).

Until 0.9.0, per-result `terms` / `match` keys on this path came from a sorted
map while JS preserves insertion order; since 0.9.0 both follow JS
`Object.keys` order (see the 0.9.0 parity section). The bulk differential still
compares tree-query terms as sets and everything else (ids, scores, both fresh
and mutated indexes) exactly.

## Batch Removal and Discard

`removeAll(documents)` and `discardAll(ids)` call the corresponding single-item
operation in order, matching MiniSearch's partial-mutation behavior when an
item fails. Calling `removeAll()` through Wasm reconstructs a fresh engine with
the same options, including resetting short document IDs and dirty-index state.
As required by the Wasm boundary, an explicit `undefined` is treated like an
omitted argument; any other non-array value throws MiniSearch's documented
missing-documents error.

## Vacuuming, Auto-Vacuum, and Async Indexing

`vacuum`, `dirtCount`, `dirtFactor`, and `isVacuuming` are implemented through
the real Wasm boundary. The Rust engine exposes synchronous `vacuum()` plus
incremental `vacuum_step(batch_size)`; Wasm drives those steps between
`setTimeout` waits so large cleanups do not monopolize the browser thread.
Discards made during a run remain counted as new dirt, matching MiniSearch's
`initialDirtCount` behavior. Concurrent vacuum requests are coalesced into the
active promise and cause one follow-up pass.

`autoVacuum` defaults to MiniSearch's settings (`minDirtCount: 20`,
`minDirtFactor: 0.1`, `batchSize: 1000`, `batchWait: 10`), accepts `false`,
`true`, or a partial options object, and is triggered once after `discardAll`.
Native Rust auto-vacuum runs synchronously; Wasm schedules the same cleanup
incrementally.

`addAllAsync(documents, { chunkSize? })` defaults to chunks of 10, yields to the
event loop before each full chunk, and preserves earlier chunks if a later
document errors, like JS MiniSearch.

## Flat Postings, Expansion Cache, and the Raw Boundary (0.8.0)

Three performance changes, all verified bit-exact by the full differential
(fresh, mutated, batch-mutated, and vacuumed phases — ALL MATCH, max score
delta unchanged at ~5.5e-16):

- **Flat sorted posting lists.** Postings are `Vec<(docId, freq)]` sorted by
  doc id instead of a per-(term, field) `HashMap` — linear scoring loops,
  snapshots write without re-sorting and read with a push loop (no hash-table
  builds on load), one allocation per list. Scores are unchanged because each
  document receives exactly one contribution per posting list, so per-document
  float sum order is unaffected. JSON round-trip shape is preserved via a
  custom `{docId: freq}` map (de)serializer.
- **Prefix/fuzzy expansion cache.** Radix-tree expansions (the dominant query
  cost — fuzzy traversal was ~86% of engine time on the jobboard corpus) are
  memoized per term (+ max distance for fuzzy) in traversal order, with
  weights recomputed from stored lengths, so replayed queries are
  bit-identical. Any index mutation clears the cache; strictly only added
  terms could invalidate a stale list (deleted terms fail their `index.get`
  replay harmlessly), but clearing on every mutation keeps the invariant
  trivial. Search-as-you-type repeats committed terms each keystroke, so this
  collapses warm-query engine time (~7× on the jobboard corpus). Since 0.9.0
  the cache entries hold handles to the postings, so the cache is cleared
  before every mutation instead of after it (see the 0.9.0 performance
  section).
- **`searchRaw` / `searchJoinedOpts` / `docIdTable`.** `searchJoinedOpts`
  adds partial per-call option overrides to the joined fast path (e.g. exact
  whole-token lookups with `{prefix: false, fuzzy: false}`). `searchRaw` is
  the fully numeric boundary: short doc ids + scores + matched-term ids into a
  per-query interned term table, resolved against the one-time `docIdTable`
  (newline-joined external ids in short-id order; mutations require
  re-fetching the table). Each distinct derived term crosses the boundary once
  per query instead of once per hit. Since 0.9.0 the table is a JSON array
  with `null` holes, paired with an `idTableVersion` generation.

## Additional Conformance Hardening

The remaining planned coverage slices are now present:

- Rust JSON and compact-binary round trips preserve dirty state, auto-vacuum
  settings, stored fields, and search results; malformed and incompatible
  snapshots are rejected.
- Default tokenization covers diacritics, contiguous punctuation, Cyrillic,
  Japanese, Greek, Arabic, and numeric terms from the JS reference suite.
- The reference movie and song ranking fixtures verify exact result ordering
  across exact, fuzzy, prefix, and multi-term searches.
