// Public-package contracts: these tests intentionally do NOT warm a dirty query
// before comparing it with the oracle. Tests of the lower-level Rust subset live
// in the existing suites, using the generated core glue explicitly.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import Original from 'minisearch';
import MiniSearch, { MiniSearchWasm as Wasm } from '../pkg/minisearch_wasm_node.js';
import { MiniSearchWasm as Core } from '../pkg/minisearch_wasm_core.js';

const rows = result => result.map(row => ({ ...row, score: Math.round(row.score * 1e10) / 1e10 }));
const pair = (options, documents) => {
  const js = new Original(options), wasm = new Wasm(options);
  js.addAll(documents); wasm.addAll(documents); return { js, wasm };
};
const options = { fields: ['text'], storeFields: ['category'], autoVacuum: false };
const documents = [{ id: 1, text: 'apple pear', category: 'fruit' }, { id: 2, text: 'apple pie', category: 'dessert' }, { id: 3, text: 'apply here', category: 'jobs' }];
let checks = 0;
const eq = (actual, expected) => { assert.deepEqual(actual, expected); checks++; };

// Both old initializer syntax and the original default constructor are usable.
await MiniSearch();
const defaultInstance = new MiniSearch(options);
eq(defaultInstance.executionMode, 'wasm'); defaultInstance.free();

// Index callbacks, original callback argument lists, token expansion and nested
// extraction. Stored Dates and objects retain their original JS identity.
{
  const date = new Date(0), metadata = { tag: 'original' };
  const settings = {
    fields: ['title', 'year'], storeFields: ['date', 'metadata'],
    extractField: (doc, field) => field === 'title' ? doc.nested.title : field === 'year' ? doc.date.getUTCFullYear() : doc[field],
    stringifyField: value => String(value),
    tokenize: (text, field) => field === 'title' ? text.split('|') : text.split(' '),
    processTerm: term => term === 'stop' ? false : [term.toLowerCase(), term.toUpperCase()], autoVacuum: false,
  };
  const docs = [{ id: 1, nested: { title: 'APPLE|stop' }, date, metadata }];
  const { js, wasm } = pair(settings, docs);
  eq(wasm.executionMode, 'javascript');
  eq(rows(wasm.search('apple')), rows(js.search('apple')));
  assert.equal(wasm.getStoredFields(1).date, date);
  assert.equal(wasm.getStoredFields(1).metadata, metadata);
  wasm.getStoredFields(1).metadata.tag = 'changed';
  eq(wasm.search('apple')[0].metadata.tag, 'changed');
  for (const loader of [() => Wasm.loadBytes(wasm.toBytes(), settings), () => Wasm.loadNativeJSON(wasm.toNativeJSONString(), settings)]) {
    const copy = loader(); eq(copy.search('apple').map(row => row.id), [1]); copy.free();
  }
  assert.throws(() => Wasm.loadBytes(wasm.toBytes()), /requires callback/);
  wasm.free();
}

// Each search callback can trigger a one-time transfer from a populated Wasm
// index. Nested query callbacks, default callbacks and suggestions are covered.
for (const search of [
  { filter: result => result.category === 'fruit' },
  { boostDocument: (id, term, stored) => stored.category === 'fruit' && term.startsWith('app') ? 4 : 0.5 },
  { boostTerm: (term, i, terms) => terms.length + i + term.length },
  { prefix: (term, i, terms) => i === terms.length - 1 },
  { fuzzy: (term, i) => i === 0 ? 0.4 : false },
  { tokenize: text => text.split('|') },
  { processTerm: term => [term, term + 'le'] },
]) {
  const { js, wasm } = pair(options, documents);
  eq(wasm.executionMode, 'wasm');
  for (const query of ['app pear', 'appel', 'apple|pie']) {
    eq(rows(wasm.search(query, search)), rows(js.search(query, search)));
    eq(rows(wasm.autoSuggest(query, search)), rows(js.autoSuggest(query, search)));
  }
  eq(wasm.executionMode, 'javascript');
  const tree = { combineWith: 'OR', queries: ['pear', { queries: ['app'], ...search }] };
  eq(rows(wasm.search(tree)), rows(js.search(tree)));
  wasm.free();
  const configured = pair({ ...options, searchOptions: search }, documents);
  eq(rows(configured.wasm.search('app pear')), rows(configured.js.search('app pear')));
  configured.wasm.free();
}

// Default functions can be reused as constructor options.
for (const key of ['extractField', 'stringifyField', 'tokenize', 'processTerm', 'logger']) {
  const settings = { ...options, [key]: Wasm.getDefault(key) };
  const { js, wasm } = pair(settings, documents);
  eq(rows(wasm.search('apple')), rows(js.search('apple'))); wasm.free();
}

// Dirty query 1 and query 2, repeated postings, prefix expansion, tree order,
// wildcard and the observable lazy termCount mutation.
for (const query of ['apple', 'app', { combineWith: 'OR', queries: ['apple', 'app'] }]) {
  const { js, wasm } = pair(options, [{ id: 1, text: 'apple' }, { id: 2, text: 'apple apple' }, { id: 3, text: 'apply' }]);
  js.discard(2); wasm.discard(2);
  for (let i = 0; i < 4; i++) eq(rows(wasm.search(query, { prefix: true })), rows(js.search(query, { prefix: true })));
  eq(wasm.termCount, js.termCount); wasm.free();
}
{
  const { js, wasm } = pair(options, [{ id: 1, text: 'apple' }, { id: 2, text: 'pear' }]);
  js.discard(1); wasm.discard(1); js.search('apple'); wasm.search('apple'); eq(wasm.termCount, js.termCount); wasm.free();
}

// Object identity, primitive special values and live stored-field references.
{
  const a = { key: 1 }, b = { key: 1 }, date = new Date(0);
  const { js, wasm } = pair({ ...options, storeFields: ['text'] }, [{ id: a, text: date }, { id: b, text: Infinity }]);
  eq(wasm.documentCount, 2);
  assert.equal(wasm.search('infinity')[0].id, b);
  assert.equal(wasm.getStoredFields(a).text, date);
  wasm.getStoredFields(a).text = 'edited'; js.getStoredFields(a).text = 'edited';
  eq(rows(wasm.search(Wasm.wildcard)), rows(js.search(Original.wildcard)));
  assert.equal(wasm.has({ key: 1 }), false); wasm.free();
}

// Missing non-JSON IDs do not require a transfer; stored fields are live refs.
{
  const { js, wasm } = pair(options, documents);
  for (const id of [NaN, Infinity, -Infinity, Symbol('missing'), 1n, {}, null, undefined]) eq(wasm.has(id), js.has(id));
  eq(wasm.executionMode, 'wasm');
  const stored = wasm.getStoredFields(1);
  assert.equal(stored, wasm.getStoredFields(1));
  stored.category = 'changed'; js.getStoredFields(1).category = 'changed';
  eq(rows(wasm.search('apple')), rows(js.search('apple'))); wasm.free();
  assert.throws(() => Wasm.loadJSON('{}'), /same options/);
  await assert.rejects(Wasm.loadJSONAsync('{}'), /same options/);
}

// A mixed mutation history compares each first query, without oracle warm-up.
{
  const { js, wasm } = pair(options, documents);
  const live = new Map(documents.map(doc => [doc.id, doc]));
  for (let i = 0; i < 60; i++) {
    const doc = { id: i + 10, text: ['apple pear', 'apply pie', 'pear pear'][i % 3], category: 'new' };
    js.add(doc); wasm.add(doc); live.set(doc.id, doc);
    const first = live.values().next().value;
    if (i % 3 === 0) { js.discard(first.id); wasm.discard(first.id); live.delete(first.id); }
    else if (i % 3 === 1) { js.remove(first); wasm.remove(first); live.delete(first.id); }
    for (const query of ['app', 'pear', { combineWith: 'And', queries: ['app', 'pear'] }]) {
      eq(rows(wasm.search(query, { prefix: true })), rows(js.search(query, { prefix: true })));
    }
    if (i % 10 === 9) { await js.vacuum(); await wasm.vacuum(); }
  }
  wasm.free();
}

// The optional jobboard tokenizer keeps its behavior after a callback transfer.
{
  const index = new Wasm(options);
  index.addAll(documents);
  const search = { prefix: true, fuzzy: 0.3, weights: { prefix: 0.2 }, bm25: { b: 0.4 } };
  const before = rows(index.search('app', search));
  eq(rows(index.search('app', { ...search, filter: () => true })), before);
  index.free();
}

{
  const index = new Wasm({ ...options, tokenizer: 'jobboard' });
  index.addAll([' C++ foo. ', 'a\u0301 \u0345 apple', 'C# ¼ dotted.name'].map((text, id) => ({ id, text })));
  const queries = ['C++', 'a\u0301', '\u0345', '¼', 'foo', 'dotted.name'];
  const before = queries.map(query => rows(index.search(query)));
  queries.forEach((query, i) => eq(rows(index.search(query, { filter: () => true })), before[i]));
  index.free();
}

// Transfer keeps compressed radix edge and terminal ordering, including ties.
{
  const docs = ['abcde', 'abc', 'ab', 'abcd', 'abcf', 'abce', '2024', '2023'].map((text, id) => ({ id, text }));
  const { js, wasm } = pair(options, docs);
  const query = 'ab';
  eq(rows(wasm.search(query, { prefix: true, filter: () => true })), rows(js.search(query, { prefix: true })));
  for (const load of [() => Wasm.loadBytes(wasm.toBytes()), () => Wasm.loadNativeJSON(wasm.toNativeJSONString())]) {
    const copy = load(); eq(rows(copy.search(query, { prefix: true })), rows(wasm.search(query, { prefix: true }))); copy.free();
  }
  wasm.free();
}

// Actual timer yields, including lazy document getters: no full-array conversion
// is permitted before addAllAsync returns. Exercise the native boundary too.
for (const C of [Wasm, Core]) {
  let reads = 0;
  const docs = Array.from({ length: 50 }, (_, id) => ({ id, get text() { reads++; return 'apple'; } }));
  const index = new C(options);
  const pending = index.addAllAsync(docs, { chunkSize: 10 });
  eq(reads, 0); await pending; eq(index.documentCount, 50); index.free();
}
{
  const js = new Original(options);
  js.addAll(Array.from({ length: 2500 }, (_, id) => ({ id, text: 'word' + id })));
  let tick = false; setTimeout(() => { tick = true; }, 0);
  const copy = await Wasm.loadJSONAsync(JSON.stringify(js), options);
  eq(tick, true); eq(copy.documentCount, 2500); copy.free();
  await assert.rejects(Wasm.loadJSONAsync('{', options), SyntaxError);
}

// Original active vs queued Promise identity; compaction happens only once
// maintenance is clean and finished, and invalidates raw ID-table generations.
{
  const index = new Wasm(options);
  for (let cycle = 0; cycle < 10; cycle++) {
    index.addAll(Array.from({ length: 1000 }, (_, id) => ({ id, text: 'apple pear' })));
    index.discardAll(Array.from({ length: 1000 }, (_, id) => id));
    const first = index.vacuum({ batchSize: 1, batchWait: 1 });
    const queued = index.vacuum({ batchSize: 1, batchWait: 1 });
    assert.notEqual(first, queued); assert.equal(queued, index.vacuum());
    await queued;
  }
  index.add({ id: 'live', text: 'apple' });
  eq(JSON.parse(index.docIdTable()), ['live']);
  const generation = index.idTableVersion;
  index.remove({ id: 'live', text: 'apple' }); index.compact();
  assert.notEqual(index.idTableVersion, generation); eq(JSON.parse(index.docIdTable()), []); index.free();
}

// Normal JSON documents stay in Wasm; persisted auto-vacuum settings survive.
// A mismatched remove can leave untracked stale postings. Compaction must
// reject it before changing mappings (never a native trap or undefined JS IDs).
for (const javascript of [false, true]) {
  const index = new Wasm(options);
  index.add({ id: 1, text: 'apple' });
  if (javascript) index.search('apple', { filter: () => true });
  index.remove({ id: 1, text: '' });
  assert.throws(() => index.compact(), /stale postings/);
  index.free();
}

{
  const legacy = new Core(options);
  legacy.add({ id: { key: 1 }, text: 'apple' });
  for (const copy of [Wasm.loadBytes(legacy.toBytes()), Wasm.loadNativeJSON(legacy.toNativeJSONString())]) {
    eq(copy.executionMode, 'javascript');
    const id = copy.search('apple')[0].id;
    assert.equal(copy.search('apple')[0].id, id);
    assert.equal(copy.has(id), true); assert.equal(copy.has({ key: 1 }), false);
    copy.free();
  }
  legacy.free();
}

{
  const index = new Wasm({ ...options, autoVacuum: { minDirtCount: 2, minDirtFactor: 0.01 } });
  index.addAll(documents); eq(index.executionMode, 'wasm');
  const copy = Wasm.loadBytes(index.toBytes());
  eq(copy.executionMode, 'wasm');
  copy.discardAll([1, 2]);
  await copy.vacuum(); eq(copy.dirtCount, 0);
  eq(JSON.parse(copy.docIdTable()), [3]);
  index.free(); copy.free();
}

// Restrict code generation in a separate process, including the actual Wasm
// path and getDefault's native helper. The test does not rely on a JS fallback.
const csp = spawnSync(process.execPath, ['--disallow-code-generation-from-strings', new URL('./csp_regression.mjs', import.meta.url).pathname.replace(/^\/(\w:)/, '$1')], { encoding: 'utf8' });
assert.equal(csp.status, 0, csp.stdout + csp.stderr);
console.log(`PUBLIC API: ALL PASS (${checks} equality checks plus callback, identity, timing and CSP assertions)`);
