// The native core, driven directly through its generated glue: no input may
// trap (a trap aborts the module for every index in it), whatever was saved
// must load again, and a rejected snapshot must not balloon Wasm memory.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import Original from 'minisearch';
import { initSync, MiniSearchWasm as Core } from '../pkg/minisearch_wasm_core.js';

const wasm = initSync({ module: readFileSync(new URL('../pkg/minisearch_wasm_bg.wasm', import.meta.url)) });
let checks = 0;
const same = (actual, expected, label = '') => { assert.deepEqual(actual, expected, label); checks++; };
// A trap is an Error too: a refusal has to be a thrown MiniSearch error.
const refused = error => error instanceof Error && !(error instanceof WebAssembly.RuntimeError);
const refuses = (fn, label) => { assert.throws(fn, refused, label); checks++; };
const core = (settings, documents = []) => { const index = new Core({ autoVacuum: false, ...settings }); index.addAll(documents); return index; };

// 1. String parameters are checked, not read blindly.
{
  const index = core({ fields: ['text'] }, [{ id: 1, text: 'apple pie' }]);
  for (const value of [42, {}, [], Symbol('query'), null, undefined, true]) {
    const label = `argument ${String(value)}`;
    for (const call of [
      () => index.searchJoined(value, false), () => index.searchJoinedOpts(value, {}), () => index.searchRaw(value), () => index.autoSuggest(value),
      () => index.autoSuggestJoined(value), () => index.searchCountDefault(value, false), () => index.searchCountOpts(value, true, true),
      () => index.queryTerms(value), () => index.addAllJSON(value), () => Core.getDefault(value), () => Core.loadNativeJSON(value),
      () => Core.loadJSON(value, { fields: ['text'] }), () => Core.loadMiniSearchJSON(value, { fields: ['text'] }),
    ]) refuses(call, label);
    await assert.rejects(Core.loadJSONAsync(value, { fields: ['text'] }), refused); checks++;
  }
  same(JSON.parse(index.searchJoined('apple', false).ids), [1], 'usable after refused arguments');
  const bytes = index.toBytes();
  for (const input of [bytes, bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength), new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength), Buffer.from(bytes)]) {
    const copy = Core.loadBytes(input); same(copy.documentCount, 1, input.constructor.name); copy.free();
  }
  for (const input of ['bytes', 5, {}, null, undefined, [1, 2]]) refuses(() => Core.loadBytes(input), `loadBytes(${String(input)})`);
  index.free();
}

// 2. An absurd fuzzy distance neither traps nor poisons the instance or the module.
{
  const index = core({ fields: ['text'] }, [{ id: 1, text: 'apple' }, { id: 2, text: 'pear' }]), other = core({ fields: ['text'] }, [{ id: 9, text: 'plum' }]);
  const long = 'a'.repeat(70);
  for (const fuzzy of [6e7, 1e300, Number.MAX_VALUE]) {
    same(index.search(long, { fuzzy }).length, 2, `search fuzzy ${fuzzy}`);
    same(index.searchJoinedOpts(long, { fuzzy }).count, 2); same(index.searchRaw(long, { fuzzy }).count, 2); same(index.autoSuggest(long, { fuzzy }).length, 2);
  }
  same(index.search('b'.repeat(20000), { fuzzy: 0.2 }), [], 'a term too long for a fuzzy matrix');
  index.add({ id: 3, text: 'apple tart' }); same(index.documentCount, 3, 'the instance still takes mutations');
  same(JSON.parse(other.searchJoined('plum', false).ids), [9], 'other instances are unaffected');
  index.free(); other.free();
}

// 3. Whatever the engine saves, it loads again; what it cannot save, it refuses when saving.
{
  const roundTrips = (index, label) => {
    for (const copy of [Core.loadBytes(index.toBytes()), Core.loadNativeJSON(index.toNativeJSONString()), Core.loadNativeJSON(JSON.stringify(index.toNativeJSON()))]) {
      same([copy.documentCount, copy.termCount, copy.dirtCount], [index.documentCount, index.termCount, index.dirtCount], label); copy.free();
    }
  };
  // Negative field average: upstream produces [1, -6] for this history as well.
  const settings = { fields: ['title', 'text'] };
  const sparse = core(settings), upstream = new Original({ ...settings, autoVacuum: false });
  for (const index of [sparse, upstream]) {
    for (let id = 0; id < 8; id++) index.add({ id, title: 'only title' });
    index.add({ id: 8, title: 'both', text: 'apple pie' }); index.add({ id: 9, title: 'both', text: 'apple tart' });
    for (let id = 0; id < 8; id++) index.remove({ id, title: 'only title' });
    index.discard(9);
  }
  same(sparse.toJSON().averageFieldLength, JSON.parse(JSON.stringify(upstream)).averageFieldLength, 'negative average like upstream');
  assert.ok(sparse.toJSON().averageFieldLength[1] < 0);
  roundTrips(sparse, 'negative field average');
  const imported = Core.loadJSON(JSON.stringify(upstream), settings); same(imported.search('apple').map(row => row.id), [8]); imported.free(); sparse.free();

  // A document that changed before removal leaves postings outside the dirt count.
  const changed = core({ fields: ['title'] }, [{ id: 1, title: 'foo bar' }, { id: 2, title: 'bar baz' }]);
  changed.remove({ id: 1, title: 'foo' }); same(changed.dirtCount, 0);
  roundTrips(changed, 'leftover postings'); changed.compact(); roundTrips(changed, 'leftover postings, compacted');
  same(changed.search('bar').map(row => row.id), [2]); changed.free();

  // Nested prefixes: every reader accepts 128 levels, every writer refuses more.
  const nested = depth => core({ fields: ['text'] }, [{ id: 1, text: Array.from({ length: depth }, (_, i) => 'a'.repeat(i + 1)).join(' ') }]);
  const deep = nested(128); roundTrips(deep, 'radix depth 128'); deep.free();
  const tooDeep = nested(129);
  for (const save of [() => tooDeep.toBytes(), () => tooDeep.toNativeJSONString(), () => tooDeep.toNativeJSON()]) assert.throws(save, /128 prefixes deep/);
  same(tooDeep.search('aaa').length, 1); tooDeep.free();

  refuses(() => new Core({ fields: ['title', 'title'] }), 'duplicate fields');
  refuses(() => new Core({ fields: ['title'], searchOptions: { boost: { title: Infinity } } }), 'nonfinite option');
}

// 4. A rejected snapshot cannot reserve more than the decode budget.
{
  const valid = core({ fields: ['text'] }, [{ id: 1, text: 'apple' }]).toBytes();
  const idAt = valid.findIndex((byte, i) => byte === 3 && valid[i + 1] === 1); // TAG_UINT 1: the document id
  const crafted = [...valid.subarray(0, idAt)], padding = 1 << 20;
  for (let level = 0; level < 60; level++) {
    crafted.push(7); // TAG_ARRAY
    for (let count = padding - level * 4; ; count = Math.floor(count / 128)) { crafted.push(count < 128 ? count : (count % 128) | 128); if (count < 128) break; }
  }
  const before = wasm.memory.buffer.byteLength;
  assert.throws(() => Core.loadBytes(new Uint8Array([...crafted, ...new Uint8Array(padding)])), /budget/);
  const grown = wasm.memory.buffer.byteLength - before;
  assert.ok(grown <= 320 * 1024 * 1024, `Wasm memory grew by ${grown} bytes`); checks++;
}

// 5. Query semantics fixed in the engine.
{
  const index = core({ fields: ['text'], storeFields: ['rating'] });
  index.addAllJSON('[{"id":1.0,"text":"foo","rating":4.0},{"id":2,"text":"foo","rating":1E2}]');
  same(index.search('foo', { filter: { rating: 4 } }).map(row => row.id), [1], 'filter 4 matches 4.0');
  same(index.search('foo', { filter: { rating: 100 } }).map(row => row.id), [2], 'filter 100 matches 1E2');
  same(index.search('foo', { filter: { id: 1 } }).length, 1); index.free();

  const repeated = core({ fields: ['text'] }, [[1, 'foobar bar'], [2, 'foo bar'], [3, 'foobar'], [4, 'bar baz']].map(([id, text]) => ({ id, text })));
  const options = { prefix: [false, false, true] }, full = repeated.search('foo bar foo', options);
  same(Array.from(repeated.searchJoinedOpts('foo bar foo', options).scores), full.map(row => row.score), 'joined scores of a repeated term');
  same(Array.from(repeated.searchRaw('foo bar foo', options).scores), full.map(row => row.score), 'raw scores of a repeated term');
  const upstream = new Original({ fields: ['text'] }); upstream.addAll([[1, 'foobar bar'], [2, 'foo bar'], [3, 'foobar'], [4, 'bar baz']].map(([id, text]) => ({ id, text })));
  const expected = upstream.autoSuggest('foo bar foo', { combineWith: 'OR' }), actual = repeated.autoSuggest('foo bar foo', { combineWith: 'OR' });
  same(actual.map(row => row.suggestion), expected.map(row => row.suggestion));
  same(actual.map(row => row.score), expected.map(row => row.score), 'suggestion scores, exactly');
  repeated.free();
}

// 6. The core's own scheduler: a vacuum queued by auto-vacuum rechecks its conditions.
{
  const settings = { fields: ['text'], autoVacuum: { minDirtCount: 2, minDirtFactor: 0.0001, batchSize: 1, batchWait: 2 } };
  const docs = Array.from({ length: 6 }, (_, id) => ({ id, text: `doc number${id}` }));
  const trace = async C => {
    const index = new C(settings); index.addAll(docs);
    for (const id of [0, 1, 2]) index.discard(id);
    while (index.isVacuuming) await new Promise(resolve => setTimeout(resolve, 5));
    return [index.dirtCount, index.termCount];
  };
  same(await trace(Core), await trace(Original), 'queued auto-vacuum is conditional');
}

console.log(`CORE ROBUSTNESS: ALL PASS (${checks} checks)`);
