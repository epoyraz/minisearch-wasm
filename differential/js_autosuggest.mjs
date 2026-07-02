import MiniSearch from 'minisearch'

const italian = [
  { id: 1, title: 'Divina Commedia', text: 'Nel mezzo del cammin di nostra vita', category: 'poetry' },
  { id: 2, title: 'I Promessi Sposi', text: 'Quel ramo del lago di Como', category: 'fiction' },
  { id: 3, title: 'Vita Nova', text: 'In quella parte del libro della mia memoria', category: 'poetry' }
]

const books = [
  { id: 1, title: 'Moby Dick', text: 'Call me Ishmael. Some years ago...', category: 'fiction' },
  { id: 2, title: 'Zen and the Art of Motorcycle Maintenance', text: 'I can see by my watch...', category: 'fiction' },
  { id: 3, title: 'Neuromancer', text: 'The sky above the port was...', category: 'fiction' },
  { id: 4, title: 'Zen and the Art of Archery', text: 'At first sight it must seem...', category: 'non-fiction' }
]

const out = {}

const run = (label, ms, query, options) => {
  out[label] = ms.autoSuggest(query, options)
}

{
  const ms = new MiniSearch({ fields: ['title', 'text'], storeFields: ['category'] })
  ms.addAll(italian)
  run('it:com', ms, 'com')
  run('it:vita no', ms, 'vita no')
  run('it:nostra vi', ms, 'nostra vi')
  run('it:vita', ms, 'vita')
  run('it:del', ms, 'del')
  run('it:de', ms, 'de')
  run('it:d', ms, 'd')
  run('it:quel', ms, 'quel')
  run('it:vita-fuzzy-prefix', ms, 'vita', { fuzzy: true, prefix: true })
  run('it:del la-OR', ms, 'del la', { combineWith: 'OR' })
  run('it:della memoria', ms, 'della memoria')
  run('it:memoria-fuzzy02', ms, 'memoia', { fuzzy: 0.2 })
  run('it:boost-title', ms, 'vita', { boost: { title: 2 } })
  run('it:weights', ms, 'com', { weights: { prefix: 0.2, fuzzy: 0.9 } })
  run('it:fields-title', ms, 'vita', { fields: ['title'] })
}

{
  const ms = new MiniSearch({
    fields: ['title', 'text'],
    autoSuggestOptions: { combineWith: 'OR', fuzzy: true }
  })
  ms.addAll(italian)
  run('ctor-suggest:nosta vi', ms, 'nosta vi')
  run('ctor-suggest:com', ms, 'com')
}

{
  const ms = new MiniSearch({
    fields: ['title', 'text'],
    searchOptions: { combineWith: 'OR', fuzzy: true }
  })
  ms.addAll(italian)
  run('ctor-search:nosta vi', ms, 'nosta vi')
}

{
  const ms = new MiniSearch({ fields: ['title', 'text'], storeFields: ['category'] })
  ms.addAll(books)
  run('bk:zen ar', ms, 'zen ar')
  run('bk:zen ar-OR', ms, 'zen ar', { combineWith: 'OR' })
  run('bk:moto', ms, 'moto')
  run('bk:nromancer-fuzzy', ms, 'nromancer', { fuzzy: 0.2 })
  run('bk:the', ms, 'the')
  run('bk:zen art mot', ms, 'zen art mot')
  run('bk:a', ms, 'a')
}

console.log(JSON.stringify(out, null, 1))
