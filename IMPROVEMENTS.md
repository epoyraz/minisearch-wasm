# MiniSearch WASM: 10 improvements

Review date: 16 September 2026. Reviewed `minisearch-wasm` 0.8.0 at commit `d7b87fd44268a8b7a306a0c33a43717f1755f72d`, against MiniSearch 7.2.0.

The port has a substantial foundation: a Rust search engine, compact WASM result formats, incremental maintenance, and differential tests. The highest-value next work is to close correctness gaps around field lengths, persistence, and result encoding, then improve memory behavior and the package consumers receive.

This review respects the documented decisions to omit JavaScript callbacks, use a separate snapshot format, and allow different ordering of equal-score results. The findings below concern additional behavior or opportunities to improve the implementation. No implementation changes were made during the review itself. Items 1 to 4 were implemented later the same day as version 0.9.0; see the **Status** note under each and the README's "Upgrading to 0.9.0" section. Items 5 to 10 remain open.

Validation performed on Windows with Node 24.21.0, Rust 1.96.0, wasm-pack 0.15.0, and TypeScript 5.8.2:

| Check | Result |
| --- | --- |
| `cargo test --locked` | All 47 integration tests passed |
| `cargo fmt -- --check` | Passed |
| `npm run build` | Release WASM package rebuilt successfully |
| `node differential/wasm_smoke.mjs` | All checks passed |
| Fresh 3,000-document bulk differential | All 1,500 labels / 765,678 rows matched under the existing comparator; maximum relative score delta `5.53e-16` |
| Additional targeted probes | Reproduced the issues described below through the rebuilt WASM package |

The passing bulk suite and failing edge cases are compatible: the generator currently gives every document populated fields and numeric IDs. Browser rendering, bundler integration, and the external jobboard performance claims were not measured in this review. Source line references below refer to the reviewed revision.

1. **Preserve exact field-length accounting. Priority: high. Confirmed correctness issue.**

   **Status:** Implemented in 0.9.0 (same day, after this review): `field_length` is now `u32` with a separate `field_present` table, null/missing fields are skipped on add, remove and discard, and the default tokenizer counts boundary empty tokens like the JS reference. Covered by `tests/high_priority_regressions.rs` and `differential/high_priority_regressions.mjs`.

   **Evidence:** [Tokenization](src/mini_search.rs#L2717) drops empty tokens before [field lengths are counted](src/mini_search.rs#L832). The JS reference counts the raw split tokens, including a boundary empty token. Indexing `{id: 1, text: "apple"}` and `{id: 2, text: " apple "}`, then searching `apple`, produces JS scores `0.3000709` and `0.2528976`; WASM gives both `0.2734823`.

   There are related gaps in the same accounting model. Null fields are indexed as empty strings instead of being skipped. [Discard](src/mini_search.rs#L933) subtracts a zero-length contribution for an absent field, whereas JS skips that field. With documents `{id: 1, text: "apple"}` and `{id: 2}`, discarding ID 2 leaves the average field length at `2` in WASM and `1` in JS.

   [Lengths are also clamped to `u16`](src/mini_search.rs#L2342) while averages use the original length. A 70,000-unique-term field is stored as 65,535; discarding it from a two-document index leaves the surviving one-term field with an average of `4466`, instead of `1`. This changes later scores as well as the long document's score.

   **Improve:** Preserve field presence separately from length, skip null fields consistently, and calculate lengths using the reference tokenizer's counting semantics. Store exact lengths with `u32` or a compact representation with an overflow table. Update the snapshot format when changing its representation.

   **Verify:** Compare scores and averages against JS for leading/trailing separators, empty/null/missing fields, and lengths around 65,535, before and after remove, discard, vacuum, and reload. These should match within the established floating-point tolerance.

2. **Preserve radix-tree traversal order in binary snapshots. Priority: high. Confirmed correctness issue.**

   **Status:** Implemented in 0.9.0: binary snapshot version 4 (`src/mini_search/snapshot.rs`) writes radix nodes with their child order and explicit leaf position, so prefix/fuzzy match order and suggestions are identical after `loadBytes(toBytes())`; a reloaded index re-serializes to the same bytes.

   **Evidence:** [Binary serialization](src/mini_search.rs#L1649) sorts terms, and [loading](src/mini_search.rs#L1775) rebuilds the tree in that sorted order. This loses the insertion-sensitive child and leaf order that [SearchableMap deliberately maintains](src/searchable_map.rs#L11).

   With one document containing `"apply application apple"`, `searchJoined("a", false)` returns terms `"apple application apply"` before saving and `"apply application apple"` after `loadBytes(toBytes())`. `autoSuggest("a")` changes its suggestion text in the same way. This is observable content changing during persistence, beyond the documented equal-score ordering difference.

   **Improve:** Serialize enough tree ordering information to restore child order and leaf positions. A compact tree representation or explicit ordering metadata can retain front-coding benefits while preserving behavior. Reconstructing only from lexically sorted keys is insufficient.

   **Verify:** Compare complete suggestions, ordered matched terms, scores, and result sets before and after binary reload, including prefix/fuzzy queries and indexes with insertions and deletions. Use several insertion permutations of the same vocabulary.

3. **Validate snapshots before constructing an operational index. Priority: high. Confirmed robustness and correctness issue.**

   **Status:** Implemented in 0.9.0: both readers construct the engine through a validated snapshot path (checked arithmetic, allocation budget, depth limits, UTF-8 and complete-consumption checks, document/field/posting consistency). JSON snapshots carry `snapshot_version`. Truncation and byte-mutation probes run natively and through the WASM boundary without a trap.

   **Evidence:** [Binary loading](src/mini_search.rs#L1677) trusts internal counts and indices. It slices `previous_term[..shared]` without checking the shared prefix's length or UTF-8 boundary. A small malformed snapshot with a first-term shared-prefix length of `1` produced `WebAssembly.RuntimeError: unreachable`, rather than a descriptive load error. Counts also feed allocations and narrowing casts without a complete invariant check.

   [JSON loading](src/lib.rs#L474) directly deserializes private engine state. Changing `document_count` to `900` in a valid one-document snapshot is accepted: `documentCount` becomes 900 and the `apple` score becomes approximately `9.59706`. The existing [malformed snapshot tests](tests/serialization_conformance.rs#L73) cover basic invalid syntax, truncation, and version rejection, but not these inconsistencies.

   **Improve:** Introduce a validated snapshot reader with checked arithmetic, bounded allocation and recursion, valid UTF-8 prefix boundaries, and checked integer conversions. Check document counts, ID mappings, field dimensions, finite statistics, and posting order/references, while explicitly allowing valid stale postings on dirty indexes. Give JSON snapshots a versioned schema and use a shared validation step for both formats. Require complete input consumption unless trailing data is explicitly part of the format.

   **Verify:** Corrupt each structural component of a valid snapshot and fuzz both readers. Invalid inputs should return a useful error without a panic, uncontrolled allocation, or an index that silently produces incorrect results.

4. **Make compact document-ID encoding lossless. Priority: high. Confirmed correctness issue.**

   **Status:** Implemented in 0.9.0: `searchJoined(...).ids` and `docIdTable()` are JSON array strings decoded with `JSON.parse`, so types and embedded newlines survive; deleted slots are `null`. A new `idTableVersion` generation changes on every mutation and is carried on each `searchRaw` result. Existing consumers must replace `.split("
")` with `JSON.parse` and rebuild persisted indexes.

   **Evidence:** [Joined results](src/mini_search.rs#L1438) and [the ID table](src/mini_search.rs#L1419) concatenate external IDs with newlines and stringify non-string IDs. The IDs `"a\nb"`, `1`, and `"1"` are accepted as three distinct documents. `search("apple")` preserves all three, but decoding the compact output gives four rows: `["a", "b", "1", "1"]`, despite `count === 3`. `docIdTable()` has the same row misalignment, so `searchRaw()` can resolve a hit to the wrong external ID.

   **Improve:** Keep numeric internal IDs for queries and expose a typed external-ID table once per index revision. Alternatives include JSON for this infrequent table transfer or explicit offsets and type tags for a packed representation. If a joined API intentionally supports only restricted string IDs, enforce and document that restriction. Provide a generation/version marker so callers can detect stale cached ID tables after mutations.

   **Verify:** Round-trip newline-containing strings, empty strings, numeric versus string IDs, and every other supported ID type. Decoded IDs must match the compatibility path exactly, including after discard, re-add, and reload.

5. **Bound retained memory under document churn and diverse queries. Priority: medium. Confirmed growth pattern; capacity recommendation.**

   **Evidence:** [Internal IDs only increase](src/mini_search.rs#L2321), field lengths are dense by internal ID, and [query scratch space](src/mini_search.rs#L165) grows to `next_id`. Vacuum removes postings but does not compact these structures. After ten add/discard/vacuum cycles of 1,000 documents, adding one live document left `documentCount = 1`, `dirtCount = 0`, but `next_id`, field-length slots, and ID-table rows all at `10001`.

   The thread-local scratch vectors retain their largest allocation independently of an individual index's lifetime. Separately, the [expansion cache](src/mini_search.rs#L699) caps the number of query keys at 4,096 per map, but each key can retain a large list of owned term strings; its actual memory cost has no byte budget.

   **Improve:** Add an explicit compaction/rebuild operation that remaps live IDs and invalidates external ID tables through a generation marker. Reusing IDs requires care while stale postings exist. Add a policy for trimming oversized scratch buffers and budget expansion caches by retained bytes or total expansions, with incremental eviction instead of clearing an entire map at capacity.

   **Verify:** Hold the live corpus size constant while repeatedly replacing documents and issuing diverse prefix/fuzzy queries. Report live allocation estimates separately from the WASM memory buffer's high-water mark. Memory should settle within a documented budget after compaction; scores and ID resolution must remain correct.

6. **Bound synchronous work inside asynchronous maintenance APIs. Priority: medium. Confirmed code-path limitation; latency not benchmarked.**

   **Evidence:** [addAllAsync](src/lib.rs#L78) converts the entire JavaScript document array into `Vec<Value>` before returning its Promise. Chunking therefore limits indexing work after conversion, but not the initial copy/parse. [Vacuum initialization](src/mini_search.rs#L988) collects and sorts every term before processing batches, and [each vacuum step](src/mini_search.rs#L1009) budgets terms rather than postings or elapsed time. A single very common term can still require a large uninterrupted scan.

   **Improve:** Convert documents one chunk at a time at the boundary, or provide a worker wrapper that owns the index. Make vacuum traversal resumable without first materializing the full vocabulary, and budget work by postings or elapsed time as well as term count. Preserve the documented partial-mutation and concurrent-vacuum behavior.

   **Verify:** Measure time until the initial call returns and the largest event-loop delay throughout the operation. Include large documents, a large vocabulary, and one term shared by nearly every document. Define a responsiveness budget on named hardware rather than using total completion time alone.

7. **Ship accurate TypeScript declarations for the supported API. Priority: medium. Confirmed consumer-facing defect.**

   **Evidence:** The generated [declarations](pkg/minisearch_wasm.d.ts#L20) expose options and results largely as `any`, and declare several optional runtime arguments as required. Compiling `ms.search("apple")`, `ms.searchRaw("apple")`, `ms.autoSuggest("app")`, and `ms.removeAll()` with TypeScript 5.8.2 produced four `TS2554` argument-count errors. These forms work at runtime and are shown or described as supported.

   **Improve:** Publish a maintained TypeScript facade or custom declarations, generated as part of the build rather than edited in `pkg/`. Define constructor/search options, query trees, suggestion results, compact result shapes, typed arrays, and `Promise<void>` maintenance returns. Mark optional parameters correctly and document the exact supported external-ID types.

   **Verify:** Compile consumer fixtures containing the README examples under strict TypeScript settings. Include expected failures for misspelled option names, invalid query shapes, and incorrect result-field access. Run this against the packed npm artifact so the published declarations are tested.

8. **Provide a working Node initialization path. Priority: medium. Confirmed installation/documentation defect.**

   **Evidence:** The [README](README.md#L203) says the same static import plus `await init()` works in Node. With the freshly rebuilt web-target package and Node 24.21.0, `await init()` fails with `TypeError: fetch failed` and cause `not implemented... yet...`: the generated loader tries to fetch a local WASM file URL. The smoke test succeeds because it explicitly reads and supplies the WASM bytes.

   **Improve:** Supply and document an official Node loader that reads the packaged WASM bytes, or publish an appropriate Node entry point. Keep browser code free of Node-only imports. The project-local working form is `await init({ module_or_path: readFileSync(wasmPath) })`; package consumers need a supported way to resolve that WASM asset. wasm-pack documents distinct loading behavior for its `web` and `nodejs` targets in its [build target reference](https://wasm-bindgen.github.io/wasm-pack/book/commands/build.html#target).

   **Verify:** Pack the package, install it into a clean Node ESM fixture, and execute the documented import/init/search sequence. Test browser and worker initialization separately. This also catches missing files or package export mistakes that relative imports from `pkg/` can hide.

9. **Make expanded conformance checks an automated release gate. Priority: medium. Confirmed coverage and workflow gap.**

   **Evidence:** No CI workflow is checked into the reviewed repository, and [publish:pkg](package.json#L10) rebuilds and publishes without running the conformance suites. The substantial bulk differential exercises the native Rust engine; the real WASM boundary has a much smaller smoke fixture. The [corpus generator](differential/gen_corpus.mjs#L39) uses populated fields and numeric IDs, which excludes several failures above.

   The [comparator](differential/compare_bulk.mjs#L18) also uses `String(row.id)` as a key, conflating numeric and string IDs, and its duplicate-key branch performs no check. Its score comparison does not explicitly reject non-finite values. These details matter when expanding the corpus.

   **Improve:** Add one verification command and a CI/release workflow covering native tests, WASM build and execution, snapshots, type fixtures, and installation smoke tests. Exercise all search result formats on equivalent inputs. Add seeded mutation/property tests and a snapshot-reload phase; preserve minimized failures as fixed regressions. Make comparator keys type-aware, fail on duplicates, and explicitly reject non-finite scores.

   **Verify:** A clean checkout can run the complete suite with one documented command. The reproductions in this document should fail the current release and pass after their respective fixes. Retain explicit allowances for documented tie ordering and dirty-index semantics.

10. **Make performance claims reproducible across cold, warm, and deployed workloads. Priority: medium. Confirmed measurement gap.**

   **Status:** Partially addressed on 16 September 2026: the checked-in benchmark now frees the instances it builds and loads, which had inflated `loadBytes` from about 16 ms to about 257 ms and made 0.8.0 look slower than JS on load. Corpus reproducibility, `searchRaw` coverage and cold/warm separation remain open.

   **Evidence:** The [headline 11x/23x claims](README.md#L162) depend on a benchmark in a sibling `keyword-search` project, outside the reviewed repository. The checked-in [WASM benchmark](differential/bench_wasm.mjs#L18) warms up and repeatedly runs the same queries, measuring a warmed expansion cache. It does not benchmark `searchRaw`, which carries the largest advertised speedup. Its checksums are printed without an equality assertion, and temporary WASM instances created for build/load measurements are not explicitly freed.

   These observations do not disprove the published measurements. They make the claims harder to reproduce and leave startup, cache misses, memory pressure, and consumer decoding costs insufficiently characterized in this repository.

   **Improve:** Include a reproducible public corpus or deterministic generator, workload description, runtime versions, and benchmark output. Separate module initialization, index load/build, first-query latency, warm repeated queries, and a realistic sequence of typing and edits. Benchmark compatibility, joined, and raw APIs with their required decoding/ID-table work. Validate equivalent results before timing, alternate engine measurement order, and explicitly manage temporary instance lifetimes.

   **Verify:** A fresh checkout can regenerate the published benchmark table. Report sample counts and latency distributions, including p95/p99, plus compressed package/index sizes and peak memory. Keep noisy timing regressions informational on shared CI; use a controlled runner for enforceable performance budgets.
