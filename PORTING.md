# Porting Tracker

The original MiniSearch Jest tests live in `reference-tests/` and are the
behavior contract for this port. Each Rust test added under `tests/` should link
back to one or more behaviors from those files.

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

## Intentional API Direction

This port is all Rust at the engine level. It does not preserve JavaScript
callback hooks such as `extractField`, `tokenize`, `processTerm`, `filter`,
`boostTerm`, or `boostDocument`. Those behaviors should become Rust-side config,
query DSL features, or explicit preprocessing steps.

The Wasm surface is intentionally two search methods: `searchJoined` (fast,
recommended) and `search` (MiniSearch-compatible objects). The earlier
`searchCompact` / `searchPacked` variants were removed once `searchJoined`
superseded them.

## Next Test Slices

1. Serialization parity for `toJSON` / `loadJSON` behavior.
2. Auto-suggestion ranking.
3. Wildcard query behavior.
4. Async/vacuum-equivalent cleanup behavior, likely as synchronous Rust cleanup
   plus optional Wasm-friendly chunking.
5. Unicode tokenizer edge cases from the original tests.
6. Larger ranking fixtures near the end of `MiniSearch.test.js`.
7. Browser or Node smoke test against the generated `pkg/` package.
