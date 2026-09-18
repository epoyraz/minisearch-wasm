# minisearch-wasm

A Rust + WebAssembly full-text search engine with a compatibility facade for
[MiniSearch](https://github.com/lucaong/minisearch) 7.2.0. Ordinary JSON
documents and declarative searches use Wasm. JavaScript callbacks, object
identity and dirty-index queries use a bundled, pinned MiniSearch implementation.
Check `index.executionMode` to see which engine owns the index.

Version **0.10.0** adds the public compatibility facade.
See [COMPATIBILITY.md](COMPATIBILITY.md) for the API, import formats, engine
selection and persistence limitations.

> This is an independent WebAssembly reimplementation that is API-compatible
> with MiniSearch. It is **not** affiliated with or endorsed by the original
> [MiniSearch](https://github.com/lucaong/minisearch) project.

The original JavaScript conformance tests are copied under `reference-tests/`,
and Rust integration tests in `tests/` translate them feature by feature. The
native engine has no per-token JavaScript callbacks. The public facade accepts
the original callbacks through its JavaScript execution mode.

## Migrating from MiniSearch

Replace the import. The documented MiniSearch 7.2.0 API, callbacks included,
keeps working:

```js
// before
import MiniSearch from "minisearch";
// after
import MiniSearch from "minisearch-wasm";
```

In a browser or module Worker, also call `await MiniSearch.init()` once at
startup. Without it every index runs on the bundled JavaScript engine, which is
fully compatible but no faster. Node loads the Wasm module on import.

**Whether it gets faster depends on how you use it.** An index runs in Wasm
while its options are declarative and its documents hold plain values (string,
number and boolean fields and ids). It moves to the JavaScript engine, once and
permanently, when it meets something only JavaScript can do:

- a callback option: `processTerm`, `tokenize`, `extractField`,
  `stringifyField`, `filter`, `boostDocument`, `boostTerm`, or `prefix` / `fuzzy`
  given as functions (the declarative per-term arrays and the `filter` object
  stay in Wasm);
- `Date`, array or object field values, or object ids;
- `getStoredFields(id)`;
- `loadJSON` / `loadJSONAsync` (load prebuilt indexes with `loadBytes` to stay
  in Wasm);
- a vacuum, or the first search or suggestion after a `discard` or `replace`
  while discarded postings remain.

The last point matters most in practice: an application that updates documents
regularly will run mostly on the JavaScript engine, at about the original
speed. `index.executionMode` reports `"wasm"` or `"javascript"`; after a
`discard` it changes at the first query, not at the `discard` itself.

### Measured speedups

Speedup over MiniSearch 7.2.0 (its time divided by this package's; higher is
faster). 20,000 synthetic documents, 38 prefix + fuzzy `AND` queries, medians of
9 rounds, two runs per engine in separate processes, Node 24 on Windows. The historical
comparison below times compact API calls without the complete decoding work
measured in the newer paired report.
`addAllJSON`, `searchJoined` and `searchRaw` have no MiniSearch counterpart and
are compared with the MiniSearch call that does the same job.

| Operation, clean index | MiniSearch 7.2.0 | 0.8.0 | 0.9.0 | 0.10.0 |
| --- | --- | --- | --- | --- |
| `addAll` from JS objects | 1.0× (708 ms) | 1.9× | 1.9× | 1.7× |
| `addAllJSON`, vs `addAll` | 1.0× (708 ms) | 2.1× | 2.2× | 2.0× |
| `search()` full result objects | 1.0× (419 ms) | 0.46× | 1.5× | 1.4× |
| `searchJoined`, vs `search()` | 1.0× (419 ms) | 6.7× | 8.6× | 8.2× |
| `searchRaw`, vs `search()` | 1.0× (419 ms) | 11× | 15× | 15× |
| `autoSuggest` | 1.0× (312 ms) | 6.8× | 8.5× | 8.2× |
| Serialize, `toBytes` vs `JSON.stringify` | 1.0× (330 ms) | 53× | 79× | 77× |
| Load, `loadBytes` vs `loadJSON` | 1.0× (193 ms) | 13× | 12× | 11× |
| Snapshot size | 1.0× (8,101 KB) | 4.4× smaller | 4.6× smaller | 4.6× smaller |

So swapping the import alone makes `search()` about 1.4× and `autoSuggest`
about 8× faster on an eligible index. The larger gains need code changes:
`searchJoined` and `searchRaw` return compact result shapes (see the API
section), and `loadBytes` replaces `loadJSON`.

After one `discard`, before a vacuum, this build answers from the JavaScript
engine, so the advantage disappears for the lifetime of that instance. 0.9.0,
which has no fallback, keeps its speed:

| Operation, dirty index | MiniSearch 7.2.0 | 0.9.0 | 0.10.0 |
| --- | --- | --- | --- |
| `search()` | 1.0× (430 ms) | 1.5× | 1.2× |
| `searchJoined`, vs `search()` | 1.0× (430 ms) | 8.2× | 1.05× |
| `autoSuggest` | 1.0× (319 ms) | 8.4× | 1.2× |

The facade also costs size: the package is 1.3 MB unpacked (0.9.0: 820 KB,
0.8.0: 631 KB), because it ships the Wasm engine, the pinned JavaScript engine
and ESM, CommonJS and browser-global builds. These numbers are workload- and
machine-dependent; measure your own corpus before relying on a ratio.

## Design: keep the boundary thin

A Wasm search engine's performance depends on its JS↔Wasm boundary. In Wasm mode:

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
MiniSearch-shaped results (it returns the full nested per-hit
`{ id, score, terms, queryTerms, match, … }` objects); rebuilding those nested
objects costs more than the compact formats. These performance properties
apply to Wasm mode; a compatibility transfer pays a one-time index conversion
cost and subsequent operations run in JavaScript. The two indexes are not kept
in sync, and an index does not switch back automatically.

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
mini.addAllJSON(rawJobsJsonText);        // inspect values, then bulk-index in Wasm when eligible

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
// Functions are also accepted; they switch this index to JavaScript mode.
mini.search("softw eng", {
  prefix: [false, true], fuzzy: [0.2, false], boostTerm: [2, 1],
  filter: { company: "ACME" },
});

// Fast path: one string + one Float64Array across the boundary; a suggestion
// row IS its space-joined terms.
const s = mini.autoSuggestJoined("softw eng");
const phrases = s.count ? s.suggestions.split("\n") : [];
for (let i = 0; i < s.count; i++) use(phrases[i], s.scores[i], phrases[i].split(" "));

// Persistence: version-4 binary in Wasm mode; a tagged JSON envelope in
// JavaScript mode. Supply callback options again when loading that envelope.
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

## Upgrading to 0.10.0

The default export is now the MiniSearch-compatible constructor. Existing
`import init, { MiniSearchWasm }` initialization remains supported. Callbacks,
JavaScript values, dirty queries and maintenance use the bundled JavaScript
engine; a transfer is permanent for that instance and can block on large indexes.
Use `executionMode` to observe it. Native version-4 snapshots from 0.9.0 remain
readable; JavaScript-mode snapshots use a distinct compatibility envelope.
See [COMPATIBILITY.md](COMPATIBILITY.md) for all migration details and limitations.

## Upgrading from versions before 0.9.0

Version 0.9.0 fixed field-length accounting, binary snapshot traversal order,
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

In the published 0.9.0 native API, external IDs are non-null JSON values and
array/object IDs use JSON-value equality. The 0.10.0 facade instead
preserves JavaScript identity for object IDs and stored values by selecting
JavaScript mode. Numeric and string IDs stay distinct. Compact result formats
still encode IDs as JSON; use full `search()` for object identities or values
that cannot round-trip through JSON. Deleted ID-table slots are `null`.

`idTableVersion` is an instance-local decimal string. Add, remove, discard and
`removeAll()` change it; each `searchRaw()` result carries the generation used
by that query. Cache an ID table with its generation, refresh it after
mutations or compaction, and fetch a new table for every newly loaded engine. Resolve raw
results before subsequently mutating their engine.

Native version-4 snapshots validate document/field mappings, dimensions, finite statistics,
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

Errors are `Error` objects with MiniSearch's messages (`e.message`, `e.stack`
and `instanceof Error` work). The published 0.9.0 native API rejected most
callbacks and copied stored object values. The 0.10.0 facade accepts
callbacks and preserves live stored-field references, including Dates and
objects, through its JavaScript engine.

Verify the changes with `cargo test --locked`, `npm run build`, then
`npm run test:wasm` (install `differential/` dependencies first).

## Compatibility and extensions

The public facade supports MiniSearch 7.2.0's documented methods, callbacks,
query trees, wildcard, default constructor, generic TypeScript types and
`SearchableMap` subpath. [COMPATIBILITY.md](COMPATIBILITY.md) lists the execution
modes and remaining limitations.

- **JavaScript semantics:** callback configuration, non-scalar indexed/stored
  values, object IDs, `getStoredFields()`, dirty searches and vacuum select the
  pinned JavaScript engine. A transfer preserves radix traversal order. Its
  first dirty query performs the same lazy cleanup as MiniSearch.
- **Asynchronous work:** `addAllAsync` converts documents within each chunk;
  `loadJSONAsync` uses MiniSearch's yielding loader. Parsing the JSON string
  itself is synchronous, as in MiniSearch.
- **Persistence:** `toJSON`/`loadJSON` use MiniSearch's format. Native version-4
  binary and JSON snapshots retain the fast native loading path. JavaScript
  indexes use a tagged compatibility envelope instead; callbacks must be
  supplied again and JSON cannot preserve object identity or prototypes.
- **Maintenance:** active and queued vacuum promises follow MiniSearch's
  completion boundaries. Once vacuum finishes cleanly, the facade compacts
  internal IDs. `compact()` also reclaims clean native ID tables and scratch
  buffers; it changes `idTableVersion`. Wasm linear memory need not shrink.
- **Extensions:** compact `searchJoined`/`searchRaw` results, `addAllJSON`,
  native snapshots, the `jobboard` tokenizer, `includeMatch: false`, declarative
  per-term arrays and equality filters remain available. Compact results
  require JSON-safe IDs and delimiter-safe terms; use full results otherwise.

## Benchmark results

Current measurements of this build against MiniSearch, 0.8.0 and 0.9.0,
including JavaScript execution mode on a dirty index, are in
[Migrating from MiniSearch](#migrating-from-minisearch) above.

The [paired public-package report](https://github.com/epoyraz/minisearch-wasm/blob/v0.10.0/differential/results/2026-09-18-public-vs-original.md)
also measures the first query batch, compact-result decoding, and the one-time
transfer to JavaScript. Median steady-state query timings do not represent that
transfer cost. On that paired run, full `search()` was 1.42x faster, decoded
`searchJoined` 4.48x and decoded `searchRaw` 13.18x. The first callback or dirty
query took 1.28-1.59 seconds including transfer on 20,000 documents. Subsequent
queries use the JavaScript engine, at roughly upstream speed. Reproduce this
comparison with `npm run bench:public`.

**Historical native measurements:** the numbers below predate the compatibility
facade. They do not measure its transfer cost, bundled JavaScript size, JSON
eligibility scan or JavaScript execution mode. Re-benchmark the public package
for your workload before applying these ratios.

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

The native differential suites compare scores within floating-point tolerance
and preserve result/term ordering. Their dirty-index checks use the original's
post-cleanup state. `differential/public_api.mjs` separately compares the public
facade's **first** dirty query without warming either side, as well as callback
and identity behavior. Native test results alone do not establish full API
compatibility.

## Install

```sh
npm install minisearch-wasm
```

Node ESM and CommonJS initialize the packaged Wasm module synchronously:

```js
import MiniSearch from "minisearch-wasm";
const index = new MiniSearch({ fields: ["text"] });
// CommonJS: const MiniSearch = require("minisearch-wasm");
```

Browser ESM and module Workers can call `await init()` before constructing an
index to select Wasm. Without initialization the constructor works immediately
in JavaScript mode; existing indexes keep their mode when initialization ends.

```js
import MiniSearch, { init } from "minisearch-wasm";
await init();
const index = new MiniSearch({ fields: ["text"] });
```

The legacy `import init, { MiniSearchWasm }` / `await init()` syntax still works.
Plain browser ESM bundles its compatibility dependency and needs no import map.
The browser-global bundle exposes `MiniSearch` and `MiniSearch.init()`.
`SearchableMap` is exported at `minisearch-wasm/SearchableMap` for both ESM and
CommonJS. See [COMPATIBILITY.md](COMPATIBILITY.md) for direct asset imports and CSP.

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
npm install               # pinned compatibility engine, esbuild and TypeScript
npm run build             # native glue + compatibility facade + ESM/CJS/global entries
npm run test:wasm         # smoke, regression, JS-parity and API-parity suites against pkg/
npm run test:types        # strict ES2022 TypeScript fixtures
npm run test:package      # install tarball; check ESM/CJS/types/SearchableMap/global bundle
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
