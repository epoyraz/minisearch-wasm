// Jest's API for the reference tests, and the list of tests that cannot hold
// for this package. A listed test must fail (`it.fails`): one that starts to
// pass, or is no longer found, fails the run, so the list cannot go stale.
import { vi, describe as realDescribe, it as realIt, afterAll } from 'vitest';
import { expectedFailures } from './expected-failures.mjs';

const control = process.env.UPSTREAM_SUITE_TARGET === 'minisearch';
const path = [], seen = new Set();
// A nested suite's body runs after the enclosing body has returned, so the
// names are captured when the suite is declared.
const describeWith = register => (name, body) => {
  const names = [...path, name];
  return register(name, () => {
    const outer = path.splice(0, path.length, ...names);
    try { body(); } finally { path.splice(0, path.length, ...outer); }
  });
};
const itWith = register => (name, ...rest) => {
  const full = [...path, name].join(' > ');
  seen.add(full);
  return (!control && expectedFailures.has(full) ? realIt.fails : register)(name, ...rest);
};

export const jest = vi;
export const describe = Object.assign(describeWith(realDescribe), { only: describeWith(realDescribe.only), skip: describeWith(realDescribe.skip) });
export const it = Object.assign(itWith(realIt), { only: itWith(realIt.only), skip: realIt.skip });
export const test = it;

afterAll(() => {
  if (control || !seen.has('MiniSearch > constructor > initializes the attributes')) return;
  const missing = [...expectedFailures.keys()].filter(name => !seen.has(name));
  if (missing.length) throw new Error(`expected failures that no longer exist:\n${missing.join('\n')}`);
});
