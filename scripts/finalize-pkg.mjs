// Post-build step for the npm package.
//
// `wasm-pack build` regenerates pkg/package.json from Cargo.toml on every run,
// emitting only a minimal manifest (name, version, files, main, types). This
// script merges in the metadata npm needs for a real published package —
// repository, homepage, keywords, author — so we never hand-edit the generated
// file. Run it right after `wasm-pack build` (see the npm `build` script).

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const pkgPath = resolve(here, "..", "pkg", "package.json");

const REPO = "https://github.com/epoyraz/minisearch-wasm";

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
};

const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
const merged = { ...pkg, ...metadata };
writeFileSync(pkgPath, JSON.stringify(merged, null, 2) + "\n");

console.log(`finalize-pkg: patched ${pkgPath} (name=${merged.name}, v${merged.version})`);
