# Public API compatibility (0.10.0)

The public facade targets the documented MiniSearch **7.2.0** API. It uses the
Rust/Wasm engine for eligible inputs and the bundled, exact-version JavaScript
implementation for behavior that depends on JavaScript values or callbacks.
This is not an implementation of every MiniSearch feature inside Rust.

## Choosing an engine

`index.executionMode` is either `"wasm"` or `"javascript"`.

| Operation or input | Execution behavior |
| --- | --- |
| Constructor after Wasm initialization, with declarative options | Wasm |
| Constructor before browser initialization | JavaScript |
| Plain documents with scalar, finite indexed/stored values and IDs | Remain in Wasm |
| Constructor callbacks, including callbacks in default search/suggestion options | JavaScript |
| Search callbacks, including nested query options | Transfer to JavaScript |
| Object IDs, Dates, arrays, getters, custom objects, nonfinite values | Transfer before indexing, preserving JS values |
| `getStoredFields(id)` | Transfer, returning the actual mutable stored-fields object |
| Search or suggestions with discarded postings | Transfer before the first query; preserve lazy cleanup and scores |
| Manual or automatic vacuum | JavaScript maintenance and Promise scheduling; compact after clean completion |
| `loadJSON` / `loadJSONAsync` | JavaScript |
| Load a native version-4 snapshot | Wasm; legacy object IDs/stored values trigger a transfer |

A transfer converts the existing index once, preserves compressed radix-tree
ordering, frees the native instance, and keeps the JavaScript index thereafter.
It temporarily needs memory for conversion and can block for a large index.
There is no automatic switch back. Historical native benchmarks do not measure
this cost or JavaScript mode. The compatibility code also increases bundle size.

The public package has a [paired performance report](https://github.com/epoyraz/minisearch-wasm/blob/v0.10.0/differential/results/2026-09-18-public-vs-original.md)
that measures these costs separately from Wasm search throughput. Its Node and
synthetic-corpus results should not be assumed to hold for other workloads.

## Original API and TypeScript

The facade exposes the documented instance methods `add`, `addAll`,
`addAllAsync`, `remove`, `removeAll`, `discard`, `discardAll`, `replace`, `has`,
`getStoredFields`, `search`, `autoSuggest`, `vacuum`, `toJSON`; the static methods
`loadJSON`, `loadJSONAsync`, `getDefault`; `wildcard`; and getters
`documentCount`, `termCount`, `dirtCount`, `dirtFactor`, `isVacuuming`.

Callbacks supported through JavaScript mode include `extractField`,
`stringifyField`, `tokenize`, `processTerm` (including term arrays), `logger`,
`filter`, `boostDocument`, `boostTerm`, `prefix`, and `fuzzy`. Default functions
returned by `getDefault` can be passed back as options. Object IDs use identity;
stored objects and Dates retain their references. `loadJSONAsync` yields while
rebuilding maps/index entries, following upstream; its initial `JSON.parse` is
still synchronous. `addAllAsync` reads document properties within its chunks.

`MiniSearch<T>`, `MiniSearchWasm<T>`, `Options<T>` and the original public option,
query and result types are exported. Case variants such as `And` are accepted.
Declarations compile with `lib: ["es2022", "dom"]`; an ESNext disposable library
is not required. Generated core glue is internal and retains a narrower API.
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
