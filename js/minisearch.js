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
// A Wasm index keeps these search callbacks on this side of the boundary:
// per-term functions are evaluated into the engine's per-term arrays, and
// `filter` runs over the finished rows, as it does upstream. `logger` is called
// by the engine itself. Every other callback runs inside indexing or scoring
// and needs the JavaScript engine.
const termCallbacks = ['fuzzy', 'prefix', 'boostTerm'];
const engineCallbacks = ['tokenize', 'processTerm', 'boostDocument'];
const isFunction = value => typeof value === 'function';
const hasEngineCallback = options => !!options && engineCallbacks.some(key => isFunction(options[key]));
const hasNonFinite = value => typeof value === 'number' ? !Number.isFinite(value)
  : !!value && typeof value === 'object' && Object.values(value).some(hasNonFinite);
// Stored fields are assigned over a result's own properties upstream, before
// the results are sorted; the native engine keeps the two apart.
const shadowsResult = (name, idField) => ['score', 'terms', 'queryTerms', 'match'].includes(name) || (name === 'id' && idField !== 'id');
// Configurations only the JavaScript engine reproduces: callbacks that run
// inside tokenization or scoring, and options the original tolerates although
// they have no native form (a `fields` string is iterated by character, a
// repeated field is indexed twice, `Infinity` is a legal boost).
const needsJavaScript = options => ['tokenize', 'processTerm'].some(key => isFunction(options[key])) ||
  hasEngineCallback(options.searchOptions) || hasEngineCallback(options.autoSuggestOptions) ||
  !Array.isArray(options.fields) || new Set(options.fields).size !== options.fields.length ||
  (options.storeFields != null && (!Array.isArray(options.storeFields) || options.storeFields.some(name => shadowsResult(name, options.idField ?? 'id')))) ||
  hasNonFinite(options.searchOptions) || hasNonFinite(options.autoSuggestOptions);
const withoutFunctions = (options, keys) => {
  if (!options || !keys.some(key => isFunction(options[key]))) return options;
  const result = { ...options };
  for (const key of keys) if (isFunction(result[key])) delete result[key];
  return result;
};
// What the native constructor sees: the callbacks the facade evaluates stay in
// `_options`. Options given as `undefined` or `null` mean "default", as they do
// upstream.
const coreOptions = options => {
  const result = { ...options, autoVacuum: false, extractField: undefined, stringifyField: undefined };
  for (const key of ['searchOptions', 'autoSuggestOptions']) result[key] = withoutFunctions(result[key], [...termCallbacks, 'filter']);
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
function definedOptions(options, allFields, hasNativeFilter) {
  if (!Object.values(options).includes(undefined)) return options;
  const result = {};
  for (const [key, value] of Object.entries(options)) {
    if (value !== undefined) result[key] = value;
    else if (key === 'prefix' || key === 'fuzzy') result[key] = false;
    else if (key === 'combineWith') result[key] = 'OR';
    else if (key === 'fields') result[key] = allFields;
    else if (key === 'boostTerm') result[key] = [];
    else if (key === 'weights') result[key] = { fuzzy: 0.45, prefix: 0.375 };
    else if (key === 'filter' ? hasNativeFilter : key !== 'includeMatch') return unsupported;
  }
  return result;
}
const isScalarId = id => typeof id === 'string' || typeof id === 'boolean' || (typeof id === 'number' && Number.isFinite(id));
const isWellFormed = String.prototype.isWellFormed ? text => text.isWellFormed()
  : text => !/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(text);
const callbackPaths = (options, prefix = '') => {
  const paths = callbacks.filter(key => typeof options?.[key] === 'function').map(key => prefix + key);
  for (const key of ['searchOptions', 'autoSuggestOptions']) paths.push(...callbackPathsNested(options?.[key], prefix + key + '.'));
  return paths;
};
const callbackPathsNested = (value, prefix) => value ? callbackPaths(value, prefix) : [];
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
    // Values the boundary would alter: a lone surrogate becomes U+FFFD, -0 becomes 0.
    if (typeof value === 'number' ? !Number.isFinite(value) || Object.is(value, -0) : typeof value === 'string' && !isWellFormed(value)) return true;
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

export class MiniSearchWasm {
  constructor(options) {
    if (!options?.fields) throw new Error('MiniSearch: option "fields" must be provided');
    this._init({ ...options });
    if (initialized && !needsJavaScript(options)) this._wasm = adoptCore(new Core(coreOptions(options)));
    else this._js = new Original(originalOptions(options));
  }
  _init(options) {
    this._options = options;
    this._version = 0n;
    this._freed = false;
    // Live objects returned by getStoredFields in Wasm mode, by document id.
    this._handed = new Map();
    this._currentVacuum = null;
    this._enqueuedVacuum = null;
    this._enqueuedConditions = defaultAutoVacuum;
    // extractField and stringifyField run once per field of a document, so a
    // Wasm index calls them here and indexes the flattened result.
    this._extracts = isFunction(options.extractField) || isFunction(options.stringifyField);
  }
  static get wildcard() { return Original.wildcard; }
  static getDefault(name) { return name === 'tokenizer' ? 'default' : Original.getDefault(name); }
  get executionMode() { this._check(); return this._js ? 'javascript' : 'wasm'; }
  _check() { if (this._freed) throw new Error('MiniSearch: index has been freed'); }
  _engine() { this._check(); return this._js ?? this._wasm; }
  _promote() {
    this._check();
    if (!this._js) {
      // The term index is rebuilt from the native tree, whose key order a list
      // of terms cannot carry; skip building it from the list first.
      const js = Original.loadJS({ ...this._wasm.toJSON(), index: [] }, originalOptions(this._options));
      js._index = new SearchableMap(orderedTree(this._wasm.indexTree()));
      // The original keeps a stored-fields object for every document once
      // `storeFields` is set; callbacks and getStoredFields rely on it.
      if (this._options.storeFields?.length) {
        for (const id of js._documentIds.keys()) if (!js._storedFields.has(id)) js._storedFields.set(id, {});
      }
      // Objects already handed out stay the live stored fields, edits included.
      for (const [id, { object }] of this._handed) js._storedFields.set(js._idToShortId.get(id), object);
      this._handed.clear();
      this._wasm.free();
      this._wasm = undefined;
      this._js = js;
    }
    return this._js;
  }
  _idOf(document) { return (this._options.extractField ?? Original.getDefault('extractField'))(document, this._options.idField ?? 'id'); }
  // The engine for a document, and the document in the form that engine takes.
  _prepare(document, withStored = true) {
    this._check();
    if (!this._js) {
      if (!this._extracts && (document === null || typeof document !== 'object')) {
        // Not a document: raise the original's error, without a transfer.
        new Original(originalOptions(this._options))[withStored ? 'add' : 'remove'](document);
      }
      const plain = this._extracts ? this._flatten(document, withStored) : document;
      if (plain !== unsupported && !needsValues(plain, this._options)) return [this._wasm, plain];
      this._promote();
    }
    return [this._js, document];
  }
  // What the original's add (or remove) reads from a document through its
  // callbacks, as a plain document: the id, the stored values, and each indexed
  // field as the string stringifyField makes of it. `unsupported` when one name
  // would need two different values, or a callback returns no string.
  _flatten(document, withStored) {
    const { idField = 'id', fields, storeFields = [] } = this._options;
    const extractField = this._options.extractField ?? Original.getDefault('extractField');
    const stringifyField = this._options.stringifyField ?? Original.getDefault('stringifyField');
    const plain = {};
    const id = extractField(document, idField);
    if (id !== undefined) plain[idField] = id;
    if (withStored) {
      for (const name of storeFields) {
        const value = extractField(document, name);
        if (value !== undefined) plain[name] = value;
      }
    }
    for (const field of fields) {
      const value = extractField(document, field);
      if (value == null) continue;
      const text = stringifyField(value, field);
      if (typeof text !== 'string') return unsupported;
      if (!Object.hasOwn(plain, field)) plain[field] = text;
      // Also the id or a stored value: the engine derives the indexed text from
      // that value, which has to be the same text.
      else if (!['string', 'number', 'boolean'].includes(typeof plain[field]) || String(plain[field]) !== text) return unsupported;
    }
    return plain;
  }
  _mutate(fn) {
    try { return fn(); } finally { this._version++; }
  }
  add(document) {
    const [engine, plain] = this._prepare(document);
    return this._mutate(() => engine.add(plain));
  }
  addAll(documents) { for (const document of documents) this.add(document); }
  addAllJSON(json) {
    this._check();
    const documents = JSON.parse(json);
    if (!Array.isArray(documents)) throw new Error('MiniSearch: documents must be an array');
    if (this._js || this._extracts || documents.some(document => needsValues(document, this._options))) return this.addAll(documents);
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
  remove(document) {
    const [engine, plain] = this._prepare(document, false);
    this._mutate(() => engine.remove(plain));
    this._handed.delete(this._idOf(document));
  }
  removeAll(documents) {
    if (arguments.length === 0) {
      this._engine().removeAll();
      this._handed.clear();
      this._version++;
    } else if (!documents) {
      throw new Error('Expected documents to be present. Omit the argument to remove all documents.');
    } else for (const document of documents) this.remove(document);
  }
  discard(id) {
    this._discard(id);
    this._autoVacuum();
  }
  _discard(id) {
    this._check();
    // The boundary would coerce other types (1n to 1); no such id is indexed.
    if (!this._js && !isScalarId(id)) throw new Error(`MiniSearch: cannot discard document with ID ${id}: it is not in the index`);
    this._mutate(() => this._engine().discard(id));
    this._handed.delete(id);
  }
  discardAll(ids) {
    this._check();
    if (this._js) this._mutate(() => this._js.discardAll(ids));
    else for (const id of ids) this._discard(id);
    this._autoVacuum();
  }
  replace(document) {
    const [engine] = this._prepare(document);
    if (engine === this._js) {
      this._mutate(() => this._js.replace(document));
      this._autoVacuum();
    } else {
      // Like the original: discard, check the auto-vacuum thresholds, add.
      this.discard(this._idOf(document));
      this.add(document);
    }
  }
  has(id) {
    this._check();
    if (!this._js && !isScalarId(id)) return false;
    return this._engine().has(id);
  }
  // The original returns its own stored-fields object: the same one on every
  // call, and edits to it show up in later results. A Wasm index hands out one
  // object per document, remembers what it held, and moves to JavaScript only
  // when an edit is found (see _syncStored).
  getStoredFields(id) {
    this._check();
    if (this._js) return this._js.getStoredFields(id);
    if (!this.has(id)) return undefined;
    const handed = this._handed.get(id);
    if (handed) return handed.object;
    const object = this._wasm.getStoredFields(id);
    if (object !== undefined) this._handed.set(id, { object, pristine: { ...object } });
    return object;
  }
  _syncStored() {
    for (const { object, pristine } of this._handed.values()) {
      const keys = Object.keys(object), saved = Object.keys(pristine);
      if (keys.length !== saved.length || keys.some((key, i) => key !== saved[i] || !Object.is(object[key], pristine[key]))) {
        this._promote();
        return;
      }
    }
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
        if (!this._freed && !this.isVacuuming && !this.dirtCount && this._compact()) this._version++;
      } catch { /* a mismatched remove left stale postings: keep the ID slots */ }
    }, () => {});
    return promise;
  }
  _compact() {
    if (this._js) return compactOriginal(this._js);
    const before = this._wasm.idTableVersion;
    this._wasm.compact();
    return this._wasm.idTableVersion !== before;
  }
  compact() {
    if (this.isVacuuming || this.dirtCount) throw new Error('MiniSearch: vacuum must finish before compacting the index');
    this._compact();
    this._version++;
  }
  get documentCount() { return this._engine().documentCount; }
  get termCount() { return this._engine().termCount; }
  get dirtCount() { return this._engine().dirtCount; }
  get dirtFactor() { return this._engine().dirtFactor; }
  get isVacuuming() { this._check(); return this._js ? this._js.isVacuuming : this._currentVacuum != null; }
  get idTableVersion() { this._check(); return String(this._version); }
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
    if (!plan.filter) return this._wasm.search(plan.query, plan.options);
    // The original filters finished result objects, `match` included.
    const rows = this._wasm.search(plan.query, { ...plan.options, includeMatch: true }).filter(row => plan.filter(row));
    if (options.includeMatch === false) for (const row of rows) delete row.match;
    return rows;
  }
  // Translate a query for the native engine: `{ query, options, filter }`, or
  // `unsupported` when it needs a callback that runs inside scoring, returns a
  // value the engine has no form for, or the stored fields were edited.
  _plan(query, options) {
    this._syncStored();
    if (this._js || hasEngineCallback(options)) return unsupported;
    const globals = this._options.searchOptions ?? {};
    options = definedOptions(options, this._options.fields, globals.filter != null && !isFunction(globals.filter));
    if (options === unsupported) return unsupported;
    const merged = { ...globals, ...options };
    let filter;
    if (isFunction(merged.filter)) {
      // A declarative default filter lives in the native options and cannot be
      // switched off for one call.
      if (globals.filter != null && !isFunction(globals.filter)) return unsupported;
      filter = merged.filter;
    }
    const inherited = {};
    for (const key of termCallbacks) if (key in merged) inherited[key] = merged[key];
    const rewritten = this._rewrite(query, inherited, true);
    if (rewritten === unsupported) return unsupported;
    return { query: rewritten.query, filter,
      options: { ...withoutFunctions(options, [...termCallbacks, 'filter']), ...rewritten.options } };
  }
  _rewrite(query, inherited, top) {
    if (query === Original.wildcard) return { query: Core.wildcard };
    if (typeof query === 'string') {
      const arrays = this._termArrays(query, inherited);
      if (arrays === unsupported) return unsupported;
      if (top || !arrays) return { query, options: arrays };
      // Per-term values belong to one string: give it a node of its own.
      return { query: { queries: [query], ...arrays } };
    }
    if (!query || typeof query !== 'object' || !Array.isArray(query.queries)) return { query };
    if (hasEngineCallback(query)) return unsupported;
    query = definedOptions(query, this._options.fields, false);
    if (query === unsupported) return unsupported;
    const own = { ...inherited };
    for (const key of termCallbacks) if (key in query) own[key] = query[key];
    const queries = [];
    for (const subquery of query.queries) {
      const rewritten = this._rewrite(subquery, own, false);
      if (rewritten === unsupported) return unsupported;
      queries.push(rewritten.query);
    }
    return { query: { ...withoutFunctions(query, [...termCallbacks, 'filter']), queries } };
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
  _hasSearchCallbacks(...layers) {
    const merged = Object.assign({}, this._options.searchOptions, ...layers);
    return [...termCallbacks, 'filter', ...engineCallbacks].some(key => isFunction(merged[key]));
  }
  autoSuggest(query, options) {
    this._check();
    options ??= {};
    if (!this._js && typeof query === 'string' && !Object.values(options).includes(undefined) &&
        !this._hasSearchCallbacks(this._options.autoSuggestOptions, options)) {
      this._syncStored();
      if (!this._js) return this._wasm.autoSuggest(query, options);
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
    return results.sort((a, b) => b.score - a.score);
  }
  // Options for a native compact search of a string query, or undefined when
  // the rows have to come from search(): callbacks over rows, other queries.
  _compactOptions(query, options) {
    if (this._js || typeof query !== 'string') return undefined;
    const plan = this._plan(query, options);
    return plan === unsupported || plan.filter ? undefined : plan.options;
  }
  searchJoined(query, orMode) {
    this._check();
    if (!this._js && typeof query === 'string' && !this._hasSearchCallbacks()) {
      this._syncStored();
      if (!this._js) return this._wasm.searchJoined(query, orMode);
    }
    return this.searchJoinedOpts(query, orMode ? { combineWith: 'OR' } : {});
  }
  searchJoinedOpts(query, options) {
    this._check();
    options ??= {};
    const native = this._compactOptions(query, options);
    if (native) return this._wasm.searchJoinedOpts(query, native);
    const rows = this._identifiedRows(query, options);
    return { count: rows.length, ids: JSON.stringify(rows.map(row => row.id)), scores: Float64Array.from(rows, row => row.score), terms: rows.map(row => row.terms.join(' ')).join('\n') };
  }
  searchRaw(query, options) {
    this._check();
    options ??= {};
    const native = this._compactOptions(query, options);
    if (native) return { ...this._wasm.searchRaw(query, native), idTableVersion: this.idTableVersion };
    const rows = this._identifiedRows(query, options), terms = new Map(), termIds = [], offsets = [0];
    for (const row of rows) {
      for (const term of row.terms) { if (!terms.has(term)) terms.set(term, terms.size); termIds.push(terms.get(term)); }
      offsets.push(termIds.length);
    }
    const shortIds = this._shortIds();
    return { count: rows.length, idTableVersion: this.idTableVersion, docIds: Uint32Array.from(rows, row => shortIds.get(row.id)),
      scores: Float64Array.from(rows, row => row.score), termTable: [...terms.keys()].join('\n'), termOffsets: Uint32Array.from(offsets), termIds: Uint32Array.from(termIds) };
  }
  // Rows for the compact forms, which identify documents: `id`, `score` and
  // `terms` always are the result's own, where a full row lets a stored field
  // of the same name take their place (it still decides the order, as it does
  // in search()).
  _identifiedRows(query, options) {
    const rows = this.search(query, options);
    const shadowed = this._options.storeFields?.filter(name => shadowsResult(name, this._options.idField ?? 'id'));
    if (!shadowed?.length || !this._js) return rows;
    const idField = this._options.idField ?? 'id', js = this._js;
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
  _shortIds() {
    if (this._js) return this._js._idToShortId;
    if (this._shortIdTable?.version !== this._wasm.idTableVersion) {
      const ids = new Map();
      JSON.parse(this._wasm.docIdTable()).forEach((id, shortId) => { if (id !== null) ids.set(id, shortId); });
      this._shortIdTable = { version: this._wasm.idTableVersion, ids };
    }
    return this._shortIdTable.ids;
  }
  docIdTable() {
    if (!this._js) return this._engine().docIdTable();
    const ids = Array(this._js._nextId).fill(null);
    for (const [id, external] of this._js._documentIds) ids[id] = external;
    return JSON.stringify(ids);
  }
  autoSuggestJoined(query) {
    this._check();
    if (!this._js && typeof query === 'string' && !this._hasSearchCallbacks(this._options.autoSuggestOptions)) {
      this._syncStored();
      if (!this._js) return this._wasm.autoSuggestJoined(query);
    }
    const rows = this.autoSuggest(query);
    return { count: rows.length, suggestions: rows.map(row => row.suggestion).join('\n'), scores: Float64Array.from(rows, row => row.score) };
  }
  searchCountDefault(query, orMode) { return this.searchJoined(query, orMode).count; }
  searchCountOpts(query, prefix, fuzzy) { return this.search(query, { prefix, fuzzy }).length; }
  toJSON() {
    this._check();
    if (this._js) return this._js.toJSON();
    this._syncStored();
    if (this._js) return this._js.toJSON();
    const json = this._wasm.toJSON();
    // The original serializes a stored-fields object for every document.
    if (this._options.storeFields?.length) for (const id of Object.keys(json.documentIds)) json.storedFields[id] ??= {};
    return json;
  }
  toJSONString() { return JSON.stringify(this.toJSON()); }
  toMiniSearchJSON() { return this.toJSONString(); }
  static _fromJS(js, options) {
    const result = Object.create(MiniSearchWasm.prototype);
    result._init({ ...options });
    result._js = js;
    return result;
  }
  // Native loading keeps a MiniSearch JSON index in Wasm. The native reader is
  // stricter than the original, and object ids or values need JavaScript, so
  // anything it refuses is loaded (or rejected) by the original loader.
  static _loadJSONNative(json, options) {
    if (!initialized || typeof json !== 'string' || options == null || needsJavaScript(options)) return undefined;
    let core;
    try { core = Core.loadJSON(json, coreOptions(options)); } catch { return undefined; }
    return this._fromCore(core, options, true);
  }
  static loadJSON(json, options) {
    return this._loadJSONNative(json, options) ??
      this._fromJS(Original.loadJSON(json, options == null ? options : originalOptions(options)), options);
  }
  static loadMiniSearchJSON(json, options) { return this.loadJSON(json, options); }
  static async loadJSONAsync(json, options) {
    if (initialized && options != null && !needsJavaScript(options)) {
      // The native loader is synchronous: yield once, then load.
      await wait();
      const native = this._loadJSONNative(json, options);
      if (native) return native;
    }
    const js = await Original.loadJSONAsync(json, options == null ? options : originalOptions(options));
    return this._fromJS(js, options);
  }
  toNativeJSON() {
    this._check();
    if (!this._js) this._syncStored();
    if (!this._js) return this._nativeSnapshot(core => core.toNativeJSON());
    return { format: 'minisearch-wasm/compat', version: 1, options: this._options, callbackOptions: callbackPaths(this._options), index: this.toJSON(), tree: snapshotTree(this._js._index._tree) };
  }
  toNativeJSONString() { return JSON.stringify(this.toNativeJSON()); }
  // `replace` takes the caller's options as they are (loadJSON); otherwise they
  // are laid over the options saved in the snapshot. Search callbacks are not
  // part of a native snapshot: pass them again to keep them.
  static _fromCore(core, options, replace = false) {
    const result = Object.create(MiniSearchWasm.prototype);
    result._init(replace ? { ...options } : mergeOptions(core.getOptions(), options));
    result._wasm = adoptCore(core);
    if (core.hasReferenceValues() || needsJavaScript(result._options)) result._promote();
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
    return this._fromCore(nativeLoader().loadNativeJSON(json), options);
  }
  toBytes() {
    this._check();
    if (!this._js) this._syncStored();
    if (!this._js) return this._nativeSnapshot(core => core.toBytes());
    const payload = new TextEncoder().encode(this.toNativeJSONString());
    const result = new Uint8Array(magic.length + payload.length);
    result.set(magic); result.set(payload, magic.length); return result;
  }
  static loadBytes(bytes, options) {
    if (bytes instanceof ArrayBuffer) bytes = new Uint8Array(bytes);
    if (magic.every((value, i) => bytes[i] === value)) return this.loadNativeJSON(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(magic.length)), options);
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
    this._handed.clear();
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
