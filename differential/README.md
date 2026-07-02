# Differential test harness

Runs identical documents and queries through the original JS MiniSearch and
this port, then compares outputs:

- **search** (packed/`searchJoined` path): result ids, BM25 scores
  (rel. tolerance `1e-12`; observed max ≈ `5e-16`, last-ulp `Math.log` vs
  `ln`), and each document's matched **terms in order**. Result order must
  match except inside near-tie score bands, where members are compared as sets
  (the documented tie-order difference).
- **autoSuggest**: suggestion phrases and terms exactly, scores as above.

Both a fresh index and one mutated by `remove`/`discard` are checked. On the
mutated index the JS side is dumped at its post-lazy-cleanup fixpoint (each
query runs twice, the second run is recorded), because JS mutates the index
during dirty searches while this port never does — the port's dirty-search
scores equal JS's post-cleanup scores by design.

## Run

```powershell
npm install
node gen_corpus.mjs corpus.json
node js_bulk.mjs corpus.json js_bulk.json
cargo run --release --example dump_bulk corpus.json rust_bulk.json   # from repo root, paths relative to it
node compare_bulk.mjs js_bulk.json rust_bulk.json
```

Small fixed-fixture autoSuggest comparison (mirrors `examples/dump_autosuggest.rs`):

```powershell
node js_autosuggest.mjs > js_out.json
cargo run --example dump_autosuggest > rust_out.json   # from repo root
node compare.mjs js_out.json rust_out.json
```

End-to-end Wasm boundary smoke test (requires `npm run build` at the repo root
first):

```powershell
node wasm_smoke.mjs
```
