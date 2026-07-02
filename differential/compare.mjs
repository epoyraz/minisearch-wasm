import { readFileSync } from 'fs'

const js = JSON.parse(readFileSync(process.argv[2], 'utf8'))
const rust = JSON.parse(readFileSync(process.argv[3], 'utf8'))

let failures = 0
const keys = new Set([...Object.keys(js), ...Object.keys(rust)])
for (const key of keys) {
  const a = js[key]
  const b = rust[key]
  if (!a || !b) {
    console.log(`FAIL ${key}: missing on one side (js=${!!a} rust=${!!b})`)
    failures++
    continue
  }
  if (a.length !== b.length) {
    console.log(`FAIL ${key}: length ${a.length} vs ${b.length}`)
    console.log('  js:  ', JSON.stringify(a))
    console.log('  rust:', JSON.stringify(b))
    failures++
    continue
  }
  for (let i = 0; i < a.length; i++) {
    const sa = a[i], sb = b[i]
    const scoreDelta = Math.abs(sa.score - sb.score) / Math.max(Math.abs(sa.score), 1e-12)
    if (
      sa.suggestion !== sb.suggestion ||
      JSON.stringify(sa.terms) !== JSON.stringify(sb.terms) ||
      scoreDelta > 1e-12
    ) {
      console.log(`FAIL ${key}[${i}]:`)
      console.log('  js:  ', JSON.stringify(sa))
      console.log('  rust:', JSON.stringify(sb))
      failures++
    }
  }
}
console.log(failures === 0 ? `ALL ${keys.size} LABELS MATCH (suggestions, terms, scores to 1e-12)` : `${failures} FAILURES across ${keys.size} labels`)
process.exit(failures === 0 ? 0 : 1)
