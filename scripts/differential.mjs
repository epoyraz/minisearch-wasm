// The native differential suites: identical corpora and queries through JS
// MiniSearch and the native Rust engine, compared row by row. Needs cargo, not
// a Wasm build. Dumps go to target/differential/.
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dir = resolve(root, "target/differential");
mkdirSync(dir, { recursive: true });
const file = name => resolve(dir, name);
const run = (command, args, { capture } = {}) => {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8", stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit", maxBuffer: 1 << 30 });
  if (result.status !== 0) process.exit(result.status ?? 1);
  if (capture) writeFileSync(capture, result.stdout);
};
const node = (script, ...args) => run(process.execPath, [resolve(root, "differential", script), ...args]);

node("gen_corpus.mjs", file("corpus.json"));
node("js_bulk.mjs", file("corpus.json"), file("js_bulk.json"));
run("cargo", ["run", "--locked", "--release", "--example", "dump_bulk", file("corpus.json"), file("rust_bulk.json")]);
node("compare_bulk.mjs", file("js_bulk.json"), file("rust_bulk.json"), ...process.argv.slice(2));

run(process.execPath, [resolve(root, "differential/js_autosuggest.mjs")], { capture: file("js_autosuggest.json") });
run("cargo", ["run", "--locked", "--example", "dump_autosuggest"], { capture: file("rust_autosuggest.json") });
node("compare.mjs", file("js_autosuggest.json"), file("rust_autosuggest.json"));
