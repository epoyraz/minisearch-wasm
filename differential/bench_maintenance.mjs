// Maintenance-path benchmark against JS MiniSearch: addAllAsync overhead and
// vacuum cleanup latency/space reclamation through the built Wasm package.
//
// Run after `npm run build`:
//   node differential/bench_maintenance.mjs
// Optional: BENCH_DOCS=10000 BENCH_CHUNK=500 BENCH_VACUUM_BATCH=1000

import { readFileSync } from 'node:fs'
import { performance } from 'node:perf_hooks'
import MiniSearch from 'minisearch'
import init, { MiniSearchWasm } from '../pkg/minisearch_wasm.js'

const documentCount = Number(process.env.BENCH_DOCS ?? 5000)
const chunkSize = Number(process.env.BENCH_CHUNK ?? 500)
const vacuumBatchSize = Number(process.env.BENCH_VACUUM_BATCH ?? 1000)

const documents = Array.from({ length: documentCount }, (_, id) => ({
  id,
  title: `Role ${id % 97} specialty ${id}`,
  text: `Software platform cloud data engineering token${id} group${id % 211}`
}))
const discardedIds = documents
  .filter((_, index) => index % 4 === 0)
  .map(document => document.id)

const options = {
  fields: ['title', 'text'],
  autoVacuum: false
}

const milliseconds = value => `${value.toFixed(2)}ms`
const megabytes = value => `${(value / 1024 / 1024).toFixed(2)} MiB`

const measure = (label, operation) => {
  const start = performance.now()
  const value = operation()
  const duration = performance.now() - start
  console.log(`${label.padEnd(34)} ${milliseconds(duration)}`)
  return { value, duration }
}

const measureAsync = async (label, operation) => {
  const start = performance.now()
  const value = await operation()
  const duration = performance.now() - start
  console.log(`${label.padEnd(34)} ${milliseconds(duration)}`)
  return { value, duration }
}

await init({
  module_or_path: readFileSync(new URL('../pkg/minisearch_wasm_bg.wasm', import.meta.url))
})

console.log('MiniSearch maintenance benchmark')
console.log(`documents=${documentCount}, async chunk=${chunkSize}, discarded=${discardedIds.length}, vacuum batch=${vacuumBatchSize}`)

console.log('\nIndexing')
const jsSync = measure('JS addAll', () => {
  const search = new MiniSearch(options)
  search.addAll(documents)
  return search
})
const wasmSync = measure('Wasm addAll', () => {
  const search = new MiniSearchWasm(options)
  search.addAll(documents)
  return search
})
const jsAsync = await measureAsync('JS addAllAsync', async () => {
  const search = new MiniSearch(options)
  await search.addAllAsync(documents, { chunkSize })
  return search
})
const wasmAsync = await measureAsync('Wasm addAllAsync', async () => {
  const search = new MiniSearchWasm(options)
  await search.addAllAsync(documents, { chunkSize })
  return search
})

console.log(`  JS async/sync overhead:   ${(jsAsync.duration / jsSync.duration).toFixed(2)}x`)
console.log(`  Wasm async/sync overhead: ${(wasmAsync.duration / wasmSync.duration).toFixed(2)}x`)

console.log('\nVacuum')
jsSync.value.discardAll(discardedIds)
wasmSync.value.discardAll(discardedIds)
const jsDirtyBytes = Buffer.byteLength(JSON.stringify(jsSync.value))
const wasmDirtyBytes = wasmSync.value.toBytes().byteLength

const jsVacuum = await measureAsync('JS vacuum', () =>
  jsSync.value.vacuum({ batchSize: vacuumBatchSize, batchWait: 1 }))
const wasmVacuum = await measureAsync('Wasm vacuum', () =>
  wasmSync.value.vacuum({ batchSize: vacuumBatchSize, batchWait: 1 }))
const jsCleanBytes = Buffer.byteLength(JSON.stringify(jsSync.value))
const wasmCleanBytes = wasmSync.value.toBytes().byteLength

console.log(`  JS snapshot:   ${megabytes(jsDirtyBytes)} -> ${megabytes(jsCleanBytes)} (${((1 - jsCleanBytes / jsDirtyBytes) * 100).toFixed(1)}% reclaimed)`)
console.log(`  Wasm snapshot: ${megabytes(wasmDirtyBytes)} -> ${megabytes(wasmCleanBytes)} (${((1 - wasmCleanBytes / wasmDirtyBytes) * 100).toFixed(1)}% reclaimed)`)
console.log(`  vacuum latency ratio JS/Wasm: ${(jsVacuum.duration / wasmVacuum.duration).toFixed(2)}x`)

const jsIds = jsSync.value.search('token42').map(result => result.id)
const wasmIds = wasmSync.value.search('token42').map(result => result.id)
if (JSON.stringify(jsIds) !== JSON.stringify(wasmIds)) {
  throw new Error('post-vacuum search result order differs')
}
if (jsSync.value.dirtCount !== 0 || wasmSync.value.dirtCount !== 0) {
  throw new Error('vacuum did not reset dirtCount')
}
if (jsAsync.value.documentCount !== documentCount || wasmAsync.value.documentCount !== documentCount) {
  throw new Error('addAllAsync did not index every document')
}

console.log('\nMAINTENANCE BENCHMARK: VERIFIED')

wasmSync.value.free()
wasmAsync.value.free()
