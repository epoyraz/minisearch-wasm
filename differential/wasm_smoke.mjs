// Smoke test of the built pkg/: autoSuggest + autoSuggestJoined through the
// real Wasm boundary, compared against JS MiniSearch.
import { readFileSync } from 'fs'
import MiniSearch from 'minisearch'
import init, { MiniSearchWasm } from '../pkg/minisearch_wasm.js'

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
// Full-path term/match ordering differs (sorted vs insertion, documented), so
// compare ids and scores; per-row terms as sorted sets.
const rowsOf = (results) => results.map(r => ({ id: r.id, score: r.score, terms: [...r.terms].sort() }))
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

console.log(failures === 0 ? 'WASM SMOKE: ALL PASS' : `${failures} FAILURES`)
process.exit(failures === 0 ? 0 : 1)
