import assert from 'node:assert/strict';
import { MiniSearchWasm } from '../pkg/minisearch_wasm_node.js';
import { MiniSearchWasm as Core } from '../pkg/minisearch_wasm_core.js';
for (const C of [Core, MiniSearchWasm]) {
  const index = new C({ fields: ['text'] });
  index.add({ id: 1, text: 'apple pear' });
  assert.equal(index.search('apple')[0].id, 1);
  assert.deepEqual(C.getDefault('tokenize')('apple pear'), ['apple', 'pear']);
  index.free();
}
console.log('CSP: core and public search/getDefault pass without string code generation');
