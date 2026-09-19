// Exact-order parity with JS MiniSearch: equal-score ties, `terms`/`match`
// order, JS number formatting of field values and ids, UTF-16 term lengths,
// the `has`/`replace`/`getStoredFields` API, and MiniSearch's own JSON format
// in both directions. Run after npm run build.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import MiniSearch from 'minisearch'
import init, { MiniSearchWasm } from '../pkg/minisearch_wasm_core.js'

await init({ module_or_path: readFileSync(new URL('../pkg/minisearch_wasm_bg.wasm', import.meta.url)) })
let checks = 0
const equal = (actual, expected, label) => { assert.deepEqual(actual, expected, label); checks++ }
const key = id => JSON.stringify(id)
const closeScores = (actual, expected, label) => {
  equal(actual.length, expected.length, label + ' count')
  actual.forEach((score, i) => {
    const delta = Math.abs(score - expected[i]) / Math.max(Math.abs(score), Math.abs(expected[i]), 1e-12)
    assert.ok(Number.isFinite(score) && delta <= 1e-12, `${label} row ${i}: ${score} vs ${expected[i]}`)
    checks++
  })
}
const optionSets = [{}, { prefix: true }, { fuzzy: 0.2 }, { combineWith: 'AND' }, { prefix: true, fuzzy: 0.2, combineWith: 'AND' }]

// Same result sets and scores, order-insensitive. Used across a JSON
// round trip, where JS itself rebuilds the radix tree in serialized order and
// so may change equal-score and matched-term order.
function scoreParity (js, wasm, label, queries) {
  for (const query of queries.filter(q => typeof q === 'string')) {
    for (const opts of optionSets) {
      js.search(query, opts)
      const expected = new Map(js.search(query, opts).map(r => [key(r.id), r]))
      const actual = wasm.search(query, opts)
      equal(actual.map(r => key(r.id)).sort(), [...expected.keys()].sort(), `${label} ${query} ids`)
      for (const row of actual) {
        const other = expected.get(key(row.id))
        closeScores([row.score], [other.score], `${label} ${query} ${key(row.id)}`)
        equal([...row.terms].sort(), [...other.terms].sort(), `${label} ${query} ${key(row.id)} terms`)
      }
    }
  }
}

// Every result format must reproduce the JS result order exactly, including
// ties, plus the per-row `terms` (JS `Object.keys(match)` order) and `match`.
function exactParity (js, wasm, label, queries) {
  equal(wasm.documentCount, js.documentCount, label + ' documentCount')
  for (const query of queries) {
    for (const opts of optionSets) {
      const tag = `${label} ${JSON.stringify(query)} ${JSON.stringify(opts)}`
      js.search(query, opts) // JS's documented dirty-index cleanup fixpoint
      const expected = js.search(query, opts)
      const full = wasm.search(query, opts)
      equal(full.map(r => key(r.id)), expected.map(r => key(r.id)), tag + ' order')
      closeScores(full.map(r => r.score), expected.map(r => r.score), tag + ' scores')
      equal(full.map(r => r.terms), expected.map(r => r.terms), tag + ' terms')
      equal(full.map(r => r.queryTerms), expected.map(r => r.queryTerms), tag + ' queryTerms')
      equal(full.map(r => Object.keys(r.match)), expected.map(r => Object.keys(r.match)), tag + ' match keys')
      equal(full.map(r => r.match), expected.map(r => r.match), tag + ' match')
      if (typeof query !== 'string') continue

      const joined = wasm.searchJoinedOpts(query, opts)
      equal(JSON.parse(joined.ids).map(key), expected.map(r => key(r.id)), tag + ' joined order')
      equal(joined.count ? joined.terms.split('\n').map(t => t ? t.split(' ') : []) : [], expected.map(r => r.terms), tag + ' joined terms')
      const raw = wasm.searchRaw(query, opts)
      const table = JSON.parse(wasm.docIdTable())
      const termTable = raw.termTable ? raw.termTable.split('\n') : []
      equal(Array.from(raw.docIds, i => key(table[i])), expected.map(r => key(r.id)), tag + ' raw order')
      equal(Array.from(raw.docIds, (_, i) => Array.from(raw.termIds.slice(raw.termOffsets[i], raw.termOffsets[i + 1]), t => termTable[t])),
        expected.map(r => r.terms), tag + ' raw terms')

      js.autoSuggest(query, opts)
      const jsSuggestions = js.autoSuggest(query, opts)
      const suggestions = wasm.autoSuggest(query, opts)
      equal(suggestions.map(s => s.suggestion), jsSuggestions.map(s => s.suggestion), tag + ' suggestions')
      equal(suggestions.map(s => s.terms), jsSuggestions.map(s => s.terms), tag + ' suggestion terms')
      closeScores(suggestions.map(s => s.score), jsSuggestions.map(s => s.score), tag + ' suggestion scores')
    }
  }
}

// A corpus with many exact ties (repeated short texts in shuffled order),
// numeric tokens, non-ASCII and astral characters.
const words = ['alpha', 'beta', 'gamma', 'delta', 'alphabet', 'alpine', 'betamax', 'gamble', '2024', '2023', '7', 'report', 'x1', 'ünïcode', '😀😀', '😀😀😀']
let seed = 0x5eed
const random = n => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed % n }
const documents = []
for (let i = 0; i < 300; i++) {
  const a = words[random(words.length)]
  const b = words[random(words.length)]
  documents.push({ id: i, title: random(3) ? a : `${a} ${b}`, text: random(2) ? `${b} ${a}` : a, tag: `t${random(4)}` })
}
const options = { fields: ['title', 'text'], storeFields: ['tag'], autoVacuum: false }
const queries = [
  'alpha', 'alp', 'beta gamma', 'gamma beta', 'report 2024', '2024 report', '7 alpha', 'alphabet alpine',
  'gamble', 'delt', 'ünïcode', 'unicode', '😀', '😀😀😀', '😀😀😀😀', 'xx', '',
  { combineWith: 'AND', queries: ['alpha', { combineWith: 'OR', queries: ['beta', 'gamma'] }] },
  { combineWith: 'AND_NOT', queries: ['alpha', 'beta'] },
  { combineWith: 'OR', queries: [{ prefix: true, queries: ['alp'] }, { fuzzy: 0.2, queries: ['gamble'] }] },
  { queries: ['2024', 'report', 'alpha'] }
]
const js = new MiniSearch(options)
const wasm = new MiniSearchWasm(options)
js.addAll(documents)
wasm.addAll(documents)
for (const phase of ['fresh', 'discarded', 'removed', 'vacuumed']) {
  if (phase === 'discarded') {
    for (const doc of documents.filter((_, i) => i % 7 === 3)) { js.discard(doc.id); wasm.discard(doc.id) }
  } else if (phase === 'removed') {
    for (const doc of documents.filter((_, i) => i % 11 === 5 && i % 7 !== 3)) { js.remove(doc); wasm.remove(doc) }
  } else if (phase === 'vacuumed') {
    await js.vacuum(); await wasm.vacuum()
  }
  exactParity(js, wasm, phase, queries)

  // MiniSearch's own JSON format, both directions, in every index state.
  // `loadJSON` re-inserts terms in serialized order on both sides, which can
  // reorder equal scores and matched terms relative to the live index, so the
  // exact-order oracle is a JS instance reloaded from the same JSON, and the
  // live index is compared on sets and scores.
  const jsJson = JSON.stringify(js)
  const fromJs = MiniSearchWasm.loadMiniSearchJSON(jsJson, options)
  const jsReloaded = MiniSearch.loadJSON(jsJson, options)
  equal(fromJs.dirtCount, js.dirtCount, phase + ' imported dirtCount')
  exactParity(jsReloaded, fromJs, phase + ' imported from JS JSON', queries)
  scoreParity(js, fromJs, phase + ' imported vs live JS', queries)
  equal(fromJs.getStoredFields(3), js.getStoredFields(3), phase + ' imported stored fields')
  fromJs.free()
  const wasmJson = wasm.toMiniSearchJSON()
  const backToJs = MiniSearch.loadJSON(wasmJson, options)
  const wasmReloaded = MiniSearchWasm.loadMiniSearchJSON(wasmJson, options)
  equal(backToJs.dirtCount, wasm.dirtCount, phase + ' exported dirtCount')
  exactParity(backToJs, wasmReloaded, phase + ' exported to JS', queries)
  scoreParity(backToJs, wasm, phase + ' exported vs live WASM', queries)
  wasmReloaded.free()
}
console.log('ok exact result, term, match and suggestion order across fresh, dirty and vacuumed indexes, and MiniSearch JSON both ways')

// has / getStoredFields / replace, MiniSearch-compatible.
equal(wasm.has(4), js.has(4), 'has')
equal(wasm.has('missing'), js.has('missing'), 'has missing')
equal(wasm.getStoredFields(4), js.getStoredFields(4), 'getStoredFields')
equal(wasm.getStoredFields('missing'), js.getStoredFields('missing'), 'getStoredFields missing')
const updated = { id: 4, title: 'replaced alpha', text: 'beta', tag: 'new' }
js.replace(updated)
wasm.replace(updated)
equal(wasm.getStoredFields(4), js.getStoredFields(4), 'getStoredFields after replace')
exactParity(js, wasm, 'replaced', ['alpha', 'replaced', 'beta', 'replaced alpha'])
assert.throws(() => wasm.replace({ title: 'no id' }), /does not have ID field/)
console.log('ok has, getStoredFields and replace')

// Field values stringify like JS `String(value)`: 10.0 is "10", 1e21 is
// "1e+21", 0.000001 stays plain, 1e-7 gets an exponent, arrays join with
// commas. Raw JSON text keeps the float spellings JS would have normalized.
const numericText = '[{"id":1,"price":10.0,"text":"ten"},{"id":2,"price":10.5,"text":"ten and a half"},' +
  '{"id":3,"price":1e21,"text":"big"},{"id":4,"price":0.000001,"text":"tiny"},{"id":5,"price":1e-7,"text":"tinier"},' +
  '{"id":6,"price":-2.0,"text":"negative"},{"id":7,"price":[1.0,2.5,null,true],"text":"list"},{"id":8,"price":123456789012345678901,"text":"huge"}]'
const numericOptions = { fields: ['price', 'text'], autoVacuum: false }
const jsNumeric = new MiniSearch(numericOptions)
jsNumeric.addAll(JSON.parse(numericText))
const wasmNumeric = new MiniSearchWasm(numericOptions)
wasmNumeric.addAllJSON(numericText)
exactParity(jsNumeric, wasmNumeric, 'numeric fields', ['10', '10 5', '1e', '21', '000001', '7', '2', '1', 'true', 'ten', '123456789012345680000'])

// A float-valued id is the same JS number as its integer form.
const jsFloatId = new MiniSearch(numericOptions)
jsFloatId.addAll(JSON.parse('[{"id":1.0,"text":"one"}]'))
const wasmFloatId = new MiniSearchWasm(numericOptions)
wasmFloatId.addAllJSON('[{"id":1.0,"text":"one"}]')
equal(wasmFloatId.has(1), jsFloatId.has(1), 'float id has')
assert.throws(() => wasmFloatId.add({ id: 1, text: 'duplicate' }), /duplicate ID 1/)
wasmFloatId.discard(1)
equal(wasmFloatId.documentCount, 0, 'float id discarded by integer')
console.log('ok JS number formatting for field values and ids')

// Import rejects other versions and mismatched options.
assert.throws(() => MiniSearchWasm.loadMiniSearchJSON(JSON.stringify({ ...js.toJSON(), serializationVersion: 3 }), options), /incompatible version/)
assert.throws(() => MiniSearchWasm.loadMiniSearchJSON(JSON.stringify(js), { fields: ['other'] }), error => error instanceof Error && !(error instanceof WebAssembly.RuntimeError) && /MiniSearch|snapshot|deserialize/i.test(error.message))
assert.throws(() => MiniSearchWasm.loadMiniSearchJSON('{"documentCount": 1}', options), error => error instanceof Error && !(error instanceof WebAssembly.RuntimeError) && /MiniSearch|snapshot|deserialize/i.test(error.message))
console.log('ok MiniSearch JSON import validation')
console.log(`COMPAT PARITY: ALL PASS (${checks} explicit checks)`)
