// The public compatibility boundary. The Rust engine remains the fast path for
// JSON documents and declarative queries. JavaScript semantics that cannot cross
// a JSON boundary use the pinned upstream implementation, with a one-time,
// order-preserving transfer of the index. Never keep two indexes in sync.
import Original from 'minisearch';
import SearchableMap from 'minisearch/SearchableMap';
import coreInit, { initSync as coreInitSync, MiniSearchWasm as Core } from './minisearch_wasm_core.js';

let initialized = false;
export async function init(input) { const result = await coreInit(input); initialized = true; return result; }
export function initSync(input) { const result = coreInitSync(input); initialized = true; return result; }
const wait = () => new Promise(resolve => setTimeout(resolve, 0));
const magic = new TextEncoder().encode('MSWJS01\n');
const callbacks = ['extractField', 'stringifyField', 'tokenize', 'processTerm', 'logger', 'filter', 'boostDocument', 'boostTerm', 'prefix', 'fuzzy'];
const callbackPaths = (options, prefix = '') => {
  const paths = callbacks.filter(key => typeof options?.[key] === 'function').map(key => prefix + key);
  for (const key of ['searchOptions', 'autoSuggestOptions']) paths.push(...callbackPathsNested(options?.[key], prefix + key + '.'));
  return paths;
};
const callbackPathsNested = (value, prefix) => value ? callbackPaths(value, prefix) : [];
const needsCallbacks = options => callbackPaths(options).length > 0;
const queryCallbacks = query => query && typeof query === 'object' &&
  (needsCallbacks(query) || query.queries?.some(queryCallbacks));
const equalValue = (a, b) => a === b || (a && b && typeof a === 'object' && typeof b === 'object' &&
  Object.keys(a).length === Object.keys(b).length && Object.keys(a).every(key => Object.hasOwn(b, key) && equalValue(a[key], b[key])));

function searchOptions(options = {}, idField = 'id') {
  const result = { ...options };
  for (const key of ['prefix', 'fuzzy', 'boostTerm']) {
    if (Array.isArray(result[key])) {
      const values = result[key];
      result[key] = (_term, i) => values[i] ?? (key === 'boostTerm' ? 1 : false);
    }
  }
  if (result.filter && typeof result.filter !== 'function') {
    const fields = result.filter;
    // Like the Wasm engine: the id field matches the document id even when it
    // is not stored; result rows expose it as `id`.
    result.filter = row => Object.entries(fields).every(([key, value]) => equalValue(key === idField ? row.id : row[key], value));
  }
  if (result.bm25) result.bm25 = { k: 1.2, b: 0.7, d: 0.5, ...result.bm25 };
  if (result.weights) result.weights = { fuzzy: 0.45, prefix: 0.375, ...result.weights };
  return result;
}

function originalOptions(options) {
  const idField = options.idField ?? 'id';
  const result = { ...options, searchOptions: searchOptions(options.searchOptions, idField) };
  if (options.autoSuggestOptions) result.autoSuggestOptions = searchOptions(options.autoSuggestOptions, idField);
  if (options.tokenizer === 'jobboard') {
    result.tokenize ??= text => text.split(/[^\p{Alphabetic}\p{Number}+#.]+/u).filter(Boolean);
    result.processTerm ??= term => term.toLowerCase().replace(/\.+$/, '');
  }
  return result;
}

function originalQuery(query, idField) {
  if (query === Symbol.for('minisearch-wasm.wildcard') || query === Original.wildcard) return Original.wildcard;
  if (query && typeof query === 'object') return { ...searchOptions(query, idField), queries: query.queries.map(subquery => originalQuery(subquery, idField)) };
  return query;
}

function coreQuery(query) {
  if (query === Original.wildcard) return Core.wildcard;
  if (query && typeof query === 'object' && Array.isArray(query.queries)) return { ...query, queries: query.queries.map(coreQuery) };
  return query;
}

function needsValues(document, options) {
  if (!document || typeof document !== 'object' || ![null, Object.prototype].includes(Object.getPrototypeOf(document))) return true;
  for (const key of [options.idField ?? 'id', ...options.fields, ...(options.storeFields ?? [])]) {
    const descriptor = Object.getOwnPropertyDescriptor(document, key);
    if (descriptor?.get || descriptor?.set) return true;
    const value = descriptor?.value;
    if (value != null && !['string', 'boolean', 'number'].includes(typeof value)) return true;
    if (typeof value === 'number' && !Number.isFinite(value)) return true;
  }
  return false;
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
  const remap = map => new Map([...map].map(([id, value]) => [mapping.get(id), value]));
  for (const data of index._index.values()) {
    for (const postings of data.values()) {
      for (const id of postings.keys()) if (!mapping.has(id)) throw new Error('MiniSearch: vacuum must remove stale postings before compacting the index');
    }
  }
  for (const data of index._index.values()) {
    for (const [field, postings] of data) data.set(field, remap(postings));
  }
  index._documentIds = remap(index._documentIds);
  index._fieldLength = remap(index._fieldLength);
  index._storedFields = remap(index._storedFields);
  index._idToShortId = new Map([...index._documentIds].map(([id, external]) => [external, id]));
  index._nextId = mapping.size;
  return true;
}

export class MiniSearchWasm {
  constructor(options) {
    if (!options?.fields) throw new Error('MiniSearch: option "fields" must be provided');
    this._options = { ...options };
    this._version = 0n;
    this._freed = false;
    // Native auto-vacuum is disabled: the facade owns scheduling and preserves
    // the original active/queued Promise completion boundaries.
    if (initialized && !needsCallbacks(options)) this._wasm = new Core({ ...options, autoVacuum: false });
    else this._js = new Original(originalOptions(options));
  }
  static get wildcard() { return Original.wildcard; }
  static getDefault(name) { return name === 'tokenizer' ? 'default' : Original.getDefault(name); }
  get executionMode() { this._check(); return this._js ? 'javascript' : 'wasm'; }
  _check() { if (this._freed) throw new Error('MiniSearch: index has been freed'); }
  _engine() { this._check(); return this._js ?? this._wasm; }
  _promote() {
    this._check();
    if (!this._js) {
      const js = Original.loadJS(this._wasm.toJSON(), originalOptions(this._options));
      js._index = new SearchableMap(orderedTree(this._wasm.toNativeJSON().index.root));
      this._wasm.free();
      this._wasm = undefined;
      this._js = js;
    }
    return this._js;
  }
  _document(document) {
    this._check();
    if (!this._js && needsValues(document, this._options)) this._promote();
    return this._engine();
  }
  _mutate(fn) {
    try { return fn(); } finally { this._version++; }
  }
  add(document) { return this._mutate(() => this._document(document).add(document)); }
  addAll(documents) { for (const document of documents) this.add(document); }
  addAllJSON(json) {
    const documents = JSON.parse(json);
    if (!Array.isArray(documents)) throw new Error('MiniSearch: documents must be an array');
    if (this._js || documents.some(document => needsValues(document, this._options))) return this.addAll(documents);
    return this._mutate(() => this._wasm.addAllJSON(json));
  }
  async addAllAsync(documents, { chunkSize = 10 } = {}) {
    this._check();
    const size = chunkSize > 0 ? Math.max(1, Math.floor(chunkSize)) : Math.max(1, documents.length);
    for (let start = 0; start < documents.length; start += size) {
      if (start + size <= documents.length) await wait();
      const end = Math.min(start + size, documents.length);
      for (let i = start; i < end; i++) this.add(documents[i]);
    }
  }
  remove(document) { return this._mutate(() => this._document(document).remove(document)); }
  removeAll(documents) {
    if (arguments.length === 0) {
      this._engine().removeAll();
      this._version++;
    } else if (!documents) {
      throw new Error('Expected documents to be present. Omit the argument to remove all documents.');
    } else for (const document of documents) this.remove(document);
  }
  discard(id) {
    this._mutate(() => this._engine().discard(id));
    this._autoVacuum();
  }
  discardAll(ids) {
    this._mutate(() => this._engine().discardAll(ids));
    this._autoVacuum();
  }
  replace(document) {
    this._mutate(() => this._document(document).replace(document));
    this._autoVacuum();
  }
  has(id) {
    this._check();
    if (!this._js && !['string', 'number', 'boolean'].includes(typeof id)) return false;
    if (!this._js && typeof id === 'number' && !Number.isFinite(id)) return false;
    return this._engine().has(id);
  }
  getStoredFields(id) { return this._promote().getStoredFields(id); }
  _autoVacuum() {
    if (this._js) {
      if (this._js._currentVacuum) this._watchVacuum(this._js._currentVacuum);
      if (this._js._enqueuedVacuum) this._watchVacuum(this._js._enqueuedVacuum);
      return;
    }
    if (this._options.autoVacuum === false) return;
    const options = typeof this._options.autoVacuum === 'object' ? this._options.autoVacuum : {};
    // `||`, not `??`: upstream's conditionalVacuum also replaces an explicit 0
    // with the default, so this keeps both engines identical.
    if (this.dirtCount >= (options.minDirtCount || 20) && this.dirtFactor >= (options.minDirtFactor || 0.1)) this.vacuum(options);
  }
  vacuum(options) {
    const js = this._promote();
    const promise = js.vacuum(options);
    return this._watchVacuum(promise);
  }
  _watchVacuum(promise) {
    const js = this._js;
    // The original reuses its current and queued Promises across calls, so
    // attach the cleanup only once per Promise.
    if (watchedVacuums.has(promise)) return promise;
    watchedVacuums.add(promise);
    // Do not wrap the Promise: the original reuses one queued Promise. Cleanup
    // only after every queued run and any newly created dirt has been handled.
    promise.then(() => {
      if (!this._freed && !js.isVacuuming && !js.dirtCount && compactOriginal(js)) this._version++;
    }, () => {});
    return promise;
  }
  compact() {
    if (this.isVacuuming || this.dirtCount) throw new Error('MiniSearch: vacuum must finish before compacting the index');
    if (this._js) { if (compactOriginal(this._js)) this._version++; }
    else { this._wasm.compact(); this._version++; }
  }
  get documentCount() { return this._engine().documentCount; }
  get termCount() { return this._engine().termCount; }
  get dirtCount() { return this._engine().dirtCount; }
  get dirtFactor() { return this._engine().dirtFactor; }
  get isVacuuming() { return this._engine().isVacuuming; }
  get idTableVersion() { this._check(); return String(this._version); }
  search(query, options = {}) {
    if (this.dirtCount || needsCallbacks(options) || queryCallbacks(query)) this._promote();
    const rows = this._js
      ? this._js.search(originalQuery(query, this._options.idField ?? 'id'), searchOptions(options, this._options.idField ?? 'id'))
      : this._wasm.search(coreQuery(query), options);
    if (this._js && options.includeMatch === false) for (const row of rows) delete row.match;
    return rows;
  }
  autoSuggest(query, options = {}) {
    if (this.dirtCount || needsCallbacks(options)) this._promote();
    return this._js ? this._js.autoSuggest(query, searchOptions(options, this._options.idField ?? 'id')) : this._wasm.autoSuggest(query, options);
  }
  searchJoined(query, orMode) {
    if (!this._js && !this.dirtCount) return this._wasm.searchJoined(query, orMode);
    return this.searchJoinedOpts(query, orMode ? { combineWith: 'OR' } : {});
  }
  searchJoinedOpts(query, options) {
    if (!this._js && !this.dirtCount && !needsCallbacks(options)) return this._wasm.searchJoinedOpts(query, options);
    const rows = this.search(query, options);
    return { count: rows.length, ids: JSON.stringify(rows.map(row => row.id)), scores: Float64Array.from(rows, row => row.score), terms: rows.map(row => row.terms.join(' ')).join('\n') };
  }
  searchRaw(query, options) {
    if (!this._js && !this.dirtCount && !needsCallbacks(options)) return { ...this._wasm.searchRaw(query, options), idTableVersion: this.idTableVersion };
    const rows = this.search(query, options), terms = new Map(), termIds = [], offsets = [0];
    for (const row of rows) {
      for (const term of row.terms) { if (!terms.has(term)) terms.set(term, terms.size); termIds.push(terms.get(term)); }
      offsets.push(termIds.length);
    }
    return { count: rows.length, idTableVersion: this.idTableVersion, docIds: Uint32Array.from(rows, row => this._js._idToShortId.get(row.id)),
      scores: Float64Array.from(rows, row => row.score), termTable: [...terms.keys()].join('\n'), termOffsets: Uint32Array.from(offsets), termIds: Uint32Array.from(termIds) };
  }
  docIdTable() {
    if (!this._js) return this._engine().docIdTable();
    const ids = Array(this._js._nextId).fill(null);
    for (const [id, external] of this._js._documentIds) ids[id] = external;
    return JSON.stringify(ids);
  }
  autoSuggestJoined(query) {
    if (!this._js && !this.dirtCount) return this._wasm.autoSuggestJoined(query);
    const rows = this.autoSuggest(query);
    return { count: rows.length, suggestions: rows.map(row => row.suggestion).join('\n'), scores: Float64Array.from(rows, row => row.score) };
  }
  searchCountDefault(query, orMode) { return this.searchJoined(query, orMode).count; }
  searchCountOpts(query, prefix, fuzzy) { return this.search(query, { prefix, fuzzy }).length; }
  toJSON() { return this._engine().toJSON(); }
  toJSONString() { return JSON.stringify(this.toJSON()); }
  toMiniSearchJSON() { return this.toJSONString(); }
  static _fromJS(js, options) {
    const result = Object.create(MiniSearchWasm.prototype);
    Object.assign(result, { _js: js, _options: { ...options }, _version: 0n, _freed: false });
    return result;
  }
  static loadJSON(json, options) { return this._fromJS(Original.loadJSON(json, options == null ? options : originalOptions(options)), options); }
  static loadMiniSearchJSON(json, options) { return this.loadJSON(json, options); }
  static async loadJSONAsync(json, options) {
    const js = await Original.loadJSONAsync(json, options == null ? options : originalOptions(options));
    return this._fromJS(js, options);
  }
  toNativeJSON() {
    if (!this._js) return this._nativeSnapshot(core => core.toNativeJSON());
    return { format: 'minisearch-wasm/compat', version: 1, options: this._options, callbackOptions: callbackPaths(this._options), index: this.toJSON(), tree: snapshotTree(this._js._index._tree) };
  }
  toNativeJSONString() { return JSON.stringify(this.toNativeJSON()); }
  static _fromCore(core) {
    const result = Object.create(MiniSearchWasm.prototype);
    const options = core.getOptions();
    core.setAutoVacuum(false);
    Object.assign(result, { _wasm: core, _options: options, _version: 0n, _freed: false });
    if (core.hasReferenceValues()) result._promote();
    return result;
  }
  static loadNativeJSON(json, options) {
    const snapshot = JSON.parse(json);
    if (snapshot.format === 'minisearch-wasm/compat') {
      if (snapshot.version !== 1) throw new Error('MiniSearch: incompatible compatibility snapshot version');
      const merged = { ...snapshot.options, ...options };
      for (const path of snapshot.callbackOptions ?? []) {
        if (typeof path.split('.').reduce((value, key) => value?.[key], merged) !== 'function') throw new Error(`MiniSearch: loading this snapshot requires callback option "${path}"`);
      }
      const js = Original.loadJS(snapshot.index, originalOptions(merged));
      if (snapshot.tree) js._index = new SearchableMap(orderedTree(snapshot.tree));
      return this._fromJS(js, merged);
    }
    return this._fromCore(Core.loadNativeJSON(json));
  }
  toBytes() {
    if (!this._js) return this._nativeSnapshot(core => core.toBytes());
    const payload = new TextEncoder().encode(this.toNativeJSONString());
    const result = new Uint8Array(magic.length + payload.length);
    result.set(magic); result.set(payload, magic.length); return result;
  }
  static loadBytes(bytes, options) {
    if (magic.every((value, i) => bytes[i] === value)) return this.loadNativeJSON(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(magic.length)), options);
    return this._fromCore(Core.loadBytes(bytes));
  }
  _nativeSnapshot(write) {
    const core = this._engine();
    core.setAutoVacuum(this._options.autoVacuum ?? true);
    try { return write(core); } finally { core.setAutoVacuum(false); }
  }
  free() {
    if (this._freed) return;
    this._wasm?.free(); this._wasm = undefined; this._js = undefined; this._freed = true;
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
