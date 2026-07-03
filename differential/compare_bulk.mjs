// Tie-tolerant differential comparison of JS vs Rust bulk dumps.
// - id sets must match; per-id score (rel 1e-12) and terms (exact order) must match
// - result ORDER must match except inside near-tie score bands (rel 1e-9),
//   where the band's members must match as a set
import { readFileSync } from 'fs'

const js = JSON.parse(readFileSync(process.argv[2], 'utf8'))
const rust = JSON.parse(readFileSync(process.argv[3], 'utf8'))

let failures = 0
let labels = 0
let rows = 0
let maxScoreDelta = 0
let tieReorders = 0

const rel = (a, b) => Math.abs(a - b) / Math.max(Math.abs(a), Math.abs(b), 1e-12)

const keyOf = (row) => 'suggestion' in row ? row.suggestion : String(row.id)

const compareLabel = (label, a, b) => {
  labels++
  if (a.length !== b.length) {
    console.log(`FAIL ${label}: length ${a.length} vs ${b.length}`)
    return false
  }
  rows += a.length

  // per-key lookups
  const bByKey = new Map(b.map(r => [keyOf(r), r]))
  if (bByKey.size !== b.length) { /* duplicate keys impossible for ids/phrases */ }
  for (const ra of a) {
    const rb = bByKey.get(keyOf(ra))
    if (!rb) {
      console.log(`FAIL ${label}: key ${keyOf(ra)} missing in rust`)
      return false
    }
    const delta = rel(ra.score, rb.score)
    maxScoreDelta = Math.max(maxScoreDelta, delta)
    if (delta > 1e-12) {
      console.log(`FAIL ${label}: score for ${keyOf(ra)}: ${ra.score} vs ${rb.score}`)
      return false
    }
    if (JSON.stringify(ra.terms) !== JSON.stringify(rb.terms)) {
      console.log(`FAIL ${label}: terms for ${keyOf(ra)}: ${JSON.stringify(ra.terms)} vs ${JSON.stringify(rb.terms)}`)
      return false
    }
  }

  // order comparison with tie bands (grouped on JS scores)
  let i = 0
  while (i < a.length) {
    let j = i + 1
    while (j < a.length && rel(a[i].score, a[j].score) <= 1e-9) j++
    // band [i, j): compare as sets
    const setA = new Set(a.slice(i, j).map(keyOf))
    const setB = new Set(b.slice(i, j).map(keyOf))
    if (setA.size !== setB.size || [...setA].some(k => !setB.has(k))) {
      console.log(`FAIL ${label}: order band [${i},${j}) differs`)
      console.log('  js:  ', [...setA].slice(0, 12))
      console.log('  rust:', [...setB].slice(0, 12))
      return false
    }
    let identical = true
    for (let k = i; k < j; k++) if (keyOf(a[k]) !== keyOf(b[k])) identical = false
    if (!identical) tieReorders++
    i = j
  }
  return true
}

for (const phase of ['fresh', 'afterMutation', 'afterBatchMutation', 'afterVacuum']) {
  const pa = js[phase], pb = rust[phase]
  const keys = new Set([...Object.keys(pa), ...Object.keys(pb)])
  for (const key of keys) {
    if (!pa[key] || !pb[key]) {
      console.log(`FAIL ${phase}/${key}: missing on one side`)
      failures++
      continue
    }
    if (!compareLabel(`${phase}/${key}`, pa[key], pb[key])) failures++
  }
}

console.log(`labels: ${labels}, rows: ${rows}, max score rel delta: ${maxScoreDelta}, tie-band reorders: ${tieReorders}`)
console.log(failures === 0 ? 'BULK DIFFERENTIAL: ALL MATCH' : `${failures} FAILURES`)
process.exit(failures === 0 ? 0 : 1)
