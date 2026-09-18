// End-to-end regressions for the four high-priority review findings.
// Run after npm run build. The pinned JS engine is the ranking oracle.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import MiniSearch from 'minisearch'
import init, { MiniSearchWasm } from '../pkg/minisearch_wasm_core.js'

await init({ module_or_path: readFileSync(new URL('../pkg/minisearch_wasm_bg.wasm', import.meta.url)) })
let assertions = 0
const equal = (actual, expected, label) => { assert.deepEqual(actual, expected, label); assertions++ }
const options = { fields: ['text', 'optional'], storeFields: ['payload'], autoVacuum: false }
const key = id => JSON.stringify(id)
const decodeJoined = r => {
  const ids = JSON.parse(r.ids)
  equal(ids.length, r.count, 'joined row count')
  const terms = r.count ? r.terms.split('\n') : []
  return ids.map((id, i) => ({ id, score: r.scores[i], terms: terms[i] ? terms[i].split(' ') : [] }))
}
const decodeRaw = (engine, r) => {
  equal(r.idTableVersion, engine.idTableVersion, 'raw generation')
  const table = JSON.parse(engine.docIdTable())
  const terms = r.termTable ? r.termTable.split('\n') : []
  return Array.from(r.docIds, (id, i) => ({ id: table[id], score: r.scores[i],
    terms: Array.from(r.termIds.slice(r.termOffsets[i], r.termOffsets[i + 1]), term => terms[term]) }))
}
function parity(js, wasm, label, queries = ['apple', 'app', 'applf', 'pear', 'apple pear', '']) {
  equal(wasm.documentCount, js.documentCount, label + ' document count')
  for (const query of queries) for (const opts of [{}, { prefix: true }, { fuzzy: 0.2 }, { prefix: true, fuzzy: 0.2, combineWith: 'AND' }]) {
    js.search(query, opts) // JS's documented dirty-index cleanup fixpoint.
    const expected = js.search(query, opts)
    const byId = new Map(expected.map(row => [key(row.id), row]))
    for (const rows of [wasm.search(query, opts), decodeJoined(wasm.searchJoinedOpts(query, opts)), decodeRaw(wasm, wasm.searchRaw(query, opts))]) {
      equal(rows.length, expected.length, label + ' result count')
      for (const row of rows) {
        const other = byId.get(key(row.id))
        assert.ok(other, label + ' missing ID ' + key(row.id))
        const delta = Math.abs(row.score - other.score) / Math.max(Math.abs(row.score), Math.abs(other.score), 1e-12)
        assert.ok(Number.isFinite(row.score) && delta <= 1e-12, `${label}: ${query} score ${row.score} vs ${other.score}`)
        equal([...row.terms].sort(), [...other.terms].sort(), label + ' matched terms')
      }
    }
  }
}
function roundTrips(engine, check) {
  for (const copy of [MiniSearchWasm.loadBytes(engine.toBytes()), MiniSearchWasm.loadNativeJSON(engine.toNativeJSONString())]) {
    try { check(copy) } finally { copy.free() }
  }
}

// Sparse fields, empty boundary tokens, null fields, and removals/discards.
const documents = [
  { id: 1, text: 'apple', optional: 'pear', payload: { tag: 'first' } },
  { id: 2, text: ' apple ' }, { id: 3, text: null }, { id: 4 },
  { id: 5, text: '' }, { id: 6, text: ' ... ' },
  { id: 7, text: 'apple,,pear!', optional: null }, { id: 8, text: 'pear' }
]
let seed = 0x92119
const random = n => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed % n }
const fields = [undefined, null, '', ' ', 'apple', ' apple ', '.pear apple!', 'apple...pear', 'pear', []]
for (let i = 0; i < 100; i++) {
  const doc = { id: i + 20 }
  for (const name of options.fields) { const value = fields[random(fields.length)]; if (value !== undefined) doc[name] = value }
  documents.push(doc)
}
const js = new MiniSearch(options), wasm = new MiniSearchWasm(options)
js.addAll(documents); wasm.addAll(documents)
for (const phase of ['fresh', 'removed', 'discarded', 'vacuumed']) {
  if (phase === 'removed') {
    for (const doc of documents.filter((_, i) => i % 5 === 0)) { js.remove(doc); wasm.remove(doc) }
  } else if (phase === 'discarded') {
    for (const doc of documents.filter((_, i) => i % 5 === 1)) { js.discard(doc.id); wasm.discard(doc.id) }
  } else if (phase === 'vacuumed') { await js.vacuum(); await wasm.vacuum() }
  parity(js, wasm, phase)
  roundTrips(wasm, copy => parity(js, copy, phase + ' reloaded'))
}
wasm.free()
console.log('ok field accounting: seeded sparse fields, all result formats, maintenance and both snapshots')

// Above the former u16 limit; compare scores both before and after discards.
const longDocs = [65535, 65536, 70000].map((length, id) => ({ id,
  text: Array.from({ length }, (_, i) => 't' + i).join(' ') }))
longDocs.push({ id: 3, text: 't0' })
const longJS = new MiniSearch(options), longWasm = new MiniSearchWasm(options)
longJS.addAll(longDocs); longWasm.addAll(longDocs)
parity(longJS, longWasm, 'long fields', ['t0'])
roundTrips(longWasm, copy => {
  const reference = new MiniSearch(options); reference.addAll(longDocs)
  for (let id = 0; id < 3; id++) { reference.discard(id); copy.discard(id) }
  equal(copy.toNativeJSON().average_field_length[0], 1, 'average after discarding long fields')
  parity(reference, copy, 'long fields discarded after reload', ['t0'])
})
longWasm.free()
console.log('ok exact field lengths at 65535, 65536 and 70000 unique tokens')

// Preserve complete suggestions, term ordering and scores, including splits,
// leaf/child interleaving, Unicode edges, deletes and dirty postings.
for (const text of ['apply application apple', 'app apply application apple', 'car cat can', 'dog data day', 'éclair écran école']) {
  const engine = new MiniSearchWasm({ ...options, searchOptions: { prefix: true, fuzzy: 0.2 } })
  engine.addAll([{ id: 1, text }, { id: 2, text: 'app cart dog' }, { id: 3, text: 'apparatus carrot day' }])
  for (let phase = 0; phase < 4; phase++) {
    if (phase === 1) engine.remove({ id: 3, text: 'apparatus carrot day' })
    if (phase === 2) engine.discard(2)
    if (phase === 3) await engine.vacuum()
    roundTrips(engine, copy => {
      for (const query of ['a', 'app', 'c', 'd', 'é', 'applf', 'dag']) {
        equal(copy.searchJoined(query, false), engine.searchJoined(query, false), 'ordered joined round trip')
        equal(copy.search(query), engine.search(query), 'full round trip')
        equal(copy.autoSuggest(query), engine.autoSuggest(query), 'suggestion round trip')
      }
    })
  }
  engine.free()
}
console.log('ok radix traversal and full suggestion preservation across snapshots')

const ids = ['a\nb', 1, '1', '', true, ['a', 1], { id: 'nested\nvalue' }, '"quoted"\\slash', '😀']
const idEngine = new MiniSearchWasm(options)
idEngine.addAll(ids.map(id => ({ id, text: 'apple' })))
const verifyIds = engine => {
  const expected = engine.search('apple').map(row => row.id)
  equal(decodeJoined(engine.searchJoined('apple', false)).map(row => row.id), expected, 'joined typed IDs')
  equal(decodeRaw(engine, engine.searchRaw('apple')).map(row => row.id), expected, 'raw typed IDs')
}
verifyIds(idEngine); roundTrips(idEngine, verifyIds)
let generation = idEngine.idTableVersion
idEngine.discard(1)
assert.notEqual(idEngine.idTableVersion, generation)
equal(JSON.parse(idEngine.docIdTable())[1], null, 'discard leaves null table slot')
generation = idEngine.idTableVersion
idEngine.add({ id: 1, text: 'apple' })
assert.notEqual(idEngine.idTableVersion, generation)
verifyIds(idEngine); roundTrips(idEngine, verifyIds)
generation = idEngine.idTableVersion
idEngine.remove({ id: true, text: 'apple' })
assert.notEqual(idEngine.idTableVersion, generation)
generation = idEngine.idTableVersion
idEngine.removeAll()
assert.notEqual(idEngine.idTableVersion, generation)
equal(JSON.parse(idEngine.docIdTable()), [], 'reset empty table')
equal(JSON.parse(idEngine.searchJoined('apple', false).ids), [], 'empty joined IDs')
assert.throws(() => idEngine.add({ id: null, text: 'apple' }))
idEngine.free()
console.log('ok lossless compact IDs, null rejection and mutation generations')

// Small deliberately malformed v4 inputs must throw descriptive errors, never
// a WebAssembly.RuntimeError. Huge advertised lengths must fail before reserve.
const v = n => { const out = []; do { const byte = n % 128; n = Math.floor(n / 128); out.push(byte | (n ? 128 : 0)) } while (n); return out }
const str = text => { const bytes = [...new TextEncoder().encode(text)]; return [...v(bytes.length), ...bytes] }
const float = n => { const bytes = new Uint8Array(8); new DataView(bytes.buffer).setFloat64(0, n, true); return [...bytes] }
const header = (count = 0, next = count, dirt = 0, average = 0) => [4, ...str('{"fields":["text"]}'), ...v(count), ...v(next), ...v(dirt), ...float(average)]
const document = [0, 3, 1, 2, 0] // short ID delta, tagged unsigned ID 1, present length, stored count
const leaf = [0, 1, 0, 1, 0, 1, 0] // leaf slot; field; one posting; no children
const valid = [...header(1, 1, 0, 1), ...document, 0, 0, 1, ...str('apple'), ...leaf]
const probe = MiniSearchWasm.loadBytes(new Uint8Array(valid)); probe.free()
let deep = leaf
for (let i = 0; i < 130; i++) deep = [0, 0, 1, ...str('a'), ...deep]
const malformed = [
  [3], [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f], [0x84, 0],
  [4, ...v(0xffffffff)], [...header(0, 0xffffffff), 0, 0, 0],
  [...header(0, 0, 0, NaN), 0, 0, 0], [...valid, 0],
  [...header(1, 1, 0, 1), ...document, ...deep],
  [...header(), 0, 0, 1, 1, 0xff, 0, 0, 0],
  [...header(1, 1, 0, 1), ...document, 0, 0, 2, ...str('a'), ...leaf, ...str('ab'), ...leaf],
  [...header(1, 1, 0, 1), ...document, 0, 0, 1, ...str('a'), 99, ...leaf.slice(1)]
]
for (const bytes of malformed) {
  assert.throws(() => MiniSearchWasm.loadBytes(new Uint8Array(bytes)), error => error instanceof Error && !(error instanceof WebAssembly.RuntimeError))
  assertions++
}
for (let length = 0; length < valid.length; length++) {
  assert.throws(() => MiniSearchWasm.loadBytes(new Uint8Array(valid.slice(0, length))), error => error instanceof Error)
}
const validEngine = MiniSearchWasm.loadBytes(new Uint8Array(valid))
for (const [field, value] of [['document_count', 900], ['next_id', 0], ['field_present', [false]], ['average_field_length', [-1]], ['snapshot_version', 3]]) {
  const state = validEngine.toNativeJSON(); state[field] = value
  assert.throws(() => MiniSearchWasm.loadNativeJSON(JSON.stringify(state)), error => error instanceof Error)
}
for (let i = 0; i < valid.length; i++) {
  const bytes = new Uint8Array(valid); bytes[i] ^= 0x80
  let copy
  try { copy = MiniSearchWasm.loadBytes(bytes) } catch (error) { assert.ok(error instanceof Error); continue }
  copy.searchJoined('apple', false); copy.free()
}
equal(validEngine.search('apple')[0].id, 1, 'valid index remains usable after invalid loads')
validEngine.free()
console.log('ok malformed snapshots, overflow, recursion, truncation and mutation probes through WASM')
console.log(`HIGH PRIORITY REGRESSIONS: ALL PASS (${assertions} explicit equality/rejection checks)`)
