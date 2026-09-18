# minisearch-wasm 0.10.0

This release adds a public compatibility facade for the documented MiniSearch
7.2.0 API. Eligible plain documents and declarative searches use Rust/Wasm;
callbacks, JavaScript value identity and dirty-index maintenance use bundled
MiniSearch 7.2.0. `index.executionMode` reports the active engine.

## Changes

- Support original callbacks, object IDs, live stored-field references, query
  trees, wildcard searches, defaults and generic TypeScript types.
- Add default-constructor, Node ESM/CommonJS, browser-global, module Worker and
  `SearchableMap` imports. Existing callable default initialization still works.
- Yield during chunked document conversion and upstream-compatible async JSON
  loading; preserve active and queued vacuum Promise behavior.
- Replace dynamic JavaScript function construction with static helpers for CSP
  compatibility, and compact internal IDs after clean maintenance.
- Preserve native version-4 snapshots and add a separate compatibility snapshot
  envelope for JavaScript-mode indexes. Re-supply callbacks when loading.

## Migration and performance

In Node, `import MiniSearch from 'minisearch-wasm'` initializes Wasm automatically.
In browsers and module Workers, call `await MiniSearch.init()` before creating
an index to use Wasm. The constructor also works without initialization in
JavaScript mode.

Transfer to JavaScript is permanent for an instance. Callbacks, non-scalar values,
`getStoredFields()`, dirty queries, vacuum and upstream JSON loading select that
mode. Native `loadBytes()` retains the fast Wasm loading path. Compact extensions
still require JSON-safe IDs and delimiter-safe terms.

On the measured 20,000-document workload, full-result `search()` was **1.42x**
faster than original MiniSearch; decoded joined/raw results were **4.48x/13.18x**
faster and `autoSuggest()` **8.27x** faster. Compact APIs return fewer fields.
The first callback or dirty query, including synchronous engine transfer, took
**1.28-1.59 seconds**. Later queries run at roughly upstream JavaScript speed.
These are workload-specific measurements, not universal speed guarantees.

Read the [compatibility and migration guide](https://github.com/epoyraz/minisearch-wasm/blob/v0.10.0/COMPATIBILITY.md)
and [paired benchmark report](https://github.com/epoyraz/minisearch-wasm/blob/v0.10.0/differential/results/2026-09-18-public-vs-original.md)
for formats, limitations, reproduction commands and timing methodology.

## Validation

The release gate covers Rust formatting, strict Clippy and native tests; Wasm
and public API parity; strict TypeScript; installation of the packed npm artifact;
Unicode separator generation; and real Chrome ESM, global bundle, module Worker,
async loading and CSP behavior.

Install with `npm install minisearch-wasm@0.10.0`.
