// The public compatibility boundary. The Rust engine indexes and scores; this
// side keeps what MiniSearch keeps in JavaScript `Map`s, exactly as it keeps
// it: document ids of any type (compared with SameValueZero) and stored fields
// as live references. The engine knows each document by its short id only.
// Semantics that cannot cross the boundary use the pinned upstream
// implementation, with a one-time, order-preserving transfer of the index.
// Never keep two indexes in sync.
import Original from 'minisearch';
import SearchableMap from 'minisearch/SearchableMap';
import coreInit, { initSync as coreInitSync, MiniSearchWasm as Core } from './minisearch_wasm_core.js';

let initialized = false;
export async function init(input) { const result = await coreInit(input); initialized = true; return result; }
export function initSync(input) { const result = coreInitSync(input); initialized = true; return result; }
// A JavaScript-engine index, as its MiniSearch JSON plus radix tree.
const magic = new TextEncoder().encode('MSWJS01\n');
// A Wasm index: the engine's binary snapshot, then the ids and stored fields.
const identityMagic = new TextEncoder().encode('MSWID01\n');
// The engine's id field when the ids live here (EXTERNAL_ID_FIELD in Rust).
const EXTERNAL_ID = '\u0000';
// Starts, and separates, query terms made on this side (GIVEN_TERMS in Rust).
const GIVEN = '\u0000';
// Documents sent to the engine per call while adding many.
const BATCH = 1024;
const callbacks = ['extractField', 'stringifyField', 'tokenize', 'processTerm', 'logger', 'filter', 'boostDocument', 'boostTerm', 'prefix', 'fuzzy'];
// A Wasm index keeps these search callbacks on this side of the boundary:
// per-term functions are evaluated into the engine's per-term arrays, `filter`
// runs over the finished rows, as it does upstream, `tokenize` and
// `processTerm` make the terms the engine searches for, and the engine calls
// `boostDocument` back while it scores (the same calls, in the same order).
const termCallbacks = ['fuzzy', 'prefix', 'boostTerm'];
const engineCallbacks = ['boostDocument'];
// Search options the engine never sees.
const facadeOptions = ['filter', 'tokenize', 'processTerm', 'boostDocument'];
const isFunction = value => typeof value === 'function';
const isWellFormed = String.prototype.isWellFormed ? text => text.isWellFormed()
  : text => !/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(text);
const hasNonFinite = value => typeof value === 'number' ? !Number.isFinite(value)
  : !!value && typeof value === 'object' && Object.values(value).some(hasNonFinite);
// Stored fields are assigned over a result's own properties upstream.
const shadowsResult = (name, idField) => ['score', 'terms', 'queryTerms', 'match'].includes(name) || (name === 'id' && idField !== 'id');
const byScore = (a, b) => b.score - a.score;
// A plan the engine's compact searches take as they are: no row callbacks.
const nativeCompact = plan => plan !== undefined && plan !== unsupported && !plan.filter && plan.boost === undefined;
const { propertyIsEnumerable } = Object.prototype;
// The native `tokenizer: 'jobboard'` in JavaScript.
const jobboardTokenize = text => text.split(/[^\p{Alphabetic}\p{Number}+#.]+/u).filter(Boolean);
const jobboardProcessTerm = term => term.toLowerCase().replace(/\.+$/, '');
// What MiniSearch's add and remove read from its merged options.
const indexDefaults = options => ({ idField: 'id', storeFields: [], extractField: Original.getDefault('extractField'),
  stringifyField: Original.getDefault('stringifyField'), logger: Original.getDefault('logger'),
  tokenize: options.tokenizer === 'jobboard' ? jobboardTokenize : Original.getDefault('tokenize'),
  processTerm: options.tokenizer === 'jobboard' ? jobboardProcessTerm : Original.getDefault('processTerm') });
// A tokenizer the engine does not run itself: a callback, or a value that is
// not a function (upstream calls it and throws). The defaults are native.
const customized = (options, key) => Object.hasOwn(options, key) && options[key] !== Original.getDefault(key);
// Terms the engine can hold as they are.
const isTerm = term => typeof term === 'string' && term !== '' && isWellFormed(term);
// Configurations only the JavaScript engine reproduces: options the original
// tolerates although they have no native form (a `fields` string is iterated
// by character, a repeated field is indexed twice, `Infinity` is a legal
// boost).
const needsJavaScript = options =>
  !Array.isArray(options.fields) || new Set(options.fields).size !== options.fields.length ||
  options.fields.some(field => typeof field !== 'string' || field === EXTERNAL_ID) ||
  hasNonFinite(options.searchOptions) || hasNonFinite(options.autoSuggestOptions);
// Per-term values the engine takes: functions become arrays (see _termArrays);
// `filter`, `tokenize`, `processTerm` and `boostDocument` are always evaluated
// here.
const engineSearchOptions = options => {
  if (!options || (!facadeOptions.some(key => key in options) && !termCallbacks.some(key => isFunction(options[key])))) return options;
  const result = { ...options };
  for (const key of termCallbacks) if (isFunction(result[key])) delete result[key];
  for (const key of facadeOptions) delete result[key];
  return result;
};
// What the native constructor sees: the engine indexes short ids and stores
// nothing; the callbacks the facade evaluates stay in `_options`. Options given
// as `undefined` or `null` mean "default", as they do upstream for these keys.
const coreOptions = options => {
  const result = { ...options, idField: EXTERNAL_ID, storeFields: [], autoVacuum: false,
    extractField: undefined, stringifyField: undefined, tokenize: undefined, processTerm: undefined, logger: undefined };
  for (const key of ['searchOptions', 'autoSuggestOptions']) result[key] = engineSearchOptions(result[key]);
  for (const key of Object.keys(result)) if (result[key] == null) delete result[key];
  return result;
};
const mergeOptions = (saved, given) => given ? { ...saved, ...given,
  ...(saved.searchOptions || given.searchOptions ? { searchOptions: { ...saved.searchOptions, ...given.searchOptions } } : {}),
  ...(saved.autoSuggestOptions || given.autoSuggestOptions ? { autoSuggestOptions: { ...saved.autoSuggestOptions, ...given.autoSuggestOptions } } : {}) } : saved;
const defaultAutoSuggest = { combineWith: 'AND', prefix: (_term, i, terms) => i === terms.length - 1 };
const defaultAutoVacuum = { minDirtCount: 20, minDirtFactor: 0.1, batchSize: 1000, batchWait: 10 };
// Signals that a query or document needs the JavaScript engine after all.
const unsupported = Symbol('unsupported');
// Upstream merges options with `{ ...defaults, ...options }`, so a key that is
// present but `undefined` replaces the constructor's default with "nothing".
// The native engine reads an absent key as "use the default": spell out what
// upstream does instead, or report `unsupported` where upstream throws.
function definedOptions(options, allFields) {
  if (!Object.values(options).includes(undefined)) return options;
  const result = {};
  for (const [key, value] of Object.entries(options)) {
    if (value !== undefined || facadeOptions.includes(key)) result[key] = value;
    else if (key === 'prefix' || key === 'fuzzy') result[key] = false;
    else if (key === 'combineWith') result[key] = 'OR';
    else if (key === 'fields') result[key] = allFields;
    else if (key === 'boostTerm') result[key] = [];
    else if (key === 'weights') result[key] = { fuzzy: 0.45, prefix: 0.375 };
    else if (key !== 'includeMatch') return unsupported;
  }
  return result;
}
const callbackPaths = (options, prefix = '') => {
  const paths = callbacks.filter(key => typeof options?.[key] === 'function').map(key => prefix + key);
  for (const key of ['searchOptions', 'autoSuggestOptions']) paths.push(...callbackPathsNested(options?.[key], prefix + key + '.'));
  return paths;
};
const callbackPathsNested = (value, prefix) => value ? callbackPaths(value, prefix) : [];
const equalValue = (a, b) => a === b || (a && b && typeof a === 'object' && typeof b === 'object' &&
  Object.keys(a).length === Object.keys(b).length && Object.keys(a).every(key => Object.hasOwn(b, key) && equalValue(a[key], b[key])));
// A `filter` given as an object: every key has to hold an equal stored value.
// The id field matches the document id even when it is not stored; result
// rows expose it as `id`.
const fieldFilter = (fields, idField) => {
  const entries = Object.entries(fields);
  return row => entries.every(([key, value]) => equalValue(key === idField ? row.id : row[key], value));
};

function searchOptions(options = {}, idField = 'id') {
  const result = { ...options };
  for (const key of ['prefix', 'fuzzy', 'boostTerm']) {
    if (Array.isArray(result[key])) {
      const values = result[key];
      result[key] = (_term, i) => values[i] ?? (key === 'boostTerm' ? 1 : false);
    }
  }
  if (result.filter && typeof result.filter !== 'function') result.filter = fieldFilter(result.filter, idField);
  if (result.bm25) result.bm25 = { k: 1.2, b: 0.7, d: 0.5, ...result.bm25 };
  if (result.weights) result.weights = { fuzzy: 0.45, prefix: 0.375, ...result.weights };
  return result;
}

function originalOptions(options) {
  const idField = options.idField ?? 'id';
  const result = { ...options, searchOptions: searchOptions(options.searchOptions, idField) };
  if (options.autoSuggestOptions) result.autoSuggestOptions = searchOptions(options.autoSuggestOptions, idField);
  if (options.tokenizer === 'jobboard') {
    result.tokenize ??= jobboardTokenize;
    result.processTerm ??= jobboardProcessTerm;
  }
  return result;
}

function originalQuery(query, idField) {
  if (query === Symbol.for('minisearch-wasm.wildcard') || query === Original.wildcard) return Original.wildcard;
  if (query && typeof query === 'object') return { ...searchOptions(query, idField), queries: query.queries.map(subquery => originalQuery(subquery, idField)) };
  return query;
}

// Serialization by term alone cannot preserve the order of compressed edges and
// terminal entries. Transfer the native ordered tree, including its leaf slot.
function orderedTree(node) {
  const tree = new Map();
  for (let i = 0; i <= node.children.length; i++) {
    if (node.leaf != null && i === node.leaf_pos) {
      tree.set('', new Map(Object.entries(node.leaf).map(([field, postings]) =>
        [Number(field), new Map(Object.entries(postings).map(([id, frequency]) => [Number(id), frequency]))])));
    }
    if (i < node.children.length) {
      const [edge, child] = node.children[i];
      tree.set(edge, orderedTree(child));
    }
  }
  return tree;
}

function snapshotTree(tree) {
  const node = { children: [], leaf: null, leaf_pos: 0 };
  for (const [edge, value] of tree) {
    if (edge === '') {
      node.leaf_pos = node.children.length;
      node.leaf = Object.fromEntries([...value].map(([field, postings]) => [field, Object.fromEntries(postings)]));
    } else node.children.push([edge, snapshotTree(value)]);
  }
  return node;
}

// These internal fields are deliberately isolated here and covered against the
// exact upstream dependency version, including mutation, ties and serialization.
const watchedVacuums = new WeakSet();

function compactOriginal(index) {
  // Cheap check first: building the map is O(documents).
  if (index._nextId === index._documentIds.size) return false;
  const mapping = new Map([...index._documentIds.keys()].map((id, i) => [id, i]));
  const remap = map => new Map([...map].filter(([id]) => mapping.has(id)).map(([id, value]) => [mapping.get(id), value]));
  // Removing a document whose content changed leaves postings outside the dirt
  // count; they have no new id, so they are dropped, as a vacuum would drop them.
  for (const [term, data] of [...index._index]) {
    for (const [field, postings] of [...data]) {
      const live = new Map([...postings].filter(([id]) => mapping.has(id)).map(([id, frequency]) => [mapping.get(id), frequency]));
      if (live.size) data.set(field, live); else data.delete(field);
    }
    if (data.size === 0) index._index.delete(term);
  }
  index._documentIds = remap(index._documentIds);
  index._fieldLength = remap(index._fieldLength);
  index._storedFields = remap(index._storedFields);
  index._idToShortId = new Map([...index._documentIds].map(([id, external]) => [external, id]));
  index._nextId = mapping.size;
  return true;
}

// Native snapshots need the Wasm module; say so instead of failing inside it.
const nativeLoader = () => {
  if (!initialized) throw new Error('MiniSearch: call `await init()` before loading a native snapshot');
  return Core;
};

const adoptCore = core => {
  // Native auto-vacuum is disabled: the facade owns scheduling and preserves
  // the original active/queued Promise completion boundaries. Dirty queries
  // reproduce the original's lazy cleanup, first query included.
  core.setAutoVacuum(false);
  core.setExactDirtyQueries(true);
  return core;
};

const invalidSnapshot = message => new Error(`invalid minisearch-wasm snapshot: ${message}`);
const asBytes = bytes => bytes instanceof ArrayBuffer ? new Uint8Array(bytes)
  : ArrayBuffer.isView(bytes) && !(bytes instanceof Uint8Array) ? new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength) : bytes;
const startsWith = (bytes, prefix) => bytes?.length >= prefix.length && prefix.every((value, i) => bytes[i] === value);
const decode = bytes => new TextDecoder('utf-8', { fatal: true }).decode(bytes);

export class MiniSearchWasm {
  constructor(options) {
    if (options?.fields == null) throw new Error('MiniSearch: option "fields" must be provided');
    this._init({ ...options });
    if (initialized && !needsJavaScript(options)) this._wasm = adoptCore(new Core(coreOptions(options)));
    else this._js = new Original(originalOptions(options));
  }
  _init(options) {
    this._options = options;
    // MiniSearch's merged options, for the callbacks add and remove call.
    this._merged = { ...indexDefaults(options), ...options };
    // `tokenize` and `processTerm` callbacks run here; the engine gets terms.
    this._tokenizes = customized(options, 'tokenize') || customized(options, 'processTerm');
    this._version = 0n;
    this._freed = false;
    this._currentVacuum = null;
    this._enqueuedVacuum = null;
    this._enqueuedConditions = defaultAutoVacuum;
    this._deferVacuum = false;
    // A Wasm index's identity, by short id: the ids, where a free slot is
    // `undefined` (MiniSearch refuses a missing id); the id → short id map;
    // the stored-field objects, `undefined` where there is none.
    this._ids = [];
    this._shortIds = new Map();
    this._stored = [];
    this._hasStored = false;
    // MiniSearch's averageFieldLength array, by what each entry is: a hole
    // (no document had the field), `null` (read from JSON), or `true` (set).
    this._averages = [];
    // While boostDocument runs: the counts it may read.
    this._boosting = null;
    // Documents whose ids are assigned but whose texts have not reached the
    // engine yet: `{ first, entries: [{ texts, open }] }`.
    this._pending = null;
  }
  static get wildcard() { return Original.wildcard; }
  static getDefault(name) { return name === 'tokenizer' ? 'default' : Original.getDefault(name); }
  get executionMode() { this._alive(); return this._js ? 'javascript' : 'wasm'; }
  _alive() { if (this._freed) throw new Error('MiniSearch: index has been freed'); }
  _check() {
    this._alive();
    // boostDocument runs inside the engine, which cannot be entered again then.
    if (this._boosting !== null) throw new Error('MiniSearch: the index cannot be searched or changed from inside boostDocument');
    // A callback that calls back into the index sees every document added before.
    if (this._pending !== null) this._flush();
  }
  _engine() { this._check(); return this._js ?? this._wasm; }
  _promote() {
    this._check();
    if (!this._js) {
      // The same ids and stored-field objects; the term index is rebuilt from
      // the native tree, whose key order a list of terms cannot carry.
      const js = Original.loadJS({ ...this.toJSON(), index: [] }, originalOptions(this._options));
      js._index = new SearchableMap(orderedTree(this._wasm.indexTree()));
      this._wasm.free();
      this._wasm = undefined;
      this._js = js;
      this._ids = this._stored = this._shortIds = undefined;
    }
    return this._js;
  }
  _mutate(fn) {
    try { return fn(); } finally { this._version++; }
  }
  add(document) {
    this._check();
    if (this._js) return this._mutate(() => this._js.add(document));
    try { this._collect(document); } finally { this._settle(); }
  }
  addAll(documents) {
    this._check();
    if (this._js) return this._mutate(() => this._js.addAll(documents));
    // A subclass that overrides add sees every document, as upstream.
    if (this.add !== MiniSearchWasm.prototype.add) {
      for (const document of documents) this.add(document);
      return;
    }
    try {
      for (const document of documents) {
        if (this._js) this._js.add(document);
        else {
          this._collect(document);
          if (this._pending?.entries.length >= BATCH) this._flush();
        }
      }
    } finally { this._settle(); }
  }
  _settle() {
    if (this._pending !== null && !this._js) this._flush();
    this._version++;
  }
  // MiniSearch's add, up to the indexing itself: the id, the stored fields,
  // then the text of each field, calling the callbacks in the same order. A
  // callback that throws leaves what came before it indexed, as upstream.
  _collect(document) {
    const { extractField, stringifyField, tokenize, processTerm, fields, idField, storeFields } = this._merged;
    const id = extractField(document, idField);
    if (id == null) {
      throw new Error(`MiniSearch: document does not have ID field "${idField}"`);
    }
    if (this._shortIds.has(id)) {
      throw new Error(`MiniSearch: duplicate ID ${id}`);
    }
    const shortId = this._ids.length;
    this._shortIds.set(id, shortId);
    this._ids.push(id);
    this._stored.push(undefined);
    const entry = { texts: new Array(fields.length).fill(null), open: true };
    if (this._pending === null) this._pending = { first: shortId, entries: [] };
    this._pending.entries.push(entry);
    try {
      if (storeFields != null && storeFields.length !== 0) {
        const documentFields = this._stored[shortId] = {};
        this._hasStored = true;
        for (const fieldName of storeFields) {
          const fieldValue = extractField(document, fieldName);
          if (fieldValue !== undefined) documentFields[fieldName] = fieldValue;
        }
      }
      for (let fieldId = 0; fieldId < fields.length; fieldId++) {
        const field = fields[fieldId];
        const fieldValue = extractField(document, field);
        if (fieldValue == null) continue;
        const text = stringifyField(fieldValue, field);
        if (this._tokenizes) {
          const tokens = tokenize(text, field);
          const uniqueTerms = new Set(tokens).size;
          // Upstream records the field before its terms: a processTerm that
          // throws leaves the field and the terms before it indexed.
          entry.texts[fieldId] = [uniqueTerms, []];
          this._averages[fieldId] = true;
          const native = this._terms(tokens, field, processTerm, entry.texts[fieldId][1], earlier => {
            entry.texts[fieldId] = null;
            entry.open = false;
            const js = this._promote(), id = js._fieldIds[field];
            js.addFieldLength(shortId, id, js._documentCount - 1, uniqueTerms);
            for (const t of earlier) js.addTerm(id, shortId, t);
            return t => js.addTerm(id, shortId, t);
          });
          if (!native) {
            this._indexInJS(document, shortId, fieldId + 1);
            return;
          }
        } else if (typeof text !== 'string' || !isWellFormed(text)) {
          // Text the engine cannot take: the original tokenizer gets it, and
          // the rest of the document, in the JavaScript engine.
          entry.open = false;
          this._promote();
          this._indexInJS(document, shortId, fieldId, { value: fieldValue, text });
          return;
        } else {
          entry.texts[fieldId] = text;
          this._averages[fieldId] = true;
        }
      }
    } finally {
      entry.open = false;
    }
  }
  // MiniSearch's loop over a field's tokens, collecting the processed terms
  // for the engine into `terms`. At the first term the engine cannot hold (''
  // or not a string), `transfer(earlier)` moves the index to JavaScript and
  // returns what takes that term and each later one, as upstream takes them:
  // the result is then false.
  _terms(tokens, field, processTerm, terms, transfer) {
    let take = null;
    const add = t => {
      if (take === null) {
        if (isTerm(t)) { terms.push(t); return; }
        take = transfer(terms);
      }
      take(t);
    };
    for (const term of tokens) {
      const processedTerm = processTerm(term, field);
      if (Array.isArray(processedTerm)) {
        for (const t of processedTerm) add(t);
      } else if (processedTerm) add(processedTerm);
    }
    return take === null;
  }
  // Hand the finished documents to the engine, in short-id order. A document
  // still being collected (a callback added another) waits with those after it.
  _flush() {
    const pending = this._pending;
    let count = 0;
    while (count < pending.entries.length && !pending.entries[count].open) count++;
    if (count === 0) return;
    const entries = pending.entries.splice(0, count);
    const first = pending.first;
    pending.first += count;
    if (pending.entries.length === 0) this._pending = null;
    const json = JSON.stringify(entries.map(entry => entry.texts));
    if (this._tokenizes) this._wasm.addTermBatch(json, first);
    else this._wasm.addTextBatch(json, first);
  }
  // The rest of MiniSearch's add, from field `fromField` on, after a transfer:
  // `known` holds that field's value and text when they were already read.
  _indexInJS(document, shortId, fromField, known) {
    const js = this._js;
    const { extractField, stringifyField, tokenize, processTerm, fields } = js._options;
    for (let fieldIndex = fromField; fieldIndex < fields.length; fieldIndex++) {
      const field = fields[fieldIndex];
      const given = fieldIndex === fromField ? known : undefined;
      const fieldValue = given ? given.value : extractField(document, field);
      if (fieldValue == null) continue;
      const tokens = tokenize(given ? given.text : stringifyField(fieldValue, field), field);
      const fieldId = js._fieldIds[field];
      const uniqueTerms = new Set(tokens).size;
      js.addFieldLength(shortId, fieldId, js._documentCount - 1, uniqueTerms);
      for (const term of tokens) {
        const processedTerm = processTerm(term, field);
        if (Array.isArray(processedTerm)) {
          for (const t of processedTerm) js.addTerm(fieldId, shortId, t);
        } else if (processedTerm) js.addTerm(fieldId, shortId, processedTerm);
      }
    }
  }
  addAllJSON(json) {
    this._check();
    const documents = JSON.parse(json);
    if (!Array.isArray(documents)) throw new Error('MiniSearch: documents must be an array');
    return this.addAll(documents);
  }
  addAllAsync(documents, options) {
    this._check();
    // The original schedules full chunks on timers and the final short chunk
    // on its promise chain, adding each chunk with addAll.
    return Original.prototype.addAllAsync.call(this, documents, options);
  }
  // MiniSearch's remove: the texts are collected with the same callbacks, then
  // taken out of the engine at once; version conflicts are logged afterwards.
  remove(document) {
    this._check();
    if (this._js) return this._mutate(() => this._js.remove(document));
    const { extractField, stringifyField, tokenize, processTerm, fields, idField } = this._merged;
    const id = extractField(document, idField);
    if (id == null) {
      throw new Error(`MiniSearch: document does not have ID field "${idField}"`);
    }
    const shortId = this._shortIds.get(id);
    if (shortId == null) {
      throw new Error(`MiniSearch: cannot remove document with ID ${id}: it is not in the index`);
    }
    const texts = new Array(fields.length).fill(null);
    let complete = false;
    try {
      for (let fieldId = 0; fieldId < fields.length; fieldId++) {
        const field = fields[fieldId];
        const fieldValue = extractField(document, field);
        if (fieldValue == null) continue;
        const text = stringifyField(fieldValue, field);
        if (this._tokenizes) {
          const tokens = tokenize(text, field);
          const uniqueTerms = new Set(tokens).size;
          // As for add: the field, then its terms, one by one.
          texts[fieldId] = [uniqueTerms, []];
          const native = this._terms(tokens, field, processTerm, texts[fieldId][1], earlier => {
            texts[fieldId] = null;
            this._removeTexts(id, shortId, texts, false);
            const js = this._promote(), fieldIndex = js._fieldIds[field];
            js.removeFieldLength(shortId, fieldIndex, js._documentCount, uniqueTerms);
            for (const t of earlier) js.removeTerm(fieldIndex, shortId, t);
            return t => js.removeTerm(fieldIndex, shortId, t);
          });
          if (!native) {
            this._unindexInJS(document, id, shortId, fieldId + 1);
            return;
          }
        } else if (typeof text !== 'string' || !isWellFormed(text)) {
          this._removeTexts(id, shortId, texts, false);
          this._promote();
          this._unindexInJS(document, id, shortId, fieldId, { value: fieldValue, text });
          return;
        } else texts[fieldId] = text;
      }
      complete = true;
    } finally {
      // A callback that threw leaves the fields before it removed, as upstream.
      if (this._wasm !== undefined) this._removeTexts(id, shortId, texts, complete);
      this._version++;
    }
  }
  _removeTexts(id, shortId, texts, finish) {
    const json = JSON.stringify(texts);
    const conflicts = this._tokenizes ? this._wasm.removeTerms(shortId, json, finish) : this._wasm.removeTexts(shortId, json, finish);
    const shown = this._ids[shortId];
    if (finish) {
      this._shortIds.delete(id);
      this._ids[shortId] = undefined;
      this._stored[shortId] = undefined;
    }
    for (let i = 0; i < conflicts.length; i += 2) {
      this._merged.logger('warn', `MiniSearch: document with ID ${shown} has changed before removal: term "${conflicts[i]}" was not present in field "${conflicts[i + 1]}". Removing a document after it has changed can corrupt the index!`, 'version_conflict');
    }
  }
  // The rest of MiniSearch's remove, from field `fromField` on, after a
  // transfer (see _indexInJS).
  _unindexInJS(document, id, shortId, fromField, known) {
    const js = this._js;
    const { extractField, stringifyField, tokenize, processTerm, fields } = js._options;
    for (let fieldIndex = fromField; fieldIndex < fields.length; fieldIndex++) {
      const field = fields[fieldIndex];
      const given = fieldIndex === fromField ? known : undefined;
      const fieldValue = given ? given.value : extractField(document, field);
      if (fieldValue == null) continue;
      const tokens = tokenize(given ? given.text : stringifyField(fieldValue, field), field);
      const fieldId = js._fieldIds[field];
      const uniqueTerms = new Set(tokens).size;
      js.removeFieldLength(shortId, fieldId, js._documentCount, uniqueTerms);
      for (const term of tokens) {
        const processedTerm = processTerm(term, field);
        if (Array.isArray(processedTerm)) {
          for (const t of processedTerm) js.removeTerm(fieldId, shortId, t);
        } else if (processedTerm) js.removeTerm(fieldId, shortId, processedTerm);
      }
    }
    js._storedFields.delete(shortId);
    js._documentIds.delete(shortId);
    js._idToShortId.delete(id);
    js._fieldLength.delete(shortId);
    js._documentCount -= 1;
  }
  removeAll(documents) {
    if (documents) {
      for (const document of documents) this.remove(document);
    } else if (arguments.length > 0) {
      throw new Error('Expected documents to be present. Omit the argument to remove all documents.');
    } else {
      this._engine().removeAll();
      if (this._wasm) {
        this._ids = [];
        this._shortIds = new Map();
        this._stored = [];
        this._averages = [];
      }
      this._version++;
    }
  }
  discard(id) {
    this._check();
    if (this._js) this._mutate(() => this._js.discard(id));
    else {
      const shortId = this._shortIds.get(id);
      if (shortId == null) {
        throw new Error(`MiniSearch: cannot discard document with ID ${id}: it is not in the index`);
      }
      this._shortIds.delete(id);
      this._ids[shortId] = undefined;
      this._stored[shortId] = undefined;
      this._wasm.discardShort(shortId);
      this._version++;
    }
    if (!this._deferVacuum) this._autoVacuum();
  }
  discardAll(ids) {
    this._check();
    if (this._js) this._mutate(() => this._js.discardAll(ids));
    else {
      // Like upstream: one discard per id, and the auto-vacuum check once at
      // the end, unless a discard threw.
      this._deferVacuum = true;
      try { for (const id of ids) this.discard(id); } finally { this._deferVacuum = false; }
    }
    this._autoVacuum();
  }
  replace(document) {
    this._check();
    if (this._js) {
      this._mutate(() => this._js.replace(document));
      this._autoVacuum();
      return;
    }
    const { idField, extractField } = this._merged;
    const id = extractField(document, idField);
    this.discard(id);
    this.add(document);
  }
  has(id) {
    this._alive();
    if (this._boosting === null) this._check();
    return this._js ? this._js.has(id) : this._shortIds.has(id);
  }
  // The live stored-fields object, as upstream: edits to it show up in results.
  getStoredFields(id) {
    this._alive();
    if (this._boosting === null) this._check();
    if (this._js) return this._js.getStoredFields(id);
    const shortId = this._shortIds.get(id);
    return shortId == null ? undefined : this._stored[shortId];
  }
  _autoVacuum() {
    if (this._js) {
      if (this._js._currentVacuum) this._watchVacuum(this._js._currentVacuum);
      if (this._js._enqueuedVacuum) this._watchVacuum(this._js._enqueuedVacuum);
      return;
    }
    if (this._options.autoVacuum === false) return;
    const { minDirtFactor, minDirtCount, batchSize, batchWait } =
      this._options.autoVacuum == null || this._options.autoVacuum === true ? defaultAutoVacuum : this._options.autoVacuum;
    this._conditionalVacuum({ batchSize, batchWait }, { minDirtCount, minDirtFactor });
  }
  vacuum(options = {}) {
    this._check();
    if (this._js) return this._watchVacuum(this._js.vacuum(options));
    return this._conditionalVacuum(options);
  }
  // The original's scheduler (conditionalVacuum / performVacuuming), driving
  // native vacuum steps: one active Promise, one queued Promise shared by all
  // later requests, and queued conditions that a manual request clears.
  _conditionalVacuum(options, conditions) {
    if (this._currentVacuum) {
      this._enqueuedConditions = this._enqueuedConditions && conditions;
      if (this._enqueuedVacuum != null) return this._enqueuedVacuum;
      this._enqueuedVacuum = this._watchVacuum(this._currentVacuum.then(() => {
        const conditions = this._enqueuedConditions;
        this._enqueuedConditions = defaultAutoVacuum;
        return this._performVacuum(options, conditions);
      }));
      return this._enqueuedVacuum;
    }
    if (!this._vacuumConditionsMet(conditions)) return Promise.resolve();
    this._currentVacuum = this._watchVacuum(this._performVacuum(options));
    return this._currentVacuum;
  }
  _vacuumConditionsMet(conditions) {
    if (conditions == null) return true;
    // `||`, not `??`: the original also replaces an explicit 0 with the default.
    return this.dirtCount >= (conditions.minDirtCount || defaultAutoVacuum.minDirtCount) &&
      this.dirtFactor >= (conditions.minDirtFactor || defaultAutoVacuum.minDirtFactor);
  }
  async _performVacuum(options, conditions) {
    if (!this._freed && this._vacuumConditionsMet(conditions)) {
      const batchSize = Math.min(0xffffffff, Math.max(1, Math.floor(options.batchSize || defaultAutoVacuum.batchSize)));
      const batchWait = options.batchWait || defaultAutoVacuum.batchWait;
      // The first batch runs synchronously, like the original's.
      while (this._wasm && !this._wasm.vacuumStep(batchSize)) await new Promise(resolve => setTimeout(resolve, batchWait));
      // A transfer to JavaScript in between leaves the rest to that engine.
      if (this._js && !this._freed) await this._js.vacuum(options);
    }
    // Make the next lines always async, so they execute after this function returns
    await null;
    this._currentVacuum = this._enqueuedVacuum;
    this._enqueuedVacuum = null;
  }
  _watchVacuum(promise) {
    // The original reuses its current and queued Promises across calls, so
    // attach the cleanup only once per Promise, and do not wrap the Promise.
    if (watchedVacuums.has(promise)) return promise;
    watchedVacuums.add(promise);
    // Cleanup only after every queued run and any newly created dirt has been
    // handled. Compaction is housekeeping: it must never fail the process.
    promise.then(() => {
      try {
        if (!this._freed && !this.isVacuuming && !this.dirtCount && this._crowded() && this._compact()) this._version++;
      } catch { /* a mismatched remove left stale postings: keep the ID slots */ }
    }, () => {});
    return promise;
  }
  // Whether most short ids are free: the dense tables are then worth
  // renumbering, which MiniSearch never does (its short ids show in toJSON).
  _crowded() {
    const nextId = this._js ? this._js._nextId : this._wasm.nextId;
    return nextId >= 2 * this.documentCount + 1024;
  }
  _compact() {
    if (this._js) return compactOriginal(this._js);
    if (this._wasm.nextId === this._wasm.documentCount) return false;
    // The engine renumbers the live documents densely: new short id s held
    // old[s]. The ids and stored fields move with them.
    const old = this._wasm.compactExternal();
    const ids = new Array(old.length), stored = new Array(old.length), shortIds = new Map();
    for (let shortId = 0; shortId < old.length; shortId++) {
      ids[shortId] = this._ids[old[shortId]];
      stored[shortId] = this._stored[old[shortId]];
      shortIds.set(ids[shortId], shortId);
    }
    this._ids = ids;
    this._stored = stored;
    this._shortIds = shortIds;
    return true;
  }
  compact() {
    if (this.isVacuuming || this.dirtCount) throw new Error('MiniSearch: vacuum must finish before compacting the index');
    this._compact();
    this._version++;
  }
  get documentCount() { return this._boosting?.documentCount ?? this._engine().documentCount; }
  get termCount() { return this._boosting?.termCount ?? this._engine().termCount; }
  get dirtCount() { return this._boosting?.dirtCount ?? this._engine().dirtCount; }
  get dirtFactor() { return this._boosting?.dirtFactor ?? this._engine().dirtFactor; }
  get isVacuuming() { this._alive(); return this._js ? this._js.isVacuuming : this._currentVacuum != null; }
  get idTableVersion() { this._alive(); return String(this._version); }
  search(query, options) {
    this._check();
    options ??= {};
    if (!this._js) {
      const rows = this._wasmSearch(query, options);
      if (rows !== unsupported) return rows;
      this._promote();
    }
    const idField = this._options.idField ?? 'id';
    const rows = this._js.search(originalQuery(query, idField), searchOptions(options, idField));
    if (options.includeMatch === false) for (const row of rows) delete row.match;
    return rows;
  }
  _wasmSearch(query, options) {
    const plan = this._plan(query, options);
    if (plan === unsupported) return unsupported;
    const includeMatch = options.includeMatch !== false;
    // Upstream sorts unless the query is the top-level wildcard without a
    // boostDocument.
    const sorted = query !== Original.wildcard || plan.boost !== undefined;
    if (!plan.filter && plan.boost === undefined) {
      const rows = this._results(this._rows(plan, plan.options, true), includeMatch);
      if (!this._shadowedScore) return rows;
      // A stored `score` replaced a result's own: the original sorts by it,
      // from traversal order.
      const unsorted = this._results(this._rows(plan, plan.options, false), includeMatch);
      if (sorted) unsorted.sort(byScore);
      return unsorted;
    }
    // The original filters finished result objects, `match` included, in raw
    // traversal order; the filter may change scores. Sort only afterwards, as
    // JavaScript sorts: boosted scores may be anything.
    const rows = this._results(this._rows(plan, { ...plan.options, includeMatch: includeMatch || !!plan.filter }, false), includeMatch || !!plan.filter, plan.filter);
    if (sorted) rows.sort(byScore);
    // The filter saw `match`; the rows it kept drop it.
    if (!includeMatch && plan.filter) {
      for (const row of rows) {
        const stored = this._stored[this._shortIds.get(row.id)];
        if (stored == null || !propertyIsEnumerable.call(stored, 'match')) delete row.match;
      }
    }
    return rows;
  }
  // The engine's rows for a plan, with its boostDocument called back while
  // the engine scores.
  _rows(plan, options, sort) {
    if (plan.boost === undefined) return this._wasm.searchRows(plan.query, options, sort);
    const boostDocument = plan.boost, ids = this._ids, stored = this._stored;
    // One call per posting list: per document a boost and a skip flag. A
    // falsy boost skips the posting; a wildcard's score is the boost itself.
    // `1 * boost` converts like upstream's product does (a BigInt throws).
    const boost = (term, shortIds) => {
      const out = new Float64Array(shortIds.length * 2);
      for (let i = 0; i < shortIds.length; i++) {
        const shortId = shortIds[i];
        const docBoost = boostDocument(ids[shortId], term, stored[shortId]);
        if (term === '' || docBoost) out[2 * i] = 1 * docBoost;
        else out[2 * i + 1] = 1;
      }
      return out;
    };
    const wasm = this._wasm;
    this._boosting = { documentCount: wasm.documentCount, termCount: wasm.termCount, dirtCount: wasm.dirtCount, dirtFactor: wasm.dirtFactor };
    try { return wasm.searchRows(plan.query, options, sort, boost); } finally { this._boosting = null; }
  }
  // MiniSearch's result objects from the engine's rows: `{ id, score, terms,
  // queryTerms, match }` with the stored fields assigned over them, then the
  // filter. `_shadowedScore` tells whether a stored value replaced a score.
  // `own`, if given, collects each kept row's own id, score and terms.
  _results(rows, includeMatch, filter, own) {
    const { docIds, scores, table, termIds, termOffsets, queryIds, queryOffsets, fieldIds, fieldOffsets } = rows;
    const terms = JSON.parse(table);
    const fields = this._options.fields, ids = this._ids, stored = this._stored;
    const count = docIds.length, results = [];
    let shadowed = false;
    for (let i = 0; i < count; i++) {
      const shortId = docIds[i], score = scores[i];
      const termList = [], queryList = [];
      for (let k = termOffsets[i]; k < termOffsets[i + 1]; k++) termList.push(terms[termIds[k]]);
      for (let k = queryOffsets[i]; k < queryOffsets[i + 1]; k++) queryList.push(terms[queryIds[k]]);
      let result;
      if (includeMatch) {
        const match = {};
        for (let k = termOffsets[i]; k < termOffsets[i + 1]; k++) {
          const fieldList = [];
          for (let m = fieldOffsets[k]; m < fieldOffsets[k + 1]; m++) fieldList.push(fields[fieldIds[m]]);
          match[terms[termIds[k]]] = fieldList;
        }
        result = { id: ids[shortId], score, terms: termList, queryTerms: queryList, match };
      } else {
        result = { id: ids[shortId], score, terms: termList, queryTerms: queryList };
      }
      const documentFields = stored[shortId];
      if (documentFields !== undefined) {
        Object.assign(result, documentFields);
        if (result.score !== score) shadowed = true;
      }
      if (filter === undefined || filter(result)) {
        results.push(result);
        if (own !== undefined) own.push({ id: ids[shortId], score, terms: termList, shortId, row: result });
      }
    }
    this._shadowedScore = shadowed;
    return results;
  }
  // Translate a query for the native engine: `{ query, options, filter,
  // boost }`, or `unsupported` when a callback returns a value the engine has
  // no form for, or query nodes bring boostDocuments of their own.
  _plan(query, options) {
    if (this._js) return unsupported;
    const globals = this._options.searchOptions ?? {};
    options = definedOptions(options, this._options.fields);
    if (options === unsupported) return unsupported;
    const merged = { ...globals, ...options };
    const boost = merged.boostDocument ?? undefined;
    if (boost !== undefined && !isFunction(boost)) return unsupported;
    let filter;
    if (isFunction(merged.filter)) filter = merged.filter;
    else if (merged.filter != null) {
      // Upstream calls whatever it is given; an object is this package's
      // declarative filter.
      if (typeof merged.filter !== 'object') return unsupported;
      filter = fieldFilter(merged.filter, this._merged.idField);
    }
    const inherited = { boostDocument: boost };
    for (const key of [...termCallbacks, 'tokenize', 'processTerm']) if (key in merged) inherited[key] = merged[key];
    const rewritten = this._rewrite(query, inherited, true);
    if (rewritten === unsupported) return unsupported;
    return { query: rewritten.query, filter, boost, options: { ...engineSearchOptions(options), ...rewritten.options } };
  }
  _rewrite(query, inherited, top) {
    if (query === Original.wildcard) return { query: Core.wildcard };
    if (typeof query === 'string') {
      const text = this._leafText(query, inherited);
      if (text === unsupported) return unsupported;
      const arrays = this._termArrays(text, inherited);
      if (arrays === unsupported) return unsupported;
      if (top || !arrays) return { query: text, options: arrays };
      // Per-term values belong to one string: give it a node of its own.
      return { query: { queries: [text], ...arrays } };
    }
    if (!query || typeof query !== 'object' || !Array.isArray(query.queries)) return { query };
    // One boostDocument for the whole query: the engine calls back only that.
    if ('boostDocument' in query && (query.boostDocument ?? undefined) !== inherited.boostDocument) return unsupported;
    query = definedOptions(query, this._options.fields);
    if (query === unsupported) return unsupported;
    const own = { ...inherited };
    for (const key of [...termCallbacks, 'tokenize', 'processTerm']) if (key in query) own[key] = query[key];
    const queries = [];
    for (const subquery of query.queries) {
      const rewritten = this._rewrite(subquery, own, false);
      if (rewritten === unsupported) return unsupported;
      queries.push(rewritten.query);
    }
    // A nested `filter` does nothing upstream: only search()'s own applies.
    return { query: { ...engineSearchOptions(query), queries } };
  }
  // The text the engine searches for a query string: the string itself, which
  // the engine tokenizes like the index does, or the terms that the effective
  // `tokenize` and `processTerm` make of it, as executeQuery makes them.
  _leafText(text, options) {
    if (!this._tokenizes && !('tokenize' in options) && !('processTerm' in options) && !text.startsWith(GIVEN)) return text;
    const { tokenize: searchTokenize, processTerm: searchProcessTerm } =
      { tokenize: this._merged.tokenize, processTerm: this._merged.processTerm, ...options };
    const terms = searchTokenize(text)
      .flatMap((term) => searchProcessTerm(term))
      .filter((term) => !!term);
    for (const term of terms) if (typeof term !== 'string' || term.includes(GIVEN) || !isWellFormed(term)) return unsupported;
    return GIVEN + terms.join(GIVEN);
  }
  // Evaluate per-term callbacks like the original's termToQuerySpec: once per
  // processed query term, as (term, index, terms), fuzzy then prefix then boost.
  _termArrays(text, inherited) {
    const keys = termCallbacks.filter(key => isFunction(inherited[key]));
    if (!keys.length) return undefined;
    const terms = this._wasm.queryTerms(text);
    const arrays = Object.fromEntries(keys.map(key => [key, []]));
    for (let i = 0; i < terms.length; i++) {
      for (const key of keys) {
        let value = inherited[key](terms[i], i, terms);
        if (key === 'prefix') value = !!value;
        else if (key === 'fuzzy') {
          if (!value || Number.isNaN(value)) value = false;
          else if (value !== true && !Number.isFinite(value)) return unsupported;
        } else if (!Number.isFinite(value)) return unsupported;
        if (typeof value !== 'boolean' && typeof value !== 'number') return unsupported;
        arrays[key].push(value);
      }
    }
    return arrays;
  }
  // Whether search options need finished rows or JavaScript callbacks.
  _hasSearchCallbacks(...layers) {
    const merged = Object.assign({}, this._options.searchOptions, ...layers);
    return merged.filter != null || [...termCallbacks, ...engineCallbacks].some(key => isFunction(merged[key]));
  }
  autoSuggest(query, options) {
    this._check();
    options ??= {};
    if (!this._js && typeof query === 'string' && !Object.values(options).includes(undefined) &&
        !this._hasSearchCallbacks(this._options.autoSuggestOptions, options) && this.search === MiniSearchWasm.prototype.search) {
      const text = this._leafText(query, { ...this._options.searchOptions, ...this._options.autoSuggestOptions, ...options });
      if (text !== unsupported) return this._wasm.autoSuggest(text, engineSearchOptions(options));
    }
    if (this._js) return this._js.autoSuggest(originalQuery(query, this._options.idField ?? 'id'), searchOptions(options, this._options.idField ?? 'id'));
    // The original's autoSuggest: a search whose rows are grouped by phrase.
    const suggestions = new Map();
    for (const { score, terms } of this.search(query, { ...defaultAutoSuggest, ...this._options.autoSuggestOptions, ...options })) {
      const phrase = terms.join(' ');
      const suggestion = suggestions.get(phrase);
      if (suggestion != null) { suggestion.score += score; suggestion.count += 1; }
      else suggestions.set(phrase, { score, terms, count: 1 });
    }
    const results = [];
    for (const [suggestion, { score, terms, count }] of suggestions) results.push({ suggestion, terms, score: score / count });
    return results.sort(byScore);
  }
  // The plan of a compact search of a string query, or undefined for other
  // queries and for a filter, which needs full rows (built with a plan of
  // their own: per-term callbacks are not evaluated speculatively here).
  _compactPlan(query, options) {
    if (this._js || typeof query !== 'string') return undefined;
    if ({ ...this._options.searchOptions, ...options }.filter != null) return undefined;
    return this._plan(query, options);
  }
  // Whether a stored `score` of one of these documents replaces its result's
  // score, which then decides the order, as it does in search().
  _storedScore(shortIds) {
    if (!this._hasStored) return false;
    for (const shortId of shortIds) {
      const fields = this._stored[shortId];
      if (fields != null && propertyIsEnumerable.call(fields, 'score')) return true;
    }
    return false;
  }
  searchJoined(query, orMode) {
    this._check();
    if (!this._js && typeof query === 'string' && !this._hasSearchCallbacks()) {
      const text = this._leafText(query, this._options.searchOptions ?? {});
      const joined = text === unsupported ? undefined : this._wasm.searchJoined(text, orMode);
      if (joined && !this._storedScore(joined.docIds)) return this._withIds(joined);
    }
    return this.searchJoinedOpts(query, orMode ? { combineWith: 'OR' } : {});
  }
  searchJoinedOpts(query, options) {
    this._check();
    options ??= {};
    const plan = this._compactPlan(query, options);
    if (nativeCompact(plan)) {
      const joined = this._wasm.searchJoinedOpts(plan.query, plan.options);
      if (!this._storedScore(joined.docIds)) return this._withIds(joined);
    }
    const rows = this._identifiedRows(query, options, plan);
    return { count: rows.length, ids: JSON.stringify(rows.map(row => row.id)), scores: Float64Array.from(rows, row => row.score), terms: rows.map(row => row.terms.join(' ')).join('\n') };
  }
  // The engine's joined results name documents by short id.
  _withIds({ count, docIds, scores, terms }) {
    const ids = this._ids, list = new Array(docIds.length);
    for (let i = 0; i < docIds.length; i++) list[i] = ids[docIds[i]];
    return { count, ids: JSON.stringify(list), scores, terms };
  }
  searchRaw(query, options) {
    this._check();
    options ??= {};
    const plan = this._compactPlan(query, options);
    if (nativeCompact(plan)) {
      const raw = this._wasm.searchRaw(plan.query, plan.options);
      if (!this._storedScore(raw.docIds)) return { ...raw, idTableVersion: this.idTableVersion };
    }
    const rows = this._identifiedRows(query, options, plan), terms = new Map(), termIds = [], offsets = [0];
    for (const row of rows) {
      for (const term of row.terms) { if (!terms.has(term)) terms.set(term, terms.size); termIds.push(terms.get(term)); }
      offsets.push(termIds.length);
    }
    const shortIds = this._js?._idToShortId;
    return { count: rows.length, idTableVersion: this.idTableVersion, docIds: Uint32Array.from(rows, row => row.shortId ?? shortIds.get(row.id)),
      scores: Float64Array.from(rows, row => row.score), termTable: [...terms.keys()].join('\n'), termOffsets: Uint32Array.from(offsets), termIds: Uint32Array.from(termIds) };
  }
  // Rows for the compact forms, which identify documents: `id`, `score` and
  // `terms` always are the result's own, where a full row lets a stored field
  // of the same name take their place (it still decides the order, as it does
  // in search()).
  _identifiedRows(query, options, plan) {
    if (!this._js) {
      if (plan === undefined) plan = this._plan(query, options);
      if (plan !== unsupported) {
        const own = [];
        this._results(this._rows(plan, { ...plan.options, includeMatch: true }, false), true, plan.filter, own);
        if (query !== Original.wildcard || plan.boost !== undefined) own.sort((a, b) => b.row.score - a.row.score);
        return own;
      }
      this._promote();
    }
    const idField = this._options.idField ?? 'id';
    const shadowed = this._options.storeFields?.filter?.(name => shadowsResult(name, idField));
    if (!shadowed?.length) return this.search(query, options);
    const js = this._js;
    const given = searchOptions(options, idField), merged = { ...js._options.searchOptions, ...given };
    const results = [];
    for (const [shortId, { score, terms, match }] of js.executeQuery(originalQuery(query, idField), given)) {
      const result = { id: js._documentIds.get(shortId), score: score * (terms.length || 1), terms: Object.keys(match), queryTerms: terms, match };
      const row = Object.assign({ ...result }, js._storedFields.get(shortId));
      if (merged.filter == null || merged.filter(row)) results.push({ ...result, order: row.score });
    }
    if (query !== Original.wildcard || merged.boostDocument != null) results.sort((a, b) => b.order - a.order);
    return results;
  }
  docIdTable() {
    this._check();
    if (this._js) {
      const ids = Array(this._js._nextId).fill(null);
      for (const [id, external] of this._js._documentIds) ids[id] = external;
      return JSON.stringify(ids);
    }
    if (this._idTable?.version !== this._version) {
      this._idTable = { version: this._version, json: JSON.stringify(Array.from(this._ids, id => id === undefined ? null : id)) };
    }
    return this._idTable.json;
  }
  autoSuggestJoined(query) {
    this._check();
    if (!this._js && typeof query === 'string' && !this._hasSearchCallbacks(this._options.autoSuggestOptions) &&
        this.search === MiniSearchWasm.prototype.search) {
      const text = this._leafText(query, { ...this._options.searchOptions, ...this._options.autoSuggestOptions });
      if (text !== unsupported) return this._wasm.autoSuggestJoined(text);
    }
    const rows = this.autoSuggest(query);
    return { count: rows.length, suggestions: rows.map(row => row.suggestion).join('\n'), scores: Float64Array.from(rows, row => row.score) };
  }
  searchCountDefault(query, orMode) { return this.searchJoined(query, orMode).count; }
  searchCountOpts(query, prefix, fuzzy) { return this.search(query, { prefix, fuzzy }).length; }
  // MiniSearch's toJSON, key order included; the stored fields are the live
  // objects, as upstream.
  toJSON() {
    this._check();
    if (this._js) return this._js.toJSON();
    const core = this._wasm.toJSON();
    const documentIds = {}, storedFields = {};
    for (let shortId = 0; shortId < this._ids.length; shortId++) {
      if (this._ids[shortId] !== undefined) documentIds[shortId] = this._ids[shortId];
    }
    for (let shortId = 0; shortId < this._stored.length; shortId++) {
      if (this._stored[shortId] !== undefined) storedFields[shortId] = this._stored[shortId];
    }
    const averageFieldLength = [];
    averageFieldLength.length = this._averages.length;
    this._averages.forEach((set, fieldId) => { averageFieldLength[fieldId] = set ? core.averageFieldLength[fieldId] : null; });
    return { documentCount: core.documentCount, nextId: core.nextId, documentIds,
      fieldIds: Object.fromEntries(this._options.fields.map((field, i) => [field, i])), fieldLength: core.fieldLength,
      averageFieldLength, storedFields, dirtCount: core.dirtCount, index: core.index,
      serializationVersion: core.serializationVersion };
  }
  toJSONString() { return JSON.stringify(this.toJSON()); }
  toMiniSearchJSON() { return this.toJSONString(); }
  static _fromJS(js, options) {
    const result = Object.create(this.prototype);
    result._init({ ...options });
    result._js = js;
    result._ids = result._stored = result._shortIds = undefined;
    return result;
  }
  // Native loading keeps a MiniSearch JSON index in Wasm, taking its ids and
  // stored fields with JSON.parse like the original loader. The native reader
  // is stricter than the original, so anything it refuses is loaded (or
  // rejected) by the original loader.
  static _loadJSONNative(json, options) {
    if (!initialized || typeof json !== 'string' || options == null || needsJavaScript(options)) return undefined;
    let core;
    try { core = Core.loadJSON(json, coreOptions(options)); } catch { return undefined; }
    return this._fromLoadedJSON(core, options);
  }
  static _fromLoadedJSON(core, options) {
    const result = Object.create(this.prototype);
    result._init({ ...options });
    result._wasm = adoptCore(core);
    const { ids, stored, averages } = core.takeIdentity();
    result._averages = JSON.parse(averages).map(average => average === null ? null : true);
    if (result._adopt(JSON.parse(ids), JSON.parse(stored))) return result;
    result.free();
    return undefined;
  }
  // Ids and stored fields as MiniSearch's JSON has them (objects keyed by short
  // id), read the way its loadJS reads them. False when a key has no place in
  // this index's tables: the original loader keeps such entries.
  _adopt(documentIds, storedFields) {
    const nextId = this._wasm.nextId;
    const ids = new Array(nextId).fill(undefined), stored = new Array(nextId).fill(undefined), shortIds = new Map();
    let live = 0;
    for (const key of Object.keys(documentIds)) {
      const shortId = parseInt(key, 10);
      if (!(shortId >= 0 && shortId < nextId) || ids[shortId] !== undefined || documentIds[key] === undefined) return false;
      ids[shortId] = documentIds[key];
      live++;
    }
    if (live !== this._wasm.documentCount) return false;
    for (let shortId = 0; shortId < nextId; shortId++) if (ids[shortId] !== undefined) shortIds.set(ids[shortId], shortId);
    return this._adoptStored(ids, shortIds, stored, storedFields);
  }
  // A snapshot's ids: one per live document, in the order of `live`.
  _adoptLive(live, idList, storedFields) {
    const nextId = this._wasm.nextId;
    if (idList.length !== live.length) return false;
    const ids = new Array(nextId).fill(undefined), stored = new Array(nextId).fill(undefined), shortIds = new Map();
    for (let i = 0; i < live.length; i++) {
      if (idList[i] === undefined) return false;
      ids[live[i]] = idList[i];
      shortIds.set(idList[i], live[i]);
    }
    return this._adoptStored(ids, shortIds, stored, storedFields);
  }
  _adoptStored(ids, shortIds, stored, storedFields) {
    if (storedFields == null || typeof storedFields !== 'object') return false;
    for (const key of Object.keys(storedFields)) {
      const shortId = parseInt(key, 10);
      if (!(shortId >= 0 && shortId < stored.length)) return false;
      stored[shortId] = storedFields[key];
      this._hasStored = true;
    }
    this._ids = ids;
    this._stored = stored;
    this._shortIds = shortIds;
    return true;
  }
  static loadJSON(json, options) {
    return this._loadJSONNative(json, options) ??
      this._fromJS(Original.loadJSON(json, options == null ? options : originalOptions(options)), options);
  }
  static loadMiniSearchJSON(json, options) { return this.loadJSON(json, options); }
  static async loadJSONAsync(json, options) {
    if (initialized && typeof json === 'string' && options != null && !needsJavaScript(options)) {
      let core;
      try { core = await Core.loadJSONAsync(json, coreOptions(options)); } catch { /* let the original loader decide */ }
      const result = core && this._fromLoadedJSON(core, options);
      if (result) return result;
    }
    const js = await Original.loadJSONAsync(json, options == null ? options : originalOptions(options));
    return this._fromJS(js, options);
  }
  // What a native snapshot adds to the engine's own: the real id and stored
  // field names, the ids of the live documents in short-id order, and the
  // stored fields by short id. JSON, like MiniSearch's own serialization.
  _identity() {
    const ids = [], stored = {};
    for (let shortId = 0; shortId < this._ids.length; shortId++) if (this._ids[shortId] !== undefined) ids.push(this._ids[shortId]);
    for (let shortId = 0; shortId < this._stored.length; shortId++) if (this._stored[shortId] !== undefined) stored[shortId] = this._stored[shortId];
    // The terms came from these callbacks: loading needs them again.
    const callbackOptions = ['tokenize', 'processTerm'].filter(key => customized(this._options, key));
    // 0: a hole, 1: null, 2: an average.
    const averages = Array.from(this._averages, set => set === undefined ? 0 : set === null ? 1 : 2);
    return { idField: this._merged.idField, storeFields: this._merged.storeFields ?? [], callbackOptions, averages, ids, stored };
  }
  toNativeJSON() {
    this._check();
    if (!this._js) return { format: 'minisearch-wasm/identity', version: 1, index: this._nativeSnapshot(core => core.toNativeJSON()), identity: this._identity() };
    return { format: 'minisearch-wasm/compat', version: 1, options: this._options, callbackOptions: callbackPaths(this._options), index: this.toJSON(), tree: snapshotTree(this._js._index._tree) };
  }
  toNativeJSONString() { return JSON.stringify(this.toNativeJSON()); }
  // `replace` takes the caller's options as they are (loadJSON); otherwise they
  // are laid over the options saved in the snapshot. Search callbacks are not
  // part of a native snapshot: pass them again to keep them.
  static _fromCore(core, options, identity) {
    const result = Object.create(this.prototype);
    try {
      const saved = core.getOptions();
      if (identity) {
        if (identity === null || typeof identity !== 'object' || !Array.isArray(identity.ids) || identity.stored === null ||
            typeof identity.stored !== 'object' || !core.externallyIdentified) throw invalidSnapshot('identity');
        saved.idField = identity.idField;
        saved.storeFields = identity.storeFields;
      }
      const merged = mergeOptions(saved, options);
      for (const path of identity?.callbackOptions ?? []) {
        if (typeof merged[path] !== 'function') throw new Error(`MiniSearch: loading this snapshot requires callback option "${path}"`);
      }
      result._init(merged);
      result._wasm = adoptCore(core);
      if (identity) {
        if (!result._adoptLive(core.liveIds(), identity.ids, identity.stored)) throw invalidSnapshot('identity does not match the index');
        const averages = identity.averages ?? [];
        result._averages.length = averages.length;
        averages.forEach((kind, fieldId) => { if (kind) result._averages[fieldId] = kind === 1 ? null : true; });
      } else {
        // A snapshot whose engine kept the ids (0.9 to 0.11). Stored names
        // come back in storeFields order, the order the original stores them in.
        const { ids, stored } = core.externalize();
        const order = result._merged.storeFields ?? [];
        const fields = JSON.parse(stored);
        for (const key of Object.keys(fields)) {
          const values = fields[key], ordered = {};
          for (const name of order) if (Object.hasOwn(values, name)) ordered[name] = values[name];
          fields[key] = Object.assign(ordered, values);
        }
        if (!result._adopt(JSON.parse(ids), fields)) throw invalidSnapshot('inconsistent document ids');
        // These snapshots kept an average for every field.
        result._averages = result._options.fields.map(() => true);
      }
    } catch (error) {
      core.free();
      throw error;
    }
    if (needsJavaScript(result._options)) result._promote();
    return result;
  }
  static loadNativeJSON(json, options) {
    const snapshot = JSON.parse(json);
    if (snapshot.format === 'minisearch-wasm/compat') {
      if (snapshot.version !== 1) throw new Error('MiniSearch: incompatible compatibility snapshot version');
      const merged = mergeOptions(snapshot.options, options);
      for (const path of snapshot.callbackOptions ?? []) {
        if (typeof path.split('.').reduce((value, key) => value?.[key], merged) !== 'function') throw new Error(`MiniSearch: loading this snapshot requires callback option "${path}"`);
      }
      const js = Original.loadJS(snapshot.index, originalOptions(merged));
      if (snapshot.tree) js._index = new SearchableMap(orderedTree(snapshot.tree));
      return this._fromJS(js, merged);
    }
    if (snapshot.format === 'minisearch-wasm/identity') {
      if (snapshot.version !== 1) throw new Error('MiniSearch: incompatible snapshot version');
      return this._fromCore(nativeLoader().loadNativeJSON(JSON.stringify(snapshot.index)), options, snapshot.identity ?? null);
    }
    return this._fromCore(nativeLoader().loadNativeJSON(json), options);
  }
  toBytes() {
    this._check();
    if (!this._js) {
      const core = this._nativeSnapshot(engine => engine.toBytes());
      const identity = new TextEncoder().encode(JSON.stringify(this._identity()));
      const start = identityMagic.length + 4;
      const result = new Uint8Array(start + core.length + identity.length);
      result.set(identityMagic);
      new DataView(result.buffer).setUint32(identityMagic.length, core.length, true);
      result.set(core, start);
      result.set(identity, start + core.length);
      return result;
    }
    const payload = new TextEncoder().encode(this.toNativeJSONString());
    const result = new Uint8Array(magic.length + payload.length);
    result.set(magic); result.set(payload, magic.length); return result;
  }
  static loadBytes(bytes, options) {
    bytes = asBytes(bytes);
    if (startsWith(bytes, magic)) return this.loadNativeJSON(decode(bytes.subarray(magic.length)), options);
    if (startsWith(bytes, identityMagic)) {
      const start = identityMagic.length + 4;
      if (bytes.length < start) throw invalidSnapshot('truncated');
      const length = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(identityMagic.length, true);
      if (bytes.length - start < length) throw invalidSnapshot('truncated');
      let identity;
      try { identity = JSON.parse(decode(bytes.subarray(start + length))); } catch { throw invalidSnapshot('identity'); }
      return this._fromCore(nativeLoader().loadBytes(bytes.subarray(start, start + length)), options, identity ?? null);
    }
    return this._fromCore(nativeLoader().loadBytes(bytes), options);
  }
  _nativeSnapshot(write) {
    const core = this._engine();
    core.setAutoVacuum(this._options.autoVacuum ?? true);
    try { return write(core); } finally { core.setAutoVacuum(false); }
  }
  free() {
    if (this._freed) return;
    this._wasm?.free(); this._wasm = undefined; this._js = undefined; this._freed = true;
    this._ids = this._stored = this._shortIds = undefined;
    this._pending = null;
  }
}

// Backwards-compatible callable initializer, and an original-style constructor.
// Before init(), construction uses JS; after init(), eligible new indexes use
// Wasm. Node's entry initializes synchronously on import.
export default function MiniSearch(options) {
  if (new.target) return Reflect.construct(MiniSearchWasm, [options], new.target);
  return init(options);
}
MiniSearch.prototype = MiniSearchWasm.prototype;
Object.setPrototypeOf(MiniSearch, MiniSearchWasm);
MiniSearch.init = init;
MiniSearch.initSync = initSync;
MiniSearch.MiniSearchWasm = MiniSearchWasm;
export { MiniSearch };
if (Symbol.dispose) Object.defineProperty(MiniSearchWasm.prototype, Symbol.dispose, { value: MiniSearchWasm.prototype.free });
