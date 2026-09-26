# TODO

What is left after the third review. Item numbers refer to [IMPROVEMENTS-3.md](IMPROVEMENTS-3.md), whose status notes say what was done for each item; [CHANGELOG.md](CHANGELOG.md) lists the changes.

## State (27 September 2026): 0.12.0 on branch `next`

0.11.0 is published. The 0.12.0 work is committed on branch `next` and pushed to `origin/next`; it is not merged into `main`, tagged or published yet: ids and stored fields on the JavaScript side, `tokenize`/`processTerm` and `boostDocument` in Wasm, faster `search()`/`addAll`/`loadJSON`, snapshot version 5, `toJSON()` byte-identical to MiniSearch's, the `core-api` build split. [CHANGELOG.md](CHANGELOG.md) has the list, [PORTING.md](PORTING.md) the design, [differential/results/2026-09-27-0.12.0-vs-original.md](differential/results/2026-09-27-0.12.0-vs-original.md) the benchmark. `npm run build` now builds `pkg/` and the engine test package `target/pkg-core`; `npm test` passed on Windows (Node 24.21, Rust 1.96.0, wasm-pack 0.15.0).

To release: merge `next` into `main`, tag `v0.12.0`, and publish the tested `pkg/` with `npm publish ./pkg` from a real terminal (npm needs the maintainer's second factor; from Claude Code it fails with EOTP). Snapshots written by 0.12.0 cannot be read by 0.11.0: consumers that ship prebuilt snapshots (jobboard-web) rebuild them with 0.12.0.

## State (19 September 2026)

Everything in sections A, B and C of the review is implemented and on `main`, in five commits on top of the 0.10.0 release commit `888b991`:

- `6c05f2d` Engine: exact dirty queries, native vacuum order, robust snapshots, linear removal, bit-exact scores
- `64f5364` Facade: keep indexes on the Wasm engine
- `898b930` Packaging and gate: deterministic package, `npm test`, CI, upstream suite, size budget
- `bfecaed` Tests and benchmarks: residency and core-robustness suites, exact scores, asserted benchmarks
- `3535732` Docs: third review with status notes, changelog, README and compatibility guide

Where the details are: [CHANGELOG.md](CHANGELOG.md) (what changed), the status note under every item of [IMPROVEMENTS-3.md](IMPROVEMENTS-3.md) (what was done, skipped and why), [PORTING.md](PORTING.md) (how the engine changes work), [differential/results/2026-09-19-unreleased-vs-original.md](differential/results/2026-09-19-unreleased-vs-original.md) (the benchmark: on a dirty index `search` 1.6×, `searchJoined` 5.2×, `autoSuggest` 9.2× MiniSearch; first query after a discard 18 ms where 0.10.0 took 1.3–1.6 s), README "Unreleased changes" and COMPATIBILITY.md (the user-facing story).

What was verified, and what was not:

- `npm run build` and the whole of `npm test` passed on macOS (x86_64, Node 26.8, Rust 1.98 from Homebrew for the native part, Rust 1.96.0 with wasm-pack 0.15.0 for the Wasm build).
- On the pinned Rust 1.96.0 only `cargo fmt --check` and strict Clippy were run natively; the native tests ran on 1.98.
- Nothing has been run on Windows or Linux yet. The first CI run (`.github/workflows/ci.yml`, Linux) started with the push of `3535732`: check it with `gh run list`.

Deliberately left alone:

- `fuzzy: 1.5` and other fractional distances of 1 or more: MiniSearch indexes a typed array with the fraction and finds nothing; here it is an edit distance.
- The raw core stores `String(value)` for a field that is both indexed and stored. The facade never sends it such a value (0.12.0: the facade stores values itself).
- `tokenize`, `processTerm` and `boostDocument` still select the JavaScript engine (0.12.0: they stay in Wasm).
- The Wasm file grew to 786 KB (0.12.0: 826 KB, with the `core-api` bindings left out).

## Continuing on another machine

Windows (or any machine with `rustup`):

```sh
rustup show                                          # installs what rust-toolchain.toml pins: 1.96.0, wasm32-unknown-unknown, rustfmt, clippy
cargo install wasm-pack --version 0.15.0 --locked    # or the release binary
npm ci
npm run build                                        # wasm-pack -> target/wasm-glue, finalize-pkg -> pkg/
npm test                                             # the whole gate
```

Things to expect there:

- `npm run check:size` compares against `scripts/size-budget.json`, recorded from the macOS build with a 5% margin. If another platform's `wasm-opt` output is larger, run `node scripts/check-size.mjs --update` once and commit the file.
- `npm run test:package` must be started through npm (it reuses `npm_execpath`), and it reassembles `pkg/` in place to prove the package is deterministic: after editing `js/`, the README or the scripts, run `npm run build:pkg` first, or run the suite twice.
- The new scripts (`scripts/differential.mjs`, `upstream-suite/`, `differential/package_contract.mjs`) were only ever run on macOS. They use `node:path`/`fileURLToPath` throughout, but path handling on Windows is unverified.
- The README's clean-index benchmark table is from the Windows machine at 0.10.0; `npm run bench:public` regenerates the 20,000-document corpus and the paired report for the current tree.

On the Mac there is no global `rustup`: the Wasm build used an isolated toolchain in a scratch directory (rustup with `RUSTUP_HOME`/`CARGO_HOME` pointing there, the `wasm-pack` 0.15.0 release binary, `WASM_PACK_CACHE`), and Homebrew's cargo needed `RUSTC=/usr/local/Cellar/rust/<version>/bin/rustc` exported to find its compiler once `rust-toolchain.toml` existed.

## Needs the maintainer

- [ ] Release: bump the version, publish (`npm run publish:pkg`). npm still shows the 0.9.0 README for 0.10.0.
- [ ] Push tags `v0.9.0` (`ed9b39c`) and `v0.10.0` (`888b991`): `RELEASE_NOTES.md` and the 0.10.0 benchmark report link to files by those tags (404 today), and the report cites release assets that were never attached.
- [ ] Re-run `npm run bench:public` on the Windows machine and refresh the README's clean-index table (the dirty-index table is from the Mac).
- [ ] Look at the first CI run; update `scripts/size-budget.json` from a Linux build if the size check is what fails.
- [ ] Optional: send MiniSearch a fix for lucaong/minisearch#306 (its vacuum iterates the live tree and never resets `_currentVacuum` after a throw); a hosted demo built from MiniSearch's `examples/`.

## Engine

- [x] **Item 13, remainder** – `tokenize` / `processTerm` stay in Wasm (0.12.0: pre-tokenized terms), and so does `boostDocument` (called back from inside scoring).
- [ ] **Item 17, size** – the Wasm file is 826 KB (0.11.0: 796 KB, 0.8.0: 581 KB). 0.12.0 leaves the `core-api` bindings out; `opt-level = "s"` measured -14% but is untried against the benchmark.
- [x] The raw core stores `String(value)` for a field that is both indexed and stored (item 10). 0.12.0: the facade stores values itself and sends the engine texts.
- [ ] A long-lived index that never vacuums cannot be saved past 2,000,000 internal id slots; the writer says so and `compact()` cures it. Saving could renumber on the fly instead.
- [x] The native importer (`loadJSON`) was about 10% slower than MiniSearch's loader. 0.12.0 reads postings directly and is about 1.8× faster; it still refuses a few inputs MiniSearch accepts (non-canonical keys, entries for unknown documents), which MiniSearch's loader then loads.
- [ ] Per-node `boostDocument` (a query-tree node with its own) still transfers: the engine calls one hook per query.
- [ ] Inside `boostDocument` the index cannot be searched or changed (the engine holds its borrow while it calls back); MiniSearch allows it.

## Facade

- [ ] `searchRaw`'s `termTable` differs by engine (the native one lists expansion terms no hit refers to); ids and offsets are consistent in each.
- [ ] Truthy and falsy coercions that MiniSearch tolerates (`prefix: 1`, `fuzzy: '0.2'`) throw in Wasm mode.
- [x] A field named like an `Object.prototype` member (`constructor`, `toString`) indexes the inherited function's text in MiniSearch and nothing here. 0.12.0: extraction runs in JavaScript.
- [ ] MiniSearch's own `wildcard` symbol (from a separately installed `minisearch`) is not recognized; `MiniSearch.wildcard` from this package is.
- [ ] Read-only `_currentVacuum` / `_dirtCount` getters would let code written against MiniSearch's private fields keep working (item 15).

## Tests and docs

- [ ] The older suites count empty-versus-empty comparisons in their "explicit checks" totals (item 16).
- [ ] The browser contract (`node differential/serve_browser.mjs`) is still a manual check; a headless run in CI would close the gap.
- [ ] `IMPROVEMENTS.md` and `IMPROVEMENTS-2.md` describe 0.8.0/0.9.0; their status notes were not revised for the facade.
