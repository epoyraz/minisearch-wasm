# Public package (unreleased tree) vs MiniSearch 7.2.0

Run of `differential/bench_public.mjs` on 19 September 2026 against the working
tree after 0.10.0 (git HEAD `888b991` plus uncommitted changes), to
measure what keeping dirty indexes, vacuum and search callbacks on the Wasm
engine changed. The [0.10.0 report](2026-09-18-public-vs-original.md) explains
the methodology; it was measured on another machine, so compare the ratios
within this table, not the milliseconds across the two reports.

- 20,000 synthetic documents, 39 queries (`gen_corpus.mjs bench_corpus.json 20000`), options `{"prefix": true, "fuzzy": 0.2, "combineWith": "AND"}`
- 5 paired rounds after two warm-ups, medians; "dirty" is a quarter of the documents discarded and not vacuumed
- Node v26.8.1 (darwin x64), Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz, 16 GiB
- Before timing, 711,139 result rows were compared with MiniSearch's: maximum relative score difference 0

| Case | MiniSearch ms | Public ms | Speedup | Engine |
| --- | ---: | ---: | ---: | --- |
| warm search: full objects | 926.9 | 635.2 | 1.46x | wasm |
| warm search: joined decoded | 931.2 | 161.8 | 5.75x | wasm |
| warm search: raw decoded | 908.5 | 67.5 | 13.47x | wasm |
| first batch: full objects | 942.5 | 620.2 | 1.52x | wasm |
| first batch: joined decoded | 928.4 | 149.8 | 6.20x | wasm |
| first batch: raw decoded | 947.9 | 67.4 | 14.07x | wasm |
| warm autoSuggest | 629.0 | 62.2 | 10.11x | wasm |
| build: addAll | 811.3 | 533.8 | 1.52x | wasm |
| build from JSON text | 864.9 | 460.0 | 1.88x | wasm |
| build: addAllAsync | 900.7 | 681.4 | 1.32x | wasm |
| save: JSON vs native binary | 430.9 | 6.1 | 70.88x | wasm |
| load: JSON vs native binary | 285.2 | 22.5 | 12.69x | wasm |
| loadJSON: same JSON input | 272.1 | 297.0 | 0.92x | wasm |
| loadJSONAsync: same JSON input | 1085.2 | 301.1 | 3.60x | wasm |
| first query after 25% discard | 17.4 | 18.0 | 0.97x | wasm |
| first filter-callback query | 19.6 | 19.0 | 1.03x | wasm |
| first boostDocument query + transfer | 18.5 | 1280.9 | 0.01x | javascript |
| dirty index, warm search: full objects | 750.0 | 485.1 | 1.55x | wasm |
| dirty index, warm search: joined decoded | 714.4 | 138.7 | 5.15x | wasm |
| dirty index, warm search: raw decoded | 710.7 | 68.5 | 10.38x | wasm |
| dirty index, warm autoSuggest | 525.6 | 57.2 | 9.19x | wasm |
| warm filter-callback search | 967.5 | 648.5 | 1.49x | wasm |
| warm boostDocument search (JS engine) | 964.2 | 1002.2 | 0.96x | javascript |
| vacuum after 25% discard | 80.8 | 24.7 | 3.27x | wasm |

In 0.10.0 the cases "first query after 25% discard", "first filter-callback
query" and "vacuum" ran on the JavaScript engine after a one-time transfer of
1.28-1.59 seconds on its machine, and every later query on such an index ran at
MiniSearch's speed. `loadJSON` now keeps the index in Wasm; its native importer
is not faster than MiniSearch's loader. Cold module import:
MiniSearch 3.1 ms, public package 15.4 ms
(the Node entry compiles the Wasm module on import).
