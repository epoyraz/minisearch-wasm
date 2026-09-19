// Reference values of Math.log for tests/js_math.rs: one line per input, the
// bits of x and of Math.log(x) in hexadecimal. Covers the arguments BM25's
// inverse document frequency really takes, 1 + (N - n + 0.5) / (n + 0.5), a
// spread of magnitudes, values next to 1, and the special cases.
import { writeFileSync } from "node:fs";

let seed = 99;
const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
const inputs = [];
for (let N = 1; N <= 40; N++) for (let n = 1; n <= N + 4; n++) inputs.push(1 + (N - n + 0.5) / (n + 0.5));
for (let i = 0; i < 300; i++) inputs.push(Math.exp((random() - 0.5) * 1400), 1 + (random() - 0.5) * 1e-5, random() * 1e6 + 1);
inputs.push(0, -0, -1, 1, 2, 0.5, Infinity, -Infinity, NaN, 5e-324, 2.2250738585072014e-308, 1.7976931348623157e308, 1 - 2 ** -53, 1 + 2 ** -52);
const bits = x => new BigUint64Array(new Float64Array([x]).buffer)[0].toString(16).padStart(16, "0");
writeFileSync(new URL("../tests/fixtures/math_log.txt", import.meta.url), inputs.map(x => `${bits(x)} ${bits(Math.log(x))}`).join("\n") + "\n");
console.log(`wrote ${inputs.length} reference values (V8 ${process.versions.v8})`);
