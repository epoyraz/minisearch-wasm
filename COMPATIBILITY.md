# Public API compatibility

The public facade targets the documented MiniSearch **7.2.0** API. It uses the
Rust/Wasm engine for everything that engine can reproduce exactly, and the
bundled, exact-version JavaScript implementation for behavior that depends on
JavaScript callbacks inside indexing or scoring, or on JavaScript values.
Sections marked *unreleased* describe this repository after 0.10.0.

## Choosing an engine

`index.executionMode` is either `"wasm"` or `"javascript"`.

| Operation or input | Execution behavior |
| --- | --- |
| Constructor after Wasm initialization | Wasm |
| Constructor before browser initialization | JavaScript |
| Plain documents with scalar, finite indexed/stored values and IDs | Remain in Wasm |
| `add`, `remove`, `removeAll`, `discard`, `discardAll`, `replace` | Remain in Wasm |
| Search or suggestions with discarded postings | Remain in Wasm; the engine reproduces MiniSearch's lazy cleanup, first query included (*unreleased*; 0.10.0 transfers) |
| Manual or automatic vacuum, `compact()` | Remain in Wasm; the facade runs MiniSearch's scheduler over native vacuum steps (*unreleased*; 0.10.0 transfers) |
| Search callbacks `filter`, `prefix`, `fuzzy`, `boostTerm`, in options, query-tree nodes or constructor defaults | Remain in Wasm; evaluated by the facade (*unreleased*; 0.10.0 transfers) |
| Constructor callbacks `extractField`, `stringifyField`, `logger` | Remain in Wasm; evaluated by the facade, or called by the engine (*unreleased*) |
| `getStoredFields(id)` | Remain in Wasm; one object per document. Editing a returned object transfers, keeping the edit (*unreleased*) |
| `loadJSON` / `loadJSONAsync` | Wasm, through the native importer; what it refuses is loaded by MiniSearch's loader (*unreleased*) |
| Load a native version-4 snapshot | Wasm; legacy object IDs/stored values trigger a transfer |
| Callbacks `tokenize`, `processTerm` (constructor or search) and `boostDocument` | JavaScript: they run inside tokenization or scoring |
| Object IDs, Dates, arrays, getters, custom objects, nonfinite values, `-0`, strings with lone surrogates | Transfer before indexing, preserving JS values |
| Options with no native form: `fields` that is not an array or repeats a name, a stored field named `score`, `terms`, `queryTerms` or `match` (or `id` under another `idField`), `Infinity` in search options | JavaScript from construction |
| A per-term callback returning something other than a boolean or a finite number; `boost` or `bm25` explicitly `undefined` | Transfer, so that MiniSearch's own behavior (or error) applies |

A transfer converts the existing index once, preserves compressed radix-tree
ordering, frees the native instance, and keeps the JavaScript index thereafter.
It temporarily needs memory for conversion and can block for a large index.
There is no automatic switch back. The compatibility code also increases
bundle size.

Differences that remain, all in Wasm mode:

- `filter` runs over the ranked rows, so it is called in rank order, not in
  MiniSearch's internal order. The rows it keeps, and their order, are the same.
- `loadJSONAsync` yields once and then loads synchronously (the native importer
  does not yield between chunks).
- `removeAll()` also resets `dirtCount`; MiniSearch keeps it.
- After a vacuum that leaves no dirt the facade renumbers internal ids
  (`idTableVersion` changes); `toJSON()` then shows the new numbers.
- `fuzzy` values of 1 or more that are not whole numbers are edit distances
  here; MiniSearch indexes a typed array with them and finds nothing.

Scores are the same bits as MiniSearch's, not approximations: the engine
computes the inverse document frequency with the logarithm algorithm V8 uses
(*unreleased*; up to 0.10.0 about 4% of scores on small indexes differed in the
last bit).

The public package has a [paired performance report](https://github.com/epoyraz/minisearch-wasm/blob/main/differential/results/2026-09-18-public-vs-original.md)
for 0.10.0 that measures transfer costs separately from Wasm search throughput.
Its Node and synthetic-corpus results should not be assumed to hold for other
workloads; reproduce it with `npm run bench:public`.

## Original API and TypeScript

The facade exposes the documented instance methods `add`, `addAll`,
`addAllAsync`, `remove`, `removeAll`, `discard`, `discardAll`, `replace`, `has`,
`getStoredFields`, `search`, `autoSuggest`, `vacuum`, `toJSON`; the static methods
`loadJSON`, `loadJSONAsync`, `getDefault`; `wildcard`; and getters
`documentCount`, `termCount`, `dirtCount`, `dirtFactor`, `isVacuuming`.

Every MiniSearch callback is supported: `extractField`, `stringifyField`,
`tokenize`, `processTerm` (including term arrays), `logger`, `filter`,
`boostDocument`, `boostTerm`, `prefix`, and `fuzzy`; the table above says which
engine each one leaves an index on. Default functions
returned by `getDefault` can be passed back as options. Object IDs use identity;
stored objects and Dates retain their references. `loadJSONAsync` yields while
rebuilding maps/index entries, following upstream; its initial `JSON.parse` is
still synchronous. `addAllAsync` reads document properties within its chunks.

`MiniSearch<T>`, `MiniSearchWasm<T>`, `Options<T>` and the original public option,
query and result types are exported, for ESM and (through a merged namespace,
`import type { SearchResult } from "minisearch-wasm"`) for CommonJS. MiniSearch's
declarations ship inside the package; it is not a dependency. Case variants such
as `And` are accepted. Declarations compile with `lib: ["es2022"]`: neither the
DOM nor an ESNext disposable library is required. Generated core glue is
internal and retains a narrower API.
Undocumented upstream internals such as `loadJS` are not facade contracts.

## Import formats

Node initializes Wasm on import or require:

```js
import MiniSearch, { MiniSearchWasm } from 'minisearch-wasm';
import SearchableMap from 'minisearch-wasm/SearchableMap';
const index = new MiniSearch({ fields: ['text'] });
```

```js
const MiniSearch = require('minisearch-wasm');
const SearchableMap = require('minisearch-wasm/SearchableMap');
```

In a browser or module Worker, initialize before constructing a Wasm index:

```js
import MiniSearch, { init } from './minisearch_wasm.js';
await init();
const index = new MiniSearch({ fields: ['text'] });
```

Serve the generated `pkg/` files together, including the Wasm and `snippets/`
directory. The ESM compatibility engine is bundled, so a plain browser does not
need an import map. A script tag loading `minisearch_wasm.umd.js` exposes
`globalThis.MiniSearch`; call `await MiniSearch.init()` to load its Wasm asset.
Without initialization, its constructor works in JavaScript mode. Existing
`import init, { MiniSearchWasm }` initialization code remains valid.

Direct browser-entry imports in Node must supply bytes to `initSync({module})`
or `init({module_or_path})`; use the normal package entry to avoid that step.

The package uses static JavaScript functions, with no `eval` or `new Function`.
Browser CSP must still allow Wasm compilation, for example
`script-src 'self' 'wasm-unsafe-eval'`; JavaScript `unsafe-eval` is unnecessary.

## Persistence and compact results

`toJSON` / `loadJSON` interoperate with upstream serialization versions 1 and 2.
Like upstream, serialization cannot preserve prototypes, object identity,
functions, Symbols, BigInts or cyclic objects. Re-supply constructor callbacks.

In Wasm mode, `toBytes` and `toNativeJSON` retain the validated native version-4
formats. In JavaScript mode they use a separate compatibility snapshot:
`format: "minisearch-wasm/compat", version: 1`. Byte envelopes start with
`MSWJS01\n` and contain UTF-8 JSON. These are not native compact binaries and do
not have the native binary reader's validation/resource limits. Use trusted
snapshots, as with upstream MiniSearch JSON. Their ordered tree preserves live
prefix/fuzzy tie order, and callback option names are recorded. Loaders require
those callbacks again instead of silently substituting defaults:

```js
const options = { fields: ['text'], tokenize: text => text.split('|') };
const index = new MiniSearch(options);
const loaded = MiniSearch.loadBytes(index.toBytes(), options);
```

Public loaders recognize both formats. Native bytes require Wasm initialization;
compatibility envelopes do not. Loading constructs a new instance: fetch a new
ID table even if its generation string matches a previous instance.

Compact APIs are extensions with narrower encodings than full `search()`:
`searchJoined(...).ids` and `docIdTable()` are JSON strings. Terms are separated
by spaces/newlines in joined results and by newlines in raw term tables. Use
full results when custom callbacks emit delimiter-containing terms, or IDs
depend on reference identity or cannot survive JSON. Full results preserve
those JavaScript values.

## Lifecycle and compaction

The active vacuum and queued vacuum have distinct promises; repeated requests
reuse the queued promise, matching upstream. ID compaction runs after maintenance
finishes with no remaining dirt. Explicit `compact()` also works on a clean
native index and rejects dirty or currently vacuuming indexes. It preserves
search/radix order while reclaiming removed ID slots and native scratch buffers.
Refresh cached raw ID tables whenever `idTableVersion` changes. Resolve raw
results before mutating or compacting their index.

Compaction reclaims internal allocations. It does not guarantee that a Wasm
linear-memory buffer or process resident memory shrinks. The native expansion
cache still has an entry-count limit, not a retained-byte budget.

## Verification

- `cargo test --locked` and strict Clippy check native behavior and compaction.
- `npm run test:wasm` checks the core subset plus `public_api.mjs`, which compares
  first dirty queries, callbacks, identities, mutations, async yields, snapshots,
  vacuum promise boundaries, compaction and CSP against pinned MiniSearch.
- `npm run test:types` checks the declared API with strict ES2022 TypeScript.
- `npm run test:package` installs the actual npm tarball and checks ESM, CommonJS,
  SearchableMap, generics and the global bundle with string codegen disabled.
- `node differential/serve_browser.mjs` serves the real-browser contract at the
  printed URL under CSP. Its page reports ESM, global bundle, Worker, async and
  Wasm checks. This is separate from the Node VM test.
