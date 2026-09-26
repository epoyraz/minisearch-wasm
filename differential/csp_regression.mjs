import assert from 'node:assert/strict';
import { MiniSearchWasm } from '../pkg/minisearch_wasm_node.js';
const index = new MiniSearchWasm({ fields: ['text'] });
index.add({ id: 1, text: 'apple pear' });
assert.equal(index.executionMode, 'wasm');
assert.equal(index.search('apple')[0].id, 1);
assert.deepEqual(MiniSearchWasm.getDefault('tokenize')('apple pear'), ['apple', 'pear']);
index.free();
console.log('CSP: Wasm indexing, search and getDefault pass without string code generation');
