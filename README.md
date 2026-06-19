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
   leaves as just three values: `scores` (a `Float64Array`, one bulk copy) and
   `ids` / `terms` as single newline-joined strings the host splits natively.
   No per-hit JS object is created on the Rust side.

That is what `searchJoined` does, and it is the recommended path for embedding
apps. `search()` is also provided for drop-in MiniSearch compatibility (it
returns the full nested per-hit `{ id, score, terms, queryTerms, match, … }`
objects) — but it is **slower than JS by design**: rebuilding MiniSearch's
nested objects across the boundary is inherently expensive, and real consumers
(e.g. the jobboard worker) only ever read `id`, `score`, and `terms`.

## API

```js
import init, { MiniSearchWasm } from "minisearch-rust";
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
// r = { count, ids: "id0\nid1\n…", scores: Float64Array, terms: "a b\nc\n…" }
const ids   = r.count ? r.ids.split("\n")   : [];
const terms = r.count ? r.terms.split("\n") : [];
for (let i = 0; i < r.count; i++) {
  use(ids[i], r.scores[i], terms[i] ? terms[i].split(" ") : []);
}

// Compatibility path — MiniSearch-shaped result objects (slower, see above):
const full = mini.search("software engineer", { combineWith: "AND" });

// Persistence: compact binary snapshot — smaller on the wire than JSON and
// faster to load.
const bytes = mini.toBytes();            // Uint8Array
const loaded = MiniSearchWasm.loadBytes(bytes);
```

## Differences from MiniSearch

This is a from-scratch reimplementation. It matches MiniSearch's **ranking** —
identical BM25 scores, verified to float epsilon (`max delta ≈ 1e-14`) on a
21k-document corpus — but deliberately diverges from its API and internals:

- **No JavaScript callbacks.** MiniSearch takes user functions for
  `extractField`, `tokenize`, `processTerm`, and search-time `filter` /
  `boostDocument`. To keep the JS↔Wasm boundary coarse, this port has none:
  tokenization and term processing are built-in Rust modes (`"default"`,
  `"jobboard"`), fields are read by name from JSON, and nothing calls back into
  JS per token/document. Custom tokenizers/filters must be added Rust-side.
- **Serialization is not interoperable.** `toJSON`/`toBytes` use this crate's
  own formats (a Rust-struct JSON and a compact varint binary snapshot). You
  cannot load a MiniSearch v1/v2 index here, nor load one of these indexes into
  JS MiniSearch. Persist and reload with the same engine.
- **Equal-score ties order differently.** Scores and the result *set* are
  identical; results with exactly equal scores are ordered by internal document
  id, whereas MiniSearch keeps them in first-scored order. This affects only
  ties (≈1 query in 30 on the test corpus).
- **Term length is counted in code points.** Prefix/fuzzy weighting uses
  `chars().count()`, which equals JS `String.length` for all BMP text; it
  differs only for astral-plane characters (emoji, etc.), where JS counts UTF-16
  code units.
- **Search never mutates the index.** MiniSearch lazily removes stale postings
  mid-query when it meets a discarded document; this port just skips them (and
  skips the liveness check entirely on a clean index).
- **Not implemented (yet).** `autoSuggest`, wildcard queries
  (`MiniSearch.wildcard`) and nested query-expression trees, async indexing
  (`addAllAsync`), `vacuum`, and batch `removeAll`/`discardAll`. The query is a
  plain string plus options, not a query tree. See `PORTING.md`.
- **Added beyond MiniSearch.** `searchJoined` (compact columnar results for a
  thin Wasm boundary), `addAllJSON` (index straight from a raw JSON string),
  `toBytes`/`loadBytes` (compact binary snapshot), the `"jobboard"` tokenizer,
  and — internally, with identical results — a bit-parallel (Myers) fuzzy
  traversal and `HashMap`-backed postings.

## Benchmark results

Measured by `keyword-search/scripts/bench-search-engines.mjs` on the real
jobboard corpus (~21k documents, 30 representative queries), comparing against
JS MiniSearch. Search ratios are reported by median (robust to system noise);
expect some run-to-run variance.

| Category | Rust vs JS MiniSearch |
|---|---|
| **Search — app workload** (`{id, score, terms}`, end to end) | **~1.5–1.6× faster** (pure engine ~1.9–2×) |
| **Index download** (prebuilt, brotli) | **~0.73× — smaller on the wire than JS** |
| Load prebuilt index — `loadBytes` vs `loadJSON` | ~2.4× faster |
| Serialize index — `toBytes` vs `JSON.stringify` | ~8× faster |
| Build index — `addAllJSON` vs `addAll` | ~1.3× faster |
| Full compat `search()` vs JS `search()` | ~0.5× (slower by design — see Design) |

The compact binary snapshot is delta+varint encoded, so it is both smaller than
the JSON index and low-entropy enough to compress well; `loadBytes` rebuilds the
term tree from the sorted terms (hence ~2.4× rather than ~8×, but still far
ahead of JS `loadJSON`).

**Truthfulness:** the benchmark verifies the two engines return identical
results — `set-identical 30/30`, `max score delta ≈ 1e-14` (float epsilon),
`0` real ranking bugs. The only ordering differences are between results with
exactly equal scores (a tie-break nuance: JS breaks ties by scoring-encounter
order, this port by document id).

## Install

```sh
npm install minisearch-wasm
```

The package is built with `wasm-pack --target bundler`, so it works out of the
box with Vite, webpack, and Rollup — no manual init step:

```js
import { MiniSearchWasm } from "minisearch-wasm";

const ms = new MiniSearchWasm({ fields: ["title", "text"] });
ms.addAll(documents);
const results = ms.search("query");
```

## Build

```powershell
cargo fmt -- --check
cargo test
rustup target add wasm32-unknown-unknown
npm run build   # wasm-pack build --target bundler --release + metadata patch
```

Install `wasm-pack` with `cargo install wasm-pack` if needed.

### Publishing

```powershell
npm run publish:pkg   # rebuilds, then `npm publish ./pkg`
```

You must `npm login` first. The publishable artifact is the generated `pkg/`
directory; its `package.json` metadata is injected by `scripts/finalize-pkg.mjs`
on every build.

See `PORTING.md` for conformance status and the next test slices.
