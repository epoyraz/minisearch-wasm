# MiniSearch WASM vs MiniSearch 7.2: second review

Review date: 16 September 2026, evening. Reviewed the current working tree of `minisearch-wasm` 0.9.0 (uncommitted on top of `d7b87fd`, 0.8.0) against MiniSearch 7.2.0 (`../minisearch`, also the pinned oracle in `differential/`).

Since the morning review (`IMPROVEMENTS.md`) the four high-priority items were implemented as 0.9.0, a performance pass landed (expansion cache with posting handles, inline snapshot validation, faster varint decoding), and a parity pass aligned tie order, `terms`/`match` order, number formatting, UTF-16 lengths, the `has`/`replace`/`getStoredFields` API and MiniSearch JSON import/export with the JavaScript library. Ranking, result order and suggestions are now verified identical by `differential/compat_parity.mjs` (102,060 checks). This review looks at what still separates the port from the original: error handling, unsupported options, value handling, Unicode tables, the remaining API surface, types, Node loading, package size, the release gate and the compatibility path's speed. Items 1 to 7 and 10 were implemented later the same day; see the **Status** note under each. Items 8 and 9 remain open. Items 5 (memory bounding under churn) and 6 (synchronous work inside async maintenance) of the morning review remain open and are not repeated here.

Validation performed on Windows with Node 24.21.0, Rust 1.96.0, wasm-pack 0.15.0 and wasm-bindgen 0.2.128:

| Check | Result |
| --- | --- |
| `cargo test --locked` | 62 tests passed |
| `cargo clippy --locked --all-targets -- -D warnings` | Clean |
| `npm run test:wasm` | Smoke passed; 30,764 high-priority regression checks; 102,060 compat parity checks |
| Behavior probes against JS MiniSearch 7.2.0 through the built package | 21 probes: 5 identical, 16 differ in behavior, error shape or API presence (detailed below) |
| Package size experiments | 3 scratch builds (without `float_roundtrip`, `panic = "abort"`, `opt-level = "s"`) |
| Node install fixture | Plain `await init()` fails; passing the `.wasm` bytes works |

1. **Throw real `Error` objects with MiniSearch's messages. Priority: high. Confirmed compatibility defect.**

   **Status:** Implemented on 16 September 2026 (same day, after this review): every error is a JS `Error`; serde-produced messages are translated to MiniSearch's wording (`option "fields" must be provided`, `Invalid combination operator: X`). Verified by `differential/api_parity.mjs`.

   **Evidence:** Every failure crosses the boundary as a plain string: `src/lib.rs` maps errors with `JsValue::from_str` at 38 sites. `typeof e === "string"`, so `e.message` is `undefined`, `e.stack` does not exist and `e instanceof Error` is false; a consumer's `catch (e) { log(e.message) }` logs nothing. Where serde produces the text, the message also diverges: constructing without `fields` throws `Error: missing field \`fields\`` (JS: `MiniSearch: option "fields" must be provided`), and `combineWith: "XOR"` throws `Error: invalid combineWith value` (JS: `Invalid combination operator: XOR`). The port's own messages (`duplicate ID 1`, `cannot remove document with ID 9: it is not in the index`) already match JS word for word. The regression suite currently asserts `typeof error === "string"`.

   **Improve:** Convert every error to a `js_sys::Error` (a `TypeError` where JS throws one) whose `message` is MiniSearch's text; translate the constructor and option deserialization errors into MiniSearch's wording and drop the `Error:` prefix serde adds; keep wasm-specific messages under the `MiniSearch:` prefix.

   **Verify:** Extend `compat_parity.mjs` with `instanceof Error` and message equality for duplicate add, remove/discard of a missing id, missing `fields`, invalid `combineWith` and invalid queries; update the regression tests that assert strings.

2. **Reject callback options instead of ignoring them. Priority: high. Confirmed silent divergence.**

   **Status:** Implemented: function-valued options throw an `Error` naming the option and its alternative (constructor, per call, query-tree nodes, `autoSuggest`); `prefix`/`fuzzy`/`boostTerm` accept per-term arrays and `filter` a stored-field object, verified against the JS callbacks.

   **Evidence:** Function-valued search options are dropped by the option parser without notice, so the same call returns different results than JS: `search("hello", { filter: () => false })` returns 1 hit (JS: 0); `search("hello", { boostDocument: () => 2 })` scores 0.4315 (JS: 0.8630). `boostTerm` goes the same way. `prefix` and `fuzzy` given as functions, which MiniSearch allows (`prefix: (term, i, terms) => boolean`), fail with a serde type message (`invalid type: JsValue(Function(prefix)), expected a boolean`). Constructor callbacks (`extractField`, `stringifyField`, `tokenize`, `processTerm`, `logger`) are likewise accepted and ignored.

   **Improve:** Detect function-valued keys in the constructor and search options and throw a clear error naming the option and the reason (callbacks do not cross the Wasm boundary), with a pointer to the supported alternative. Offer declarative forms for the two options that are usually functions: per-term arrays for `prefix` and `fuzzy` (`prefix: [false, true]`), and a stored-field equality filter. `filter`, `boostDocument` and `boostTerm` stay unsupported but loud.

   **Verify:** A probe per callback option asserts a thrown error with the option name; declarative forms are compared with JS results computed through the equivalent callbacks.

3. **Stringify field values the way MiniSearch does. Priority: high. Confirmed indexing defect.**

   **Status:** Implemented: documents are converted through a schema so indexed object values (`Date`, `toString()`, arrays with objects) become `String(value)`; primitives, ids and stored-only fields stay JSON; function properties are skipped. A field that is both indexed and stored keeps the string form.

   **Evidence:** MiniSearch 7.2 indexes `stringifyField(value)`, by default `value.toString()`. A document whose field holds a `Date` or an object with `toString()` indexes the string form in JS (a `Date` becomes `thu jan 01 1970 ...`; a custom `toString` yields `custom`). Through `add`/`addAll` the port converts the whole document with `serde_wasm_bindgen::from_value`, which rejects any function-valued property (`invalid type: JsValue(Function(toString)), expected any valid JSON value`) and turns a `Date` into `{}`, indexed as `object`. `addAllJSON` is unaffected because JSON text cannot carry these values.

   **Improve:** Convert indexed field values at the boundary with JS semantics: strings, numbers, booleans and arrays pass through; anything else goes through `String(value)` on the JS side before reaching Rust; function-valued properties are skipped rather than fatal. Keep ids and stored fields as JSON values (they are compared and returned, not tokenized). Document that a custom `stringifyField` is not available.

   **Verify:** Parity probes for `Date`, custom `toString`, nested arrays containing objects, documents with methods, and `null`/`undefined` mixes; only unserializable ids may still throw.

4. **Bring the tokenizer to Unicode 16. Priority: medium. Confirmed tokenization difference.**

   **Status:** Implemented: `src/separators.rs` is generated from the running Node's `\p{Z}`/`\p{P}` by `scripts/gen-separators.mjs` (Node 24 reports Unicode 17.0, so the two probe characters are Unicode 16 and 17 additions rather than 16 as stated above); `npm run check:separators` guards it. The Unicode 15 crate is gone.

   **Evidence:** The default tokenizer classifies characters with `unicode-general-category` 0.6.0, which implements Unicode 15.0. Node 24 (ICU with Unicode 16) treats U+1B7F BALINESE PANTI BAWAK and U+10D6E GARAY punctuation as `\p{P}`: JS splits `a᭿b` and `e𐵮f` into `a`, `b`, `e`, `f`; the port keeps them as single tokens. Every other probe matched (final sigma, İ, ǅ, ẞ, soft hyphen, word joiner, zero-width space, tab).

   **Improve:** Move separator classification to a Unicode 16 source (`icu_properties`, or tables generated from UCD 16 at build time), pin the Unicode version in the README, and add the Unicode 16 additions to `tests/tokenization_conformance.rs`. Track the browsers' Unicode version at release time.

   **Verify:** A one-off script compares `is_space_or_punctuation` with Node's `/[\n\r\p{Z}\p{P}]/u` over every code point and reports differences; the differences become a fixed fixture.

5. **Finish the API surface: `getDefault`, `loadJSONAsync`, `logger`, and the `toJSON` naming trap. Priority: medium. Confirmed API gaps.**

   **Status:** Implemented: `getDefault`, `loadJSONAsync`, `logger` (with the `version_conflict` warning) added; `toJSON`/`toJSONString`/`loadJSON` now use MiniSearch's format and the engine's own JSON moved to `toNativeJSON`/`toNativeJSONString`/`loadNativeJSON` (breaking rename, documented in the README).

   **Evidence:** `MiniSearchWasm.getDefault` is undefined (JS returns `"id"` for `idField`); `loadJSONAsync` does not exist. The `logger` option is accepted and ignored: removing a document whose content changed makes JS warn `MiniSearch: document with ID 1 has changed before removal ... can corrupt the index!` with code `version_conflict`, while the port continues silently. `toJSON()` returns the native Rust-struct format, so `JSON.stringify(wasmIndex)` produces something `MiniSearch.loadJSON` cannot read while `toMiniSearchJSON()` produces exactly that; JS users expect `JSON.stringify(index)` to be the portable form.

   **Improve:** Add `getDefault(name)` for the non-callback defaults, `loadJSONAsync` that yields between chunks like JS, and an optional `logger(level, message, code)` callback invoked only for the rare version-conflict warning (not per token, so it does not violate the boundary rule). Decide the `toJSON` naming before 1.0: either make `toJSON()` emit the MiniSearch format and rename the native one, or document the difference prominently in the API section.

   **Verify:** An API checklist test enumerates MiniSearch's public members and asserts presence or a documented exclusion; a remove-after-change test captures the logger call.

6. **Ship accurate TypeScript declarations. Priority: medium. Confirmed consumer-facing defect (carried from the morning review).**

   **Status:** Implemented: typed declarations via wasm-bindgen annotations plus a custom TypeScript section, optional arguments declared through interface merging, and strict fixtures in `types/` compiled with TypeScript 7.0.2 (`npm run test:types`).

   **Evidence:** The generated `pkg/minisearch_wasm.d.ts` contains 39 `any` types. `search(query: any, options: any)`, `searchRaw(query: string, options: any)`, `autoSuggest(query: string, options: any)` and `removeAll(documents: any)` declare optional runtime arguments as required, so the README's one-argument calls fail with `TS2554`; today's additions (`has`, `getStoredFields`, `replace`, `loadMiniSearchJSON`) are typed `any` as well. TypeScript 7.0.2 is current and the project has no TypeScript dependency.

   **Improve:** Generate a typed facade at build time: constructor and search options, query trees, `SearchResult`, `Suggestion`, joined and raw result shapes, typed arrays, `Promise<void>` maintenance methods, and correct optionality. Compile strict fixtures containing the README examples against the packed artifact with TypeScript 7.

   **Verify:** `tsc --strict` on the fixtures, including expected failures for misspelled options and wrong result-field access.

7. **Make Node loading work out of the box. Priority: medium. Confirmed installation defect (carried, now documented).**

   **Status:** Implemented: `finalize-pkg.mjs` adds `minisearch_wasm_node.js` and a conditional `exports` map, so a bare `await init()` works in Node while browser bundles keep the web loader; verified with a scratch install.

   **Evidence:** Verified today against a scratch install of the packed package: plain `await init()` fails with `TypeError: fetch failed` because the web-target loader fetches a `file:` URL; `init({ module_or_path: readFileSync(new URL(import.meta.resolve("minisearch-wasm/minisearch_wasm_bg.wasm"))) })` works. The README now documents the workaround, but MiniSearch itself needs no setup step anywhere.

   **Improve:** Make `init()` fall back to reading the file when it detects a `file:` URL under Node (dynamic `import("node:fs")`), or add a `minisearch-wasm/node` conditional export built with `--target nodejs`. Keep browser bundles free of Node imports.

   **Verify:** Pack, install into a clean ESM fixture, and run the documented import plus a search in Node, a browser and a worker.

8. **Put the package on a size budget. Priority: medium. Confirmed growth.**

   **Evidence:** `minisearch_wasm_bg.wasm` grew from 580,890 bytes at 0.8.0 to 718,724 bytes today (+24%); brotli 200 KB and gzip 262 KB, against 18 KB gzip for the JS library. Scratch builds: without `float_roundtrip` 684,136 bytes (−4.8%); `panic = "abort"` unchanged; `opt-level = "s"` 615,406 bytes (−14%, speed impact not measured). The rest of today's growth comes from the MiniSearch JSON import/export (`serde_json::Value` construction), `MatchInfo` serialization and float formatting through `format!("{:e}")`.

   **Improve:** Add a size check to the release gate and record sizes per release. Try `opt-level = "s"` or `"z"` with `wasm-opt -Oz`, guarded by the benchmark. Gate the interop code and `float_roundtrip` behind cargo features or a separate entry point so the default bundle stays lean, and write the export through a streaming serializer instead of a `Value` tree.

   **Verify:** A size table per feature in the README; CI fails on unexplained growth above an agreed percentage.

9. **One command and a CI gate for the whole verification. Priority: medium. Confirmed workflow gap (carried, sharpened).**

   **Evidence:** The gate is five commands across two `package.json` files (`cargo fmt`, `cargo clippy`, `cargo test`, `npm run build`, `npm run test:wasm`); `differential/` needs a manual `npm install` first; no CI workflow is checked in; `bench_wasm.mjs` prints checksums without asserting them; the review documents (`IMPROVEMENTS*.md/html`) and three test files are untracked.

   **Improve:** A root `npm test` that runs everything including the differential install and a size check; a GitHub Actions workflow on push and pull request; benchmark checksum assertions; publishing only from CI. Decide whether the review documents are committed or ignored.

   **Verify:** A fresh clone passes with one documented command; the workflow runs green.

10. **Speed up the compatibility `search()` path, or state its cost. Priority: medium. Confirmed measurement gap.**

   **Status:** Implemented: the compat path transfers ids, scores and interned term/field tables and builds result objects in one JavaScript call. Measured 304 ms vs 502 ms in JS (about 1.7× faster; 0.58× before), 236 ms with `includeMatch: false`; `bench_wasm.mjs` now has a compat row and the README row is updated.

   **Evidence:** Measured today on the 20,000-document corpus with prefix and fuzzy queries: full `search()` takes 754 ms for 38 queries against 441 ms in JS (0.58×; 0.8.0 measured 0.56×), and 0.85× with `includeMatch: false`. The README benchmark table claims `~parity (~1.3× faster with includeMatch: false)`, a figure from the jobboard benchmark. Per hit the path allocates a JS object and arrays through `Reflect::set`, interns strings across the boundary, clones stored fields into each `SearchResult`, and builds a `MatchInfo`.

   **Improve:** Either make the compat path cheaper (build results on the JS side from `searchRaw` plus one interned string table, avoid cloning stored fields, skip `match` construction unless requested), or document the compat path as a drop-in convenience with the measured range. Update the README row either way.

   **Verify:** Add the compat `search()` to `bench_wasm.mjs` and re-measure on both corpora; the README table reflects the new numbers.
