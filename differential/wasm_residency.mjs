// An index with plain documents should stay on the Wasm engine through
// everyday use: discards and replaces, dirty queries, vacuum, search callbacks
// over terms and rows, getStoredFields and MiniSearch JSON loading. Every block
// compares against pinned MiniSearch without warming it up and asserts the
// execution mode, so a silent fall back to JavaScript fails the suite.
import assert from 'node:assert/strict';
import Original from 'minisearch';
import MiniSearch from '../pkg/minisearch_wasm_node.js';

let checks = 0;
const same = (actual, expected, label = '') => { assert.deepEqual(actual, expected, label); checks++; };
// Scores are compared exactly: the engine computes the bits V8 computes.
const close = (actual, expected) => Object.is(actual, expected);
// Rows must agree in order, ids, terms, queryTerms, match keys (and their
// order), stored fields and score.
function sameRows(actual, expected, label) {
  assert.equal(actual.length, expected.length, `${label}: row count`);
  actual.forEach((row, i) => {
    const { score, ...rest } = row, { score: expectedScore, ...expectedRest } = expected[i];
    assert.deepEqual(rest, expectedRest, `${label}: row ${i}`);
    if (rest.match) assert.deepEqual(Object.keys(rest.match), Object.keys(expectedRest.match), `${label}: match order ${i}`);
    assert.ok(close(score, expectedScore), `${label}: score ${i} ${score} vs ${expectedScore}`);
  });
  checks++;
}
const wasmMode = (index, label) => { assert.equal(index.executionMode, 'wasm', label); checks++; };

let seed = 20260919;
const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
const pick = list => list[Math.floor(random() * list.length)];
const syllables = ['ap', 'ple', 'pe', 'ar', 'pi', 'e', 'ta', 'rt', 'a', 'b', 'ab', 'abc', '20', '24', 'x'];
const word = () => Array.from({ length: 1 + Math.floor(random() * 3) }, () => pick(syllables)).join('');
const text = () => Array.from({ length: 1 + Math.floor(random() * 6) }, word).join(' ');

// 1. A seeded mutation and query history. MiniSearch cannot be mutated while it
// vacuums (its open issue 306), so vacuums are awaited before the next step.
{
  const settings = { fields: ['title', 'text'], storeFields: ['category', 'rank'], autoVacuum: false,
    searchOptions: { boost: { title: 2 } } };
  const js = new Original(settings), wasm = new MiniSearch(settings);
  const live = new Map();
  let nextId = 0;
  const document = id => ({ id, title: text(), text: random() < 0.15 ? undefined : text(), category: pick(['a', 'b', 'c']), rank: Math.floor(random() * 5) });
  const both = fn => { fn(js); fn(wasm); };
  const callbacks = {
    prefix: term => term.length > 2,
    fuzzy: (term, i) => i % 2 ? 0.3 : term.length > 3,
    boostTerm: (_term, i, terms) => terms.length - i,
    filter: row => row.category !== 'b' && row.rank < 4,
  };
  const searchOptions = () => {
    const options = {};
    for (const key of Object.keys(callbacks)) if (random() < 0.4) options[key] = callbacks[key];
    if (!('prefix' in options) && random() < 0.5) options.prefix = random() < 0.7;
    if (!('fuzzy' in options) && random() < 0.4) options.fuzzy = pick([true, 0.2, 1, 2]);
    if (random() < 0.3) options.combineWith = pick(['AND', 'OR', 'AND_NOT']);
    if (random() < 0.2) options.fields = [pick(['title', 'text'])];
    return options;
  };
  const query = () => {
    const kind = random();
    if (kind < 0.6) return Array.from({ length: 1 + Math.floor(random() * 3) }, word).join(' ');
    if (kind < 0.7) return [Original.wildcard, MiniSearch.wildcard];
    const tree = wildcard => ({ combineWith: pick(['AND', 'OR', 'AND_NOT']),
      queries: [word(), { queries: [word() + ' ' + word(), wildcard].slice(0, random() < 0.3 ? 2 : 1), ...searchOptions() }, word()] });
    const state = seed, a = tree(Original.wildcard);
    seed = state;
    return [a, tree(MiniSearch.wildcard)];
  };

  for (let step = 0; step < 900; step++) {
    const action = random();
    if (action < 0.35 || live.size < 5) {
      const doc = document(nextId++); live.set(doc.id, doc); both(index => index.add(doc));
    } else if (action < 0.5) {
      const doc = pick([...live.values()]); live.delete(doc.id); both(index => index.discard(doc.id));
    } else if (action < 0.6) {
      const doc = pick([...live.values()]); live.delete(doc.id); both(index => index.remove(doc));
    } else if (action < 0.75) {
      const doc = document(pick([...live.keys()])); live.set(doc.id, doc); both(index => index.replace(doc));
    } else if (action < 0.8) {
      const ids = [...live.keys()].filter(() => random() < 0.2); ids.forEach(id => live.delete(id)); both(index => index.discardAll(ids));
    } else if (action < 0.85) {
      const options = pick([undefined, { batchSize: 3, batchWait: 1 }]);
      await Promise.all([js.vacuum(options), wasm.vacuum(options)]);
    }
    for (let i = 0; i < 2; i++) {
      const pairOf = query(), [jsQuery, wasmQuery] = Array.isArray(pairOf) ? pairOf : [pairOf, pairOf];
      const options = searchOptions(), label = `step ${step} ${JSON.stringify(wasmQuery, (_key, value) => typeof value === 'symbol' ? '*' : value)} ${Object.keys(options)}`;
      sameRows(wasm.search(wasmQuery, options), js.search(jsQuery, options), label);
      if (random() < 0.4) sameRows(wasm.autoSuggest(wasmQuery, options), js.autoSuggest(jsQuery, options), `suggest ${label}`);
      if (typeof wasmQuery === 'string' && random() < 0.3) {
        // Compact forms against the original's rows. A dirty query changes the
        // index on both sides, so each native call is paired with one original call.
        const forJoined = js.search(jsQuery, options), joined = wasm.searchJoinedOpts(wasmQuery, options);
        same(JSON.parse(joined.ids), forJoined.map(row => row.id), `joined ids ${label}`);
        same(joined.count ? joined.terms.split('\n') : [], forJoined.map(row => row.terms.join(' ')), `joined terms ${label}`);
        const forRaw = js.search(jsQuery, options), raw = wasm.searchRaw(wasmQuery, options), table = JSON.parse(wasm.docIdTable());
        same(Array.from(raw.docIds, id => table[id]), forRaw.map(row => row.id), `raw ids ${label}`);
        assert.ok(forJoined.every((row, k) => close(joined.scores[k], row.score)) && forRaw.every((row, k) => close(raw.scores[k], row.score)), `compact scores ${label}`);
      }
    }
    same([wasm.documentCount, wasm.termCount, wasm.dirtCount, wasm.dirtFactor, wasm.isVacuuming],
      [js.documentCount, js.termCount, js.dirtCount, js.dirtFactor, js.isVacuuming], `counts at step ${step}`);
  }
  wasmMode(wasm, 'after the mutation history');
  // The facade renumbers internal ids after a clean vacuum; everything else in
  // the serialized index is the original's, radix term order included.
  const canonical = index => {
    const { documentIds, fieldLength, storedFields, index: terms, nextId: _nextId, ...rest } = JSON.parse(JSON.stringify(index));
    const external = table => Object.fromEntries(Object.entries(table).map(([id, value]) => [JSON.stringify(documentIds[id]), value]));
    return { ...rest, fieldLength: external(fieldLength), storedFields: external(storedFields),
      terms: terms.map(([term, fields]) => [term, Object.fromEntries(Object.entries(fields).map(([field, postings]) => [field, external(postings)]))]) };
  };
  same(canonical(wasm), canonical(js), 'serialized index after the history');
  wasm.free();
}

// 2. Per-term callbacks see the original argument lists, in the original order.
{
  const settings = { fields: ['text'], autoVacuum: false };
  const docs = [{ id: 1, text: 'Apple, PEAR and apple-pie' }, { id: 2, text: 'pear' }];
  const calls = { js: [], wasm: [] };
  const recorder = (log, name, result) => (term, i, terms) => { log.push([name, term, i, [...terms]]); return result(term, i); };
  const optionsFor = log => ({ fuzzy: recorder(log, 'fuzzy', () => 0.2), prefix: recorder(log, 'prefix', (_term, i) => i === 1), boostTerm: recorder(log, 'boostTerm', (_term, i) => i + 1) });
  const js = new Original(settings), wasm = new MiniSearch(settings);
  js.addAll(docs); wasm.addAll(docs);
  const tree = options => ({ combineWith: 'AND', queries: ['Apple  PEAR!', { queries: ['app-le pi'], ...options }] });
  sameRows(wasm.search(tree(optionsFor(calls.wasm)), optionsFor(calls.wasm)), js.search(tree(optionsFor(calls.js)), optionsFor(calls.js)), 'recorded callbacks');
  same(calls.wasm, calls.js, 'callback arguments'); assert.ok(calls.js.length > 0);
  wasmMode(wasm, 'after per-term callbacks');
  // Values the engine has no per-term form for fall back to JavaScript.
  sameRows(wasm.search('apple pear', { boostTerm: () => undefined }).map(row => ({ ...row, score: 0 })), js.search('apple pear', { boostTerm: () => undefined }).map(row => ({ ...row, score: 0 })), 'NaN boosts');
  same(wasm.executionMode, 'javascript');
  wasm.free();
}

// 3. Auto-vacuum with default settings, searched and mutated while it runs.
// MiniSearch's own vacuum fails here (issue 306); the native one must finish.
{
  const docs = Array.from({ length: 3000 }, (_, id) => ({ id, text: `w${id} x${id} y${id} common` }));
  const index = new MiniSearch({ fields: ['text'] });
  index.addAll(docs);
  index.discardAll(docs.slice(0, 1500).map(doc => doc.id));
  same(index.isVacuuming, true, 'auto-vacuum started by discardAll');
  await new Promise(resolve => setTimeout(resolve, 5));
  for (const prefix of ['w', 'x', 'y']) index.search(prefix, { prefix: true });
  index.replace({ id: 2000, text: 'replaced common' }); index.remove(docs[2001]); index.add({ id: 'new', text: 'common new' });
  const queued = index.vacuum();
  assert.equal(queued, index.vacuum(), 'one queued Promise');
  await queued;
  same([index.isVacuuming, index.dirtCount], [false, 0], 'vacuum finished under load');
  const expected = new Original({ fields: ['text'] });
  expected.addAll([...docs.slice(1500).filter(doc => doc.id !== 2000 && doc.id !== 2001), { id: 2000, text: 'replaced common' }, { id: 'new', text: 'common new' }]);
  same(index.termCount, expected.termCount, 'terms after vacuum');
  const byId = rows => rows.map(row => [row.id, row.score]).sort((a, b) => String(a[0]).localeCompare(String(b[0])));
  const [actual, wanted] = [byId(index.search('common new w2999')), byId(expected.search('common new w2999'))];
  same(actual.map(row => row[0]), wanted.map(row => row[0]), 'ids after vacuum');
  assert.ok(actual.every((row, i) => close(row[1], wanted[i][1])));
  wasmMode(index, 'after auto-vacuum under load');
  // A vacuum of a clean index is a no-op that stays in Wasm.
  await index.vacuum(); wasmMode(index, 'after vacuuming a clean index');
  index.free();
}

// 4. The original's vacuum scheduling, observed at the same points.
{
  const settings = { fields: ['text'], autoVacuum: { minDirtCount: 2, minDirtFactor: 0.0001, batchSize: 1, batchWait: 2 } };
  const docs = Array.from({ length: 6 }, (_, id) => ({ id, text: `doc number${id}` }));
  const trace = async C => {
    const index = new C(settings), seen = [];
    index.addAll(docs);
    for (const id of [0, 1, 2]) { index.discard(id); seen.push([index.isVacuuming, index.dirtCount]); }
    const queued = index.vacuum();
    seen.push(queued === index.vacuum());
    await queued;
    seen.push([index.isVacuuming, index.dirtCount, index.termCount]);
    index.discard(3); seen.push([index.isVacuuming, index.dirtCount]);
    return [seen, index];
  };
  const [expected] = await trace(Original), [actual, index] = await trace(MiniSearch);
  same(actual, expected, 'vacuum scheduling trace'); wasmMode(index, 'after scheduled vacuums'); index.free();
}

// 5. getStoredFields without a transfer.
{
  const settings = { fields: ['text'], storeFields: ['category'], autoVacuum: false };
  const docs = [{ id: 1, text: 'divina commedia' }, { id: 2, text: 'vita nova', category: 'poetry' }];
  const js = new Original(settings), wasm = new MiniSearch(settings);
  js.addAll(docs); wasm.addAll(docs);
  same(wasm.getStoredFields(1), js.getStoredFields(1), 'a document without stored values still has an object');
  same(wasm.getStoredFields(2), js.getStoredFields(2)); assert.equal(wasm.getStoredFields(2), wasm.getStoredFields(2));
  same(wasm.getStoredFields('2'), js.getStoredFields('2'), 'ids keep their type');
  const before = wasm.getStoredFields(2);
  both(index => index.replace({ id: 2, text: 'vita nuova', category: 'prose' }));
  assert.notEqual(wasm.getStoredFields(2), before); same(wasm.getStoredFields(2), js.getStoredFields(2), 'a replaced document has new stored fields');
  both(index => index.discard(1)); same(wasm.getStoredFields(1), js.getStoredFields(1), 'discarded');
  same(JSON.parse(JSON.stringify(wasm)).storedFields, JSON.parse(JSON.stringify(js)).storedFields, 'serialized stored fields');
  wasmMode(wasm, 'after reading stored fields');
  // The original hands `{}` to boostDocument for a document without stored values.
  const fresh = new MiniSearch(settings), oracle = new Original(settings);
  fresh.addAll(docs); oracle.addAll(docs);
  const boostDocument = (_id, _term, stored) => stored.category === 'poetry' ? 2 : 1;
  sameRows(fresh.search('divina vita', { boostDocument }), oracle.search('divina vita', { boostDocument }), 'boostDocument stored fields');
  // An edit made through the returned object survives the transfer it causes.
  wasm.getStoredFields(2).category = 'edited'; js.getStoredFields(2).category = 'edited';
  sameRows(wasm.search('vita'), js.search('vita'), 'edited stored fields'); same(wasm.executionMode, 'javascript');
  wasm.free(); fresh.free();
  function both(fn) { fn(js); fn(wasm); }
}

// 6. MiniSearch JSON loads natively, clean or dirty, sync or async; what the
// native reader cannot represent is left to the original loader.
{
  const settings = { fields: ['text'], storeFields: ['category'], autoVacuum: false };
  const source = new Original(settings);
  source.addAll(Array.from({ length: 200 }, (_, id) => ({ id, text: text(), category: pick(['a', 'b']) })));
  for (const id of [3, 50, 51, 120]) source.discard(id);
  const json = JSON.stringify(source);
  const filter = row => row.category === 'a';
  for (const load of [() => MiniSearch.loadJSON(json, settings), () => MiniSearch.loadJSONAsync(json, settings)]) {
    const js = Original.loadJSON(json, settings), wasm = await load();
    wasmMode(wasm, 'loaded from MiniSearch JSON');
    for (const query of ['ap', 'pear tart', 'ab abc', '2024']) sameRows(wasm.search(query, { prefix: true, fuzzy: 0.2, filter }), js.search(query, { prefix: true, fuzzy: 0.2, filter }), `loaded ${query}`);
    same([wasm.documentCount, wasm.termCount, wasm.dirtCount], [js.documentCount, js.termCount, js.dirtCount], 'loaded counts');
    wasmMode(wasm, 'after searching a loaded dirty index'); wasm.free();
  }
  const withCallbacks = MiniSearch.loadJSON(json, { ...settings, searchOptions: { filter } });
  sameRows(withCallbacks.search('ap', { prefix: true }), Original.loadJSON(json, { ...settings, searchOptions: { filter } }).search('ap', { prefix: true }), 'load-time search callbacks');
  wasmMode(withCallbacks, 'loaded with search callbacks'); withCallbacks.free();
  const objects = new Original(settings); const key = { key: 1 }; objects.add({ id: key, text: 'apple' });
  const fallback = MiniSearch.loadJSON(JSON.stringify(objects), settings);
  same(fallback.search('apple').map(row => row.id), [{ key: 1 }]); fallback.free();
  assert.throws(() => MiniSearch.loadJSON('{', settings), SyntaxError);
  assert.throws(() => MiniSearch.loadJSON('{"serializationVersion":99}', settings), /created with an incompatible version/);
}

// 7. Native snapshots do not carry functions: search callbacks given when
// loading apply again, and engine callbacks select the JavaScript engine.
{
  const settings = { fields: ['text'], storeFields: ['category'], searchOptions: { prefix: true } };
  const index = new MiniSearch({ ...settings, searchOptions: { prefix: true, filter: row => row.category === 'fruit' } });
  index.addAll([{ id: 1, text: 'apple', category: 'fruit' }, { id: 2, text: 'apply', category: 'jobs' }]);
  same(index.search('app').map(row => row.id), [1]); wasmMode(index, 'constructor-level filter');
  const bytes = index.toBytes();
  same(MiniSearch.loadBytes(bytes).search('app').map(row => row.id).sort(), [1, 2], 'loaded without the callback');
  const reloaded = MiniSearch.loadBytes(bytes, { searchOptions: { filter: row => row.category === 'jobs' } });
  same(reloaded.search('app').map(row => row.id), [2], 'saved prefix option plus the given filter'); wasmMode(reloaded, 'reloaded with a filter');
  const boosted = MiniSearch.loadBytes(bytes, { searchOptions: { boostDocument: id => id } });
  same(boosted.executionMode, 'javascript'); same(boosted.search('app').map(row => row.id), [2, 1]);
  index.free(); reloaded.free(); boosted.free();
}

// 8. Queries that are not strings reach every search entry point safely.
{
  const settings = { fields: ['text'], autoVacuum: false };
  const docs = [{ id: 1, text: 'apple pie' }, { id: 2, text: 'apple tart' }];
  const js = new Original(settings), wasm = new MiniSearch(settings);
  js.addAll(docs); wasm.addAll(docs);
  sameRows(wasm.autoSuggest(MiniSearch.wildcard), js.autoSuggest(Original.wildcard), 'suggest wildcard');
  const tree = { combineWith: 'AND', queries: ['app', 'pi'], prefix: true };
  sameRows(wasm.autoSuggest(tree), js.autoSuggest(tree), 'suggest tree');
  same(JSON.parse(wasm.searchJoined(MiniSearch.wildcard).ids), [1, 2]);
  same(JSON.parse(wasm.searchJoinedOpts(tree, {}).ids), [1]);
  same(Array.from(wasm.searchRaw(tree).docIds), [0]);
  same(wasm.autoSuggestJoined(tree).suggestions, 'apple pie');
  same(wasm.searchCountDefault(MiniSearch.wildcard), 2);
  for (const bad of [42, {}, null, undefined]) {
    for (const call of [() => wasm.searchJoined(bad), () => wasm.searchRaw(bad), () => wasm.autoSuggest(bad), () => wasm.autoSuggestJoined(bad)]) {
      assert.throws(call, error => error instanceof Error && !(error instanceof WebAssembly.RuntimeError)); checks++;
    }
  }
  wasmMode(wasm, 'after non-string queries'); wasm.free();
}

// 9. extractField and stringifyField are evaluated on this side of the boundary.
{
  const settings = { fields: ['title', 'author.name', 'published'], storeFields: ['title', 'year'], idField: 'key', autoVacuum: false,
    extractField: (document, field) => field === 'year' ? document.published?.getUTCFullYear() : field.split('.').reduce((value, key) => value?.[key], document),
    stringifyField: (value, field) => field === 'published' ? String(value.getUTCFullYear()) : String(value) };
  const book = (key, title, name, year) => ({ key, title, author: { name }, published: year ? new Date(Date.UTC(year, 0, 1)) : undefined });
  const docs = [book('a', 'Divina Commedia', 'Dante Alighieri', 1320), book('b', 'Vita Nova', 'Dante Alighieri', 1294), book('c', 'Decameron', 'Giovanni Boccaccio')];
  const js = new Original(settings), wasm = new MiniSearch(settings);
  const both = fn => { fn(js); fn(wasm); };
  both(index => index.addAll(docs));
  wasmMode(wasm, 'constructed with extractField and stringifyField');
  for (const query of ['dante', '1320', 'vita', 'giovanni boccaccio']) sameRows(wasm.search(query, { prefix: true }), js.search(query, { prefix: true }), `extracted ${query}`);
  both(index => index.remove(docs[1]));
  both(index => index.replace(book('a', 'Commedia', 'Dante', 1321)));
  both(index => index.discard('c'));
  for (const query of ['dante', 'alighieri', '1321', 'vita', 'decameron']) sameRows(wasm.search(query), js.search(query), `extracted, after changes: ${query}`);
  same(wasm.getStoredFields('a'), js.getStoredFields('a'), 'extracted stored fields');
  same([wasm.documentCount, wasm.termCount, wasm.dirtCount], [js.documentCount, js.termCount, js.dirtCount], 'extracted counts');
  wasmMode(wasm, 'after a history with extracted fields');
  // A native snapshot carries no functions: given again, they keep working.
  const copy = MiniSearch.loadBytes(wasm.toBytes(), settings);
  copy.add(book('d', 'Il Canzoniere', 'Francesco Petrarca', 1374)); js.add(book('d', 'Il Canzoniere', 'Francesco Petrarca', 1374));
  sameRows(copy.search('petrarca 1374 dante'), js.search('petrarca 1374 dante'), 'extracted, reloaded'); wasmMode(copy, 'reloaded with extractField'); copy.free();
  // One name that needs two values (indexed text and a different stored value) has no native form.
  const dated = { ...settings, fields: ['title', 'published'], storeFields: ['published'] };
  const datedJs = new Original(dated), datedWasm = new MiniSearch(dated);
  datedJs.addAll(docs.slice(0, 2)); datedWasm.addAll(docs.slice(0, 2));
  sameRows(datedWasm.search('1320'), datedJs.search('1320'), 'indexed text differs from the stored value'); same(datedWasm.executionMode, 'javascript');
  wasm.free(); datedWasm.free();
}

// 10. What the native engine cannot represent exactly selects the JavaScript engine.
{
  const probes = [
    [{ fields: ['t'], storeFields: ['t'] }, [{ id: 1, t: 'ab\ud800cd ef' }], 'ef'],
    [{ fields: ['t'] }, [{ id: 'x\ud800', t: 'hello' }], 'hello'],
    [{ fields: ['t'], storeFields: ['t'] }, [{ id: -0, t: -0 }], '0'],
    [{ fields: 't' }, [{ id: 1, t: 'apple' }], 'apple'],
    [{ fields: ['t', 't'] }, [{ id: 1, t: 'apple' }], 'apple'],
    [{ fields: ['t'], storeFields: ['score', 'terms'] }, [{ id: 1, t: 'x y', score: 99, terms: 'zz' }, { id: 2, t: 'x', score: 5, terms: 'q' }], 'x'],
    [{ fields: ['t', 'u'], searchOptions: { boost: { t: Infinity } } }, [{ id: 1, t: 'apple', u: 'pear' }, { id: 2, t: 'pear', u: 'apple' }], 'apple'],
  ];
  for (const [settings, docs, query] of probes) {
    const js = new Original(settings), wasm = new MiniSearch(settings);
    js.addAll(docs); wasm.addAll(docs);
    same(wasm.executionMode, 'javascript', JSON.stringify(settings));
    const outcome = index => { try { return index.search(query); } catch (error) { return error.message; } };
    same(outcome(wasm), outcome(js), `exact values: ${JSON.stringify(settings)}`);
    same(docs.map(doc => wasm.has(doc.id)), docs.map(doc => js.has(doc.id)));
    wasm.free();
  }
  // A zero boost means "no boost" upstream, and stays native.
  const js = new Original({ fields: ['t', 'u'] }), wasm = new MiniSearch({ fields: ['t', 'u'] });
  for (const index of [js, wasm]) index.add({ id: 1, t: 'a', u: 'a' });
  sameRows(wasm.search('a', { boost: { t: 0 } }), js.search('a', { boost: { t: 0 } }), 'zero boost'); wasmMode(wasm, 'zero boost'); wasm.free();
}

// 11. An option explicitly set to undefined replaces the constructor's default, as upstream's object spread does.
{
  const settings = { fields: ['t', 'u'], searchOptions: { prefix: true, fuzzy: 0.3, combineWith: 'AND', fields: ['t'], boostTerm: (_term, i) => i ? 1 : 3, weights: { prefix: 0.9, fuzzy: 0.8 } } };
  const docs = [{ id: 1, t: 'apple pie', u: 'tart' }, { id: 2, t: 'apply', u: 'apple' }, { id: 3, t: 'pear tart', u: 'pie' }];
  const js = new Original(settings), wasm = new MiniSearch(settings);
  js.addAll(docs); wasm.addAll(docs);
  for (const key of ['prefix', 'fuzzy', 'combineWith', 'fields', 'boostTerm', 'weights', 'filter', 'includeMatch']) {
    for (const query of ['app tart', 'aple pie', { queries: ['app', 'tart'], [key]: undefined }]) {
      const options = { [key]: undefined };
      sameRows(wasm.search(query, options), js.search(query, options), `${key}: undefined, ${JSON.stringify(query)}`);
      if (typeof query === 'string') sameRows(wasm.autoSuggest(query, options), js.autoSuggest(query, options), `suggest ${key}: undefined`);
    }
  }
  wasmMode(wasm, 'after undefined options');
  for (const key of ['boost', 'bm25']) {
    let expected; try { js.search('app', { [key]: undefined }); } catch (error) { expected = error.message; }
    assert.throws(() => wasm.search('app', { [key]: undefined }), error => error.message === expected, `${key}: undefined`); checks++;
  }
  wasm.free();
}

// 12. Misuse is refused like upstream refuses it, without a trap and without a transfer.
{
  const settings = { fields: ['text'], autoVacuum: false };
  const js = new Original(settings), wasm = new MiniSearch(settings);
  for (const index of [js, wasm]) index.addAll([{ id: 1, text: 'apple' }, { id: 2, text: 'pear' }]);
  const message = fn => { try { fn(); return 'no error'; } catch (error) { return `${error.constructor.name}: ${error.message}`; } };
  for (const id of [1n, {}, null, undefined, NaN]) same(message(() => wasm.discard(id)), message(() => js.discard(id)), `discard(${String(id)})`);
  same(message(() => wasm.discardAll([2n])), message(() => js.discardAll([2n])));
  for (const document of [null, undefined, 5, 'text']) {
    same(message(() => wasm.add(document)), message(() => js.add(document)), `add(${String(document)})`);
    same(message(() => wasm.remove(document)), message(() => js.remove(document)), `remove(${String(document)})`);
  }
  same([wasm.documentCount, wasm.dirtCount], [js.documentCount, js.dirtCount]); wasmMode(wasm, 'after refused calls');
  const bytes = wasm.toBytes();
  const fromBuffer = MiniSearch.loadBytes(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
  same(fromBuffer.search('apple').map(row => row.id), [1]); fromBuffer.free(); wasm.free();
}

console.log(`WASM RESIDENCY: ALL PASS (${checks} checks against MiniSearch, unwarmed dirty queries included)`);
