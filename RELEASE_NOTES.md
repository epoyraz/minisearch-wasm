# minisearch-wasm 0.11.0

This release keeps more MiniSearch-compatible operations on the Rust/Wasm
engine, fixes callback and asynchronous scheduling differences, and strengthens
snapshot safety and release validation. The compatibility target is MiniSearch
7.2.0; `index.executionMode` reports the engine in use.

## Changes

- Dirty queries reproduce MiniSearch's first-query scores and lazy cleanup in
  Wasm. Vacuum, stored-field reads, JSON loading and supported search/extraction
  callbacks also stay native instead of transferring the whole index.
- Scores use V8's logarithm algorithm for exact parity with MiniSearch on V8.
- Filters observe traversal order before sorting, including stateful callbacks
  and score edits. Compact searches evaluate each query and callback once.
- Native `loadJSONAsync` yields between document-map and posting batches.
  `addAllAsync` uses MiniSearch's scheduler, including deferred short batches.
- Removal uses posting tombstones instead of repeatedly shifting large lists.
- Snapshot writers reject states they cannot reload; readers charge allocations
  before reserving memory. Fuzzy-query limits avoid trapping the Wasm module.
- Native vacuum preserves radix order and supports searches and mutations while
  maintenance is running. Compact results handle repeated query terms correctly.
- Deterministic packaging ships the current README and licenses, improves
  CommonJS types and browser bundling, and gates publication on `npm test`.

## Compatibility and migration

Existing default/named imports, initialization, MiniSearch JSON versions 1 and
2, and native version-4 snapshots remain supported. No reindexing is required
solely to upgrade from 0.10.0. Internal ID tables must still be refreshed after
mutations or compaction when `idTableVersion` changes.

`tokenize`, `processTerm`, `boostDocument`, reference-valued documents and edits
to returned stored-field objects still select the bundled JavaScript engine.
Native async JSON loading batches reconstruction; parsing, initial allocation,
per-list sorting and final validation remain synchronous.

Read the [compatibility guide](https://github.com/epoyraz/minisearch-wasm/blob/v0.11.0/COMPATIBILITY.md)
for remaining differences, engine selection and persistence limitations.
Historical benchmark reports retain their original versions and environments;
their timings are not new measurements of this final release artifact.

## Validation

The release gate includes formatting, strict Clippy, native and release-mode
snapshot tests, differential comparisons with MiniSearch, Wasm and facade
regressions, TypeScript, packed-package checks, upstream tests, Unicode tables
and package-size budgets. The upstream runner explicitly records expected
failures for private implementation details and unsupported internals.

The browser contract separately checks ESM, SearchableMap, the global bundle,
module Workers, native callbacks and incremental JSON loading under CSP that
allows Wasm compilation without JavaScript string evaluation.

Install with `npm install minisearch-wasm@0.11.0`.
