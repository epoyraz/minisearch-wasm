// Install the actual tarball, then exercise package resolution, strict types,
// CommonJS and a browser-global bundle with dynamic JS code generation disabled.
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, readdirSync, rmSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { resolve, join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import vm from 'node:vm';
import { build } from 'esbuild';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const fixture = mkdtempSync(join(tmpdir(), 'minisearch-package-contract-'));
const npm = process.env.npm_execpath;
assert.ok(npm, 'Run with npm run test:package so the active npm CLI is reused');

// The package is assembled from scratch and deterministically: assembling it
// again changes nothing. A difference means pkg/ did not match the sources.
const hashOf = directory => {
  const hash = createHash('sha256');
  for (const entry of readdirSync(directory, { recursive: true, withFileTypes: true }).filter(entry => entry.isFile()).map(entry => join(entry.parentPath, entry.name)).sort()) {
    hash.update(entry.slice(directory.length)).update(readFileSync(entry));
  }
  return hash.digest('hex');
};
const tested = hashOf(join(root, 'pkg'));
const finalize = spawnSync(process.execPath, [join(root, 'scripts/finalize-pkg.mjs')], { cwd: root, encoding: 'utf8' });
assert.equal(finalize.status, 0, finalize.stdout + finalize.stderr);
assert.equal(hashOf(join(root, 'pkg')), tested, 'pkg/ was stale and has been reassembled from the current sources: run the suites again');
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
import type { SearchResult, Options, Query, JoinedResults } from 'minisearch-wasm';
const settings:Options<{id:number;text:string}>={fields:['text']};
const rows:SearchResult[]=new MiniSearch(settings).search('a' as Query);
const joined:JoinedResults|MiniSearch.RawResults=new MiniSearch(settings).searchJoined('a');
const index=new MiniSearch<{id:number;text:string}>({fields:['text']});
const annotated:MiniSearch<{id:number;text:string}>=index;
index.search('a',{combineWith:'And'});
const map=new SearchableMap<number>();map.set('a',1);
`);
// Without the DOM library, and with declaration files checked: the package's
// types stand on their own, MiniSearch's included.
run([join(root, 'node_modules/typescript/bin/tsc'), '--ignoreConfig', '--noEmit', '--strict', '--target', 'es2022', '--module', 'nodenext', '--lib', 'es2022', '--skipLibCheck', 'false', 'esm.mts', 'cjs.cts']);

const packageDir = join(fixture, 'node_modules/minisearch-wasm');
const version = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version;
assert.equal(readFileSync(join(packageDir, 'README.md'), 'utf8'), readFileSync(join(root, 'README.md'), 'utf8'), 'the package ships the current README');
assert.ok(readFileSync(join(packageDir, 'README.md'), 'utf8').includes(version), `the README mentions version ${version}`);
assert.equal(readFileSync(join(packageDir, 'LICENSE.txt'), 'utf8'), readFileSync(join(root, 'LICENSE.txt'), 'utf8'));
assert.equal(JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf8')).dependencies, undefined, 'MiniSearch is bundled, not a dependency');
assert.equal(existsSync(join(fixture, 'node_modules/minisearch')), false);
assert.match(readFileSync(join(packageDir, 'LICENSE.minisearch.txt'), 'utf8'), /Luca Ongaro/);
assert.match(readFileSync(join(packageDir, 'COMPATIBILITY.md'), 'utf8'), /executionMode/);
const context = vm.createContext({ console, TextEncoder, TextDecoder, WebAssembly, URL, setTimeout, clearTimeout,
  wasmBytes: readFileSync(join(packageDir, 'minisearch_wasm_bg.wasm')),
  document: { currentScript: { src: 'https://example.test/minisearch_wasm.umd.js' } },
}, { codeGeneration: { strings: false, wasm: true } });
// The global script defines MiniSearch and nothing else, however often it loads.
const globalsBefore = new Set(Object.keys(vm.runInContext('globalThis', context)));
for (let load = 0; load < 2; load++) vm.runInContext(readFileSync(join(packageDir, 'minisearch_wasm.umd.js'), 'utf8'), context);
assert.deepEqual(Object.keys(vm.runInContext('globalThis', context)).filter(name => !globalsBefore.has(name)), ['MiniSearch']);
assert.equal(vm.runInContext(`(() => {
  const before=new MiniSearch({fields:['text']}); before.add({id:1,text:'apple'});
  if(before.search('apple')[0].id!==1) return false;
  MiniSearch.initSync({module:wasmBytes});
  const after=new MiniSearch({fields:['text']}); after.add({id:2,text:'pear'});
  const ok=after.executionMode==='wasm' && after.search('pear')[0].id===2;
  before.free();after.free();return ok;
})()`, context), true);
// A bundler that leaves the .wasm file behind must not break the import: the
// Node entry then warns and runs on the JavaScript engine.
writeFileSync(join(fixture, 'bundled.mjs'), `import MiniSearch from 'minisearch-wasm';
const index = new MiniSearch({ fields: ['text'] }); index.add({ id: 1, text: 'apple' });
console.log(index.executionMode, index.search('apple')[0].id);
`);
await build({ absWorkingDir: fixture, entryPoints: ['bundled.mjs'], bundle: true, platform: 'node', format: 'esm', outfile: 'out/bundled.mjs', logLevel: 'error' });
const bundled = spawnSync(process.execPath, ['out/bundled.mjs'], { cwd: fixture, encoding: 'utf8' });
assert.equal(bundled.status, 0, bundled.stderr); assert.match(bundled.stdout, /^javascript 1/); assert.match(bundled.stderr, /could not be loaded/);
// require() from browser code resolves to the fetch-based entry, never node:fs.
writeFileSync(join(fixture, 'browser.cjs'), "const MiniSearch = require('minisearch-wasm'); module.exports = new MiniSearch({ fields: ['text'] });\n");
await build({ absWorkingDir: fixture, entryPoints: ['browser.cjs'], bundle: true, platform: 'browser', outfile: 'out/browser.js', logLevel: 'error' });
assert.doesNotMatch(readFileSync(join(fixture, 'out/browser.js'), 'utf8'), /node:fs/);
rmSync(fixture, { recursive: true });
console.log('PACKAGE CONTRACT: ESM, CommonJS, SearchableMap, strict ES2022 types without the DOM library, README and licenses, global script, bundlers and CSP passed');
