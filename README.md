# minisearch-wasm

A Rust + WebAssembly full-text search engine built against the
[MiniSearch](https://github.com/lucaong/minisearch) behavior contract. It
computes **identical rankings** to MiniSearch (same ids, same BM25 scores) and
is **faster than the JavaScript original** on the real workload — verified
continuously by the benchmark in the sibling `keyword-search` project.

> This is an independent WebAssembly reimplementation that is API-compatible
> with MiniSearch. It is **not** affiliated with or endorsed by the original
> [MiniSearch](https://github.com/lucaong/minisearch) project.

The original JavaScript conformance tests are copied under `reference-tests/`,
and Rust integration tests in `tests/` translate them feature by feature. There
are no JavaScript callbacks in the engine — everything runs in Wasm.

## Design: keep the boundary thin

A Wasm search engine lives or dies at the JS↔Wasm boundary. Two rules:

1. **Do all the work in Wasm.** Tokenization, the radix tree, BM25, prefix and
   fuzzy traversal, scoring, and serialization all run in Rust.
2. **Cross the boundary as little as possible.** A query enters as a string plus
   one `orMode` flag — no per-call options object to deserialize. The result set
   leaves as just three values: `scores` (a `Float64Array`, one bulk copy),
   `ids` (a JSON array string), and `terms` (a newline-joined string).
   No per-hit JS object is created on the Rust side.

That is what `searchJoined` does — and `searchRaw` goes further still
(everything numeric: typed arrays plus one small interned term table). These
are the recommended paths for embedding apps. `search()` is also provided for
drop-in MiniSearch compatibility (it returns the full nested per-hit
`{ id, score, terms, queryTerms, match, … }` objects); rebuilding those nested
objects across the boundary costs most of the engine's win, leaving it at
roughly JS parity — real consumers (e.g. the jobboard worker) only ever read
`id`, `score`, and `terms`.

## API

```js
import init, { MiniSearchWasm } from "minisearch-wasm";
await init();

// Build (or load a prebuilt index — far cheaper, see below).
const mini = new MiniSearchWasm({
  idField: "id",
  fields: ["title", "description", "company", "location", "org"],
  tokenizer: "jobboard",                 // or "default"
  searchOptions: { boost: { title: 4 }, prefix: true, fuzzy: 0.2, combineWith: "AND" },
});
mini.addAllJSON(rawJobsJsonText);        // index straight from JSON text, in Wasm

// Fast path — everything in Wasm, minimal boundary crossing:
const r = mini.searchJoined("software engineer", /* orMode */ false);
// r = { count, ids: '["id0","id1"]', scores: Float64Array, terms: "a b\nc" }
const ids   = JSON.parse(r.ids);
const terms = r.count ? r.terms.split("\n") : [];
for (let i = 0; i < r.count; i++) {
  use(ids[i], r.scores[i], terms[i] ? terms[i].split(" ") : []);
}

// …with per-call option overrides (partial, like search()):
const exact = mini.searchJoinedOpts("java", { prefix: false, fuzzy: false });

// Fastest path — everything numeric. Fetch the id table once, then per query
// only typed arrays + one small interned term table cross the boundary:
let idTable = JSON.parse(mini.docIdTable()); // slot n = internal doc id n
let idTableVersion = mini.idTableVersion;
const raw = mini.searchRaw("software engineer"); // optional options 2nd arg
if (raw.idTableVersion !== idTableVersion) {
  idTable = JSON.parse(mini.docIdTable());
  idTableVersion = mini.idTableVersion;
}
// { count, idTableVersion, docIds: Uint32Array, scores: Float64Array, termTable: "a\nb\n…",
//   termOffsets: Uint32Array, termIds: Uint32Array }
const termTable = raw.termTable ? raw.termTable.split("\n") : [];
for (let i = 0; i < raw.count; i++) {
  const terms = [];
  for (let k = raw.termOffsets[i]; k < raw.termOffsets[i + 1]; k++)
    terms.push(termTable[raw.termIds[k]]);
  use(idTable[raw.docIds[i]], raw.scores[i], terms);
}

// Compatibility path — MiniSearch-shaped result objects (slower, see above):
const full = mini.search("software engineer", { combineWith: "AND" });

// Query-expression trees and wildcard, like JS MiniSearch: subqueries combine
// with AND / OR / AND_NOT and nest arbitrarily; a node's other keys override
// the search options for its subtree. `MiniSearchWasm.wildcard` matches every
// document (e.g. "everything except…" via AND_NOT).
const advanced = mini.search({
  combineWith: "AND_NOT",
  queries: [
    { combineWith: "OR", queries: ["designer", { prefix: true, queries: ["develop"] }] },
    "senior",
  ],
});
const everything = mini.search(MiniSearchWasm.wildcard);

// Auto-suggest (search-as-you-type), MiniSearch-compatible: AND + prefix on
// the last term by default; defaults configurable via the constructor's
// `autoSuggestOptions`, overridable per call.
const suggestions = mini.autoSuggest("softw eng", { fuzzy: 0.2 });
// => [{ suggestion: "software engineer", terms: ["software", "engineer"], score: … }, …]

// Non-blocking batch indexing, yielding between chunks:
await mini.addAllAsync(documents, { chunkSize: 100 });

// Discard leaves stale postings. Auto-vacuum is enabled by default and can be
// tuned or disabled in the constructor; manual vacuuming is also asynchronous.
mini.discard(oldDocumentId);
await mini.vacuum({ batchSize: 1000, batchWait: 10 });
console.log(mini.dirtCount, mini.dirtFactor, mini.isVacuuming);

// The rest of the MiniSearch document API:
mini.replace(updatedDocument);           // discard + add, by id field
mini.has(id);                            // true for a live document
mini.getStoredFields(id);                // stored fields object, or undefined
MiniSearchWasm.getDefault("idField");    // "id"; callback defaults come back as functions

// MiniSearch's callback search options have declarative forms here: one entry
// per query term for prefix / fuzzy / boostTerm, and a stored-field filter.
// Passing a function for any of them throws an Error that names the option.
mini.search("softw eng", {
  prefix: [false, true], fuzzy: [0.2, false], boostTerm: [2, 1],
  filter: { company: "ACME" },
});

// Fast path: one string + one Float64Array across the boundary; a suggestion
// row IS its space-joined terms.
const s = mini.autoSuggestJoined("softw eng");
const phrases = s.count ? s.suggestions.split("\n") : [];
for (let i = 0; i < s.count; i++) use(phrases[i], s.scores[i], phrases[i].split(" "));

// Persistence: compact binary snapshot — smaller on the wire than JSON and
// faster to load.
const bytes = mini.toBytes();            // Uint8Array
const loaded = MiniSearchWasm.loadBytes(bytes);

// toJSON / loadJSON use MiniSearch's own JSON format, so JSON.stringify(mini)
// loads in the JavaScript library and vice versa (same constructor options).
const forJs  = MiniSearch.loadJSON(JSON.stringify(mini), options);
const fromJs = MiniSearchWasm.loadJSON(JSON.stringify(jsMiniSearch), options);
const later  = await MiniSearchWasm.loadJSONAsync(JSON.stringify(jsMiniSearch), options);

// The engine's own, smaller JSON snapshot (not readable by the JS library):
const native = mini.toNativeJSONString();
const reloaded = MiniSearchWasm.loadNativeJSON(native);
```

## Upgrading to 0.9.0

This release fixes field-length accounting, binary snapshot traversal order,
snapshot validation, and compact ID encoding, and aligns the JSON API with
MiniSearch. Three migrations are required:

- **`toJSON()` / `loadJSON()` now use MiniSearch's format.** `toJSON()` and
  `toJSONString()` produce what the JavaScript library's `toJSON()` produces
  (so `JSON.stringify(index)` is loadable on either side), and `loadJSON(json,
  options)` takes MiniSearch's signature. The engine's own JSON snapshot moved
  to `toNativeJSON()`, `toNativeJSONString()` and `loadNativeJSON(json)`.

- **Rebuild persisted indexes from the original documents.** Binary snapshots
  now use version 4; JSON snapshots require `snapshot_version: 4`. Older
  snapshots are rejected because they did not preserve field presence, exact
  long-field lengths, or all binary tree ordering information. Do not change
  a snapshot's version number by hand.
- **Decode compact IDs with `JSON.parse`.** Both `searchJoined(...).ids`
  (including `searchJoinedOpts`) and `docIdTable()` are JSON array strings.
  Replace their `.split("\n")` calls with `JSON.parse(...)`. The `terms` and
  `termTable` formats are unchanged. Empty ID results are `"[]"`.

External IDs can be non-null JSON values; numeric and string IDs stay distinct,
and strings can contain newlines, quotes, and backslashes. Array/object IDs use
JSON-value equality in this port. Deleted ID-table slots are `null`; null IDs
are rejected, matching MiniSearch's missing-ID behavior.

`idTableVersion` is an instance-local decimal string. Add, remove, discard and
`removeAll()` change it; each `searchRaw()` result carries the generation used
by that query. Cache an ID table with its generation, refresh it after
mutations, and fetch a new table for every newly loaded engine. Resolve raw
results before subsequently mutating their engine.

Snapshots validate document/field mappings, dimensions, finite statistics,
postings, and radix-tree structure. Dirty indexes may retain stale postings.
Loads reject trailing binary data, integer overflow, invalid UTF-8 and
excessive nesting. Snapshot limits are 256 MiB of input, 1,024 fields,
2,000,000 internal ID slots, 16,000,000 field slots, radix depth 128, and stored
JSON value depth 64. The binary reader also applies a 256 MiB allocation
budget; this is a defensive accounting limit, not a measurement of total
process memory. JSON parsing retains serde_json's default recursion limit.
Snapshots exceeding these limits require splitting or rebuilding the index.

Result ordering and field handling now follow JS MiniSearch more closely:
equal-score results come out in MiniSearch's `Map` order instead of internal
document-id order; a hit's `terms` and `match` keys follow JS `Object.keys`
order (array-index-like terms such as `2024` first); numeric field values and
ids are stringified like JS (`10.0` indexes as `10`, and `1` and `1.0` are the
same id); and prefix/fuzzy term lengths and fuzzy distances count UTF-16 code
units like `String.length`. Indexes with such content rank identically but may
order ties and matched terms differently than 0.8.0 did.

The default tokenizer now includes boundary empty tokens in field-length
statistics, skips null/missing fields, and retains exact lengths above 65,535.
Its separator table is generated from the JavaScript engine's Unicode data
(Node 24, Unicode 17.0) instead of a Rust crate's Unicode 15 tables. The
built-in `jobboard` tokenizer keeps its existing tokenization behavior.

Errors are now `Error` objects with MiniSearch's messages (`e.message`,
`e.stack` and `instanceof Error` work), instead of plain strings. Function-valued
options (`filter`, `boostDocument`, `boostTerm`, `prefix`, `fuzzy`, `tokenize`,
`processTerm`, `extractField`, `stringifyField`) throw instead of being silently
ignored; `logger` is the one callback that is supported. Indexed field values
that are objects (a `Date`, an object with `toString()`, an array containing
objects) are stringified with `String(value)` like MiniSearch's default
`stringifyField`; when such a field is also stored, the stored copy is that
string rather than the original object.

Verify the changes with `cargo test --locked`, `npm run build`, then
`npm run test:wasm` (install `differential/` dependencies first).

## Differences from MiniSearch

This is a from-scratch reimplementation. It matches MiniSearch's **ranking** —
identical BM25 scores, verified to float epsilon on a 21k-document corpus
(`max delta ≈ 1e-14`) and on a 3k-document differential suite covering
search + autoSuggest before and after removes/discards (`max delta ≈ 5e-16`,
i.e. last-ulp `Math.log` vs `ln` rounding; per-document matched-term order
identical, since the radix tree replicates JS MiniSearch's key order and
traversal exactly) — but deliberately diverges from its API and internals:

- **No per-document or per-token JavaScript callbacks.** MiniSearch takes
  user functions for `extractField`, `stringifyField`, `tokenize`,
  `processTerm`, and search-time `filter` / `boostDocument` / `boostTerm` /
  `prefix` / `fuzzy`. To keep the JS↔Wasm boundary coarse, this port rejects
  them with an `Error` naming the option and offers declarative forms instead:
  per-term arrays for `prefix`, `fuzzy` and `boostTerm`, and a `filter` object
  of stored-field (or id) values that must match. Tokenization and term
  processing are built-in modes (`"default"`, `"jobboard"`), fields are read as
  `document[field]`, and object values are stringified with `String(value)`.
  `logger(level, message, code)` is supported; it receives the same
  `version_conflict` warning JS emits when a changed document is removed.
- **JSON is interoperable; the binary snapshot is not.** `toJSON()` and
  `loadJSON(json, options)` use the JavaScript library's format (serialization
  versions 1 and 2), so indexes move between the engines with the same options.
  Like JS `loadJSON`, loading re-inserts terms in serialized order, so a
  reloaded index may order equal scores and matched terms differently from the
  live one; scores and result sets are unchanged. `toBytes`/`loadBytes` (a
  compact validated binary snapshot) and `toNativeJSON`/`loadNativeJSON` are
  this crate's own formats.
- **Search never mutates the index.** MiniSearch lazily removes stale postings
  mid-query when it meets a discarded document; this port just skips them (and
  skips the liveness check entirely on a clean index). Explicit and automatic
  vacuuming remove those stale postings in asynchronous Wasm-side batches.
- **Added beyond MiniSearch.** `searchJoined` / `searchJoinedOpts` /
  `autoSuggestJoined` (compact columnar results for a thin Wasm boundary),
  `searchRaw` with `docIdTable` / `idTableVersion` (fully numeric results),
  `addAllJSON` (index straight from a raw JSON string), `toBytes`/`loadBytes`
  and `toNativeJSON`/`loadNativeJSON` (the engine's own snapshots), the
  `"jobboard"` tokenizer, `search`'s `includeMatch: false` option, the
  declarative `prefix`/`fuzzy`/`boostTerm` arrays and `filter` object, and —
  internally, with identical results — a bit-parallel (Myers) fuzzy traversal
  and flat sorted posting lists.

## Benchmark results

The figures below were measured on 0.8.0 with the external jobboard
benchmark. A 0.9.0 check with the checked-in `differential/bench_wasm.mjs`
(20,000 synthetic documents, 38 queries, medians of 9 rounds) against the
0.8.0 build on the same machine showed `searchJoined` and `autoSuggest` 10-20%
faster (run-to-run variance is about 5%), `toBytes` about 30% faster,
`loadBytes` within 5% (the snapshot is now validated while it is decoded) and
a 3% smaller binary snapshot. The download row was measured on the version 3
snapshot format and has not been re-measured for version 4. The compat row is
from 0.9.0: `search()` now hands ids, scores and interned term tables to one
JavaScript function that builds the result objects natively, instead of one
boundary call per property per hit (it measured 0.58× before that change).

Measured by `keyword-search/scripts/bench-minisearch-vs-wasm.mjs` on the real
jobboard corpus (~22k documents, 30 representative queries), comparing against
JS MiniSearch. Search ratios are reported by median (robust to system noise);
expect some run-to-run variance.

| Category | Rust vs JS MiniSearch |
|---|---|
| **Search — app workload** (`{id, score, terms}` via `searchJoined`) | **~11× faster** (median; mean ~7×) |
| **Search — same workload via `searchRaw`** | **~23× faster** (median; mean ~16×) |
| **Index download** (prebuilt, brotli) | **~0.73× — smaller on the wire than JS** |
| Load prebuilt index — `loadBytes` vs `loadJSON` | ~13× faster |
| Serialize index — `toBytes` vs `JSON.stringify` | ~12× faster |
| Build index — `addAllJSON` vs `addAll` | ~2.9× faster |
| Full compat `search()` vs JS `search()` | ~1.7× faster (~2.1× with `includeMatch: false`; 0.9.0, 20k synthetic documents, `differential/bench_wasm.mjs`) |

Search-as-you-type re-issues the same committed terms every keystroke; the
engine memoizes prefix/fuzzy tree expansions (bit-identical replay,
invalidated on any mutation), which is where most of the search speedup comes
from on repeated queries. Cold first-run queries are ~2-3× faster than JS.

The synthetic maintenance benchmark (`npm run bench:maintenance`, 5,000
documents, 500-document async chunks) measured `addAllAsync` at 2.89× the
synchronous Wasm indexing time, reflecting its deliberate event-loop yields.
Vacuuming 1,250 discarded documents took 122 ms and reduced the binary snapshot
by 20%. These timings are workload- and machine-dependent; the benchmark always
verifies post-maintenance search parity and dirt cleanup.

The compact binary snapshot is delta+varint encoded, so it is both smaller than
the JSON index and low-entropy enough to compress well. Since version 4 it
stores the radix tree node by node, so loading is a single validated pass with
no tree rebuild (about 13× faster than JS `loadJSON` in the table above).

**Truthfulness:** the benchmark verifies the two engines return identical
results — `set-identical 30/30`, `max score delta ≈ 1e-14` (float epsilon),
`0` real ranking bugs. Since 0.9.0 the result order matches JS exactly, ties
included, as does the per-hit `terms`/`match` order;
`differential/compat_parity.mjs` checks this against the JS engine across
fresh, dirty and vacuumed indexes.

## Install

```sh
npm install minisearch-wasm
```

The package is a `wasm-pack --target web` build: it loads with a static import
in every environment — Vite, webpack, Turbopack (dev and build), plain ESM,
Node, and Web Workers — with one explicit `await init()` to load the
WebAssembly module up front:

```js
import init, { MiniSearchWasm } from "minisearch-wasm";

await init();                              // load the .wasm once, before first use

const ms = new MiniSearchWasm({ fields: ["title", "text"] });
ms.addAll(documents);
const results = ms.search("query");
```

In a **Web Worker** the same static import works — just `await init()` before
the first call. (The `--target web` build is what makes worker and Turbopack use
friction-free; a bundler-target build would require a dynamic `import()` there.)

In **Node** the same `await init()` works: the package's `node` export
condition selects `minisearch_wasm_node.js`, which reads the packaged `.wasm`
itself (the web loader would try to `fetch` a `file:` URL). Browser bundles
never see that entry or its `node:fs` import. If you import
`minisearch-wasm/minisearch_wasm.js` directly in Node, pass the bytes:

```js
import { readFileSync } from "node:fs";
import init, { MiniSearchWasm } from "minisearch-wasm/minisearch_wasm.js";

await init({
  module_or_path: readFileSync(
    new URL(import.meta.resolve("minisearch-wasm/minisearch_wasm_bg.wasm"))
  ),
});
```

The package ships typed declarations for its whole surface
(`MiniSearchWasmOptions`, `SearchOptions`, `Query`, `SearchResult`,
`Suggestion`, `JoinedResults`, `RawResults`, `MiniSearchJSON`, …) with optional
arguments marked optional; `types/fixtures.ts` compiles the README examples
under `strict` with TypeScript 7 as part of the checks below.

## Build

```powershell
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
rustup target add wasm32-unknown-unknown
npm install               # TypeScript 7 for the type fixtures
npm run build             # wasm-pack build --target web --release + node entry + metadata
npm run test:wasm         # smoke, regression, JS-parity and API-parity suites against pkg/
npm run test:types        # strict TypeScript fixtures against the generated declarations
npm run check:separators  # tokenizer table still matches this Node's Unicode data
```

Install `wasm-pack` with `cargo install wasm-pack` if needed. `npm run
test:wasm` compares against the JavaScript engine, so run `npm install` in
`differential/` once. After a Node upgrade, regenerate the tokenizer's separator
table with `node scripts/gen-separators.mjs` (it writes `src/separators.rs`).

### Publishing

```powershell
npm run publish:pkg   # rebuilds, then `npm publish ./pkg`
```

You must `npm login` first. The publishable artifact is the generated `pkg/`
directory; its `package.json` metadata is injected by `scripts/finalize-pkg.mjs`
on every build.

See `PORTING.md` for the porting history and conformance status, and
`IMPROVEMENTS.md` for the open review items.
