// Smoke test of the built pkg through the real Wasm boundary, compared against
// JS MiniSearch: search, suggestions, mutations, vacuuming, and async indexing.
import { readFileSync } from 'fs'
import MiniSearch from 'minisearch'
import init, { MiniSearchWasm } from '../pkg/minisearch_wasm_core.js'

await init(readFileSync(new URL('../pkg/minisearch_wasm_bg.wasm', import.meta.url)))

const documents = [
  { id: 1, title: 'Divina Commedia', text: 'Nel mezzo del cammin di nostra vita', category: 'poetry' },
  { id: 2, title: 'I Promessi Sposi', text: 'Quel ramo del lago di Como', category: 'fiction' },
  { id: 3, title: 'Vita Nova', text: 'In quella parte del libro della mia memoria', category: 'poetry' }
]

const js = new MiniSearch({ fields: ['title', 'text'], storeFields: ['category'] })
js.addAll(documents)

const wasm = new MiniSearchWasm({ fields: ['title', 'text'], storeFields: ['category'] })
wasm.addAll(documents)

let failures = 0
const check = (name, expected, actual) => {
  const je = JSON.stringify(expected)
  const ja = JSON.stringify(actual)
  if (je !== ja) {
    console.log(`FAIL ${name}\n  js:   ${je}\n  wasm: ${ja}`)
    failures++
  } else {
    console.log(`ok  ${name}`)
  }
}

for (const query of ['com', 'vita no', 'nostra vi', 'del', '']) {
  check(`autoSuggest(${JSON.stringify(query)})`, js.autoSuggest(query), wasm.autoSuggest(query))
}
check("autoSuggest('vita', {fuzzy, prefix})",
  js.autoSuggest('vita', { fuzzy: true, prefix: true }),
  wasm.autoSuggest('vita', { fuzzy: true, prefix: true }))
check("autoSuggest('memoia', {fuzzy: 0.2})",
  js.autoSuggest('memoia', { fuzzy: 0.2 }),
  wasm.autoSuggest('memoia', { fuzzy: 0.2 }))

// constructor autoSuggestOptions through the wasm constructor
const jsCtor = new MiniSearch({ fields: ['title', 'text'], autoSuggestOptions: { combineWith: 'OR', fuzzy: true } })
jsCtor.addAll(documents)
const wasmCtor = new MiniSearchWasm({ fields: ['title', 'text'], autoSuggestOptions: { combineWith: 'OR', fuzzy: true } })
wasmCtor.addAll(documents)
check("ctor autoSuggestOptions: autoSuggest('nosta vi')", jsCtor.autoSuggest('nosta vi'), wasmCtor.autoSuggest('nosta vi'))

// joined fast path: rows must equal the object path's suggestions
const joined = wasm.autoSuggestJoined('vita no')
const rows = joined.count ? joined.suggestions.split('\n') : []
const objPath = wasm.autoSuggest('vita no')
check('autoSuggestJoined rows', objPath.map(s => s.suggestion), rows)
check('autoSuggestJoined scores', objPath.map(s => s.score), Array.from(joined.scores))
check('autoSuggestJoined terms-from-rows', objPath.map(s => s.terms), rows.map(r => r.split(' ')))

// binary snapshot round-trip keeps autoSuggest options + results
const reloaded = MiniSearchWasm.loadBytes(wasmCtor.toBytes())
check('loadBytes autoSuggest', wasmCtor.autoSuggest('nosta vi'), reloaded.autoSuggest('nosta vi'))

// --- query trees + wildcard through the wasm boundary -----------------------
// Full-path rows compare exactly, including result order on ties and the
// per-row `terms` order (JS `Object.keys(match)` order).
const rowsOf = (results) => results.map(r => ({ id: r.id, score: r.score, terms: r.terms }))
const checkSearch = (name, jsQuery, wasmQuery, options) =>
  check(name, rowsOf(js.search(jsQuery, options)), rowsOf(wasm.search(wasmQuery, options)))

checkSearch('search plain string parity', 'vita del', 'vita del')
checkSearch('search partial per-call options', 'vit', 'vit', { prefix: true })

const tree = {
  combineWith: 'OR',
  queries: [
    { combineWith: 'AND', queries: ['vita', 'cammin'] },
    'como sottomarino',
    { combineWith: 'AND', queries: ['nova', 'pappagallo'] }
  ]
}
checkSearch('search query tree', tree, tree)

const cascade = {
  fuzzy: true,
  weights: { fuzzy: 0.2, prefix: 0.75 },
  queries: [
    { prefix: true, fields: ['title'], queries: ['vit'] },
    { combineWith: 'AND', queries: ['bago', 'coomo'] }
  ]
}
checkSearch('search tree option cascade', cascade, cascade)

check('wildcard is a stable symbol', true,
  typeof MiniSearchWasm.wildcard === 'symbol' && MiniSearchWasm.wildcard === MiniSearchWasm.wildcard)
checkSearch('search wildcard', MiniSearch.wildcard, MiniSearchWasm.wildcard)
const andNotTree = (wildcard) => ({ combineWith: 'AND_NOT', queries: [wildcard, 'vita'] })
checkSearch('search AND_NOT wildcard tree', andNotTree(MiniSearch.wildcard), andNotTree(MiniSearchWasm.wildcard))
check("search('*') is a plain term", js.search('*'), wasm.search('*'))

// --- removeAll / discardAll through the wasm boundary ----------------------
const jsBatch = new MiniSearch({ fields: ['title', 'text'], storeFields: ['category'], autoVacuum: false })
jsBatch.addAll(documents)
const wasmBatch = new MiniSearchWasm({ fields: ['title', 'text'], storeFields: ['category'] })
wasmBatch.addAll(documents)

jsBatch.removeAll([documents[0]])
wasmBatch.removeAll([documents[0]])
check('removeAll([...])', rowsOf(jsBatch.search(MiniSearch.wildcard)), rowsOf(wasmBatch.search(MiniSearchWasm.wildcard)))

let removeAllError
try {
  wasmBatch.removeAll(null)
} catch (error) {
  removeAllError = error instanceof Error ? error.message : String(error)
}
check('removeAll(non-array) error',
  'Expected documents to be present. Omit the argument to remove all documents.',
  removeAllError)

jsBatch.discardAll([2])
wasmBatch.discardAll([2])
const jsDirtyRows = rowsOf(jsBatch.search('lago vita'))
const wasmDirtyRows = rowsOf(wasmBatch.search('lago vita'))
const withoutScores = (rows) => rows.map(({ score, ...row }) => row)
const scoresMatch = jsDirtyRows.length === wasmDirtyRows.length &&
  jsDirtyRows.every((row, index) => {
    const other = wasmDirtyRows[index]
    return Math.abs(row.score - other.score) /
      Math.max(Math.abs(row.score), Math.abs(other.score), 1e-12) <= 1e-12
  })
check('discardAll([...]) dirty search rows', withoutScores(jsDirtyRows), withoutScores(wasmDirtyRows))
check('discardAll([...]) dirty search scores', true, scoresMatch)

jsBatch.removeAll()
wasmBatch.removeAll()
check('removeAll() resets index', rowsOf(jsBatch.search(MiniSearch.wildcard)), rowsOf(wasmBatch.search(MiniSearchWasm.wildcard)))

// --- vacuum / autoVacuum ---------------------------------------------------
const jsVacuum = new MiniSearch({ fields: ['title', 'text'], autoVacuum: false })
jsVacuum.addAll(documents)
const wasmVacuum = new MiniSearchWasm({ fields: ['title', 'text'], autoVacuum: false })
wasmVacuum.addAll(documents)
jsVacuum.discardAll([1, 2])
wasmVacuum.discardAll([1, 2])
check('dirtCount before vacuum', jsVacuum.dirtCount, wasmVacuum.dirtCount)
check('dirtFactor before vacuum', jsVacuum.dirtFactor, wasmVacuum.dirtFactor)

const jsVacuumPromise = jsVacuum.vacuum({ batchSize: 1, batchWait: 1 })
const wasmVacuumPromise = wasmVacuum.vacuum({ batchSize: 1, batchWait: 1 })
check('vacuum returns Promise', true, wasmVacuumPromise instanceof Promise)
check('isVacuuming while active', jsVacuum.isVacuuming, wasmVacuum.isVacuuming)
await Promise.all([jsVacuumPromise, wasmVacuumPromise])
check('vacuum search parity',
  rowsOf(jsVacuum.search(MiniSearch.wildcard)),
  rowsOf(wasmVacuum.search(MiniSearchWasm.wildcard)))
check('dirtCount after vacuum', jsVacuum.dirtCount, wasmVacuum.dirtCount)
check('isVacuuming after completion', jsVacuum.isVacuuming, wasmVacuum.isVacuuming)

const autoVacuumOptions = {
  minDirtCount: 1,
  minDirtFactor: 0.01,
  batchSize: 1,
  batchWait: 1
}
const jsAutoVacuum = new MiniSearch({ fields: ['title', 'text'], autoVacuum: autoVacuumOptions })
jsAutoVacuum.addAll(documents)
const wasmAutoVacuum = new MiniSearchWasm({ fields: ['title', 'text'], autoVacuum: autoVacuumOptions })
wasmAutoVacuum.addAll(documents)
jsAutoVacuum.discard(1)
wasmAutoVacuum.discard(1)
check('autoVacuum starts at threshold', jsAutoVacuum.isVacuuming, wasmAutoVacuum.isVacuuming)
for (let waited = 0; jsAutoVacuum.isVacuuming || wasmAutoVacuum.isVacuuming; waited++) {
  if (waited >= 5000) throw new Error('an auto-vacuum did not finish within five seconds')
  await new Promise(resolve => setTimeout(resolve, 1))
}
check('autoVacuum dirtCount', jsAutoVacuum.dirtCount, wasmAutoVacuum.dirtCount)
check('autoVacuum search parity',
  rowsOf(jsAutoVacuum.search(MiniSearch.wildcard)),
  rowsOf(wasmAutoVacuum.search(MiniSearchWasm.wildcard)))

// --- addAllAsync -----------------------------------------------------------
const jsAsync = new MiniSearch({ fields: ['title', 'text'] })
const wasmAsync = new MiniSearchWasm({ fields: ['title', 'text'] })
const jsAddPromise = jsAsync.addAllAsync(documents, { chunkSize: 2 })
const wasmAddPromise = wasmAsync.addAllAsync(documents, { chunkSize: 2 })
check('addAllAsync returns Promise', true, wasmAddPromise instanceof Promise)
check('addAllAsync yields before first full chunk', 0, wasmAsync.documentCount)
await Promise.all([jsAddPromise, wasmAddPromise])
check('addAllAsync custom chunk parity',
  rowsOf(jsAsync.search(MiniSearch.wildcard)),
  rowsOf(wasmAsync.search(MiniSearchWasm.wildcard)))

const jsAsyncDefault = new MiniSearch({ fields: ['title', 'text'] })
const wasmAsyncDefault = new MiniSearchWasm({ fields: ['title', 'text'] })
await Promise.all([
  jsAsyncDefault.addAllAsync(documents),
  wasmAsyncDefault.addAllAsync(documents)
])
check('addAllAsync default chunk parity',
  rowsOf(jsAsyncDefault.search(MiniSearch.wildcard)),
  rowsOf(wasmAsyncDefault.search(MiniSearchWasm.wildcard)))

// --- searchRaw / searchJoinedOpts / docIdTable ------------------------------
{
  const decodeRaw = (r, table) => {
    const terms = r.termTable ? r.termTable.split('\n') : []
    const out = []
    for (let i = 0; i < r.count; i++) {
      const row = []
      for (let k = r.termOffsets[i]; k < r.termOffsets[i + 1]; k++) row.push(terms[r.termIds[k]])
      out.push({ id: table[r.docIds[i]], score: r.scores[i], terms: row })
    }
    return out
  }
  const decodeJoined2 = (r) => {
    if (!r.count) return []
    const ids = JSON.parse(r.ids)
    const termRows = r.terms.split('\n')
    return ids.map((id, i) => ({ id, score: r.scores[i], terms: termRows[i] ? termRows[i].split(' ') : [] }))
  }

  const idTable = JSON.parse(wasm.docIdTable())
  for (const q of ['vita', 'del', 'vit', 'nosuchterm']) {
    check(`searchRaw parity (${q})`,
      decodeJoined2(wasm.searchJoined(q, false)),
      decodeRaw(wasm.searchRaw(q), idTable))
  }
  check('searchRaw with options',
    decodeJoined2(wasm.searchJoinedOpts('vit', { prefix: true, fuzzy: 0.2 })),
    decodeRaw(wasm.searchRaw('vit', { prefix: true, fuzzy: 0.2 }), idTable))
  check('searchJoinedOpts exact lookup',
    js.search('vita', { prefix: false, fuzzy: false, combineWith: 'OR' }).map(r => r.id),
    (() => { const r = wasm.searchJoinedOpts('vita', { prefix: false, fuzzy: false, combineWith: 'OR' }); return r.count ? JSON.parse(r.ids) : [] })())
}

console.log(failures === 0 ? 'WASM SMOKE: ALL PASS' : `${failures} FAILURES`)
process.exitCode = failures === 0 ? 0 : 1
