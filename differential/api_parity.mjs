// API-level parity with JS MiniSearch through the built package: real Error
// objects with MiniSearch's messages, rejection of callback options and the
// declarative forms that replace them, JS field stringification (Date,
// toString), the JS engine's Unicode tables, getDefault / loadJSONAsync /
// logger, and toJSON / loadJSON in MiniSearch's format. Run after npm run build.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import MiniSearch from 'minisearch'
import init, { MiniSearchWasm } from '../pkg/minisearch_wasm_core.js'

await init({ module_or_path: readFileSync(new URL('../pkg/minisearch_wasm_bg.wasm', import.meta.url)) })
let checks = 0
const equal = (actual, expected, label) => { assert.deepEqual(actual, expected, label); checks++ }
const key = id => JSON.stringify(id)
const thrown = (fn, label) => { try { fn() } catch (error) { return error } assert.fail(label + ': expected an error') }
const sameError = (label, jsCall, wasmCall) => {
  const js = thrown(jsCall, label + ' (js)')
  const wasm = thrown(wasmCall, label + ' (wasm)')
  assert.ok(wasm instanceof Error, label + ': wasm throws an Error, got ' + typeof wasm)
  equal(wasm.message, js.message, label + ': message')
  checks++
}
const rows = results => results.map(r => ({ id: key(r.id), score: Math.round(r.score * 1e9) / 1e9, terms: r.terms, queryTerms: r.queryTerms, match: r.match }))
const sameResults = (label, js, wasm, query, jsOptions, wasmOptions) => {
  js.search(query, jsOptions)
  equal(rows(wasm.search(query, wasmOptions)), rows(js.search(query, jsOptions)), label + ' search')
  if (typeof query === 'string') {
    const joined = wasm.searchJoinedOpts(query, wasmOptions)
    equal(JSON.parse(joined.ids).map(key), js.search(query, jsOptions).map(r => key(r.id)), label + ' joined')
    js.autoSuggest(query, jsOptions)
    equal(wasm.autoSuggest(query, wasmOptions).map(s => s.suggestion), js.autoSuggest(query, jsOptions).map(s => s.suggestion), label + ' suggestions')
  }
}
const indexTerms = engine => (engine instanceof MiniSearch ? engine.toJSON() : engine.toJSON()).index.map(([term]) => term).sort()

const options = { fields: ['title', 'text'], storeFields: ['category', 'title'] }
const documents = [
  { id: 1, title: 'apple pie', text: 'sweet apple', category: 'books' },
  { id: 2, title: 'apply now', text: 'apple jobs', category: 'toys' },
  { id: 3, title: 'pear pie', text: 'pear', category: 'books' },
  { id: 4, title: 'pineapple', text: 'apple pine', category: 'toys' }
]
const fresh = () => { const js = new MiniSearch(options), wasm = new MiniSearchWasm(options); js.addAll(documents); wasm.addAll(documents); return { js, wasm } }

// 1. Errors are Error objects with MiniSearch's messages.
{
  const { js, wasm } = fresh()
  sameError('duplicate add', () => js.add({ id: 1, title: 'x' }), () => wasm.add({ id: 1, title: 'x' }))
  sameError('remove missing', () => js.remove({ id: 9, title: 'x' }), () => wasm.remove({ id: 9, title: 'x' }))
  sameError('discard missing', () => js.discard(9), () => wasm.discard(9))
  sameError('missing id field', () => js.add({ title: 'no id' }), () => wasm.add({ title: 'no id' }))
  sameError('constructor without fields', () => new MiniSearch({ storeFields: ['a'] }), () => new MiniSearchWasm({ storeFields: ['a'] }))
  sameError('constructor without options', () => new MiniSearch(), () => new MiniSearchWasm())
  sameError('invalid combineWith', () => js.search('apple', { combineWith: 'XOR' }), () => wasm.search('apple', { combineWith: 'XOR' }))
  sameError('invalid combineWith in a tree', () => js.search({ combineWith: 'NOR', queries: ['apple'] }), () => wasm.search({ combineWith: 'NOR', queries: ['apple'] }))
  sameError('removeAll with a non-array', () => js.removeAll(5), () => wasm.removeAll(5))
  sameError('removeAll with null', () => js.removeAll(null), () => wasm.removeAll(null))
  sameError('unknown getDefault', () => MiniSearch.getDefault('nope'), () => MiniSearchWasm.getDefault('nope'))
  for (const bad of [() => wasm.search(42), () => MiniSearchWasm.loadNativeJSON('{'), () => MiniSearchWasm.loadBytes(new Uint8Array([9]))]) {
    const error = thrown(bad, 'invalid input')
    assert.ok(error instanceof Error && error.message.length > 0, 'invalid input throws an Error'); checks++
  }
  wasm.free()
}
console.log('ok errors are Error objects with MiniSearch messages')

// 2. Callback options are rejected loudly; declarative forms match JS callbacks.
{
  const { js, wasm } = fresh()
  for (const [name, value] of [['filter', () => true], ['boostDocument', () => 2], ['boostTerm', () => 2], ['prefix', () => true], ['fuzzy', () => 0.2], ['tokenize', s => [s]], ['processTerm', t => t]]) {
    const error = thrown(() => wasm.search('apple', { [name]: value }), name)
    assert.ok(error instanceof Error && error.message.includes(`"${name}"`), `${name}: ${error.message}`); checks++
    const nested = thrown(() => new MiniSearchWasm({ ...options, searchOptions: { [name]: value } }), name + ' in constructor searchOptions')
    assert.ok(nested.message.includes(`"${name}"`)); checks++
    const tree = thrown(() => wasm.search({ queries: ['apple'], [name]: value }), name + ' in a query node')
    assert.ok(tree.message.includes(`"${name}"`)); checks++
    if (name !== 'boostDocument') {
      const suggest = thrown(() => wasm.autoSuggest('app', { [name]: value }), name + ' in autoSuggest')
      assert.ok(suggest.message.includes(`"${name}"`)); checks++
    }
  }
  for (const name of ['extractField', 'stringifyField', 'tokenize', 'processTerm']) {
    const error = thrown(() => new MiniSearchWasm({ ...options, [name]: x => x }), name)
    assert.ok(error instanceof Error && error.message.includes(`"${name}"`)); checks++
  }
  // The logger is the one callback that is supported.
  new MiniSearchWasm({ ...options, logger: () => {} }).free(); checks++

  sameResults('per-term prefix', js, wasm, 'app pi', { prefix: (term, i) => i === 1 }, { prefix: [false, true] })
  sameResults('per-term prefix (all true)', js, wasm, 'app pi', { prefix: () => true }, { prefix: [true, true] })
  sameResults('per-term fuzzy', js, wasm, 'appel pie', { fuzzy: (term, i) => (i === 0 ? 0.4 : false) }, { fuzzy: [0.4, false] })
  sameResults('per-term fuzzy true', js, wasm, 'appel pei', { fuzzy: (term, i) => i === 1 }, { fuzzy: [false, true] })
  sameResults('boostTerm', js, wasm, 'apple pie', { boostTerm: (term, i) => (i === 0 ? 2 : 1) }, { boostTerm: [2, 1] })
  sameResults('boostTerm with prefix', js, wasm, 'app pie', { prefix: true, boostTerm: (term, i) => [3, 0.5][i] }, { prefix: true, boostTerm: [3, 0.5] })
  sameResults('filter on a stored field', js, wasm, 'apple', { filter: r => r.category === 'books' }, { filter: { category: 'books' } })
  sameResults('filter on two stored fields', js, wasm, 'apple pie', { prefix: true, filter: r => r.category === 'books' && r.title === 'apple pie' }, { prefix: true, filter: { category: 'books', title: 'apple pie' } })
  sameResults('filter on the id', js, wasm, 'apple', { filter: r => r.id === 2 }, { filter: { id: 2 } })
  sameResults('filter matching nothing', js, wasm, 'apple', { filter: r => r.category === 'nope' }, { filter: { category: 'nope' } })
  const jsTree = { combineWith: 'OR', queries: ['apple', 'pear'], filter: r => r.category === 'toys' }
  const wasmTree = { combineWith: 'OR', queries: ['apple', 'pear'], filter: { category: 'toys' } }
  js.search(jsTree)
  equal(rows(wasm.search(wasmTree)), rows(js.search(jsTree)), 'filter in a query tree')
  wasm.free()
}
console.log('ok callbacks rejected; per-term prefix/fuzzy/boostTerm and stored-field filter match JS callbacks')

// 3. Field values stringify like MiniSearch's default `stringifyField`.
{
  const odd = [
    { id: 1, title: new Date(0), text: 'dated' },
    { id: 2, title: { toString () { return 'custom string' } }, text: 'custom' },
    { id: 3, title: ['a', { toString () { return 'nested' } }, 5, true, null], text: 'array' },
    { id: 4, title: 1e21, text: 'exponent' },
    { id: 5, title: 'plain', text: null, method () { return 1 }, category: new Date(86400000) },
    { id: 6, title: 12.5, text: false }
  ]
  const js = new MiniSearch(options), wasm = new MiniSearchWasm(options)
  js.addAll(odd); wasm.addAll(odd)
  equal(indexTerms(wasm), indexTerms(js), 'index terms for Date, toString, arrays, numbers, booleans')
  for (const term of ['1970', 'custom', 'nested', 'true', '1e+21', '12', 'false', 'plain']) {
    equal(wasm.search(term).map(r => key(r.id)), js.search(term).map(r => key(r.id)), 'search ' + term)
  }
  equal(JSON.stringify(wasm.getStoredFields(5)), JSON.stringify(js.getStoredFields(5)), 'stored Date is JSON-equivalent')
  js.remove(odd[0]); wasm.remove(odd[0])
  js.remove(odd[1]); wasm.remove(odd[1])
  equal(indexTerms(wasm), indexTerms(js), 'index terms after removing odd documents')
  equal(wasm.documentCount, js.documentCount, 'document count after removals')
  wasm.free()
}
console.log('ok Date, toString, array and primitive field values index like JS')

// 4. Tokenizer separators come from the JavaScript engine's Unicode tables.
{
  const text = 'İSTANBUL ΟΔΟΣ ǅemal a\u{1B7F}b c\u{2E5D}d e\u{10D6E}f g\u{00AD}h i\u{2060}j k\u{200B}l m\u{0009}n x\u{2E5C}y'
  const js = new MiniSearch({ fields: ['text'] }), wasm = new MiniSearchWasm({ fields: ['text'] })
  js.add({ id: 1, text }); wasm.add({ id: 1, text })
  equal(indexTerms(wasm), indexTerms(js), `tokenization of Unicode ${process.versions.unicode} separators and lowercasing`)
  wasm.free()
}
console.log(`ok tokenizer matches Node's Unicode ${process.versions.unicode} tables`)

// 5. getDefault, loadJSONAsync, logger, toJSON/loadJSON in MiniSearch's format.
{
  for (const name of ['idField', 'storeFields', 'autoVacuum', 'fields', 'searchOptions']) {
    equal(MiniSearchWasm.getDefault(name), MiniSearch.getDefault(name), 'getDefault ' + name)
  }
  equal(MiniSearchWasm.getDefault('tokenize')('a, b-c d'), MiniSearch.getDefault('tokenize')('a, b-c d'), 'getDefault tokenize')
  equal(MiniSearchWasm.getDefault('processTerm')('ÄBC'), MiniSearch.getDefault('processTerm')('ÄBC'), 'getDefault processTerm')
  equal(MiniSearchWasm.getDefault('extractField')({ a: 1 }, 'a'), MiniSearch.getDefault('extractField')({ a: 1 }, 'a'), 'getDefault extractField')
  equal(MiniSearchWasm.getDefault('stringifyField')(12.5), MiniSearch.getDefault('stringifyField')(12.5), 'getDefault stringifyField')
  equal(typeof MiniSearchWasm.getDefault('logger'), 'function', 'getDefault logger')

  const jsLog = [], wasmLog = []
  const js = new MiniSearch({ ...options, logger: (...args) => jsLog.push(args) })
  const wasm = new MiniSearchWasm({ ...options, logger: (...args) => wasmLog.push(args) })
  js.add({ id: 1, title: 'a b' }); wasm.add({ id: 1, title: 'a b' })
  js.remove({ id: 1, title: 'a c' }); wasm.remove({ id: 1, title: 'a c' })
  equal(wasmLog, jsLog, 'logger receives the version_conflict warning')
  equal(wasm.documentCount, js.documentCount, 'document removed despite the conflict')

  const { js: full, wasm: wasmFull } = fresh()
  const shape = JSON.parse(JSON.stringify(wasmFull))
  equal(Object.keys(shape).sort(), Object.keys(JSON.parse(JSON.stringify(full))).sort(), 'JSON.stringify(index) has MiniSearch\'s shape')
  equal(shape.serializationVersion, 2, 'serializationVersion')
  const jsFromWasm = MiniSearch.loadJSON(JSON.stringify(wasmFull), options)
  const wasmFromJs = MiniSearchWasm.loadJSON(JSON.stringify(full), options)
  const wasmAsync = await MiniSearchWasm.loadJSONAsync(JSON.stringify(full), options)
  for (const query of ['apple', 'pie', 'app']) {
    const expected = full.search(query, { prefix: true }).map(r => key(r.id)).sort()
    equal(jsFromWasm.search(query, { prefix: true }).map(r => key(r.id)).sort(), expected, 'JS loads JSON.stringify(wasm) ' + query)
    equal(wasmFromJs.search(query, { prefix: true }).map(r => key(r.id)).sort(), expected, 'wasm.loadJSON(JSON.stringify(js)) ' + query)
    equal(wasmAsync.search(query, { prefix: true }).map(r => key(r.id)).sort(), expected, 'loadJSONAsync ' + query)
  }
  equal(wasmFromJs.getStoredFields(1), full.getStoredFields(1), 'stored fields survive loadJSON')
  const native = wasmFull.toNativeJSONString()
  assert.ok(JSON.parse(native).snapshot_version === 4, 'native JSON is the engine format')
  equal(MiniSearchWasm.loadNativeJSON(native).search('apple').map(r => key(r.id)), wasmFull.search('apple').map(r => key(r.id)), 'native round trip')
  await assert.rejects(MiniSearchWasm.loadJSONAsync('{"serializationVersion":9}', options), error => error instanceof Error)
  checks++
  for (const engine of [wasm, wasmFull, wasmFromJs, wasmAsync]) engine.free()
}
console.log('ok getDefault, logger, loadJSONAsync and MiniSearch-format toJSON/loadJSON')
console.log(`API PARITY: ALL PASS (${checks} explicit checks)`)
