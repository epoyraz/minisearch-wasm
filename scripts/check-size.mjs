// Size budget for the published files. A change that grows one of them past
// its budget fails here; raise the budget in scripts/size-budget.json in the
// same change, so that growth is a decision and not an accident.
//
//   node scripts/check-size.mjs            # check pkg/ against the budget
//   node scripts/check-size.mjs --update   # record the current sizes plus 5%

import { readFileSync, writeFileSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const budgetPath = resolve(root, "scripts/size-budget.json");
const files = ["minisearch_wasm_bg.wasm", "minisearch_wasm.js", "minisearch_wasm_core.js", "minisearch_wasm.cjs", "minisearch_wasm.umd.js"];
const sizes = Object.fromEntries(files.map(name => {
  const path = resolve(root, "pkg", name);
  return [name, { bytes: statSync(path).size, gzip: gzipSync(readFileSync(path), { level: 9 }).length }];
}));

if (process.argv.includes("--update")) {
  const budget = Object.fromEntries(Object.entries(sizes).map(([name, { bytes, gzip }]) => [name, { bytes: Math.ceil(bytes * 1.05), gzip: Math.ceil(gzip * 1.05) }]));
  writeFileSync(budgetPath, JSON.stringify(budget, null, 2) + "\n");
  console.log(`check-size: recorded budgets in ${budgetPath}`);
} else {
  const budget = JSON.parse(readFileSync(budgetPath, "utf8"));
  let failed = false;
  for (const [name, size] of Object.entries(sizes)) {
    for (const unit of ["bytes", "gzip"]) {
      const over = size[unit] > budget[name][unit];
      failed ||= over;
      console.log(`${over ? "OVER" : "ok  "} ${name} ${unit}: ${size[unit].toLocaleString("en")} of ${budget[name][unit].toLocaleString("en")}`);
    }
  }
  if (failed) { console.error("check-size: over budget; shrink the file or raise its budget deliberately (--update)"); process.exit(1); }
}
