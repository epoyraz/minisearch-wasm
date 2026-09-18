// Install the actual tarball, then exercise package resolution, strict types,
// CommonJS and a browser-global bundle with dynamic JS code generation disabled.
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import vm from 'node:vm';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const fixture = mkdtempSync(join(tmpdir(), 'minisearch-package-contract-'));
const npm = process.env.npm_execpath;
assert.ok(npm, 'Run with npm run test:package so the active npm CLI is reused');
const run = (args, cwd = fixture) => {
  const result = spawnSync(process.execPath, args, { cwd, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  return result.stdout;
};
writeFileSync(join(fixture, 'package.json'), JSON.stringify({ private: true, type: 'module' }));
const packed = JSON.parse(run([npm, 'pack', join(root, 'pkg'), '--json', '--pack-destination', fixture], root));
run([npm, 'install', '--ignore-scripts', '--no-audit', '--no-fund', join(fixture, packed[0].filename)]);
writeFileSync(join(fixture, 'esm.mjs'), `
import assert from 'node:assert/strict';
import MiniSearch, { MiniSearchWasm, init } from 'minisearch-wasm';
import SearchableMap from 'minisearch-wasm/SearchableMap';
const index = new MiniSearch({fields:['text']});
assert.equal(index.executionMode, 'wasm');
index.add({id:1,text:'apple'});
assert.equal(index.search('apple')[0].id,1);
assert.ok(index instanceof MiniSearch && index instanceof MiniSearchWasm);
const map=SearchableMap.from([['apple',1],['apricot',2]]);
assert.deepEqual([...map.atPrefix('app')],[['apple',1]]);
assert.equal(map.fuzzyGet('appl',1).get('apple')[0],1);
await init(); await MiniSearch(); index.free();
`);
writeFileSync(join(fixture, 'cjs.cjs'), `
const assert=require('node:assert/strict');
const MiniSearch=require('minisearch-wasm');
const SearchableMap=require('minisearch-wasm/SearchableMap');
const index=new MiniSearch({fields:['text']});
assert.equal(index.executionMode,'wasm');
index.add({id:1,text:'apple'}); assert.equal(index.search('apple')[0].id,1);
assert.equal(SearchableMap.from([['apple',1]]).get('apple'),1); index.free();
`);
run(['--disallow-code-generation-from-strings', 'esm.mjs']);
run(['--disallow-code-generation-from-strings', 'cjs.cjs']);
writeFileSync(join(fixture, 'esm.mts'), `
import MiniSearch, {MiniSearchWasm, type Options, type BM25Params, type CombinationOperator} from 'minisearch-wasm';
import SearchableMap from 'minisearch-wasm/SearchableMap';
const options:Options<{id:number;text:string}>={fields:['text']};
const index=new MiniSearch<{id:number;text:string}>(options);
const annotated:MiniSearch<{id:number;text:string}>=index;
const named=new MiniSearchWasm<{id:number;text:string}>(options);
const operator:CombinationOperator='And';
index.search('a',{combineWith:operator,filter:result=>result.score>0});
MiniSearch.getDefault('tokenize')('a b');
const map=new SearchableMap<number>();map.set('a',1);
// @ts-expect-error wrong document ID type
index.add({id:'x',text:'a'});
`);
writeFileSync(join(fixture, 'cjs.cts'), `
import MiniSearch = require('minisearch-wasm');
import SearchableMap = require('minisearch-wasm/SearchableMap');
const index=new MiniSearch<{id:number;text:string}>({fields:['text']});
const annotated:MiniSearch<{id:number;text:string}>=index;
index.search('a',{combineWith:'And'});
const map=new SearchableMap<number>();map.set('a',1);
`);
run([join(root, 'node_modules/typescript/bin/tsc'), '--ignoreConfig', '--noEmit', '--strict', '--target', 'es2022', '--module', 'nodenext', '--lib', 'es2022,dom', 'esm.mts', 'cjs.cts']);

const packageDir = join(fixture, 'node_modules/minisearch-wasm');
assert.match(readFileSync(join(packageDir, 'LICENSE.minisearch.txt'), 'utf8'), /Luca Ongaro/);
assert.match(readFileSync(join(packageDir, 'COMPATIBILITY.md'), 'utf8'), /executionMode/);
const context = vm.createContext({ console, TextEncoder, TextDecoder, WebAssembly, URL, setTimeout, clearTimeout,
  wasmBytes: readFileSync(join(packageDir, 'minisearch_wasm_bg.wasm')),
  document: { currentScript: { src: 'https://example.test/minisearch_wasm.umd.js' } },
}, { codeGeneration: { strings: false, wasm: true } });
vm.runInContext(readFileSync(join(packageDir, 'minisearch_wasm.umd.js'), 'utf8'), context);
assert.equal(vm.runInContext(`(() => {
  const before=new MiniSearch({fields:['text']}); before.add({id:1,text:'apple'});
  if(before.search('apple')[0].id!==1) return false;
  MiniSearch.initSync({module:wasmBytes});
  const after=new MiniSearch({fields:['text']}); after.add({id:2,text:'pear'});
  const ok=after.executionMode==='wasm' && after.search('pear')[0].id===2;
  before.free();after.free();return ok;
})()`, context), true);
console.log('PACKAGE CONTRACT: ESM, CommonJS, SearchableMap, strict ES2022 types, UMD and CSP passed');
console.log(`Packed-artifact fixture: ${fixture}`);
