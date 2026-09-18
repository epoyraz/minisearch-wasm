// Public-package benchmark against pinned upstream MiniSearch. Run sequentially,
// with no other benchmarks competing for CPU:
// node --expose-gc differential/bench_public.mjs [corpus.json] [output.json] [rounds]
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';
import { cpus, totalmem } from 'node:os';
import { gzipSync, brotliCompressSync, constants } from 'node:zlib';
import { execFileSync, spawnSync } from 'node:child_process';
import Original from 'minisearch';
import Public from '../pkg/minisearch_wasm_node.js';

assert.equal(typeof global.gc, 'function', 'Run Node with --expose-gc');
const root = fileURLToPath(new URL('../', import.meta.url));
const corpusPath = resolve(process.argv[2] ?? resolve(root, 'differential/bench_corpus.json'));
const outputPath = resolve(process.argv[3] ?? resolve(root, 'differential/results/public-benchmark.json'));
const rounds = Number(process.argv[4] ?? 9);
assert.ok(Number.isInteger(rounds) && rounds >= 3);
const corpusBytes = readFileSync(corpusPath);
const { docs, queries } = JSON.parse(corpusBytes);
const docsJSON = JSON.stringify(docs);
const options = { fields: ['title', 'text'], autoVacuum: false,
  searchOptions: { prefix: true, fuzzy: 0.2, combineWith: 'AND' } };
const filter = () => true;
const discarded = docs.filter((_, i) => i % 4 === 0).map(doc => doc.id);
const firstQuery = queries[0];
const sha256 = value => createHash('sha256').update(value).digest('hex');
const quantile = (values, p) => {
  const sorted = [...values].sort((a, b) => a - b);
  const at = (sorted.length - 1) * p, low = Math.floor(at), high = Math.ceil(at);
  return sorted[low] + (sorted[high] - sorted[low]) * (at - low);
};
const stats = samples => ({ medianMs: quantile(samples, 0.5), p25Ms: quantile(samples, 0.25), p75Ms: quantile(samples, 0.75), samplesMs: samples });
const report = {
  createdAt: new Date().toISOString(), originalVersion: JSON.parse(readFileSync(resolve(root, 'node_modules/minisearch/package.json'))).version,
  publicVersion: JSON.parse(readFileSync(resolve(root, 'pkg/package.json'))).version,
  publicRevision: 'unreleased working tree with JavaScript compatibility facade',
  gitHead: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(),
  environment: { node: process.version, v8: process.versions.v8, platform: process.platform, arch: process.arch,
    cpu: cpus()[0].model.trim(), logicalCPUs: cpus().length, ramGiB: totalmem() / 2 ** 30 },
  corpus: { path: corpusPath, documents: docs.length, queries: queries.length, sha256: sha256(corpusBytes), documentsSha256: sha256(docsJSON), queryTexts: queries },
  options, rounds, warmupRounds: 2,
  runnerSha256: sha256(readFileSync(fileURLToPath(import.meta.url))),
  artifacts: Object.fromEntries(['pkg/minisearch_wasm.js', 'pkg/minisearch_wasm_core.js', 'pkg/minisearch_wasm_bg.wasm', 'js/minisearch.js']
    .map(path => [path, sha256(readFileSync(resolve(root, path)))])),
  methodology: [
    'Public facade, never the private core entry; exact upstream version pinned.',
    'Two warmups, then paired alternating engine order; GC outside timed regions.',
    'Search batches consume IDs, scores and term strings; compact decoding is included, with a cached ID table.',
    'Cold means a fresh index and first query batch, not cold process/JIT. Index setup is excluded from search timings.',
    'Construction/loading timings exclude verification and disposal. Intrinsic allocations/GC and async yields remain inside timing.',
    'All search outputs verified before timing; first callback and dirty queries checked without warm-up.',
    'Snapshot timings compare different native-binary/JS-JSON formats; same-format JSON load is also measured.',
    'Node on one synthetic corpus; not browser latency, production workloads or a memory allocation benchmark.',
  ],
  validation: { comparedRows: 0, maxRelativeScoreDelta: 0 }, cases: [],
};
mkdirSync(dirname(outputPath), { recursive: true });
const checkpoint = () => writeFileSync(outputPath, JSON.stringify(report, null, 2) + '\n');
function build(kind, mode = 'wasm') {
  const index = kind === 'original' ? new Original(options) : new Public(mode === 'javascript' ? { ...options, logger: () => {} } : options);
  if (kind === 'public' && mode === 'wasm') index.addAllJSON(docsJSON); else index.addAll(docs);
  if (kind === 'public') assert.equal(index.executionMode, mode);
  return index;
}
const free = index => index?.free?.();
function compare(actual, expected, full = true) {
  assert.equal(actual.length, expected.length);
  for (let i = 0; i < actual.length; i++) {
    const a = actual[i], b = expected[i];
    if ('id' in b) assert.equal(a.id, b.id, `row ${i} ID/order`);
    if ('suggestion' in b) assert.equal(a.suggestion, b.suggestion);
    assert.deepEqual(a.terms, b.terms);
    const delta = Math.abs(a.score - b.score) / Math.max(1, Math.abs(a.score), Math.abs(b.score));
    assert.ok(delta <= 1e-12, `score delta ${delta}`);
    report.validation.maxRelativeScoreDelta = Math.max(report.validation.maxRelativeScoreDelta, delta);
    if (full && 'match' in b) { assert.deepEqual(a.match, b.match); assert.deepEqual(a.queryTerms, b.queryTerms); }
    report.validation.comparedRows++;
  }
}
const unpackJoined = result => {
  const ids = JSON.parse(result.ids), terms = result.count ? result.terms.split('\n') : [];
  return ids.map((id, i) => ({ id, score: result.scores[i], terms: terms[i] ? terms[i].split(' ') : [] }));
};
const unpackRaw = (result, ids) => {
  const terms = result.termTable ? result.termTable.split('\n') : [];
  return Array.from(result.docIds, (id, i) => ({ id: ids[id], score: result.scores[i],
    terms: Array.from(result.termIds.subarray(result.termOffsets[i], result.termOffsets[i + 1]), id => terms[id]) }));
};
function consume(index, kind = 'full', queryList = queries, searchOptions, idTable) {
  let count = 0, checksum = 0;
  for (const query of queryList) {
    if (kind === 'joined') {
      const result = index.searchJoined(query), ids = JSON.parse(result.ids);
      const terms = result.count ? result.terms.split('\n') : [];
      count += result.count;
      for (let i = 0; i < result.count; i++) {
        checksum += Number(ids[i]) + result.scores[i];
        if (terms[i]) for (const term of terms[i].split(' ')) checksum += term.length;
      }
      continue;
    }
    if (kind === 'raw') {
      const result = index.searchRaw(query), terms = result.termTable ? result.termTable.split('\n') : [];
      count += result.count;
      for (let i = 0; i < result.count; i++) {
        checksum += Number(idTable[result.docIds[i]]) + result.scores[i];
        for (let j = result.termOffsets[i]; j < result.termOffsets[i + 1]; j++) checksum += terms[result.termIds[j]].length;
      }
      continue;
    }
    const rows = index.search(query, searchOptions);
    count += rows.length;
    for (const row of rows) {
      checksum += Number(row.id) + row.score;
      for (const term of row.terms) checksum += term.length;
    }
  }
  return { count, checksum };
}
function consumeSuggestions(index) {
  let count = 0, checksum = 0;
  for (const query of queries) {
    const rows = index.autoSuggest(query); count += rows.length;
    for (const row of rows) checksum += row.suggestion.length + row.score;
  }
  return { count, checksum };
}
function sameChecksum(a, b) {
  assert.equal(a.count, b.count);
  assert.ok(Math.abs(a.checksum - b.checksum) <= 1e-11 * Math.max(1, Math.abs(a.checksum), Math.abs(b.checksum)));
}
async function paired(name, variants, { unit = 'operation', mode = 'wasm', samples = rounds, compareValues = sameChecksum } = {}) {
  const times = { original: [], public: [] };
  for (let round = -2; round < samples; round++) {
    const values = {};
    for (const kind of round % 2 === 0 ? ['original', 'public'] : ['public', 'original']) {
      const variant = variants[kind], state = await variant.setup?.();
      global.gc();
      const start = performance.now();
      const value = await variant.run(state);
      const elapsed = performance.now() - start;
      if (round >= 0) times[kind].push(elapsed);
      values[kind] = variant.value ? variant.value(value, state) : value;
      if (kind === 'public' && value?.executionMode) assert.equal(value.executionMode, mode);
      variant.verify?.(value, state);
      variant.cleanup?.(value, state);
    }
    compareValues?.(values.public, values.original);
  }
  const result = { name, unit, publicExecutionMode: mode, original: stats(times.original), public: stats(times.public) };
  result.speedup = result.original.medianMs / result.public.medianMs;
  report.cases.push(result); checkpoint();
  console.log(`${name.padEnd(37)} JS ${result.original.medianMs.toFixed(2).padStart(9)} ms  public ${result.public.medianMs.toFixed(2).padStart(9)} ms  ${result.speedup.toFixed(2)}x`);
}
console.log(`Public API vs MiniSearch ${report.originalVersion}: ${docs.length} documents, ${queries.length} queries, ${rounds} paired rounds`);
const original = build('original'), current = build('public');
const idTable = JSON.parse(current.docIdTable());
for (const query of queries) {
  const expected = original.search(query);
  compare(current.search(query), expected);
  compare(unpackJoined(current.searchJoined(query)), expected, false);
  compare(unpackRaw(current.searchRaw(query), idTable), expected, false);
  compare(current.autoSuggest(query), original.autoSuggest(query));
}
for (const dirty of [false, true]) {
  const a = build('original'), b = build('public');
  if (dirty) { a.discardAll(discarded); b.discardAll(discarded); }
  const perCall = dirty ? undefined : { filter };
  compare(b.search(firstQuery, perCall), a.search(firstQuery, perCall));
  assert.equal(b.executionMode, 'javascript'); free(b);
}
console.log(`Verified ${report.validation.comparedRows} result rows, max relative score delta ${report.validation.maxRelativeScoreDelta}`);
const warmVariants = kind => ({ original: { run: () => consume(original) }, public: { run: () => consume(current, kind, queries, undefined, idTable) } });
await paired('warm search: full objects', warmVariants('full'), { unit: `${queries.length} queries` });
await paired('warm search: joined decoded', warmVariants('joined'), { unit: `${queries.length} queries` });
await paired('warm search: raw decoded', warmVariants('raw'), { unit: `${queries.length} queries` });
for (const [api, label] of [['full', 'full objects'], ['joined', 'joined decoded'], ['raw', 'raw decoded']]) {
  await paired(`first batch: ${label}`, Object.fromEntries(['original', 'public'].map(kind => [kind, {
    setup: () => {
      const index = build(kind);
      return { index, ids: kind === 'public' && api === 'raw' ? JSON.parse(index.docIdTable()) : undefined };
    },
    run: ({ index, ids }) => consume(index, kind === 'original' ? 'full' : api, queries, undefined, ids),
    cleanup: (_, { index }) => free(index),
  }])), { unit: `${queries.length} queries` });
}
await paired('warm autoSuggest', { original: { run: () => consumeSuggestions(original) }, public: { run: () => consumeSuggestions(current) } }, { unit: `${queries.length} queries` });

const countValue = index => ({ count: index.documentCount, checksum: index.termCount });
for (const api of ['addAll', 'addAllJSON', 'addAllAsync']) {
  await paired(api === 'addAllJSON' ? 'build from JSON text' : `build: ${api}`, Object.fromEntries(['original', 'public'].map(kind => [kind, {
    setup: () => kind === 'original' ? new Original(options) : new Public(options),
    run: async index => {
      if (api === 'addAllAsync') await index.addAllAsync(docs, { chunkSize: 500 });
      else if (api === 'addAllJSON') { if (kind === 'original') index.addAll(JSON.parse(docsJSON)); else index.addAllJSON(docsJSON); }
      else index.addAll(docs);
      return index;
    }, value: countValue, cleanup: index => free(index),
  }])), { unit: `${docs.length} documents` });
}
const originalJSON = JSON.stringify(original), binary = current.toBytes();
await paired('save: JSON vs native binary', { original: { run: () => JSON.stringify(original) }, public: { run: () => current.toBytes() } }, { compareValues: null });
await paired('load: JSON vs native binary', {
  original: { run: () => Original.loadJSON(originalJSON, options), value: countValue },
  public: { run: () => Public.loadBytes(binary), value: countValue, cleanup: free },
});
for (const api of ['loadJSON', 'loadJSONAsync']) {
  await paired(`${api}: same JSON input`, {
    original: { run: () => Original[api](originalJSON, options), value: countValue },
    public: { run: () => Public[api](originalJSON, options), value: countValue, cleanup: free },
  }, { mode: 'javascript', samples: api === 'loadJSONAsync' ? Math.min(rounds, 5) : rounds });
}
for (const dirty of [false, true]) {
  await paired(dirty ? 'first query after 25% discard' : 'first callback query + transfer', Object.fromEntries(['original', 'public'].map(kind => [kind, {
    setup: () => { const index = build(kind); if (dirty) index.discardAll(discarded); return index; },
    run: index => consume(index, 'full', [firstQuery], dirty ? undefined : { filter }),
    verify: (_, index) => { if (kind === 'public') assert.equal(index.executionMode, 'javascript'); },
    cleanup: (_, index) => free(index),
  }])), { mode: 'javascript', unit: `one query: ${firstQuery}` });
}
const fallback = build('public', 'javascript');
await paired('warm callback search (JS fallback)', {
  original: { run: () => consume(original, 'full', queries, { filter }) },
  public: { run: () => consume(fallback, 'full', queries, { filter }) },
}, { mode: 'javascript', unit: `${queries.length} queries` });
await paired('vacuum after 25% discard', Object.fromEntries(['original', 'public'].map(kind => [kind, {
  setup: () => { const index = build(kind); index.discardAll(discarded); return index; },
  run: async index => { await index.vacuum({ batchSize: 1000, batchWait: 1 }); return index; },
  value: countValue, verify: index => assert.equal(index.dirtCount, 0), cleanup: free,
}])), { mode: 'javascript', unit: `${discarded.length} discarded documents`, samples: Math.min(rounds, 5) });

const size = value => ({ rawBytes: Buffer.byteLength(value), gzipBytes: gzipSync(value, { level: 9 }).length,
  brotliQuality6Bytes: brotliCompressSync(value, { params: { [constants.BROTLI_PARAM_QUALITY]: 6 } }).length });
report.snapshotSizes = { originalJSON: size(originalJSON), publicNativeBinary: size(binary), publicFallbackEnvelope: size(fallback.toBytes()) };
const publicAssets = ['pkg/minisearch_wasm.js', 'pkg/minisearch_wasm_core.js', 'pkg/minisearch_wasm_bg.wasm'];
report.moduleSizes = { originalESM: size(readFileSync(resolve(root, 'node_modules/minisearch/dist/es/index.js'))),
  publicMainAssets: Object.fromEntries(publicAssets.map(path => [path, size(readFileSync(resolve(root, path)))])) };
// Fresh processes isolate module parsing/initialization from previous imports.
const imports = { original: pathToFileURL(resolve(root, 'node_modules/minisearch/dist/es/index.js')).href,
  public: pathToFileURL(resolve(root, 'pkg/minisearch_wasm_node.js')).href };
const importTimes = { original: [], public: [] };
for (let i = 0; i < rounds; i++) {
  for (const kind of i % 2 ? ['public', 'original'] : ['original', 'public']) {
    const code = `const t=performance.now();const m=await import(${JSON.stringify(imports[kind])});const ms=performance.now()-t;const x=new m.default({fields:['text']});x.free?.();console.log(ms);`;
    const run = spawnSync(process.execPath, ['--input-type=module', '-e', code], { encoding: 'utf8' });
    assert.equal(run.status, 0, run.stderr); importTimes[kind].push(Number(run.stdout.trim()));
  }
}
report.coldModuleImport = { original: stats(importTimes.original), public: stats(importTimes.public),
  note: 'Fresh Node process, OS file cache warm; measured import only, excluding process startup. Public Node entry initializes Wasm.' };
report.completedAt = new Date().toISOString();
report.matchesDiskAtCompletion = Object.fromEntries(Object.entries(report.artifacts)
  .map(([path, hash]) => [path, sha256(readFileSync(resolve(root, path))) === hash]));
assert.equal(current.executionMode, 'wasm'); assert.equal(fallback.executionMode, 'javascript');
free(current); free(fallback); checkpoint();
assert.ok(Object.values(report.matchesDiskAtCompletion).every(Boolean), 'Code changed during the run; results refer to the recorded artifact hashes');
console.log(`VERIFIED; raw samples and artifact hashes: ${outputPath}`);
