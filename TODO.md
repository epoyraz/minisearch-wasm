# TODO

What is left after the third review. Item numbers refer to [IMPROVEMENTS-3.md](IMPROVEMENTS-3.md), whose status notes say what was done for each item; [CHANGELOG.md](CHANGELOG.md) lists the changes.

## State of the working tree (19 September 2026)

Everything in sections A, B and C of the review is implemented and **uncommitted**. The gate passed on macOS (x86_64, Node 26, Rust 1.98 natively and 1.96.0 for the Wasm build): `npm run build`, then `npm test`.

The Mac has no global `rustup`: the Wasm build used an isolated toolchain in a scratch directory (rustup 1.96.0 with the `wasm32-unknown-unknown` target via `RUSTUP_HOME`/`CARGO_HOME`, and the `wasm-pack` 0.15.0 release binary). With Homebrew's cargo, `RUSTC=/usr/local/Cellar/rust/<version>/bin/rustc` had to be set for cargo to find its compiler.

## Needs the maintainer

- [ ] Review and commit the working tree (one commit per theme would follow the changelog: engine, facade, packaging, tests, docs).
- [ ] Release: bump the version, publish (`npm run publish:pkg`). npm still shows the 0.9.0 README for 0.10.0.
- [ ] Push tags `v0.9.0` and `v0.10.0`: `RELEASE_NOTES.md` and the benchmark report link to files by those tags (404 today), and the report cites release assets that were never attached.
- [ ] Re-run `npm run bench:public` on the machine the README's numbers come from, and refresh the clean-index table (the numbers in the README's dirty-index table are from the Mac).
- [ ] Optional: send MiniSearch a fix for lucaong/minisearch#306 (its vacuum iterates the live tree and never resets `_currentVacuum` after a throw); a hosted demo built from MiniSearch's `examples/`.
- [ ] First CI run: `.github/workflows/ci.yml` has not run yet. `scripts/size-budget.json` was recorded from a macOS build; a Linux build may need `node scripts/check-size.mjs --update` once.

## Engine

- [ ] **Item 13, remainder** – keep indexes with `tokenize` / `processTerm` in Wasm: a pre-tokenized `add` path, or declarative `stopWords`, `minTermLength` and diacritic folding for the common cases. `boostDocument` needs a callback inside scoring and will stay JavaScript.
- [ ] **Item 17, size** – the Wasm file is 786 KB (0.8.0: 581 KB). Try `opt-level = "s"` with the benchmark as a guard, and put the MiniSearch-JSON interop behind a feature.
- [ ] The raw core stores `String(value)` for a field that is both indexed and stored (item 10). The facade never sends such a value; fixing it needs the engine to take the indexed text and the stored value separately.
- [ ] A long-lived index that never vacuums cannot be saved past 2,000,000 internal id slots; the writer says so and `compact()` cures it. Saving could renumber on the fly instead.
- [ ] The native importer (`loadJSON`) is about 10% slower than MiniSearch's loader on 20,000 documents and refuses a few inputs MiniSearch accepts (the facade falls back to MiniSearch's loader for those).

## Facade

- [ ] `searchRaw`'s `termTable` differs by engine (the native one lists expansion terms no hit refers to); ids and offsets are consistent in each.
- [ ] Truthy and falsy coercions that MiniSearch tolerates (`prefix: 1`, `fuzzy: '0.2'`) throw in Wasm mode.
- [ ] A field named like an `Object.prototype` member (`constructor`, `toString`) indexes the inherited function's text in MiniSearch and nothing here.
- [ ] MiniSearch's own `wildcard` symbol (from a separately installed `minisearch`) is not recognized; `MiniSearch.wildcard` from this package is.
- [ ] Read-only `_currentVacuum` / `_dirtCount` getters would let code written against MiniSearch's private fields keep working (item 15).

## Tests and docs

- [ ] The older suites count empty-versus-empty comparisons in their "explicit checks" totals (item 16).
- [ ] The browser contract (`node differential/serve_browser.mjs`) is still a manual check; a headless run in CI would close the gap.
- [ ] `IMPROVEMENTS.md` and `IMPROVEMENTS-2.md` describe 0.8.0/0.9.0; their status notes were not revised for the facade.
