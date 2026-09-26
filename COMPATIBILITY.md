# Public API compatibility

The public facade targets the documented MiniSearch **7.2.0** API. The
Rust/Wasm engine indexes and scores; the facade keeps what MiniSearch keeps in
JavaScript (document ids and stored fields) the way MiniSearch keeps it, and
runs the JavaScript callbacks, calling them as MiniSearch calls them. The
bundled, exact-version JavaScript implementation takes over only for the few
inputs listed below. Changes marked *since 0.12.0* distinguish this release
from 0.11.0, *since 0.11.0* from 0.10.0.

## Choosing an engine

`index.executionMode` is either `"wasm"` or `"javascript"`.

| Operation or input | Execution behavior |
| --- | --- |
| Constructor after Wasm initialization | Wasm |
| Constructor before browser initialization | JavaScript |
| Documents, whatever their ids and stored values: objects, Dates, arrays, getters, `-0`, `NaN`, strings with lone surrogates | Remain in Wasm; ids and stored fields live in JavaScript `Map`s and objects, as in MiniSearch (*since 0.12.0*; 0.11.0 transfers) |
| `add`, `remove`, `removeAll`, `discard`, `discardAll`, `replace` | Remain in Wasm |
| Search or suggestions with discarded postings | Remain in Wasm; the engine reproduces MiniSearch's lazy cleanup, first query included (*since 0.11.0*; 0.10.0 transfers) |
| Manual or automatic vacuum, `compact()` | Remain in Wasm; the facade runs MiniSearch's scheduler over native vacuum steps (*since 0.11.0*; 0.10.0 transfers) |
| Search callbacks `filter`, `prefix`, `fuzzy`, `boostTerm`, in options, query-tree nodes or constructor defaults | Remain in Wasm; evaluated by the facade (*since 0.11.0*; 0.10.0 transfers) |
| Callbacks `tokenize` and `processTerm`, in the constructor, search options or query-tree nodes, term arrays included | Remain in Wasm; the facade calls them as MiniSearch does and the engine indexes and searches their terms (*since 0.12.0*; 0.11.0 transfers) |
| `boostDocument` in the constructor's or a search's options | Remain in Wasm; the engine calls it back while it scores, for the same documents and terms in the same order as MiniSearch (*since 0.12.0*; 0.11.0 transfers) |
| Constructor callbacks `extractField`, `stringifyField`, `logger` | Remain in Wasm; evaluated by the facade (*since 0.11.0*) |
| `getStoredFields(id)` | Remain in Wasm; the live stored-fields object, as upstream: edits show up in results (*since 0.12.0*; 0.11.0 transfers on an edit) |
| `loadJSON` / `loadJSONAsync` | Wasm, through the native importer; the async loader reconstructs document maps and postings in batches with timer yields. What it refuses is loaded by MiniSearch's loader (*since 0.11.0*) |
| Load a native snapshot of 0.9.0 or later | Wasm |
| A query-tree node with a `boostDocument` of its own | Transfer: the engine calls one `boostDocument` per query |
| A field text (what `stringifyField` returns) that is not a string, or has lone surrogates; a term from `processTerm` that is `''` or not a string | Transfer at that document, without calling any callback twice |
| A query term containing U+0000 or a lone surrogate | Transfer |
| Options with no native form: `fields` that is not an array or repeats a name, `Infinity` in search options | JavaScript from construction |
| A per-term callback returning something other than a boolean or a finite number; `boost` or `bm25` explicitly `undefined` | Transfer, so that MiniSearch's own behavior (or error) applies |

A transfer converts the existing index once, preserves compressed radix-tree
ordering, frees the native instance, and keeps the JavaScript index thereafter.
It temporarily needs memory for conversion and can block for a large index.
There is no automatic switch back. The compatibility code also increases
bundle size.

Differences that remain, all in Wasm mode:

- `fuzzy` values of 1 or more that are not whole numbers are edit distances
  here; MiniSearch indexes its Levenshtein matrix with the fraction, which
  gives arbitrary results.
- Inside `boostDocument` the index cannot be searched or changed (it throws);
  `has`, `getStoredFields` and the count getters work. MiniSearch allows both.
- Once most short ids are free (removed documents outnumber live ones), a
  vacuum that leaves no dirt renumbers them (`idTableVersion` changes);
  `toJSON()` then shows the new numbers. MiniSearch never renumbers.
- `remove` logs its `version_conflict` warnings after reading the whole
  document, not interleaved with the callbacks that read it.

Before 0.12.0, `removeAll()` also reset `dirtCount`, every clean vacuum
renumbered short ids, `toJSON()` had its own key order and a zero for every
field average, and a field repeated in the search option `fields` was scored
twice.

Scores are the same bits as MiniSearch's, not approximations: the engine
computes the inverse document frequency with the logarithm algorithm V8 uses
(*since 0.11.0*; up to 0.10.0 about 4% of scores on small indexes differed in the
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
stored objects and Dates retain their references. A subclass that overrides
`add`, `discard` or `search` sees `addAll`, `discardAll` and `autoSuggest` call
its methods, and the static loaders return instances of the subclass. Native `filter` callbacks see
rows in traversal order, before sorting, so stateful predicates and score edits
behave like MiniSearch. `loadJSONAsync` yields while rebuilding document maps
and postings, including within a common term's posting list. JSON parsing,
initial table allocation, per-list sorting and final structural validation are
still synchronous; it does not promise a fixed maximum pause. `addAllAsync`
uses MiniSearch's chunk scheduler, including the deferred final short batch,
and reads document properties within those chunks.

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

In Wasm mode, `toBytes` writes the engine's validated binary snapshot (version
5 *since 0.12.0*: frequency-1 postings take one byte less) followed by the ids
and stored fields as JSON, in a byte envelope that starts with `MSWID01\n`;
`toNativeJSON` writes `format: "minisearch-wasm/identity", version: 1` with the
engine's JSON snapshot and the same identity. Ids and stored values therefore
follow JSON's rules, like MiniSearch's own serialization. An index built with
`tokenize` or `processTerm` callbacks records their names, and loading requires
them again. Snapshots written by 0.9.0 to 0.11.0 (version 4) still load. In
JavaScript mode `toBytes` and `toNativeJSON` use a separate compatibility snapshot:
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
finishes with no remaining dirt, once most short ids are free (*since 0.12.0*;
before, after every such vacuum). Explicit `compact()` also works on a clean
native index and rejects dirty or currently vacuuming indexes. It preserves
search/radix order while reclaiming removed ID slots and native scratch buffers.
Refresh cached raw ID tables whenever `idTableVersion` changes. Resolve raw
results before mutating or compacting their index.

Compaction reclaims internal allocations. It does not guarantee that a Wasm
linear-memory buffer or process resident memory shrinks. The native expansion
cache still has an entry-count limit, not a retained-byte budget.

## Verification

- `cargo test --locked` and strict Clippy check native behavior and compaction.
- `npm run test:wasm` checks the engine's own API (built with the `core-api`
  feature into `target/pkg-core`) plus the facade suites, which compare first
  dirty queries, callbacks and their call sequences, identities, mutations,
  async yields, snapshots, vacuum promise boundaries, compaction, the
  serialized index byte for byte, and CSP against pinned MiniSearch.
- `npm run test:types` checks the declared API with strict ES2022 TypeScript.
- `npm run test:package` installs the actual npm tarball and checks ESM, CommonJS,
  SearchableMap, generics and the global bundle with string codegen disabled.
- `node differential/serve_browser.mjs` serves the real-browser contract at the
  printed URL under CSP. Its page reports ESM, global bundle, Worker, async and
  Wasm checks. This is separate from the Node VM test.
