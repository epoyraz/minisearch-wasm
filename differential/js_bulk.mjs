// Dump JS MiniSearch results for the synthetic corpus: search (several option
// combos) + autoSuggest, on a fresh index and again after removes/discards.
import MiniSearch from 'minisearch'
import { readFileSync, writeFileSync } from 'fs'

const { docs, queries, treeQueries = [] } = JSON.parse(readFileSync(process.argv[2], 'utf8'))

// Map the harness's `{wildcard: true}` JSON sentinel to the real symbol.
const toQuery = (node) => {
  if (typeof node === 'string') return node
  if (node.wildcard === true) return MiniSearch.wildcard
  return { ...node, queries: node.queries.map(toQuery) }
}

const makeIndex = () => {
  const ms = new MiniSearch({
    fields: ['title', 'text'],
    autoVacuum: false
  })
  ms.addAll(docs)
  return ms
}

const optionCombos = {
  plain: {},
  and: { combineWith: 'AND' },
  prefix: { prefix: true },
  fuzzy: { fuzzy: 0.2 },
  pf_and: { prefix: true, fuzzy: 0.2, combineWith: 'AND' },
  pf_or: { prefix: true, fuzzy: 0.2 }
}

const dumpSearch = (ms, query, options) =>
  ms.search(query, options).map(r => ({ id: r.id, score: r.score, terms: r.terms }))

const dumpSuggest = (ms, query, options) => ms.autoSuggest(query, options)

const dump = (ms) => {
  const out = {}
  for (const query of queries) {
    for (const [name, options] of Object.entries(optionCombos)) {
      out[`s:${name}:${query}`] = dumpSearch(ms, query, options)
    }
    out[`a:default:${query}`] = dumpSuggest(ms, query)
    out[`a:fuzzy:${query}`] = dumpSuggest(ms, query, { fuzzy: 0.2 })
    out[`a:or:${query}`] = dumpSuggest(ms, query, { combineWith: 'OR' })
  }
  // Query trees: ids, scores and the matched terms in order.
  for (const { name, tree } of treeQueries) {
    out[`t:${name}`] = ms.search(toQuery(tree)).map(r =>
      ({ id: r.id, score: r.score, terms: r.terms }))
    out[`tand:${name}`] = ms.search(toQuery(tree), { combineWith: 'AND' }).map(r =>
      ({ id: r.id, score: r.score, terms: r.terms }))
  }
  return out
}

const fresh = dump(makeIndex())

// Mutated index: remove some docs entirely, discard others (dirty index).
const mutated = makeIndex()
for (let i = 0; i < docs.length; i += 7) mutated.remove(docs[i])
for (let i = 3; i < docs.length; i += 11) {
  if (i % 7 !== 0) mutated.discard(docs[i].id)
}
// JS lazily deletes dead postings DURING search and its matchingFields (idf)
// depends on where dead postings sit in iteration order; the wasm port instead
// skips dead postings without mutating, which equals JS's state after the lazy
// cleanup. Dump the SECOND run, i.e. the post-cleanup fixpoint.
dump(mutated)
const afterMutation = dump(mutated)

// Same mutations through the batch APIs.
const batchMutated = makeIndex()
const documentsToRemove = docs.filter((_, index) => index % 7 === 0)
const idsToDiscard = docs
  .filter((_, index) => index % 11 === 3 && index % 7 !== 0)
  .map(document => document.id)
batchMutated.removeAll(documentsToRemove)
batchMutated.discardAll(idsToDiscard)
// As above, record the post-lazy-cleanup fixpoint.
dump(batchMutated)
const afterBatchMutation = dump(batchMutated)

// Explicitly vacuum a dirty index and compare the fully clean state.
const vacuumed = makeIndex()
for (let i = 0; i < docs.length; i += 7) vacuumed.remove(docs[i])
for (let i = 3; i < docs.length; i += 11) {
  if (i % 7 !== 0) vacuumed.discard(docs[i].id)
}
await vacuumed.vacuum({ batchSize: vacuumed.termCount + 1 })
const afterVacuum = dump(vacuumed)

writeFileSync(process.argv[3], JSON.stringify({
  fresh,
  afterMutation,
  afterBatchMutation,
  afterVacuum
}))
console.log('js dump written:', Object.keys(fresh).length, 'labels per phase')
