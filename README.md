# minisearch-rust

A Rust + WebAssembly full-text search engine built against the
[MiniSearch](https://github.com/lucaong/minisearch) behavior contract. It
computes **identical rankings** to MiniSearch (same ids, same BM25 scores) and
is **faster than the JavaScript original** on the real workload — verified
continuously by the benchmark in the sibling `keyword-search` project.

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

// Persistence: binary snapshot is ~10x faster to write and load than JSON.
const bytes = mini.toBytes();            // Uint8Array
const loaded = MiniSearchWasm.loadBytes(bytes);
```

## Benchmark results

Measured by `keyword-search/scripts/bench-search-engines.mjs` on the real
jobboard corpus (~21k documents, 30 representative queries), comparing against
JS MiniSearch. Search ratios are reported by median (robust to system noise);
expect some run-to-run variance.

| Category | Rust vs JS MiniSearch |
|---|---|
| **Search — app workload** (`{id, score, terms}`, end to end) | **~1.3–1.4× faster** (pure engine ~1.6×) |
| Load prebuilt index — `loadBytes` vs `loadJSON` | ~8× faster |
| Load prebuilt index — `loadJSON` vs `loadJSON` | ~2.4× faster |
| Serialize index — `toBytes` vs `JSON.stringify` | ~12× faster |
| Serialize index — `toJSONString` vs `JSON.stringify` | ~10× faster |
| Build index — `addAllJSON` vs `addAll` | ~1.3× faster |
| Full compat `search()` vs JS `search()` | ~0.5× (slower by design — see Design) |

**Truthfulness:** the benchmark verifies the two engines return identical
results — `set-identical 30/30`, `max score delta ≈ 1e-14` (float epsilon),
`0` real ranking bugs. The only ordering differences are between results with
exactly equal scores (a tie-break nuance: JS breaks ties by scoring-encounter
order, this port by document id).

## Build

```powershell
cargo fmt -- --check
cargo test
rustup target add wasm32-unknown-unknown
wasm-pack build --target web
```

Install `wasm-pack` with `cargo install wasm-pack` if needed.

See `PORTING.md` for conformance status and the next test slices.
