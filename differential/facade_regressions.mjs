// Regressions from the review of every commit since 0.7. Test observable
// callback results and scheduling against MiniSearch, including native residency.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import Original from 'minisearch';
import MiniSearch from '../pkg/minisearch_wasm_node.js';
import { MiniSearchWasm as Core } from '../pkg/minisearch_wasm_core.js';

const options = { fields: ['text'], autoVacuum: false };
const documents = [{ id: 1, text: 'apple pear pear pear pear pear' }, { id: 2, text: 'apple' }];

await test('native filters observe traversal order and sort mutated scores afterwards', () => {
  for (const dirty of [false, true]) for (const kind of ['first', 'mutate', 'ties']) {
    for (const query of ['apple', { queries: ['apple', 'pear'], combineWith: 'OR' }, 'wildcard']) {
      const run = C => {
        const index = new C(options); index.addAll(documents);
        if (dirty) { index.add({ id: 3, text: 'apple' }); index.discard(3); }
        const seen = [];
        const rows = index.search(query === 'wildcard' ? C.wildcard : query, { filter: row => {
          assert.ok(row.match); seen.push(row.id);
          if (kind === 'first') return seen.length === 1;
          row.score = kind === 'ties' ? 1 : row.id === 1 ? 100 : 0;
          return true;
        } });
        if (C === MiniSearch) { assert.equal(index.executionMode, 'wasm'); index.free(); }
        return { seen, rows };
      };
      assert.deepEqual(run(MiniSearch), run(Original), `${kind}, dirty=${dirty}, query=${String(query)}`);
    }
  }
  // Constructor defaults and includeMatch:false must still give the callback
  // a full row, then omit the match data only from the returned results.
  const run = C => {
    let calls = 0;
    const index = new C({ ...options, searchOptions: { filter: row => {
      assert.ok(row.match); return ++calls === 1;
    } } });
    index.addAll(documents);
    const rows = index.search('apple', { includeMatch: false });
    if (C === Original) for (const row of rows) delete row.match;
    if (C === MiniSearch) { assert.equal(index.executionMode, 'wasm'); index.free(); }
    return rows;
  };
  assert.deepEqual(run(MiniSearch), run(Original));
});

await test('compact queries evaluate shadowed-field callbacks and dirty cleanup once', () => {
  for (const method of ['searchJoinedOpts', 'searchRaw']) {
    for (const shadow of ['score', 'terms', 'match']) for (const dirty of [false, true]) {
      const settings = { ...options, storeFields: [shadow] };
      const docs = documents.map(doc => ({ ...doc, [shadow]: shadow === 'score' ? doc.id * 10 : `stored-${doc.id}` }));
      const original = new Original(settings), index = new MiniSearch(settings);
      original.addAll(docs); index.addAll(docs);
      if (dirty) for (const target of [original, index]) { target.add({ id: 3, text: 'apple' }); target.discard(3); }
      let expectedCalls = 0, actualCalls = 0;
      const expected = original.search('apple', { filter: () => ++expectedCalls === 1 });
      const actual = index[method]('apple', { filter: () => ++actualCalls === 1 });
      const table = JSON.parse(index.docIdTable());
      const ids = method === 'searchRaw' ? Array.from(actual.docIds, id => table[id]) : JSON.parse(actual.ids);
      assert.deepEqual(ids, expected.map(row => row.id));
      assert.equal(actualCalls, expectedCalls);
      assert.deepEqual(index.search('apple'), original.search('apple'), 'same lazy-cleanup state after one query');
      index.free();
    }
    // A native filter also must not cause speculative per-term evaluation.
    const index = new MiniSearch(options); index.addAll(documents);
    let calls = 0;
    const actual = index[method]('apple', { prefix: () => { calls++; return true; }, filter: () => true });
    assert.equal(actual.count, 2); assert.equal(calls, 1);
    assert.equal(index.executionMode, 'wasm'); index.free();
  }
});

await test('async indexing preserves original microtask and timer scheduling', async () => {
  for (const count of [0, 1, 9, 10, 11, 21]) for (const chunkSize of [undefined, 3]) {
    const run = async C => {
      const reads = [];
      const index = new C({ ...options, extractField: (doc, field) => { reads.push([doc.id, field]); return doc[field]; } });
      const docs = Array.from({ length: count }, (_, id) => ({ id, text: 'apple' }));
      const promise = index.addAllAsync(docs, chunkSize === undefined ? undefined : { chunkSize });
      const snapshots = [[index.documentCount, reads.length]];
      for (let i = 0; i < 4; i++) { await Promise.resolve(); snapshots.push([index.documentCount, reads.length]); }
      await new Promise(resolve => setTimeout(resolve, 0));
      snapshots.push([index.documentCount, reads.length]);
      await promise;
      const rows = index.search('apple');
      if (C === MiniSearch) { assert.equal(index.executionMode, 'wasm'); index.free(); }
      return { snapshots, reads, rows };
    };
    assert.deepEqual(await run(MiniSearch), await run(Original), `count=${count}, chunkSize=${chunkSize}`);
  }
});

await test('async JSON reconstruction yields repeatedly and remains native', async () => {
  for (const vocabulary of ['common', 'distinct', 'stale-common']) for (const version of [1, 2]) {
    const settings = { ...options, storeFields: ['category'] };
    const source = new Original(settings);
    source.addAll(Array.from({ length: 3500 }, (_, id) => ({ id, text: vocabulary === 'distinct' ? `apple term${id}` : 'apple', category: id % 2 })));
    // Only two live documents and one term: timer yields must happen within
    // its stale posting list, not just while rebuilding document maps.
    source.discardAll(vocabulary === 'stale-common' ? Array.from({ length: 3498 }, (_, id) => id + 2) : [2, 900, 2400]);
    const serialized = source.toJSON();
    if (version === 1) {
      serialized.serializationVersion = 1;
      for (const [, data] of serialized.index) for (const field of Object.keys(data)) data[field] = { ds: data[field] };
    }
    const json = JSON.stringify(serialized);
    let ticks = 0;
    const interval = setInterval(() => ticks++, 0);
    let index;
    try { index = await MiniSearch.loadJSONAsync(json, settings); } finally { clearInterval(interval); }
    assert.ok(ticks >= 3, `${vocabulary}, version ${version}: ${ticks} timer ticks`);
    assert.equal(index.executionMode, 'wasm');
    const original = await Original.loadJSONAsync(json, settings);
    for (const query of ['apple', 'term', 'missing']) {
      assert.deepEqual(index.search(query, { prefix: true }), original.search(query, { prefix: true }));
    }
    assert.deepEqual([index.documentCount, index.dirtCount], [original.documentCount, original.dirtCount]);
    index.free();
  }
  // Malformed input still rejects; errors after several batches never expose
  // a partial index or damage another instance in the same Wasm module.
  await assert.rejects(MiniSearch.loadJSONAsync('{', options), SyntaxError);
  await assert.rejects(MiniSearch.loadJSONAsync('{"serializationVersion":99}', options), /incompatible version/);
  const source = new Original(options);
  source.addAll(Array.from({ length: 3500 }, (_, id) => ({ id, text: 'apple' })));
  const bad = source.toJSON(); bad.index[0][1][0][0] = 0;
  await assert.rejects(Core.loadJSONAsync(JSON.stringify(bad), options), /invalid term frequency/);
  const good = await Core.loadJSONAsync(JSON.stringify(source), options);
  assert.equal(good.search('apple').length, 3500); good.free();
  const objectIndex = new Original(options); objectIndex.add({ id: { key: 1 }, text: 'apple' });
  const fallback = await MiniSearch.loadJSONAsync(JSON.stringify(objectIndex), options);
  assert.equal(fallback.executionMode, 'javascript');
  assert.deepEqual(fallback.search('apple'), objectIndex.search('apple')); fallback.free();
});
