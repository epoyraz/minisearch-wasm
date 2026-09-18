# Updated minisearch-wasm vs original MiniSearch

Measured on 18 September 2026 against MiniSearch **7.2.0**, before the version bump to **0.10.0**. The facade source below is the implementation released in 0.10.0; recorded package metadata still says 0.9.0. These results do not describe the earlier published 0.9.0 package.

The same full-result search API is **1.42x faster** while the index stays in Wasm. Decoded compact outputs are faster again, but return fewer fields. The main cost is switching to JavaScript: the first callback query takes **1279.13 ms** and the first dirty query **1589.25 ms**. After transfer, that instance stays in JavaScript, including after vacuum.

## Workload and method

- 20,000 deterministic synthetic documents, 38 queries, fields `title` and `text`, no stored fields. Options: `prefix: true, fuzzy: 0.2, combineWith: 'AND', autoVacuum: false`.
- 9 paired measured rounds after two warmups. Async JSON load and vacuum use five measured rounds each. Engine order alternates. Each row has its own paired original baseline.
- AMD Ryzen 9 3900X 12-Core Processor; win32/x64; 63.91 GiB RAM; Node v24.21.0, V8 13.6.233.17-node.53.
- Main run: 2026-09-18T12:19:13.024Z to 2026-09-18T12:25:40.566Z. Recorded source and build hashes matched disk at completion.
- First-batch measurements use a freshly built index with no previous query; setup is outside timing. Code/JIT is already warmed, so these are not cold-process timings. Later queries within that first batch can reuse expansions.
- Search timings consume IDs, scores and matched-term strings. Joined timings include JSON-ID parsing and term splitting. Raw timings include term-table decoding and ID resolution using a cached ID table, without allocating a full result object per hit. The original always constructs its full match objects.
- Explicit GC, validation and disposal are outside timed regions. Allocations and any GC during the operation remain inside. Index construction/loading excludes destruction. Async timings include timer yields.
- Verified **693,377 result rows**, including order, terms, full match/query-term metadata and first dirty/callback calls. Maximum normalized score difference: `4.2199936548842345e-16` (tolerance `1e-12`). Compact counts and consumed-output checksums are also checked on every timed round.

## Query throughput

Every row below is a complete 38-query batch. Compact APIs are alternatives for callers that need IDs/scores/terms, not the identical full-result API.

| Operation | Unit | Original, ms | Updated, ms | Comparison | Updated p25–p75, ms |
| --- | --- | ---: | ---: | --- | ---: |
| warm search: full objects | 38 queries | 458.62 | 323.94 | 1.42x faster | 311.51–339.37 |
| warm search: joined decoded | 38 queries | 472.79 | 105.46 | 4.48x faster | 104.64–111.80 |
| warm search: raw decoded | 38 queries | 450.75 | 34.21 | 13.18x faster | 32.81–42.59 |
| first batch: full objects | 38 queries | 445.33 | 317.36 | 1.40x faster | 308.56–321.75 |
| first batch: joined decoded | 38 queries | 575.60 | 152.49 | 3.77x faster | 139.78–163.27 |
| first batch: raw decoded | 38 queries | 569.20 | 52.16 | 10.91x faster | 46.39–57.99 |
| warm autoSuggest | 38 queries | 443.86 | 53.67 | 8.27x faster | 46.22–53.94 |

## Transfer and maintenance costs

These first queries each use `engine` on a freshly prepared index; there is no query warm-up on that instance. The discard cases remove 25% of documents (5,000). Their timed operations include the transfer. Vacuum also includes compaction of the public facade's internal IDs, an additional operation upstream does not perform.

| Operation | Unit | Original, ms | Updated, ms | Comparison | Updated p25–p75, ms |
| --- | --- | ---: | ---: | --- | ---: |
| first callback query + transfer | one query: engine | 15.42 | 1279.13 | 82.97x slower | 1259.56–1335.05 |
| first query after 25% discard | one query: engine | 18.24 | 1589.25 | 87.12x slower | 1542.15–1618.21 |
| warm callback search (JS fallback) | 38 queries | 611.57 | 552.95 | 1.11x faster | 492.49–577.63 |
| vacuum after 25% discard | 5000 discarded documents | 61.07 | 1691.85 | 27.71x slower | 1644.63–1783.90 |

The transfer is synchronous and can block its calling thread. Source inspection shows `_promote()` exporting MiniSearch JSON, loading a JS index, then exporting native JSON and rebuilding the ordered radix tree again before freeing Wasm. This identifies an optimization target; the benchmark does not assign separate timings to those steps.

Automatic vacuum is disabled here to isolate the first dirty query. With default automatic maintenance, transfer can occur earlier during mutation. Calling `vacuum()` does not restore Wasm mode. Callback configuration at construction starts directly in JavaScript and avoids a later transfer, but provides no Wasm search acceleration.

Steady callback search varied from 1.00x in the earlier run to 1.11x in the current run. Treat that as performance close to the original JavaScript engine, not a native speedup.

## Index construction and persistence

`addAllAsync` uses chunks of 500 documents. The JSON-text build comparison includes `JSON.parse` for the original and the facade's own JSON eligibility scan. Binary persistence rows compare different formats; same-format JSON loader rows are included separately and return JavaScript-backed facade instances.

| Operation | Unit | Original, ms | Updated, ms | Comparison | Updated p25–p75, ms |
| --- | --- | ---: | ---: | --- | ---: |
| build: addAll | 20000 documents | 820.83 | 526.72 | 1.56x faster | 512.09–532.53 |
| build from JSON text | 20000 documents | 830.56 | 422.75 | 1.96x faster | 386.46–458.71 |
| build: addAllAsync | 20000 documents | 1240.45 | 854.94 | 1.45x faster | 837.72–902.24 |
| save: JSON vs native binary | operation | 382.10 | 7.02 | 54.46x faster | 6.09–7.05 |
| load: JSON vs native binary | operation | 242.72 | 25.45 | 9.54x faster | 25.24–25.54 |
| loadJSON: same JSON input | operation | 237.51 | 232.94 | 1.02x faster | 219.46–251.00 |
| loadJSONAsync: same JSON input | operation | 8027.64 | 8051.00 | 1.00x slower | 8006.04–8084.24 |

## Sizes and initialization

Snapshot sizes below are MiB. Gzip uses level 9; Brotli uses quality 6. The compatibility envelope preserves ordered-tree metadata and is considerably larger than either native binary or upstream JSON. It is not the native binary fast path.

| Snapshot | Uncompressed MiB | Gzip MiB | Brotli q6 MiB |
| --- | ---: | ---: | ---: |
| originalJSON | 7.91 | 2.13 | 1.82 |
| publicNativeBinary | 1.74 | 0.87 | 0.83 |
| publicFallbackEnvelope | 15.33 | 4.11 | 3.09 |

Fresh-process module import took **3.73 ms** for the original and **13.04 ms** for the public Node entry, which initializes Wasm. Process startup is excluded and the OS file cache is warm. Main browser-asset sizes are recorded in the raw results; they are not a production-bundler or network-latency measurement.

## Reproduce

The current corpus generator emits the same documents plus an additional 70-term stress query. Trim that last query to reproduce this recorded 38-query run. The regenerated document hash was checked against the recorded corpus.

~~~powershell
npm ci
node differential/gen_corpus.mjs differential/bench_corpus.json 20000
node --input-type=module -e 'import fs from "node:fs"; const p="differential/bench_corpus.json"; const c=JSON.parse(fs.readFileSync(p)); c.queries=c.queries.slice(0,38); fs.writeFileSync(p,JSON.stringify(c));'
npm run bench:public -- differential/bench_corpus.json differential/results/recheck.json 9
~~~

Document-array SHA-256 (`JSON.stringify(docs)`): `02567d97291066b43a906cc0caa259860c9369162a0fb96e8f32c863aa5e5f90`.
Original corpus-file SHA-256: `79bae2a62906e678d024526c05bb30b13551e3a845f7cecd7be8e8ac06b5bc52` (formatting or other corpus metadata can change this file hash without changing documents or queries).

[Benchmark runner](../bench_public.mjs), [current raw samples and metadata](https://github.com/epoyraz/minisearch-wasm/releases/download/v0.10.0/2026-09-18-public-vs-original-current.json), [earlier run](https://github.com/epoyraz/minisearch-wasm/releases/download/v0.10.0/2026-09-18-public-vs-original.json). The earlier run used the preceding facade revision; its timings are corroborating context, not substituted for current-build results. Raw JSON outputs are attached to the release rather than tracked in Git.

Runner SHA-256: `c03fbc82df70790dae1a442433ff96d27cc43ec20dd4c7dbc47fb74570958dd6`.

- `pkg/minisearch_wasm.js`: `cca20d894398bc2a9ec0d00621ae142bef49f92f35ec4d9033b6e6c0ce72fa6d`
- `pkg/minisearch_wasm_core.js`: `1a37d6bfb643d2ef4beee03f2c239651e2d808bc2c030c6b53370cb26f75f3ab`
- `pkg/minisearch_wasm_bg.wasm`: `705b981d6bae28714e9dc759a346edb4ec0230e54e1ebed6e6b4e28822c4bc7f`
- `js/minisearch.js`: `35ece67caefd136b86293aaabb510ada7d7dbe17eb27de6072f19ca68d2b433b`

## Limits

This is one Node/Windows machine and one synthetic corpus, with no stored fields. It does not establish browser latency, production-jobboard performance, heap usage or behavior at larger scale. Absolute times changed between the two runs; paired ratios for clean full/compact searches were similar. Earlier README tables use separate measurements and should not be combined with this run. Historical native speedups do not apply after an instance has transferred to JavaScript.
