# Task: Implement `removeAll` / `discardAll` in minisearch-wasm

## Context

This repo is a Rust + WebAssembly port of JS [MiniSearch](https://github.com/lucaong/minisearch) v7.2.0, API-compatible and differentially verified against it (identical BM25 scores and result sets). Read `PORTING.md` first — it documents the porting conventions, verified-parity areas, and intentional divergences. The engine lives in `src/mini_search.rs`; the wasm-bindgen boundary in `src/lib.rs`; conformance tests in `tests/minisearch_conformance.rs`.

The single remaining functional-parity gaps are `removeAll`, `discardAll`, `vacuum`/`autoVacuum`, and `addAllAsync`. **This task covers only `removeAll` and `discardAll`.** Do not implement vacuuming or async indexing.

## Reference semantics (JS MiniSearch v7.2.0, `src/MiniSearch.ts` in the sibling `../minisearch` checkout)

`removeAll(documents?)` — lines ~890–905:

- With an array argument: `for (const document of documents) this.remove(document)`.
- Called with **no argument at all**: reset the entire index — fresh term index, `documentCount = 0`, empty `documentIds` / `idToShortId` / `storedFields`, empty field lengths, `avgFieldLength = []`, `nextId = 0`.
- Called with an explicitly falsy/undefined argument (`arguments.length > 0` but no documents): **throw** `"Expected documents to be present. Omit the argument to remove all documents."`

`discardAll(ids)` — lines ~993–1007: loops `this.discard(id)` for each id, with autoVacuum suppressed during the loop (irrelevant here — this port has no autoVacuum yet; a plain loop is correct). Any error from an individual `discard` (unknown id) propagates, and in JS the earlier discards remain applied — match that (no rollback).

## Implementation notes

- **Rust engine (`src/mini_search.rs`)**: add `pub fn remove_all(&mut self, documents: Vec<Value>) -> Result<(), String>`, `pub fn remove_all_documents(&mut self)` (the no-arg reset; also reset `dirt_count` to 0 — a fresh index has no dirt), and `pub fn discard_all(&mut self, ids: &[Value]) -> Result<(), String>`. Follow the existing code style: reuse `remove`/`discard`, mirror the JS error messages exactly (see `remove`/`discard` for the message format convention).
- The no-arg reset must leave the instance byte-identical (via `to_bytes()`) to a freshly constructed `MiniSearch::new(same options)` — that is the test for "reset everything". Check `MiniSearch::new` for every field that needs resetting, including the dense `field_length: Vec<u16>` table and `average_field_length`.
- **Wasm boundary (`src/lib.rs`)**: JS distinguishes `removeAll()` from `removeAll(docs)` by `arguments.length`; wasm-bindgen can't. Bind `removeAll` as taking one optional `JsValue`: `undefined`/missing → full reset; an array → batch remove; anything else non-array → throw the JS error message above. Add `discardAll(ids: JsValue)` expecting an array of ids. Follow the existing binding patterns (`add_all_js`, `discard_js`) for serde conversion and error mapping.

## Verification (all must pass; run from the repo root)

1. `cargo fmt` and `cargo test --release` — extend `tests/minisearch_conformance.rs` with cases for: batch remove matches sequential removes; `removeAll()` reset makes `to_bytes()` equal a fresh index and searches return empty; reset index accepts re-adding documents (ids restart at 0 — verify a search works after re-add); `discardAll` matches sequential discards including the dirty-index search behavior; unknown id in `discardAll` errors and leaves earlier discards applied.
2. Differential harness (`differential/README.md` has the commands): extend `differential/js_bulk.mjs` and `examples/dump_bulk.rs` with a third phase (e.g. `afterBatchMutation`) that uses `removeAll(someDocs)` + `discardAll(someIds)` instead of the sequential loops, then run the full pipeline — `node gen_corpus.mjs corpus.json && node js_bulk.mjs corpus.json js_bulk.json && cargo run --release --example dump_bulk differential/corpus.json differential/rust_bulk.json && node compare_bulk.mjs js_bulk.json rust_bulk.json` (js_bulk/compare run from `differential/`). Must print `ALL MATCH`. Note the dirty-index fixpoint convention documented in `js_bulk.mjs` (dump the SECOND run of each query on mutated indexes).
3. `npm run build`, then `node differential/wasm_smoke.mjs` — add smoke checks for `removeAll([...])`, `removeAll()`, and `discardAll([...])` through the real wasm boundary, compared against JS MiniSearch.
4. Update `README.md` ("Not implemented (yet)" bullet: remove `removeAll`/`discardAll`) and `PORTING.md` (add a short section; drop the item from the tracker if listed).

## Conventions

- No JS callbacks cross the boundary (see PORTING.md "Intentional API Direction").
- Error messages must match JS MiniSearch character-for-character where a JS equivalent exists.
- Do not change the binary snapshot format (`SNAPSHOT_VERSION`), the fused search fast path, or any existing public API behavior.
- Do not bump the version or publish; leave that to the maintainer.
