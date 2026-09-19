# Differential test harness

There are two distinct contracts. The generated `minisearch_wasm_core.js` tests
the native subset. `public_api.mjs` tests the public compatibility facade against
the pinned MiniSearch engine, including callbacks and the first dirty query.
Public tests do not warm dirty queries before comparison.

The older native bulk comparator runs identical documents and queries through
the original JS MiniSearch and the Rust engine, then compares outputs:

- **search** (packed/`searchJoined` path): result ids, BM25 scores
  (rel. tolerance `1e-12`; observed max ≈ `5e-16`, last-ulp `Math.log` vs
  `ln`), and each document's matched **terms in order**. Result order must
  match except inside near-tie score bands, where members are compared as sets
  (a historical comparator allowance, not the current public contract).
- **autoSuggest**: suggestion phrases and terms exactly, scores as above.
- **query trees + wildcard** (full `search(query)` path): ids and scores as
  above; per-row terms as sorted sets (`{wildcard: true}` in the corpus JSON
  stands in for the wildcard symbol). The stricter `compat_parity.mjs` suite
  also checks exact match-key, term and tie order through the native boundary.

A fresh index, one mutated by `remove`/`discard`, one mutated by
`removeAll`/`discardAll`, and one explicitly vacuumed are checked. On dirty
indexes the JS side is dumped at its post-lazy-cleanup fixpoint (each query runs
twice, the second run is recorded), because JS mutates the index during dirty
searches while the native core does not. This native-only check does not verify
public first-query parity; `public_api.mjs` covers that separately.

## Run

From the repository root (the dependencies are the root's; nothing is installed
in this directory):

```sh
npm run test:differential   # both native comparisons below; dumps go to target/differential/
```

which runs the bulk comparison (`gen_corpus.mjs` → `js_bulk.mjs` and
`cargo run --release --example dump_bulk` → `compare_bulk.mjs`) and the small
fixed-fixture autoSuggest comparison (`js_autosuggest.mjs` and `cargo run
--example dump_autosuggest` → `compare.mjs`). `compare_bulk.mjs` fails on
duplicate or type-confused ids, nonfinite scores, and on near-tie bands in a
different order (`--allow-tie-reorders` for a platform whose native `ln` rounds
the other way; through Wasm, scores are MiniSearch's bit for bit).

The Wasm suites require `npm run build` first:

```sh
node differential/wasm_smoke.mjs
node differential/high_priority_regressions.mjs
node differential/compat_parity.mjs
node differential/api_parity.mjs
node differential/public_api.mjs
node differential/wasm_residency.mjs
node differential/core_robustness.mjs
node differential/facade_regressions.mjs
```

Or run `npm run test:wasm` from the repository root. The high-priority suite
checks field statistics against JS on a seeded sparse corpus and fields with
65,535/65,536/70,000 distinct tokens, all three result formats, both snapshot
formats, tree-order preservation, mixed ID types and generation changes, plus
malformed/truncated/mutated binary inputs through the actual WASM boundary.
`compat_parity.mjs` checks exact result order (ties included), `terms`/`match`
order, suggestions, JS number formatting of field values and ids, UTF-16 term
lengths, `has`/`replace`/`getStoredFields`, and MiniSearch JSON import/export
in both directions against the JS engine on fresh, dirty and vacuumed indexes.
`api_parity.mjs` covers the **core subset**: `Error` objects and messages,
rejection of callback options and the declarative forms that replace them,
`Date`/`toString` field values, the tokenizer's Unicode tables, `getDefault`,
`logger`, `loadJSONAsync` and MiniSearch-format `toJSON`/`loadJSON`.

`wasm_residency.mjs` asserts that an index stays on the Wasm engine through a
seeded 900-step history of mutations, unwarmed dirty queries, vacuums, search
callbacks, compact forms and suggestions, comparing every step with MiniSearch
(scores exactly), and covers extracted fields, `getStoredFields`, native
`loadJSON`, options set to `undefined` and inputs that select the JavaScript
engine. `core_robustness.mjs` drives the raw core with hostile arguments,
absurd fuzzy distances, snapshots of every state the engine reaches and a
crafted snapshot that must not outgrow the decode budget.

`facade_regressions.mjs` compares pre-sort stateful filters, single-evaluation
compact callbacks, short-batch async scheduling and incremental native JSON
loading with the original. It covers both JSON versions, dirty indexes, and a
single common term with thousands of stale postings to require yields within
the posting list.

The public suite verifies all supported callbacks, JS object identity and stored
references, first dirty-query scores and lazy cleanup, mixed mutation histories,
radix ordering across engine transfers, actual async yields, snapshots, active
versus queued vacuum promises, compaction and ID generations. Its CSP child
process also exercises the actual Wasm result builder with string code generation
disabled. Core callback-rejection checks do not imply public callback rejection.

From the repository root, `npm run test:package` packs and installs the actual
artifact in a temporary directory. It tests ESM and CommonJS constructors,
SearchableMap, strict ES2022 types (including generic type annotations), and the
global bundle in a Node VM with string code generation disabled.

For a real browser, run `node differential/serve_browser.mjs` and open the printed
URL. The page loads the generated files without an import map, under CSP that
allows Wasm compilation but forbids JavaScript eval. It reports ESM, global
bundle, module Worker, callback and async JSON checks in the page and in
`globalThis.browserContract`. A Node VM pass alone is not a browser pass.

## Benchmarks

The public compatibility package has a separate paired benchmark:

```sh
npm run bench:public -- differential/bench_corpus.json differential/results/public-benchmark.json 9
```

It compares pinned MiniSearch with the public package, verifies full/compact
results before timing, alternates engine order, and reports medians plus raw
samples. It separates first-batch and warmed queries, includes compact-result
decoding, and measures callbacks, first dirty queries, vacuum, asynchronous
loading, snapshot sizes and fresh-process module import. Setup, forced GC and
disposal are outside the timed regions. Both engines use the same input and
search options. First transfer timings include the compatibility conversion.

See the [18 September public-package report](results/2026-09-18-public-vs-original.md)
for measured results and limitations. These Node/synthetic measurements do not
replace a browser or production-workload benchmark.

`gen_corpus.mjs` takes an optional document count for a larger benchmark
corpus. Native engine benchmark and end-to-end Wasm-vs-JS benchmark (the
latter needs `npm run build` first):

```sh
node gen_corpus.mjs bench_corpus.json 20000
cargo run --release --example bench_search differential/bench_corpus.json   # from repo root
node bench_wasm.mjs bench_corpus.json
```

Maintenance-path benchmark for `addAllAsync` overhead and vacuum
latency/snapshot reclamation (uses 5,000 documents by default):

```sh
npm run bench:maintenance
# Optional: BENCH_DOCS=10000 BENCH_CHUNK=500 BENCH_VACUUM_BATCH=1000 npm run bench:maintenance
```
