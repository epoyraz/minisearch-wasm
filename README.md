# minisearch-wasm

A Rust + WebAssembly full-text search engine with a compatibility facade for
[MiniSearch](https://github.com/lucaong/minisearch) 7.2.0. Ordinary JSON
documents, searches (including `filter`, `prefix`, `fuzzy` and `boostTerm`
callbacks), updates and vacuuming use Wasm, with results that are MiniSearch's
down to the last bit of the score. Callbacks that run inside tokenization or
scoring, and JavaScript values such as object ids, use a bundled, pinned
MiniSearch implementation. Check `index.executionMode` to see which engine owns
the index.

Version **0.10.0** added the public compatibility facade. This repository is
ahead of it: see [Unreleased changes](#unreleased-changes-after-0100) and
[CHANGELOG.md](CHANGELOG.md).
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
while the native engine can reproduce MiniSearch exactly: plain documents
(string, number and boolean fields and ids), every mutation, searches on a
clean or a dirty index, vacuuming, `getStoredFields`, `loadJSON`, and the
callbacks `filter`, `prefix`, `fuzzy`, `boostTerm`, `extractField`,
`stringifyField` and `logger`, which the facade evaluates on the JavaScript
side. It moves to the JavaScript engine, once and permanently, when it meets
something only JavaScript can do:

- `tokenize` or `processTerm` (constructor or search option) and
  `boostDocument`, which run inside tokenization and scoring;
- `Date`, array or object field values, object ids, `-0`, strings with lone
  surrogates;
- an edit to an object returned by `getStoredFields(id)` (MiniSearch hands out
  its live stored fields; reading them is free);
- options with no native form: `fields` that repeats a name, stored fields
  named like result properties (`score`, `terms`, …), `Infinity` in search
  options.

`index.executionMode` reports `"wasm"` or `"javascript"`. In 0.10.0 on npm the
list is longer: there a `discard` or `replace` followed by a search, any vacuum,
any search callback, `getStoredFields` and `loadJSON` also transfer.

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

The table above is 0.10.0, measured on Windows with Node 24. After a `discard`
0.10.0 answers from the JavaScript engine for the lifetime of the instance.
This repository keeps a dirty index in Wasm, reproducing MiniSearch's lazy
cleanup query by query. The same 20,000 documents and queries with a quarter of
the documents discarded and not yet vacuumed, on the unreleased tree (macOS,
Intel i7-9750H, Node 26, medians of 5 paired rounds, `npm run bench:public`;
[full report](differential/results/2026-09-19-unreleased-vs-original.md)):

| Operation, dirty index | MiniSearch 7.2.0 | 0.10.0 (Windows) | unreleased (macOS) |
| --- | --- | --- | --- |
| `search()` | 1.0× (750 ms) | 1.2× | 1.6× |
| `searchJoined`, decoded, vs `search()` | 1.0× (714 ms) | 1.05× | 5.2× |
| `searchRaw`, decoded, vs `search()` | 1.0× (711 ms) | – | 10× |
| `autoSuggest` | 1.0× (526 ms) | 1.2× | 9.2× |
| First query after discarding 25% | 17 ms | 1.3–1.6 s (transfer) | 18 ms |
| First query with a `filter` callback | 20 ms | 1.3–1.6 s (transfer) | 19 ms |
| Vacuum of 5,000 discarded documents | 81 ms | JavaScript | 25 ms |

On the same run a clean index measured `search()` 1.5×, decoded `searchJoined`
5.8×, decoded `searchRaw` 13×, `autoSuggest` 10×, `toBytes` 71×, `loadBytes`
13× and `loadJSON` 0.9× (the native importer keeps the index in Wasm but is
not faster than MiniSearch's loader). The columns come from different machines:
compare within a column's ratios, not across columns. A query with a
`boostDocument`, `tokenize` or `processTerm` callback still transfers the index
once (1.3 s here) and then runs at MiniSearch's speed.

The facade also costs size: the package is 1.4 MB unpacked, 437 KB as a tarball
(0.10.0: 1.3 MB unpacked; 0.9.0: 820 KB; 0.8.0: 631 KB), because it ships the Wasm engine, the pinned JavaScript engine
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
// Functions are accepted too; the facade evaluates them, and the index stays in Wasm.
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

## Unreleased changes (after 0.10.0)

Not on npm yet; [CHANGELOG.md](CHANGELOG.md) has the full list.

- **Indexes stay in Wasm.** Discards and replaces, queries on a dirty index
  (the engine reproduces MiniSearch's lazy cleanup, first query included),
  vacuuming, `getStoredFields`, `loadJSON`/`loadJSONAsync`, and the callbacks
  `filter`, `prefix`, `fuzzy`, `boostTerm`, `extractField`, `stringifyField` and
  `logger` no longer move an index to the JavaScript engine.
- **Scores are MiniSearch's, bit for bit.** The inverse document frequency uses
  the logarithm algorithm V8 uses; about 4% of scores on small indexes used to
  differ in the last bit.
- **Snapshots.** Everything the engine can save loads again (negative field
  averages, postings left by a changed document); what no reader accepts is
  refused when saving, with a way out. A rejected snapshot can no longer grow
  Wasm memory past the 256 MiB decode budget. `loadBytes` takes an
  `ArrayBuffer` too.
- **Robustness.** No query aborts the module (an absurd `fuzzy` distance used
  to), and string parameters of the core are type-checked instead of trapping.
- **Removal is linear.** Removing the oldest 50,000 of 100,000 documents took
  48.7 s and takes 0.6 s.
- **Fixes:** vacuum empties terms in MiniSearch's order (the radix key order
  could differ afterwards), compact results score a repeated query term
  correctly, the declarative `filter` compares numbers by value, a zero `boost`
  means none, a mismatched `remove` no longer blocks `compact()`.
- **Package:** ships this README and `LICENSE.txt`; named types for TypeScript
  CommonJS consumers; declarations without the DOM library; MiniSearch is
  bundled, not a dependency; the Node entry survives bundlers; the global
  script is minified and defines only `MiniSearch`.

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

Verify the changes with `npm run build` and `npm test`.

## Compatibility and extensions

The public facade supports MiniSearch 7.2.0's documented methods, callbacks,
query trees, wildcard, default constructor, generic TypeScript types and
`SearchableMap` subpath. [COMPATIBILITY.md](COMPATIBILITY.md) lists the execution
modes and remaining limitations.

- **JavaScript semantics:** `tokenize`, `processTerm` and `boostDocument`
  callbacks, non-scalar indexed/stored values, object IDs and an edited
  `getStoredFields()` object select the pinned JavaScript engine. A transfer
  preserves radix traversal order. Dirty searches and vacuum stay native and
  perform the same lazy cleanup as MiniSearch, query by query.
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

Measurements of the public package against MiniSearch, 0.8.0 and 0.9.0, and
of the unreleased tree on a dirty index, are in
[Migrating from MiniSearch](#migrating-from-minisearch) above. There are two
ways of timing the compact APIs in this document: the first table times the
calls alone, the paired runs (`npm run bench:public`) include decoding the
result (`JSON.parse` of the ids, splitting the terms), which is why
`searchJoined` reads 8.2× in one and 4.5–5.8× in the other.

The [paired public-package report](https://github.com/epoyraz/minisearch-wasm/blob/main/differential/results/2026-09-18-public-vs-original.md)
for 0.10.0 also measures the first query batch and that version's one-time
transfer to JavaScript: full `search()` 1.42x faster, decoded `searchJoined`
4.48x, decoded `searchRaw` 13.18x, and 1.28-1.59 seconds for the first callback
or dirty query on 20,000 documents, after which 0.10.0 runs at MiniSearch's
speed. The unreleased tree no longer transfers in those cases (see above).

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

### Bundlers

Browser bundles resolve to the fetch-based entry (also for `require`), which
loads `minisearch_wasm_bg.wasm` from a URL next to the module. A bundler that
does not copy that file has to be given it; pass the URL it produces to `init`:

```js
import MiniSearch, { init } from "minisearch-wasm";
import wasmUrl from "minisearch-wasm/minisearch_wasm_bg.wasm?url"; // Vite; webpack: new URL("…", import.meta.url)
await init(wasmUrl);
```

With esbuild use `--loader:.wasm=file`. A Node bundle that leaves the `.wasm`
file behind still imports: the entry warns once and runs on the JavaScript
engine until `initSync({ module: bytes })` is called.

The package ships typed declarations for its whole surface
(`MiniSearchWasmOptions`, `SearchOptions`, `Query`, `SearchResult`,
`Suggestion`, `JoinedResults`, `RawResults`, `MiniSearchJSON`, …) with optional
arguments marked optional; `types/fixtures.ts` compiles the README examples
under `strict` with TypeScript 7 as part of the checks below.

## Build

```sh
npm install        # esbuild, TypeScript, Vitest and the pinned MiniSearch
npm run build      # wasm-pack -> target/wasm-glue, then scripts/finalize-pkg.mjs -> pkg/
npm test           # everything below
```

`rust-toolchain.toml` pins Rust (with the `wasm32-unknown-unknown` target);
`wasm-pack` 0.15.0 is needed on the `PATH` (`cargo install wasm-pack --version
0.15.0`, or a release binary). `npm test` runs:

```sh
npm run test:native        # cargo fmt --check, clippy -D warnings, cargo test (+ release-mode snapshot tests)
npm run test:differential  # native engine vs MiniSearch on a generated corpus (765,678 rows)
npm run test:wasm          # smoke, regression, parity, facade and core-robustness suites against pkg/
npm run test:types         # strict ES2022 TypeScript fixtures
npm run test:package       # installs the tarball: ESM/CJS/types/global script/bundlers, README, determinism
npm run test:upstream      # MiniSearch's own test suite against the facade
npm run check:separators   # tokenizer table still matches this Node's Unicode data
npm run check:size         # published files against scripts/size-budget.json
```

`npm run build:pkg` reassembles `pkg/` from the last Wasm build after a change
to `js/`, the README or the scripts. After a Node upgrade that brings a new
Unicode version, regenerate the tokenizer's separator table with `node
scripts/gen-separators.mjs` (it writes `src/separators.rs`). CI
(`.github/workflows/ci.yml`) builds and runs `npm test` on every push and pull
request.

### Publishing

```sh
npm run publish:pkg   # build, npm test, then `npm publish ./pkg`
```

You must `npm login` first. The publishable artifact is the generated `pkg/`
directory, assembled from scratch by `scripts/finalize-pkg.mjs` on every build
(manifest, entries, declarations, this README and the licenses). Tag the
release (`vX.Y.Z`) when publishing: the documents link to files by tag.

See `PORTING.md` for the porting history and conformance status,
`CHANGELOG.md` for what changed, and `TODO.md` / `IMPROVEMENTS-3.md` for the
open review items.
