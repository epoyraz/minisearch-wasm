// End-to-end Wasm benchmark vs JS MiniSearch through the built pkg/.
// Usage: node bench_wasm.mjs corpus.json [rounds]
import { readFileSync } from 'fs'
import MiniSearch from 'minisearch'
import init, { MiniSearchWasm } from '../pkg/minisearch_wasm.js'

await init(readFileSync(new URL('../pkg/minisearch_wasm_bg.wasm', import.meta.url)))

const { docs, queries } = JSON.parse(readFileSync(process.argv[2], 'utf8'))
const rounds = Number(process.argv[3] || 9)
const docsJson = JSON.stringify(docs)

const searchOptions = { prefix: true, fuzzy: 0.2, combineWith: 'AND' }

const median = (xs) => { const s = [...xs].sort((a, b) => a - b); return s[Math.floor(s.length / 2)] }

const bench = (name, run) => {
  let checksum = run() // warmup
  const samples = []
  for (let i = 0; i < rounds; i++) {
    const start = performance.now()
    checksum += run()
    samples.push(performance.now() - start)
  }
  const ms = median(samples)
  console.log(`${name.padEnd(40)} median ${ms.toFixed(3).padStart(9)} ms  (checksum ${checksum})`)
  return ms
}

console.log(`corpus: ${docs.length} docs, ${queries.length} queries, rounds=${rounds}`)

// --- build ---
const jsBuild = bench('js  addAll', () => {
  const ms = new MiniSearch({ fields: ['title', 'text'], searchOptions, autoVacuum: false })
  ms.addAll(docs)
  return ms.termCount
})
const wasmBuild = bench('wasm addAllJSON', () => {
  const ms = new MiniSearchWasm({ fields: ['title', 'text'], searchOptions })
  ms.addAllJSON(docsJson)
  return 1
})

const js = new MiniSearch({ fields: ['title', 'text'], searchOptions, autoVacuum: false })
js.addAll(docs)
const wasm = new MiniSearchWasm({ fields: ['title', 'text'], searchOptions })
wasm.addAllJSON(docsJson)

// --- search: app workload (ids + scores + terms consumed) ---
const jsSearch = bench('js  search AND prefix+fuzzy (app)', () => {
  let n = 0
  for (const q of queries) {
    for (const r of js.search(q)) { n += r.terms.length + (r.score > 0 ? 1 : 0) }
  }
  return n
})
const wasmSearch = bench('wasm searchJoined AND prefix+fuzzy', () => {
  let n = 0
  for (const q of queries) {
    const r = wasm.searchJoined(q, false)
    if (!r.count) continue
    const ids = r.ids.split('\n')
    const terms = r.terms.split('\n')
    for (let i = 0; i < r.count; i++) {
      n += terms[i] ? terms[i].split(' ').length : 0
      n += r.scores[i] > 0 ? 1 : 0
      if (ids[i] === '') n += 1
    }
  }
  return n
})

// --- autoSuggest ---
const jsSuggest = bench('js  autoSuggest', () => {
  let n = 0
  for (const q of queries) n += js.autoSuggest(q).length
  return n
})
const wasmSuggest = bench('wasm autoSuggest', () => {
  let n = 0
  for (const q of queries) n += wasm.autoSuggest(q).length
  return n
})
const wasmSuggestJoined = bench('wasm autoSuggestJoined', () => {
  let n = 0
  for (const q of queries) {
    const r = wasm.autoSuggestJoined(q)
    n += r.count
  }
  return n
})

// --- persistence ---
const jsSerialized = JSON.stringify(js)
const wasmBytes = wasm.toBytes()
console.log(`js JSON: ${jsSerialized.length} chars, wasm bytes: ${wasmBytes.length}`)
const jsSave = bench('js  JSON.stringify', () => JSON.stringify(js).length)
const wasmSave = bench('wasm toBytes', () => wasm.toBytes().length)
const jsLoad = bench('js  loadJSON', () => MiniSearch.loadJSON(jsSerialized, { fields: ['title', 'text'], searchOptions }).termCount)
const wasmLoad = bench('wasm loadBytes', () => { const m = MiniSearchWasm.loadBytes(wasmBytes); return 1 })

console.log('--- ratios (js/wasm, >1 means wasm faster) ---')
console.log(`build:        ${(jsBuild / wasmBuild).toFixed(2)}x`)
console.log(`search app:   ${(jsSearch / wasmSearch).toFixed(2)}x`)
console.log(`autoSuggest:  ${(jsSuggest / wasmSuggest).toFixed(2)}x   (joined: ${(jsSuggest / wasmSuggestJoined).toFixed(2)}x)`)
console.log(`serialize:    ${(jsSave / wasmSave).toFixed(2)}x`)
console.log(`load:         ${(jsLoad / wasmLoad).toFixed(2)}x`)
