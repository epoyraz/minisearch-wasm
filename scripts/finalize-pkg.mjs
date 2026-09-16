// Post-build step for the npm package.
//
// `wasm-pack build` regenerates pkg/package.json from Cargo.toml on every run,
// emitting only a minimal manifest (name, version, files, main, types). This
// script merges in the metadata npm needs for a real published package —
// repository, homepage, keywords, author — and adds the Node entry point, so we
// never hand-edit the generated files. Run it right after `wasm-pack build`
// (see the npm `build` script).
//
// Node entry: the `--target web` loader fetches its `.wasm` by URL, which Node
// cannot do for `file:` URLs, so `minisearch_wasm_node.js` reads the bytes
// itself when `init()` is called without an argument. Conditional exports send
// Node there and every other environment to the web loader, so browser bundles
// never see a `node:` import. The generated `.d.ts` describes both entries.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const pkgDir = resolve(here, "..", "pkg");
const pkgPath = resolve(pkgDir, "package.json");

const REPO = "https://github.com/epoyraz/minisearch-wasm";

const NODE_ENTRY = "minisearch_wasm_node.js";
const nodeEntry = `// Node entry point (selected by the "node" export condition).
// The web-target loader fetches the .wasm by URL, which Node cannot do for
// file: URLs; read the packaged bytes instead when no module is given.
import { readFileSync } from "node:fs";
import init from "./minisearch_wasm.js";
export * from "./minisearch_wasm.js";

export default function initNode(module_or_path) {
  if (module_or_path === undefined) {
    module_or_path = {
      module_or_path: readFileSync(new URL("./minisearch_wasm_bg.wasm", import.meta.url)),
    };
  }
  return init(module_or_path);
}
`;
writeFileSync(resolve(pkgDir, NODE_ENTRY), nodeEntry);

const metadata = {
  description:
    "Fast WebAssembly full-text search engine, API-compatible with MiniSearch. Identical BM25 rankings, faster on real workloads.",
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
  exports: {
    ".": {
      types: "./minisearch_wasm.d.ts",
      node: `./${NODE_ENTRY}`,
      default: "./minisearch_wasm.js",
    },
    "./minisearch_wasm.js": "./minisearch_wasm.js",
    "./minisearch_wasm_bg.wasm": "./minisearch_wasm_bg.wasm",
    "./package.json": "./package.json",
  },
};

const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
const files = new Set([...(pkg.files ?? []), NODE_ENTRY]);
const merged = { ...pkg, ...metadata, files: [...files] };
writeFileSync(pkgPath, JSON.stringify(merged, null, 2) + "\n");

console.log(`finalize-pkg: patched ${pkgPath} (name=${merged.name}, v${merged.version}) and wrote ${NODE_ENTRY}`);
