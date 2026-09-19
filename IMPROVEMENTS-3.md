# MiniSearch WASM 0.10.0: third review

Review date: 19 September 2026. Reviewed `minisearch-wasm` 0.10.0 at `888b991` against MiniSearch 7.2.0 (`lucaong/minisearch` at `3d239d1`, still the latest upstream release, so the pin is current). This review covers two things: a fresh look at the repository for work worth doing, and a review of the commits from 0.7.0 to 0.10.0 (`5e93d0f`, `1d5c4c2`, `ed9b39c`, `888b991`) for problems that still exist at HEAD.

Environment: macOS (x86_64), Node 26.8.1, Rust 1.98.0 (Homebrew, no `rustup`, no `wasm32-unknown-unknown` target, no `wasm-pack`). **The Wasm could not be rebuilt here.** All boundary probes ran against the published npm artifact `minisearch-wasm@0.10.0`, unpacked into the gitignored `pkg/`; its facade re-bundles byte-for-byte from `js/minisearch.js` apart from path comments, so it represents HEAD. Every item below says whether it was reproduced or only read.

| Check | Result |
| --- | --- |
| `cargo test --locked` | Passed |
| `cargo fmt --check` | Passed |
| `cargo clippy --locked --all-targets -- -D warnings` | **Fails** on Rust 1.98 (`for_kv_map`, `src/mini_search.rs:1648`); that is the only lint |
| `npm run test:wasm`, `test:types`, `test:package` against published 0.10.0 | Passed |
| `npm run check:separators` | **Fails** on Node 26 although the table is identical (item 12) |
| Upstream's own Jest suite (134 tests) run against the facade | 119 pass; 13 fail only because they read private fields; 1 is `loadJS` subclassing (documented exclusion); **1 is a real bug** (item 6) |
| Binary snapshot fuzz, native debug build | 425,152 mutated inputs, 0 panics |
| Native vacuum/mutation fuzz vs JS (core, vacuum active on about 99% of steps) | 0 mismatches in about 160,000 checks |

**Status, 19 September 2026 (later the same day):** every item below has a status note. All of sections A, B and C are implemented in the working tree except where a note says otherwise; what remains is listed in `TODO.md`.

The summary: the native engine is in good shape (the fuzzing above found nothing in the reader or in vacuum batching), but the 0.10.0 facade gives up on it far more often than it has to, and two of the fallbacks are worse than what they replaced. The release process also let a stale README onto npm. Items are grouped; within a group they are ordered by value.

## A. Defects

1. **npm shows the 0.9.0 README for 0.10.0. Priority: high. Reproduced.**

   **Status:** Implemented on 19 September 2026: `scripts/finalize-pkg.mjs` assembles `pkg/` from scratch out of `target/wasm-glue` (idempotent, refuses anything that is not generated glue) and ships `README.md` and `LICENSE.txt`; `package_contract.mjs` asserts the packed README is the current one, names the version, and that reassembling changes nothing. Publishing 0.10.1 is left to the maintainer.

   **Evidence:** `scripts/finalize-pkg.mjs:19` copies `COMPATIBILITY.md` and the upstream licence but never `README.md` or `LICENSE.txt`, and `wasm-pack --no-pack` does not refresh them either. `git show ed9b39c:README.md | diff - pkg/README.md` is empty for the published tarball: the npm page says callbacks throw, "everything runs in Wasm", and documents "Upgrading to 0.9.0". A build from a fresh clone would ship no README at all. The script is also not idempotent: it reads `pkg/minisearch_wasm.js` as core glue and then overwrites that file with the facade, so a second run without `wasm-pack` silently copies the facade into `minisearch_wasm_core.js` (exit 0, then infinite `initSync` recursion on import).

   **Improve:** Build `pkg/` from a clean staging directory, copy `README.md` and `LICENSE.txt`, assert the glue contains `__wbg_`, assert in `package_contract.mjs` that the packed README names the current version, and publish 0.10.1. The upstream maintainer replied in lucaong/minisearch#313 that he may link this project from MiniSearch's README, so the npm page matters now.

2. **An ordinary search during auto-vacuum permanently wedges maintenance, silently. Priority: high. Reproduced on default settings.**

   **Status:** Fixed by item 12: vacuum runs natively, under a port of MiniSearch's scheduler. The reproduction ends with `isVacuuming: false`, `dirtCount: 0`; a random operation mix passes on 5 of 5 seeds. Covered by `wasm_residency.mjs` section 3. The upstream fix is not sent.

   **Evidence:** 0.10.0 routes every vacuum to the bundled JS engine (`js/minisearch.js:226-243`). Upstream's `performVacuuming` iterates the live tree; if a dirty search's lazy cleanup (or `remove`/`replace`) deletes a node while the vacuum is parked between batches, it throws, and upstream never resets `_currentVacuum`. This is upstream's open issue #306 (`Cannot read properties of undefined (reading 'keys')`). The facade adds `promise.then(..., () => {})`, so the rejection is swallowed. Repro: 3,000 documents, `discardAll` of 1,500, wait 5 ms, three prefix searches. Result: `isVacuuming: true` forever, `dirtCount` 1500 and growing, every later `vacuum()` rejects with `fieldsData is not iterable`, and `compact()` always throws. A random operation mix wedges the facade on 5 of 5 seeds. The native vacuum from 0.7.0 (`src/mini_search.rs:1541-1612`) snapshots its term list first and passes the same scripts on every seed.

   **Improve:** Run vacuum natively (item 9). If the JS route stays, reset `_currentVacuum`/`_enqueuedVacuum` on rejection and surface the error. Consider sending upstream a fix for #306.

   **Verify:** Promote the mutation-during-vacuum fuzz to a permanent test at the Wasm boundary.

3. **Release builds write snapshots that the loader rejects. Priority: high. Reproduced through the facade and the core.**

   **Status:** Implemented: readers accept negative field averages and stale postings outside the dirt count (`compact()` drops those instead of refusing); both readers take radix depth 128 (the JSON reader bounds nesting itself); writers check the readers' limits in release builds too (`check_persistable`) and fail with a way out; duplicate `fields` and nonfinite options are refused at construction. `tests/snapshot_robustness.rs` runs in debug and release; `core_robustness.mjs` repeats it through Wasm.

   **Evidence:** Since 0.9.0, `to_bytes` validates only under `cfg!(debug_assertions)` (`src/mini_search/snapshot.rs:273-279`), and every test runs in debug. States the engine reaches organically save successfully and fail at the next startup:
   - negative `averageFieldLength`: 8 documents without `text`, 2 with, remove the 8, discard one. Upstream produces the same `[1, -6]` and reloads it; this port fails with `field averages must be finite and nonnegative`;
   - a version-conflict `remove` (the engine only warns): `stale postings exceed dirt count`. `compact()` then throws forever and auto-vacuum never fires because `dirtCount` is 0;
   - 2,000,001 add/remove cycles on a one-document index: `field dimensions exceed snapshot limits`. Any long-lived index that uses `replace()` without `compact()` gets there;
   - nested-prefix vocabularies (`a aa aaa …`): binary load fails at radix depth 129, native JSON at depth 41 (`recursion limit exceeded`), although the README promises depth 128;
   - duplicate names in `fields`, and `boost: { title: Infinity }` (written as `null`).

   **Improve:** Keep an O(1) check in release writers and fail at save time, and make the reader accept what the engine can produce (negative averages, stale postings with `dirt_count == 0`, auto-compact or lift the ID limit on write). Reject duplicate fields and non-finite options in the constructor.

   **Verify:** A release-mode build → mutate → save → load property test.

4. **A rejected 1 MiB snapshot grows Wasm memory to 1.5 GiB. Priority: high. Reproduced in the published Wasm.**

   **Status:** Implemented: the array reservation is charged to the budget first. The crafted 1 MiB input now fails with `decoded allocation budget exceeded` and Wasm memory grows by less than the budget (asserted natively with a tracking allocator and through Wasm). Bounding `next_id` relative to the document count was not done: removed documents legitimately leave it arbitrarily far ahead.

   **Evidence:** `snapshot.rs:685-690`: for `TAG_ARRAY`, the count is bounded only by the remaining input bytes, then `try_reserve_exact(count)` reserves `count × size_of::<Value>()` before anything is claimed from the 256 MiB budget, and arrays nest 64 deep. A 1.0 MiB input is correctly rejected with `unexpected end of snapshot`, but linear memory goes from 1 MiB to 1,542 MiB (2 MiB input: 3,080 MiB), and Wasm memory never shrinks. Every other reservation in the reader is claimed first.

   **Improve:** `claim(count.checked_mul(size_of::<Value>())?)` before reserving, or cap the pre-reservation. Also consider bounding `next_id` relative to `document_count + dirt_count`: a valid 118-byte snapshot currently allocates about 200 MB on load plus first search.

5. **One trap poisons the module; several inputs trap. Priority: high for the poisoning, medium for the triggers. Reproduced.**

   **Status:** Implemented: fuzzy distances are clamped to the largest value that can change the result and the fallback matrix is allocated fallibly and capped; the fused path takes its scratch out of the thread-local for the query, so an abort cannot leave it borrowed; every string parameter of the core is a checked value (`MiniSearch: … must be a string`); `loadBytes` takes an `ArrayBuffer` or any view; `autoSuggest` and the compact searches accept query trees and the wildcard through the facade.

   **Evidence:** On wasm32 a panic aborts without unwinding, so `RefCell` borrow flags are never released.
   - `search('a'.repeat(70), { fuzzy: 6e7 })` reaches `vec![sentinel; rows*columns]` in the banded-matrix fallback (`src/searchable_map.rs:175-178`) and throws `RuntimeError: unreachable`. Afterwards `add` on that instance traps forever. The same trigger through `searchJoinedOpts` leaves the thread-local `SCRATCH` locked: `searchJoined`, `searchRaw` and `autoSuggest` then trap on **every other and every new instance** in the module.
   - Every export that takes `&str` traps with `memory access out of bounds` when given a number, object or symbol, because wasm-bindgen's `passStringToWasm0` does no type check (`malloc(undefined)` returns a dangling pointer, then `realloc` reads in front of it). The heap survives this one. Through the facade: `autoSuggest(MiniSearch.wildcard)` and `autoSuggest({ queries: [...] })` trap, while upstream returns suggestions for both; `searchJoined(42)`, `searchRaw({})`, `autoSuggestJoined({})` trap.
   - `loadBytes(arrayBuffer)`, the natural result of `fetch().arrayBuffer()`, reports `unexpected end of snapshot` instead of a type error.

   **Improve:** Clamp `max_distance` (or `try_reserve` and return `Err`); take the `Scratch` out of its cell for the duration of a query so a trap cannot poison the global; take `JsValue` and check `as_string()` in the exports, or guard `typeof query === 'string'` in the facade; give `autoSuggest` the same query parser as `search`; accept `ArrayBuffer` in `loadBytes`.

6. **After a transfer to JS, documents without stored values lose their `{}`. Priority: medium. Reproduced; found by upstream's own test suite.**

   **Status:** Implemented in the facade: a transfer gives every document its `{}` when `storeFields` is set, and `toJSON()` serializes it, like MiniSearch.

   **Evidence:** The native `toJSON` omits `storedFields` entries that are empty. With `storeFields: ['cat']` and a document that has no `cat`, upstream passes `{}` to `boostDocument` and returns `{}` from `getStoredFields`; the facade passes `undefined`, so `(id, term, stored) => stored.cat === 'x' ? 2 : 1` throws `Cannot read properties of undefined`, and `getStoredFields(1)` is `undefined` for a live document.

   **Improve:** Emit `{}` for every live document when `storeFields` is configured (as upstream's `saveStoredFields` does). `toJSON` also sorts keys, which is harmless but means `JSON.stringify(index)` is not byte-equal to upstream's.

7. **Compact paths mis-score repeated query terms that expand differently. Priority: medium. Reproduced.**

   **Status:** Implemented: the fused path credits distinct query terms per document with a bit mask (a set beyond 64 terms). `autoSuggest('foo bar foo', { combineWith: 'OR' })` now scores 1.687824161083053, MiniSearch's value.

   **Evidence:** The fused pass assumes "duplicate query terms touch identical doc sets" (`src/mini_search.rs:2283-2303`), which is false when the occurrences have different prefix/fuzzy settings: per-term arrays, or `autoSuggest` with `combineWith: 'OR'`, where only the last term is prefix-expanded. With documents `foobar bar`, `foo bar`, `foobar`, `bar baz`, `autoSuggest('foo bar foo', { combineWith: 'OR' })` scores "bar foobar" 1.6878 in JS and 0.8439 here; `searchJoinedOpts`/`searchRaw` with `prefix: [false, false, true]` disagree with this package's own `search()` in the same way.

   **Improve:** Track credited query terms per document as a bitmask (`u64` with a slow path) and use its popcount for the quality factor. Add duplicate-term queries with per-term settings to `compat_parity.mjs`.

8. **`removeAll(oldDocuments)` is quadratic since flat postings. Priority: high for rolling-window indexes. Reproduced.**

   **Status:** Implemented more generally than proposed: posting lists keep emptied entries as tombstones and sweep them once they outnumber the live ones, so every removal pattern is amortized constant per posting. Removing the oldest 50,000 of 100,000 documents one by one: 48.7 s before, 0.59 s after, identical snapshots (`tests/bulk_removal.rs`); `bench_maintenance.mjs` has a removal case.

   **Evidence:** `Postings::decrement` calls `Vec::remove(index)` (`src/mini_search.rs:175-187`), shifting the tail of every common term's list for every removed document. Removing the oldest half: 50,000 documents 4,275 ms vs 797 ms upstream; 100,000 documents 35 s vs 1.8 s. Removing the newest half in reverse order is faster than upstream, which confirms the cause.

   **Improve:** Batch in `remove_all`: collect ids per (term, field) and run one `retain`, or tombstone through `alive` and sweep once, as vacuum does. Add a removal case to `bench_maintenance.mjs`, which has none.

9. **The declarative `filter` treats `4.0` and `4` as different. Priority: medium. Reproduced.**

   **Status:** Implemented: numbers are compared by value, recursively.

   **Evidence:** `src/mini_search.rs:1769-1777` compares `serde_json::Value`s, where `PosInt(4) != Float(4.0)`. After `addAllJSON('[{"id":1.0,"t":"foo","rating":4.0}]')`, `filter: { rating: 4 }` returns 0 hits while `r => r.rating === 4` returns 1; the mismatch survives `toBytes`/`loadBytes`. Python, Go and Rust serializers emit `4.0` routinely.

   **Improve:** Compare numbers through `as_f64()`, recursively.

10. **Smaller divergences from upstream in Wasm mode. Priority: low. Reproduced with a side-by-side probe (16 of 43 probes differ).**

    **Status:** Implemented: a zero `boost` means none; lone surrogates, `-0`, non-array or repeated `fields`, stored fields named like result properties and `Infinity` in options select the JavaScript engine; `replace` checks the thresholds between its discard and its add; the core's queued vacuum rechecks its conditions, uses the queued options and starts synchronously like MiniSearch's. Not changed: `fuzzy: 1.5` (MiniSearch indexes a typed array with the fraction and finds nothing; an edit distance is the more useful reading), `removeAll()` resetting `dirtCount` (documented in COMPATIBILITY.md), and the raw core keeping `String(value)` for a field that is both indexed and stored (the facade never sends it such a value).

    - `boost: { title: 0 }`: upstream uses `|| 1`, this port multiplies by 0.
    - Duplicate names in `fields` double the score (upstream: unchanged).
    - `fuzzy: 1.5` matches here and not upstream; `fields: 'title'` (a string) works upstream and throws here.
    - Lone surrogates become U+FFFD in stored values and ids, so `has('x�')` is true here; `-0` ids and values come back as `0`.
    - A stored field named `score` overrides the computed score upstream *before* sorting; here it only overrides the property.
    - `removeAll()` resets `dirtCount`; upstream keeps it (this port's behaviour is arguably better; document it).
    - `replace` checks the auto-vacuum thresholds after the add, upstream inside `discard` before it, so the dirt factor differs by one document at the boundary.
    - Core-only: an enqueued auto-vacuum reruns unconditionally with the first call's options (`src/lib.rs:756-791`; 1,121 ms vs 641 ms for one extra `discard`), and a field that is both indexed and stored keeps `String(value)`.

    **Improve:** Route the unrepresentable inputs to JS mode in `needsValues` (`isWellFormed()`, `Object.is(value, -0)`), fix the option semantics natively, and add these probes to `public_api.mjs`.

## B. Stop leaving Wasm

The README is candid that "an application that updates documents regularly will run mostly on the JavaScript engine". Measured here on 20,000 documents, the transfer stalls for **1.46 s** and is permanent, and these everyday calls each trigger it: one `search` with a `filter`, `prefix`, `fuzzy` or `boostTerm` function; `getStoredFields`; `vacuum()` even on a clean index; one `replace` followed by a search; `loadJSON`. For most real MiniSearch users the package therefore behaves as upstream plus an unused 737 KB Wasm file. This group has the largest payoff.

11. **Evaluate search callbacks on the JavaScript side instead of transferring. Priority: high. Prototype verified.**

    **Status:** Implemented, including query-tree nodes, constructor-level defaults, suggestions and the compact forms. Options explicitly set to `undefined` follow MiniSearch's object-spread semantics. `boostDocument`, `tokenize` and `processTerm` still transfer.

    **Evidence:** Upstream applies `filter` to finished result objects before sorting, and calls `prefix`/`fuzzy`/`boostTerm` once per query term with `(term, i, terms)`. The native core already accepts per-term arrays. A prototype that tokenizes the query with upstream's default `tokenize`/`processTerm`, turns the functions into arrays and post-filters the Wasm results matched upstream on 20 of 20 query/option combinations (ids, order, scores within 1e-12, `match`) and stayed in Wasm: 44 ms against the 1,456 ms transfer. `autoSuggest` with a filter is `search` plus a 15-line aggregation.

    **Improve:** Do this in `search`/`autoSuggest`/`searchJoinedOpts`/`searchRaw`, including query-tree nodes. Only `boostDocument` and search-time `tokenize`/`processTerm` still need the JS engine, and those could use a temporary JS index rather than a permanent transfer.

12. **Answer dirty queries and run vacuum natively. Priority: high. Analysed; first-query difference reproduced.**

    **Status:** Implemented as proposed (`src/mini_search/lazy_cleanup.rs`), with one refinement: the non-mutating fast path runs first and the mutating replica only when a stale posting was actually met. It also showed that the native vacuum had emptied terms in sorted order, not MiniSearch's tree order, which changed the radix key order in 27 of 300 random histories; fixed.

    **Evidence:** The transfer exists to reproduce upstream's first dirty query. Upstream starts `matchingFields` at the posting count *including* stale entries and decrements it as it meets them, so live documents ahead of a stale posting are scored with an inflated document frequency: twelve documents, three discarded, `search('apple')` gives upstream scores from 0.077 down to **−0.335** on the first query and 0.077 for all nine on the second. The core always returns the second answer. Note also that upstream's lazy `removeTerm` lowers a stale posting's frequency by one per query, so the "fixpoint" needs more than the two runs `differential/README.md` prescribes when a frequency exceeds 2.

    **Improve:** Emulate the quirk in `term_results`/`accumulate_dense` on the dirty path only: a running count in posting order, a per-query set of already cleaned (term, field) pairs, and physical removal after the query (the wasm-bindgen wrapper already holds a `RefCell`). `public_api.mjs` already compares unwarmed first dirty queries, so the oracle exists. Then drive the native `begin_vacuum`/`vacuum_step` from a JS copy of upstream's 30-line active/queued scheduler to keep the Promise identities, iterating in tree order so mid-vacuum queries match. This removes item 2 as well. A cheaper interim step is an opt-in such as `dirtyQueries: 'native'`.

13. **Remove the remaining involuntary transfers. Priority: medium. Reproduced.**

    **Status:** Implemented: a vacuum of a clean index stays native; `getStoredFields` hands out one tracked object per document and transfers only when one was edited; `loadJSON`/`loadJSONAsync` use the native importer and fall back to MiniSearch's loader for what it refuses; `extractField`, `stringifyField` and `logger` stay in Wasm; a transfer no longer builds the term index twice. Open: a pre-tokenized `add` path, or declarative `stopWords` / diacritic folding, for `tokenize` and `processTerm`.

    - `vacuum()` on a clean index transfers permanently; return a resolved Promise after a tick instead.
    - `getStoredFields(id)` transfers to hand out a live object. In Wasm mode stored values are scalars only, so a JS-side `Map` of handed-out objects (written back on the next query, or simply owning stored fields in JS) preserves identity and mutation visibility. Owning stored fields in JS would also make `search()` cheaper, since they would no longer be cloned across the boundary per hit.
    - `loadJSON`/`loadJSONAsync` always produce a JS index, although the 0.9.0 importer (`src/mini_search/interop.rs`) gives exact parity with `Original.loadJSON` on clean indexes (ids and order identical on 18,597 hits, scores within 5e-16). At HEAD `from_minisearch_json` is unreachable from the public package: wire it in (with a fallback for object ids) or drop it to save Wasm bytes. It is slower than upstream's loader (355 ms vs 248 ms on 20,000 documents) and stricter than upstream in three ways listed under item 3.
    - Index-time callbacks: `extractField` and `stringifyField` run once per field, so the facade can call them and hand flattened documents to Wasm; `tokenize`/`processTerm` would need a pre-tokenized `add` path. Declarative `stopWords`, `minTermLength` and diacritic folding would cover most real `processTerm` functions.

## C. Release engineering and tests

14. **One gate, run by CI, before publish. Priority: high. Carried from both earlier reviews and now shown to matter (item 1).**

    **Status:** Implemented: `npm test` runs everything, `publish:pkg` runs it before publishing, `.github/workflows/ci.yml` builds and tests on push and pull request, `rust-toolchain.toml` pins 1.96.0 with the wasm target, `engines` is set, `check:separators` compares the table and not the Node version, and the bulk and autosuggest differentials are wired as `test:differential`. Open for the maintainer: push tags `v0.9.0` and `v0.10.0`.

    - No `test` script, no `prepublishOnly`; `publish:pkg` is build + publish. `test:wasm` never rebuilds, so a stale `pkg/` passes. No workflow is checked in.
    - Tags `v0.9.0` and `v0.10.0` were never pushed (`git ls-remote --tags` ends at `v0.8.0`), so the `blob/v0.10.0/...` links in README, COMPATIBILITY.md (shipped in the package) and RELEASE_NOTES are 404, as is the release asset the benchmark report cites.
    - Pin the toolchain (`rust-toolchain.toml`, wasm-pack and wasm-bindgen versions that `finalize-pkg.mjs` depends on, an `engines` field). The strict Clippy gate already fails on Rust 1.98.
    - `check:separators` embeds `process.versions.node` in the compared header (`scripts/gen-separators.mjs:29-30`), so it fails on any Node other than 24.21.0 even though Node 26 generates an identical table.
    - Never run by any script: the bulk differential (passes by hand: 765,678 rows, max relative delta 5.5e-16, 0 tie reorders), the autosuggest comparator, `bench_wasm.mjs`, the browser contract.

15. **Run upstream's test suite against the facade. Priority: medium. Prototyped.**

    **Status:** Implemented as `npm run test:upstream` (`upstream-suite/`): MiniSearch's two test files run unmodified against `pkg/`, with the 14 tests that read private state or deep-compare instances listed as expected failures. A listed test that passes or disappears fails the run. 151 pass; the control run against MiniSearch passes 165 of 165. Read-only private getters were not added.

    **Evidence:** `reference-tests/` is byte-identical to upstream 7.2.0 but is only a reading copy. Swapping the import and running it under Vitest (`globalThis.jest = vi`) takes 2 seconds and found item 6, which 102,060 parity checks had not. Thirteen failures read private fields (`_dirtCount`, `_index`, …); one awaits `ms._currentVacuum`, which is `undefined` on the facade and spins forever, so exposing read-only `_currentVacuum`/`_enqueuedVacuum`/`_dirtCount` getters would be cheap compatibility for code written against upstream.

    **Improve:** Check in the runner with an explicit skip list for the private-field tests.

16. **Tighten the existing suites. Priority: medium. Partly reproduced.**

    **Status:** Implemented: a trap no longer counts as a rejection, bare `assert.throws` calls have matchers, `public_api.mjs` and `wasm_residency.mjs` compare scores exactly, the path handling and the unbounded wait are fixed, both benchmarks assert their checksums and the engine they measured, and `compare_bulk.mjs` has type-aware keys, duplicate and nonfinite checks and fails on reordered ties. The share of empty-versus-empty comparisons in the older suites' counts was left as it is.

    - `high_priority_regressions.mjs:171-181` accepts any `Error`; `WebAssembly.RuntimeError` is one, so a trapping truncation probe passes.
    - Bare `assert.throws` without a matcher in `compat_parity.mjs` and `high_priority_regressions.mjs`.
    - `public_api.mjs:246` uses `new URL(...).pathname` and fails when the checkout path contains a space (reproduced); `:10` rounds scores to 1e-10 absolute.
    - 12-20% of the advertised "explicit checks" compare empty arrays with empty arrays.
    - `bench_maintenance.mjs` and `bench_wasm.mjs` import the facade but label results "Wasm": today they print `Wasm vacuum 384 ms` vs `JS vacuum 66 ms` and `-508.8% reclaimed`, then `VERIFIED`, while the README still claims 122 ms and a 20% smaller snapshot. `bench_wasm.mjs` prints checksums without asserting them.
    - `tests/` covers cache invalidation only for `add` and `remove`, and vacuum only with `discard` during a run. The reviewers' scratch fuzzers (all mutations × partial `vacuum_step` × `compact`, warm vs cold clone) found nothing and would make good permanent tests.

17. **Packaging. Priority: medium. Reproduced with the repo's esbuild and TypeScript.**

    **Status:** Implemented: a merged namespace in `minisearch_wasm.d.cts` exports every named type; the Node entry warns and falls back to JavaScript when the `.wasm` file is missing; browser bundlers resolve `require` to the fetch-based entry; the global script is one minified function scope (154 KB to 64 KB) that defines only `MiniSearch`; the declarations compile without the DOM library; MiniSearch's declarations ship in the package and it is no longer a dependency; `npm run check:size` enforces `scripts/size-budget.json`. The Wasm file itself grew to 786 KB with this work; shrinking it is open.

    - TypeScript CommonJS consumers cannot import named types: `import type { SearchResult } from 'minisearch-wasm'` in a `.cts` file fails with TS2305, because `minisearch_wasm.d.cts` is a bare `export =`. The same import from `minisearch` compiles, so "replace the import" breaks for them.
    - The Node entry crashes at import when bundled (`ENOENT` for the `.wasm`), although the facade has a JS mode to fall back to. `require` has no browser branch (`Could not resolve "node:fs"`). There is no Vite/webpack/esbuild section, and a bundled browser build 404s on a bare `init()`.
    - MiniSearch is inlined into the ESM, CJS and global bundles **and** declared as a runtime dependency that only the `.d.ts` files use; the main bundle contains two `SearchableMap` classes. The global bundle is an unminified 154 KB IIFE that leaks `MiniSearchWasmModule` and throws if loaded twice.
    - The declarations need the DOM lib (`RequestInfo`, `WebAssembly`). `MiniSearch.name` is `"MiniSearch2"` in the bundle.
    - Size budget (open since the second review): the Wasm is now 737,513 bytes, up from 718,724.

18. **Documentation hygiene. Priority: low.**

    **Status:** Implemented: the README is rewritten for the current tree with one benchmark section, `CHANGELOG.md` added, the licence names the port's author next to MiniSearch's, `codex.md` is marked historical, the generated `IMPROVEMENTS*.html` are removed.

    The README has two benchmark sections with different 0.10.0 numbers for the same calls (`searchJoined` 8.2× and 4.48×) and a table from a benchmark that is not in the repository; it still tells readers to `npm install` in `differential/` (unnecessary) and calls `IMPROVEMENTS.md` "open review items" although its status notes are obsolete. `IMPROVEMENTS*.html` are 97 KB of generated duplicates. `codex.md` describes `Vec<u16>` lengths. There is no cumulative changelog. `LICENSE.txt` is upstream's file verbatim, without a copyright line for this port.

## D. Looking at upstream

- **Nothing to catch up on:** upstream's last release is 7.2.0 (September 2025). Open upstream PRs worth tracking: #305 (filter in sub-queries) and #295 (partial `weights` type; the facade already accepts partial weights at runtime, but its re-exported types do not).
- **Something to give back:** the native vacuum is immune to upstream #306 (item 2). A fix for upstream (iterate a snapshot of the terms, reset `_currentVacuum` on failure) would be a natural follow-up to #313.
- **Bit-exact scores (implemented):** a sweep of small clean indexes showed 506 of 11,480 scores differing from MiniSearch in the last bit through Wasm. Rust's wasm32 `ln` is musl's variant of fdlibm, V8 uses the original; `src/js_math.rs` ports the original and is proven against Node on 1,894 inputs. The sweep now shows 0 differences and the Wasm suites compare scores exactly. Originally: It reorders near-ties in long suggestion lists (20,000 documents, `autoSuggest('alp')`: the same 547 suggestions, first order difference at position 504). Porting fdlibm's `log`, which V8 uses, would remove the tolerance from every comparator.
- **Showcase:** upstream's `examples/` demo and `benchmarks/` corpus are reusable for a hosted demo and a benchmark that readers can reproduce, which the README's historical table is not.

## Suggested order

1. 0.10.1: items 1, 6, the `typeof` guards from 5, the Clippy fix, tags, and `prepublishOnly` (hours).
2. Robustness: items 3, 4, 5 (trap poisoning), 8, 7, 9.
3. 0.11.0: items 11 and 13, then 12, which together keep ordinary applications in Wasm.
4. CI, the upstream suite runner, packaging.
