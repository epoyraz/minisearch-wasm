// Served with CSP that allows Wasm but forbids eval/new Function. No import map,
// bundler or dependency server: these are the actual generated browser assets.
import MiniSearch, { init } from '../pkg/minisearch_wasm.js';
import SearchableMap from '../pkg/SearchableMap.js';

const assert = (value, message) => { if (!value) throw new Error(message); };
async function basic() {
  await init();
  const index = new MiniSearch({ fields: ['text'], autoVacuum: false });
  index.addAll([{ id: 1, text: 'apple' }, { id: 2, text: 'pear' }]);
  assert(index.executionMode === 'wasm', 'native execution');
  assert(index.search('apple')[0].id === 1, 'native result builder under CSP');
  assert(index.searchRaw('pear').scores instanceof Float64Array, 'raw results');
  assert(index.search('apple', { filter: row => row.id === 1 })[0].id === 1, 'filter result');
  assert(index.executionMode === 'wasm', 'filter stays native');
  index.search('apple', { boostDocument: () => 1 });
  assert(index.executionMode === 'javascript', 'scoring callback promotion');
  index.free();
}

if (typeof document === 'undefined') {
  basic().then(() => postMessage({ ok: true }), error => postMessage({ ok: false, error: String(error.stack) }));
} else {
  const violations = [];
  document.addEventListener('securitypolicyviolation', event => violations.push(event.violatedDirective));
  (async () => {
    await basic();
    const map = SearchableMap.from([['apple', 1]]);
    assert([...map.atPrefix('app')][0][1] === 1, 'SearchableMap ESM');
    // Global bundle fetches Wasm relative to its own script URL.
    await globalThis.MiniSearch.init();
    const globalIndex = new globalThis.MiniSearch({ fields: ['text'] });
    globalIndex.add({ id: 3, text: 'apple' });
    assert(globalIndex.executionMode === 'wasm' && globalIndex.search('apple')[0].id === 3, 'global bundle');
    globalIndex.free();
    const large = new MiniSearch({ fields: ['text'] });
    large.addAll(Array.from({ length: 2500 }, (_, id) => ({ id, text: 'word' + id })));
    let ticks = 0;
    const timer = setInterval(() => { ticks++; }, 0);
    let loaded;
    try { loaded = await MiniSearch.loadJSONAsync(JSON.stringify(large), { fields: ['text'] }); }
    finally { clearInterval(timer); }
    assert(ticks >= 3 && loaded.documentCount === 2500 && loaded.executionMode === 'wasm', 'native async loader yields repeatedly to browser');
    large.free(); loaded.free();
    await new Promise((resolve, reject) => {
      const worker = new Worker(new URL('./browser_contract.mjs', import.meta.url), { type: 'module' });
      const timeout = setTimeout(() => { worker.terminate(); reject(new Error('worker timeout')); }, 15000);
      const finish = error => { clearTimeout(timeout); worker.terminate(); error ? reject(error) : resolve(); };
      worker.onmessage = event => finish(event.data.ok ? undefined : new Error(event.data.error));
      worker.onerror = event => finish(new Error(event.message));
    });
    assert(violations.length === 0, 'CSP violations: ' + violations.join(', '));
    return { ok: true, checks: ['browser ESM', 'SearchableMap', 'global bundle', 'Wasm CSP', 'callbacks', 'async JSON', 'module Worker'], violations };
  })().then(result => {
    globalThis.browserContract = result;
    document.querySelector('#result').textContent = JSON.stringify(result, null, 2);
  }, error => {
    globalThis.browserContract = { ok: false, error: String(error.stack) };
    document.querySelector('#result').textContent = JSON.stringify(globalThis.browserContract, null, 2);
  });
}
