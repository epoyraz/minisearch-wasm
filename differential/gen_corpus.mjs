// Deterministic synthetic corpus with rich prefix/fuzzy structure.
import { writeFileSync } from 'fs'

let seed = 0x2f6e2b1
const rand = () => {
  // xorshift32
  seed ^= seed << 13; seed >>>= 0
  seed ^= seed >> 17
  seed ^= seed << 5; seed >>>= 0
  return seed / 0x100000000
}
const pick = (arr) => arr[Math.floor(rand() * arr.length)]

const roots = [
  'engine', 'develop', 'manage', 'search', 'index', 'query', 'token', 'parse',
  'cloud', 'data', 'python', 'java', 'react', 'rust', 'wasm', 'kernel',
  'net', 'work', 'soft', 'hard', 'ware', 'test', 'bench', 'mark', 'score',
  'field', 'term', 'match', 'fuzz', 'prefix', 'suggest', 'auto', 'complete',
  'zürich', 'münchen', 'café', 'naïve', 'straße', 'москва', 'tokyo', 'kyoto',
  'graph', 'node', 'edge', 'tree', 'radix', 'trie', 'hash', 'map', 'vector',
  'stream', 'batch', 'shard', 'merge', 'split', 'compact', 'vacuum', 'snapshot'
]
const suffixes = ['', '', '', 's', 'ing', 'ed', 'er', 'ers', 'ation', 'ment', 'ity', 'ify']
const vocab = []
for (const root of roots) for (const suffix of suffixes) vocab.push(root + suffix)
// a few exact short words too
vocab.push('a', 'an', 'the', 'of', 'in', 'on', 'at', 'to', 'and', 'or', 'not')

const words = (count) => Array.from({ length: count }, () => pick(vocab)).join(' ')

const docs = []
const docCount = Number(process.argv[3] || 3000)
for (let i = 0; i < docCount; i++) {
  docs.push({
    id: i,
    title: words(3 + Math.floor(rand() * 6)),
    text: words(15 + Math.floor(rand() * 45))
  })
}

const queries = []
// single-term, multi-term, prefixes, typos
const queryTerms = [
  'engine', 'engineer', 'engineering', 'develop', 'devel', 'eng', 'en',
  'search index', 'query token', 'data cloud', 'rust wasm engine',
  'managment', 'serch', 'pythn', 'javasript', 'kernal', 'nod', 'gra',
  'zürich', 'zurich', 'café', 'cafe', 'straße', 'москва',
  'suggest auto', 'auto compl', 'radix tri', 'hash map vector',
  'test bench mark', 'sco', 'fie', 'term match fuzz', 'the of in',
  'splitting', 'compacted', 'snapshoting', 'merg spli', 'batch shard str'
]
for (const q of queryTerms) queries.push(q)
// wide query (>64 terms, with duplicates) exercising the non-bitmask fallback
queries.push(Array.from({ length: 70 }, (_, i) => vocab[(i * 37) % 40]).join(' '))

writeFileSync(process.argv[2] || 'corpus.json', JSON.stringify({ docs, queries }))
console.log(`corpus: ${docs.length} docs, ${queries.length} queries, vocab ${vocab.length}`)
