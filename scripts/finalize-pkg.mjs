// Post-build step for the npm package.
//
// wasm-pack --no-pack generates core glue and assets. This script owns the
// public facade, browser/Node/CommonJS entries, declarations and npm manifest.
// Node initializes synchronously from packaged bytes; browser entries retain
// explicit init() and also support a synchronous JS constructor before init.

import { readFileSync, writeFileSync, copyFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { build } from "esbuild";

const here = dirname(fileURLToPath(import.meta.url));
const pkgDir = resolve(here, "..", "pkg");
const pkgPath = resolve(pkgDir, "package.json");

const REPO = "https://github.com/epoyraz/minisearch-wasm";
const upstreamLicense = readFileSync(resolve(here, "../node_modules/minisearch/LICENSE.txt"), "utf8");
copyFileSync(resolve(here, "../COMPATIBILITY.md"), resolve(pkgDir, "COMPATIBILITY.md"));
writeFileSync(resolve(pkgDir, "LICENSE.minisearch.txt"), upstreamLicense);
const licenseBanner = `/*! Bundles MiniSearch 7.2.0.\n${upstreamLicense.trim()}\n*/`;

// Preserve generated glue separately; the public module is the compatibility facade.
const coreGlue = readFileSync(resolve(pkgDir, "minisearch_wasm.js"), "utf8")
  .replace('@ts-self-types="./minisearch_wasm.d.ts"', '@ts-self-types="./minisearch_wasm_core.d.ts"');
writeFileSync(resolve(pkgDir, "minisearch_wasm_core.js"), coreGlue);
// Incremental builds can leave older hashed snippet directories in pkg/.
// Publish only the files imported by this build's generated glue.
const snippets = [...coreGlue.matchAll(/from ['"]\.\/(snippets\/[^'"]+)['"]/g)].map(match => match[1]);
copyFileSync(resolve(pkgDir, "minisearch_wasm.d.ts"), resolve(pkgDir, "minisearch_wasm_core.d.ts"));
copyFileSync(resolve(here, "../js/minisearch.js"), resolve(pkgDir, "minisearch_wasm.js"));
copyFileSync(resolve(here, "../js/minisearch.d.ts"), resolve(pkgDir, "minisearch_wasm.d.ts"));
// Plain browser ESM must not require an import map for the compatibility engine.
// Keep core glue external so its relative Wasm URL and snippets stay intact.
const esm = await build({ entryPoints: [resolve(pkgDir, "minisearch_wasm.js")], bundle: true,
  platform: "browser", format: "esm", target: "es2022", write: false,
  banner: { js: licenseBanner },
  external: ["./minisearch_wasm_core.js"] });
writeFileSync(resolve(pkgDir, "minisearch_wasm.js"), esm.outputFiles[0].text);
const NODE_ENTRY = "minisearch_wasm_node.js";
writeFileSync(resolve(pkgDir, NODE_ENTRY), `import { readFileSync } from "node:fs";
import MiniSearch, { initSync } from "./minisearch_wasm.js";
initSync({ module: readFileSync(new URL("./minisearch_wasm_bg.wasm", import.meta.url)) });
export * from "./minisearch_wasm.js";
export default MiniSearch;
`);
writeFileSync(resolve(pkgDir, "SearchableMap.js"), 'export { default } from "minisearch/SearchableMap";\n');
writeFileSync(resolve(pkgDir, "SearchableMap.d.ts"), 'export { default } from "minisearch/SearchableMap";\n');
const mapEsm = await build({ entryPoints: [resolve(pkgDir, "SearchableMap.js")], bundle: true,
  platform: "browser", format: "esm", target: "es2022", write: false, banner: { js: licenseBanner } });
writeFileSync(resolve(pkgDir, "SearchableMap.js"), mapEsm.outputFiles[0].text);
await build({ entryPoints: [resolve(pkgDir, NODE_ENTRY)], outfile: resolve(pkgDir, "minisearch_wasm.cjs"), bundle: true,
  platform: "node", format: "cjs", target: "node18", define: { "import.meta.url": "__minisearchModuleUrl" },
  banner: { js: 'const __minisearchModuleUrl = require("node:url").pathToFileURL(__filename).href;' },
  footer: { js: 'module.exports = Object.assign(module.exports.default, module.exports);' } });
await build({ entryPoints: [resolve(pkgDir, "minisearch_wasm.js")], outfile: resolve(pkgDir, "minisearch_wasm.umd.js"), bundle: true,
  platform: "browser", format: "iife", globalName: "MiniSearchWasmModule", target: "es2022",
  define: { "import.meta.url": "__minisearchScriptUrl" },
  banner: { js: 'const __minisearchScriptUrl = typeof document !== "undefined" ? document.currentScript?.src : globalThis.location?.href;' },
  footer: { js: 'globalThis.MiniSearch = Object.assign(MiniSearchWasmModule.default, MiniSearchWasmModule);' } });
await build({ entryPoints: [resolve(pkgDir, "SearchableMap.js")], outfile: resolve(pkgDir, "SearchableMap.cjs"), bundle: true,
  platform: "node", format: "cjs", footer: { js: 'module.exports = module.exports.default;' } });
// CommonJS declarations must themselves be CommonJS under NodeNext resolution.
writeFileSync(resolve(pkgDir, "minisearch_wasm.d.cts"), `declare const MiniSearch: typeof import("./minisearch_wasm.js", { with: { "resolution-mode": "import" } }).default;
type MiniSearch<T = any> = import("./minisearch_wasm.js", { with: { "resolution-mode": "import" } }).MiniSearchWasm<T>;
export = MiniSearch;
`);
writeFileSync(resolve(pkgDir, "SearchableMap.d.cts"), `declare const SearchableMap: typeof import("./SearchableMap.js", { with: { "resolution-mode": "import" } }).default;
export = SearchableMap;
`);

const metadata = {
  description:
    "WebAssembly full-text search with a MiniSearch-compatible JavaScript fallback for callbacks and value semantics.",
  author: "Enes Poyraz",
  repository: { type: "git", url: `git+${REPO}.git` },
  homepage: `${REPO}#readme`,
  bugs: { url: `${REPO}/issues` },
  keywords: [
    "search",
    "full-text-search",
    "fulltext",
    "wasm",
    "webassembly",
    "minisearch",
    "bm25",
    "fuzzy-search",
    "prefix-search",
  ],
  main: "minisearch_wasm.cjs",
  module: "minisearch_wasm.js",
  unpkg: "minisearch_wasm.umd.js",
  jsdelivr: "minisearch_wasm.umd.js",
  dependencies: { minisearch: "7.2.0" },
  exports: {
    ".": {
      require: { types: "./minisearch_wasm.d.cts", default: "./minisearch_wasm.cjs" },
      import: { types: "./minisearch_wasm.d.ts", node: `./${NODE_ENTRY}`, default: "./minisearch_wasm.js" },
      default: "./minisearch_wasm.js",
    },
    "./SearchableMap": {
      require: { types: "./SearchableMap.d.cts", default: "./SearchableMap.cjs" },
      import: { types: "./SearchableMap.d.ts", default: "./SearchableMap.js" },
    },
    "./minisearch_wasm.js": "./minisearch_wasm.js",
    "./minisearch_wasm_bg.wasm": "./minisearch_wasm_bg.wasm",
    "./package.json": "./package.json",
  },
};

// wasm-bindgen 0.2.128 writes nested npm dependency metadata; wasm-pack 0.15
// expects a flat map. --no-pack avoids that incompatible metadata reader, and
// this script owns the complete publish manifest instead.
const root = JSON.parse(readFileSync(resolve(here, "../package.json"), "utf8"));
const pkg = { name: "minisearch-wasm", version: root.version, type: "module", license: "MIT",
  types: "minisearch_wasm.d.ts", sideEffects: ["./minisearch_wasm_node.js", "./minisearch_wasm.js", "./snippets/**"],
  files: ["minisearch_wasm.js", "minisearch_wasm.d.ts", "minisearch_wasm_bg.wasm", "minisearch_wasm_bg.wasm.d.ts"] };

const files = new Set([...(pkg.files ?? []), NODE_ENTRY, "minisearch_wasm_core.js", "minisearch_wasm_core.d.ts", "minisearch_wasm.cjs", "minisearch_wasm.d.cts", "minisearch_wasm.umd.js", "SearchableMap.js", "SearchableMap.d.ts", "SearchableMap.cjs", "SearchableMap.d.cts", ...snippets, "COMPATIBILITY.md", "LICENSE.minisearch.txt"]);
const merged = { ...pkg, ...metadata, files: [...files] };
writeFileSync(pkgPath, JSON.stringify(merged, null, 2) + "\n");

console.log(`finalize-pkg: patched ${pkgPath} (name=${merged.name}, v${merged.version}) and wrote ${NODE_ENTRY}`);
