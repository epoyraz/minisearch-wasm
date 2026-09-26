# minisearch-wasm 0.12.0

This release keeps indexes on the Rust/Wasm engine in almost every case and
makes the drop-in paths fast. The compatibility target is MiniSearch 7.2.0;
`index.executionMode` reports the engine in use.

## Changes

- Document ids and stored fields live on the JavaScript side, kept as
  MiniSearch keeps them: ids of any type, stored fields as live objects. Object
  ids, Dates and other JavaScript values stay in Wasm, and `getStoredFields`
  returns the live object, edits included.
- `tokenize` and `processTerm` callbacks run on the JavaScript side exactly as
  MiniSearch calls them, and the engine indexes and searches their terms.
  `boostDocument` runs in Wasm: the engine calls it back while it scores, in
  MiniSearch's order. Custom tokenizers, stop words, stemmers and document
  boosts no longer move an index to the JavaScript engine.
- `search()` builds its results in one JavaScript loop from short ids: about
  2.4× faster than 0.11.0 (2.2× MiniSearch). `addAll` batches documents and
  indexes with fewer allocations (about 2× faster, 3.3× MiniSearch), and
  `loadJSON` reads postings straight into the engine (about 2.5× faster, 1.8×
  MiniSearch's loader). `loadJSONAsync` yields where MiniSearch's does.
- Binary snapshots (version 5) are about 40% smaller; ids and stored fields
  travel as JSON next to the engine snapshot.
- `toJSON()` matches MiniSearch's byte for byte in ordinary use: key order,
  field averages, short ids (renumbered only after heavy churn), and
  `removeAll()` keeps the dirt count.
- The published Wasm module leaves out engine bindings the facade no longer
  uses.

## Compatibility and migration

Imports, initialization and the MiniSearch API are unchanged. MiniSearch JSON
(versions 1 and 2) and native snapshots from 0.9.0 to 0.11.0 still load; new
snapshots cannot be read by 0.11.0 or earlier. An index built with `tokenize`
or `processTerm` callbacks needs them again when its snapshot is loaded, and
says so otherwise.

What still selects the JavaScript engine: a query-tree node with a
`boostDocument` of its own, text or terms the engine cannot hold (not strings,
lone surrogates), `fields` that is not an array or repeats a name, and
`Infinity` in search options. Inside `boostDocument` the index can be read
(`has`, `getStoredFields`, counts) but not searched or changed.

Read the [compatibility guide](https://github.com/epoyraz/minisearch-wasm/blob/v0.12.0/COMPATIBILITY.md)
for the details, and the [benchmark report](https://github.com/epoyraz/minisearch-wasm/blob/v0.12.0/differential/results/2026-09-27-0.12.0-vs-original.md)
for the measurements.

## Validation

The release gate includes formatting, strict Clippy for both builds, native and
release-mode snapshot tests, differential comparisons with MiniSearch, the
engine suites (built with `core-api`), the facade suites (callback call
sequences, `boostDocument` calls on clean and dirty indexes, the serialized
index byte for byte after a random mutation history), TypeScript,
packed-package checks, MiniSearch's own test suite, Unicode tables and
package-size budgets.

Install with `npm install minisearch-wasm@0.12.0`.
