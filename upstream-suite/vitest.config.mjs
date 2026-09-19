// Runs MiniSearch's own test files (the byte-identical copies under
// reference-tests/) against this package instead of against MiniSearch:
//
//   npm run test:upstream                      # the public facade in pkg/
//   UPSTREAM_SUITE_TARGET=minisearch npm run test:upstream   # control run
//
// The files are not modified; their imports are redirected while they load.
import { fileURLToPath } from 'node:url';
import { resolve, dirname } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const control = process.env.UPSTREAM_SUITE_TARGET === 'minisearch';
const targets = control
  ? { './MiniSearch': resolve(root, 'node_modules/minisearch/dist/es/index.js'), './SearchableMap': resolve(root, 'node_modules/minisearch/dist/es/SearchableMap.js') }
  : { './MiniSearch': resolve(root, 'pkg/minisearch_wasm_node.js'), './SearchableMap': resolve(root, 'pkg/SearchableMap.js') };

export default {
  root,
  plugins: [{
    name: 'redirect-reference-test-imports',
    enforce: 'pre',
    resolveId(source, importer) {
      if (importer?.includes('/reference-tests/') && targets[source]) return targets[source];
    },
    // The files use Jest's globals; give them this suite's instead.
    transform(code, id) {
      if (!id.includes('/reference-tests/')) return;
      return { code: `import { describe, it, test, jest } from ${JSON.stringify(resolve(root, 'upstream-suite/harness.mjs'))};${code}`, map: null };
    },
  }],
  test: {
    globals: true,
    include: ['reference-tests/src/**/*.test.js'],
    testTimeout: 20000,
  },
};
