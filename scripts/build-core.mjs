// Builds the engine test package, target/pkg-core: the same sources as pkg/,
// with the engine's own JavaScript API (Cargo feature `core-api`), which the
// public facade does not call and the published package leaves out. The
// engine suites in differential/ drive that API against MiniSearch.
//
// A target directory of its own keeps both builds cached.
//
//   node scripts/build-core.mjs

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const run = (command, args, options = {}) =>
  execFileSync(command, args, { cwd: root, stdio: "inherit", ...options });

// A shell finds wasm-pack's launcher on Windows too.
run("wasm-pack", ["build", "--target", "web", "--release", "--no-pack", "--out-dir", "target/wasm-glue-core", "--", "--features", "core-api"],
  { shell: process.platform === "win32", env: { ...process.env, CARGO_TARGET_DIR: resolve(root, "target/core-api") } });
run(process.execPath, [resolve(root, "scripts/finalize-pkg.mjs"), "--glue", "target/wasm-glue-core", "--out", "target/pkg-core"]);
