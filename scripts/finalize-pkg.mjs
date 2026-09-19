// Assembles the publishable npm package.
//
// wasm-pack --no-pack writes the core glue and the .wasm file to a glue
// directory (target/wasm-glue). This script builds pkg/ from that glue and the
// repository's sources: the public facade, browser/Node/CommonJS entries,
// declarations, documentation and the npm manifest. pkg/ is created from
// scratch every time and nothing is read from it, so running the script again
// (with or without a new Wasm build) gives the same package.
//
//   node scripts/finalize-pkg.mjs [--glue <dir>] [--out <dir>]

import { readFileSync, writeFileSync, copyFileSync, mkdirSync, rmSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { build } from "esbuild";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const option = (name, fallback) => {
  const at = process.argv.indexOf(name);
  return resolve(root, at === -1 ? fallback : process.argv[at + 1]);
};
const glueDir = option("--glue", "target/wasm-glue");
const pkgDir = option("--out", "pkg");
const fail = message => { console.error(`finalize-pkg: ${message}`); process.exit(1); };

const REPO = "https://github.com/epoyraz/minisearch-wasm";
const manifest = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8"));
const upstream = JSON.parse(readFileSync(resolve(root, "node_modules/minisearch/package.json"), "utf8"));
const upstreamLicense = readFileSync(resolve(root, "node_modules/minisearch/LICENSE.txt"), "utf8");
const licenseBanner = `/*! Bundles MiniSearch ${upstream.version}.\n${upstreamLicense.trim()}\n*/`;

// Generated glue imports "__wbg_" functions; the facade never does. Refuse
// anything else, or the facade ends up wrapping itself.
const gluePath = resolve(glueDir, "minisearch_wasm.js");
if (!existsSync(gluePath) || !readFileSync(gluePath, "utf8").includes("__wbg_")) {
  fail(`no wasm-bindgen glue in ${glueDir}; run "npm run build"`);
}

rmSync(pkgDir, { recursive: true, force: true });
mkdirSync(pkgDir, { recursive: true });
const out = name => resolve(pkgDir, name);
const write = (name, content) => { mkdirSync(dirname(out(name)), { recursive: true }); writeFileSync(out(name), content); };

// The generated glue keeps its own name; the public module is the facade.
const coreGlue = readFileSync(gluePath, "utf8")
  .replace('@ts-self-types="./minisearch_wasm.d.ts"', '@ts-self-types="./minisearch_wasm_core.d.ts"');
write("minisearch_wasm_core.js", coreGlue);
copyFileSync(resolve(glueDir, "minisearch_wasm.d.ts"), out("minisearch_wasm_core.d.ts"));
for (const name of ["minisearch_wasm_bg.wasm", "minisearch_wasm_bg.wasm.d.ts"]) copyFileSync(resolve(glueDir, name), out(name));
// Incremental builds leave older hashed snippet directories behind: ship only
// the ones this build's glue imports.
const snippets = [...coreGlue.matchAll(/from ['"]\.\/(snippets\/[^'"]+)['"]/g)].map(match => match[1]);
for (const name of snippets) write(name, readFileSync(resolve(glueDir, name)));

for (const name of ["README.md", "LICENSE.txt", "COMPATIBILITY.md"]) copyFileSync(resolve(root, name), out(name));
write("LICENSE.minisearch.txt", upstreamLicense);

// Declarations. Upstream's are self-contained, so they ship with the package
// and MiniSearch does not have to be installed next to it.
const upstreamTypes = name => readFileSync(resolve(root, "node_modules/minisearch/dist/es", name), "utf8");
write("minisearch.d.ts", upstreamTypes("index.d.ts"));
write("SearchableMap.d.ts", upstreamTypes("SearchableMap.d.ts"));
const facadeTypes = readFileSync(resolve(root, "js/minisearch.d.ts"), "utf8").replaceAll("'minisearch'", "'./minisearch.js'");
write("minisearch_wasm.d.ts", facadeTypes);
// CommonJS declarations must themselves be CommonJS under NodeNext resolution.
// `export =` carries the constructor; a merged namespace carries every named
// type, so `import type { SearchResult } from "minisearch-wasm"` works there too.
const esmTypes = name => `import("./${name}.js", { with: { "resolution-mode": "import" } })`;
const exportedTypes = new Map();
for (const [, names] of facadeTypes.matchAll(/^export type \{([^}]+)\}/gm)) for (const name of names.split(",")) exportedTypes.set(name.trim(), false);
for (const [, name, generic] of facadeTypes.matchAll(/^export (?:type|interface) (\w+)(<T = any>)?/gm)) exportedTypes.set(name, !!generic);
const upstreamGenerics = new Set([...upstreamTypes("index.d.ts").matchAll(/^(?:export )?(?:declare )?(?:type|interface) (\w+)<T = any>/gm)].map(match => match[1]));
exportedTypes.set("MiniSearchWasm", true);
const namespaceTypes = [...exportedTypes].map(([name, generic]) => generic || upstreamGenerics.has(name)
  ? `  export type ${name}<T = any> = ${esmTypes("minisearch_wasm")}.${name}<T>;`
  : `  export type ${name} = ${esmTypes("minisearch_wasm")}.${name};`);
write("minisearch_wasm.d.cts", `declare const MiniSearch: typeof ${esmTypes("minisearch_wasm")}.default;
type MiniSearch<T = any> = ${esmTypes("minisearch_wasm")}.MiniSearchWasm<T>;
declare namespace MiniSearch {
${namespaceTypes.join("\n")}
}
export = MiniSearch;
`);
write("SearchableMap.d.cts", `declare const SearchableMap: typeof ${esmTypes("SearchableMap")}.default;
type SearchableMap<T = any> = ${esmTypes("SearchableMap")}.default<T>;
export = SearchableMap;
`);

const bundle = async options => (await build({ bundle: true, write: false, absWorkingDir: root, logLevel: "warning", ...options })).outputFiles[0].text;

// Plain browser ESM must not require an import map for the compatibility engine.
// Keep core glue external so its relative Wasm URL and snippets stay intact.
write("minisearch_wasm.js", await bundle({ stdin: { contents: readFileSync(resolve(root, "js/minisearch.js"), "utf8"), resolveDir: resolve(root, "js"), sourcefile: "minisearch.js" },
  platform: "browser", format: "esm", target: "es2022", banner: { js: licenseBanner }, external: ["./minisearch_wasm_core.js"] }));
write("SearchableMap.js", await bundle({ stdin: { contents: 'export { default } from "minisearch/SearchableMap";\n', resolveDir: root },
  platform: "browser", format: "esm", target: "es2022", banner: { js: licenseBanner } }));

// Node initializes synchronously from the packaged bytes. Where they cannot be
// read (the package was bundled without its .wasm file) the facade still works,
// on its JavaScript engine: say so once instead of failing the import.
const NODE_ENTRY = "minisearch_wasm_node.js";
write(NODE_ENTRY, `import { readFileSync } from "node:fs";
import MiniSearch, { initSync } from "./minisearch_wasm.js";
try {
  initSync({ module: readFileSync(new URL("./minisearch_wasm_bg.wasm", import.meta.url)) });
} catch (error) {
  console.warn(\`minisearch-wasm: minisearch_wasm_bg.wasm could not be loaded (\${error.message}); using the JavaScript engine. Ship the file next to the bundle, or call initSync({ module: bytes }) yourself.\`);
}
export * from "./minisearch_wasm.js";
export default MiniSearch;
`);
write("minisearch_wasm.cjs", await bundle({ entryPoints: [out(NODE_ENTRY)], platform: "node", format: "cjs", target: "node18",
  define: { "import.meta.url": "__minisearchModuleUrl" },
  banner: { js: 'const __minisearchModuleUrl = require("node:url").pathToFileURL(__filename).href;' },
  footer: { js: "module.exports = Object.assign(module.exports.default, module.exports);" } }));
write("SearchableMap.cjs", await bundle({ entryPoints: [out("SearchableMap.js")], platform: "node", format: "cjs",
  footer: { js: "module.exports = module.exports.default;" } }));

// Browser global: everything lives inside one function scope, so the script
// defines MiniSearch and nothing else, and may be loaded more than once.
write("global-entry.js", `import MiniSearch, * as named from "./minisearch_wasm.js";
globalThis.MiniSearch = Object.assign(MiniSearch, named);
`);
write("global-url.js", `export const __minisearchScriptUrl = typeof document !== "undefined" && document.currentScript ? document.currentScript.src : globalThis.location?.href;
`);
write("minisearch_wasm.umd.js", await bundle({ entryPoints: [out("global-entry.js")], platform: "browser", format: "iife", target: "es2022", minify: true,
  define: { "import.meta.url": "__minisearchScriptUrl" }, inject: [out("global-url.js")], banner: { js: licenseBanner } }));
for (const name of ["global-entry.js", "global-url.js"]) rmSync(out(name));

// wasm-bindgen 0.2.128 writes nested npm dependency metadata; wasm-pack 0.15
// expects a flat map. --no-pack avoids that incompatible metadata reader, and
// this script owns the complete publish manifest instead.
const entry = (types, file) => ({ types: `./${types}`, default: `./${file}` });
write("package.json", JSON.stringify({
  name: "minisearch-wasm",
  version: manifest.version,
  description: "WebAssembly full-text search with a MiniSearch-compatible JavaScript fallback for callbacks and value semantics.",
  license: "MIT",
  author: "Enes Poyraz",
  repository: { type: "git", url: `git+${REPO}.git` },
  homepage: `${REPO}#readme`,
  bugs: { url: `${REPO}/issues` },
  keywords: ["search", "full-text-search", "fulltext", "wasm", "webassembly", "minisearch", "bm25", "fuzzy-search", "prefix-search"],
  type: "module",
  engines: manifest.engines,
  main: "minisearch_wasm.cjs",
  module: "minisearch_wasm.js",
  types: "minisearch_wasm.d.ts",
  unpkg: "minisearch_wasm.umd.js",
  jsdelivr: "minisearch_wasm.umd.js",
  sideEffects: ["./minisearch_wasm_node.js", "./minisearch_wasm.cjs", "./minisearch_wasm.js", "./minisearch_wasm.umd.js", "./snippets/**"],
  exports: {
    ".": {
      // Bundlers for the browser take the fetch-based entry whether the
      // importing code says import or require; only Node reads the file system.
      browser: entry("minisearch_wasm.d.ts", "minisearch_wasm.js"),
      require: entry("minisearch_wasm.d.cts", "minisearch_wasm.cjs"),
      import: { types: "./minisearch_wasm.d.ts", node: `./${NODE_ENTRY}`, default: "./minisearch_wasm.js" },
      default: entry("minisearch_wasm.d.ts", "minisearch_wasm.js"),
    },
    "./SearchableMap": {
      require: entry("SearchableMap.d.cts", "SearchableMap.cjs"),
      import: entry("SearchableMap.d.ts", "SearchableMap.js"),
      default: entry("SearchableMap.d.ts", "SearchableMap.js"),
    },
    "./minisearch_wasm.js": entry("minisearch_wasm.d.ts", "minisearch_wasm.js"),
    "./minisearch_wasm_bg.wasm": "./minisearch_wasm_bg.wasm",
    "./package.json": "./package.json",
  },
  files: ["minisearch_wasm.js", "minisearch_wasm.d.ts", "minisearch_wasm.d.cts", "minisearch_wasm.cjs", "minisearch_wasm.umd.js", NODE_ENTRY,
    "minisearch_wasm_core.js", "minisearch_wasm_core.d.ts", "minisearch_wasm_bg.wasm", "minisearch_wasm_bg.wasm.d.ts", "minisearch.d.ts",
    "SearchableMap.js", "SearchableMap.d.ts", "SearchableMap.cjs", "SearchableMap.d.cts", ...snippets,
    "README.md", "COMPATIBILITY.md", "LICENSE.txt", "LICENSE.minisearch.txt"],
}, null, 2) + "\n");

console.log(`finalize-pkg: assembled ${pkgDir} (minisearch-wasm ${manifest.version}) from ${glueDir}`);
