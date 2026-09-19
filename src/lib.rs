mod js_math;
mod mini_search;
mod searchable_map;
mod separators;

pub use js_math::log as js_log;
pub use mini_search::{
    AutoSuggestOptions, AutoVacuumOptions, AutoVacuumSetting, Bm25Params, CombineWith,
    CompactSearchResult, CompatTransfer, FuzzySetting, JoinedSearchResults, MatchInfo, MiniSearch,
    MiniSearchOptions, PackedSearchResults, PartialSearchOptions, PrefixSetting, Query,
    QueryCombination, RawSearchResults, SearchOptions, SearchResult, Suggestion, TokenizerMode,
    VacuumOptions, Weights,
};
pub use searchable_map::{FuzzyMatch, SearchableMap};
pub use separators::UNICODE_VERSION;

use js_sys::{Array, Float64Array, Function, Object, Promise, Reflect, Uint32Array};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{future_to_promise, JsFuture};

/// Registry key of the wildcard query symbol (`Symbol.for(WILDCARD_KEY)`), so
/// `MiniSearchWasm.wildcard` returns the same symbol on every access.
const WILDCARD_KEY: &str = "minisearch-wasm.wildcard";

#[derive(Default)]
struct VacuumRuntime {
    active: bool,
    /// A vacuum requested while one is running: the options of the first such
    /// request, and whether every request so far was automatic. Like JS
    /// MiniSearch's queued vacuum, it then runs only if the auto-vacuum
    /// conditions still hold; one manual request makes it unconditional.
    rerun: Option<(mini_search::ResolvedVacuumOptions, bool)>,
    current: Option<Promise>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddAllAsyncOptions {
    #[serde(default = "default_async_chunk_size")]
    chunk_size: usize,
}

#[wasm_bindgen]
pub struct MiniSearchWasm {
    inner: Rc<RefCell<MiniSearch>>,
    vacuum_runtime: Rc<RefCell<VacuumRuntime>>,
    /// MiniSearch's `logger` option; `None` uses `console[level]` like JS.
    logger: Option<Function>,
    /// Answer queries on a dirty index exactly like JS MiniSearch, including
    /// its lazy removal of stale postings. Off by default: the queries of a
    /// bare core instance never mutate the index.
    exact_dirty_queries: Cell<bool>,
}

#[wasm_bindgen]
impl MiniSearchWasm {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "MiniSearchWasmOptions")] options: JsValue,
    ) -> Result<MiniSearchWasm, JsValue> {
        let (options, logger) = parse_constructor_options(&options)?;
        Ok(MiniSearchWasm::from_inner(MiniSearch::new(options)).with_logger(logger))
    }

    /// MiniSearch's `getDefault(optionName)`: the default value of a
    /// constructor option. The callback defaults are returned as JavaScript
    /// functions equivalent to the built-in behavior.
    #[wasm_bindgen(js_name = getDefault, unchecked_return_type = "unknown")]
    pub fn get_default_js(
        #[wasm_bindgen(unchecked_param_type = "string")] option_name: JsValue,
    ) -> Result<JsValue, JsValue> {
        let option_name = string_arg(&option_name, "the option name")?;
        let option_name = option_name.as_str();
        Ok(match option_name {
            "idField" => JsValue::from_str("id"),
            "fields" | "searchOptions" | "autoSuggestOptions" => JsValue::UNDEFINED,
            "storeFields" => Array::new().into(),
            "autoVacuum" => JsValue::TRUE,
            "tokenizer" => JsValue::from_str("default"),
            "extractField" | "stringifyField" | "tokenize" | "processTerm" | "logger" => {
                callback_default(option_name)?
            }
            _ => {
                return Err(js_error(&format!(
                    "MiniSearch: unknown option \"{option_name}\""
                )))
            }
        })
    }

    #[wasm_bindgen(js_name = add)]
    pub fn add_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "object")] document: JsValue,
    ) -> Result<(), JsValue> {
        let document = normalize_document(&document, &self.schema())?;

        self.inner
            .borrow_mut()
            .add(document)
            .map_err(|err| js_error(&err))
    }

    #[wasm_bindgen(js_name = addAll)]
    pub fn add_all_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "readonly object[]")] documents: JsValue,
    ) -> Result<(), JsValue> {
        let documents = normalize_documents(&documents, &self.schema())?;

        self.inner
            .borrow_mut()
            .add_all(documents)
            .map_err(|err| js_error(&err))
    }

    #[wasm_bindgen(js_name = addAllAsync, skip_typescript)]
    pub fn add_all_async_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "readonly object[]")] documents: JsValue,
        #[wasm_bindgen(unchecked_param_type = "AddAllAsyncOptions")] options: Option<JsValue>,
    ) -> Result<Promise, JsValue> {
        if !Array::is_array(&documents) {
            return Err(js_error("MiniSearch: documents must be an array"));
        }
        let documents: Array = documents.unchecked_into();
        let schema = self.schema();
        let options: AddAllAsyncOptions = match options {
            None => AddAllAsyncOptions {
                chunk_size: default_async_chunk_size(),
            },
            Some(options) => serde_wasm_bindgen::from_value(options).map_err(boundary_error)?,
        };
        let inner = Rc::clone(&self.inner);

        Ok(future_to_promise(async move {
            let size = if options.chunk_size == 0 {
                documents.length().max(1) as usize
            } else {
                options.chunk_size
            };
            for start in (0..documents.length() as usize).step_by(size) {
                // Yield before inspecting or converting a full batch.
                if start + size <= documents.length() as usize {
                    yield_to_timer(0).await?;
                }
                let end = (start + size).min(documents.length() as usize);
                for i in start..end {
                    let document = normalize_document(&documents.get(i as u32), &schema)?;
                    inner
                        .borrow_mut()
                        .add(document)
                        .map_err(|err| js_error(&err))?;
                }
            }
            Ok(JsValue::UNDEFINED)
        }))
    }

    #[wasm_bindgen(js_name = addAllJSON)]
    pub fn add_all_json_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] documents: JsValue,
    ) -> Result<(), JsValue> {
        let documents = &string_arg(&documents, "the documents JSON")?;
        let documents: Vec<Value> = serde_json::from_str(documents).map_err(boundary_error)?;

        self.inner
            .borrow_mut()
            .add_all(documents)
            .map_err(|err| js_error(&err))
    }

    #[wasm_bindgen(js_name = remove)]
    pub fn remove_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "object")] document: JsValue,
    ) -> Result<(), JsValue> {
        let document = normalize_document(&document, &self.schema())?;
        let mut warnings = Vec::new();
        let result = self
            .inner
            .borrow_mut()
            .remove_with_warnings(&document, &mut warnings);
        self.log_version_conflicts(&warnings);
        result.map_err(|err| js_error(&err))
    }

    #[wasm_bindgen(js_name = removeAll, skip_typescript)]
    pub fn remove_all_js(&self, documents: JsValue) -> Result<(), JsValue> {
        // A plain `JsValue`, not `Option`: wasm-bindgen treats `null` like an
        // omitted argument, but JS MiniSearch throws for `removeAll(null)`.
        if documents.is_undefined() {
            self.inner.borrow_mut().remove_all_documents();
            return Ok(());
        }

        // Like JS: a falsy argument gets MiniSearch's message, any other
        // non-iterable value the TypeError `for…of` would raise.
        if documents.is_falsy() {
            return Err(js_error(
                "Expected documents to be present. Omit the argument to remove all documents.",
            ));
        }
        if !Array::is_array(&documents) {
            return Err(js_sys::TypeError::new("documents is not iterable").into());
        }

        let documents = normalize_documents(&documents, &self.schema())?;
        let mut warnings = Vec::new();
        let result = self
            .inner
            .borrow_mut()
            .remove_all_with_warnings(documents, &mut warnings);
        self.log_version_conflicts(&warnings);
        result.map_err(|err| js_error(&err))
    }

    #[wasm_bindgen(js_name = discard)]
    pub fn discard_js(&self, id: JsValue) -> Result<(), JsValue> {
        let id: Value = serde_wasm_bindgen::from_value(id).map_err(boundary_error)?;

        self.inner
            .borrow_mut()
            .discard_deferred(&id)
            .map_err(|err| js_error(&err))?;
        self.maybe_schedule_auto_vacuum();
        Ok(())
    }

    #[wasm_bindgen(js_name = discardAll)]
    pub fn discard_all_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "readonly any[]")] ids: JsValue,
    ) -> Result<(), JsValue> {
        let ids: Vec<Value> = serde_wasm_bindgen::from_value(ids).map_err(boundary_error)?;

        self.inner
            .borrow_mut()
            .discard_all_deferred(&ids)
            .map_err(|err| js_error(&err))?;
        self.maybe_schedule_auto_vacuum();
        Ok(())
    }

    /// MiniSearch-compatible `replace(document)`: discards the previous version
    /// of the document (by its id field) and adds the new one.
    #[wasm_bindgen(js_name = replace)]
    pub fn replace_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "object")] document: JsValue,
    ) -> Result<(), JsValue> {
        let document = normalize_document(&document, &self.schema())?;

        self.inner
            .borrow_mut()
            .replace_deferred(document)
            .map_err(|err| js_error(&err))?;
        self.maybe_schedule_auto_vacuum();
        Ok(())
    }

    /// MiniSearch-compatible `has(id)`.
    #[wasm_bindgen(js_name = has)]
    pub fn has_js(&self, id: JsValue) -> Result<bool, JsValue> {
        let id: Value = serde_wasm_bindgen::from_value(id).map_err(boundary_error)?;
        Ok(self.inner.borrow().has(&id))
    }

    /// MiniSearch-compatible `getStoredFields(id)`: the document's stored
    /// fields, or `undefined` when it is not in the index.
    #[wasm_bindgen(
        js_name = getStoredFields,
        unchecked_return_type = "Record<string, unknown> | undefined"
    )]
    pub fn get_stored_fields_js(&self, id: JsValue) -> Result<JsValue, JsValue> {
        let id: Value = serde_wasm_bindgen::from_value(id).map_err(boundary_error)?;
        match self.inner.borrow().stored_fields_of(&id) {
            Some(fields) => {
                let object = Object::new();
                for (key, value) in &fields {
                    let _ = Reflect::set(&object, &JsValue::from_str(key), &json_to_js(value));
                }
                Ok(object.into())
            }
            None => Ok(JsValue::UNDEFINED),
        }
    }

    #[wasm_bindgen(js_name = vacuum, skip_typescript)]
    pub fn vacuum_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "VacuumOptions")] options: Option<JsValue>,
    ) -> Result<Promise, JsValue> {
        let options: VacuumOptions = match options {
            None => VacuumOptions::default(),
            Some(options) => serde_wasm_bindgen::from_value(options).map_err(boundary_error)?,
        };

        Ok(schedule_vacuum(
            Rc::clone(&self.inner),
            Rc::clone(&self.vacuum_runtime),
            options.resolved(),
            false,
        ))
    }

    #[wasm_bindgen(getter, js_name = isVacuuming)]
    pub fn is_vacuuming_js(&self) -> bool {
        self.vacuum_runtime.borrow().active || self.inner.borrow().is_vacuuming()
    }

    /// Reclaim dense tables after vacuuming. Changes `idTableVersion`.
    pub fn compact(&self) -> Result<(), JsValue> {
        if self.vacuum_runtime.borrow().active {
            return Err(js_error(
                "MiniSearch: vacuum must finish before compacting the index",
            ));
        }
        self.inner
            .borrow_mut()
            .compact()
            .map_err(|err| js_error(&err))
    }

    /// The JS facade owns scheduling and restores this setting for snapshots.
    #[wasm_bindgen(js_name = setAutoVacuum)]
    pub fn set_auto_vacuum_js(&self, setting: JsValue) -> Result<(), JsValue> {
        let setting = serde_wasm_bindgen::from_value(setting).map_err(boundary_error)?;
        self.inner.borrow_mut().set_auto_vacuum(setting);
        Ok(())
    }

    /// Facade switch: answer queries on a dirty index exactly like JS
    /// MiniSearch, which removes stale postings as a query meets them.
    #[wasm_bindgen(js_name = setExactDirtyQueries)]
    pub fn set_exact_dirty_queries_js(&self, enabled: bool) {
        self.exact_dirty_queries.set(enabled);
    }

    /// One synchronous slice of a vacuum run, for a JS scheduler that owns the
    /// timers and Promises: cleans at most `max_terms` terms, starting a run
    /// when none is active. Returns `true` once the run is complete.
    #[wasm_bindgen(js_name = vacuumStep)]
    pub fn vacuum_step_js(&self, max_terms: u32) -> bool {
        self.inner.borrow_mut().vacuum_step(max_terms as usize)
    }

    /// The processed terms of a query string, as `search` derives them. Lets
    /// the facade evaluate per-term callbacks with the engine's tokenization.
    #[wasm_bindgen(js_name = queryTerms)]
    pub fn query_terms_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] query: JsValue,
    ) -> Result<Vec<String>, JsValue> {
        Ok(self.inner.borrow().query_terms(&query_arg(&query)?))
    }

    /// Internal facade metadata, without serializing the entire index on load.
    #[wasm_bindgen(js_name = getOptions)]
    pub fn get_options_js(&self) -> Result<JsValue, JsValue> {
        to_json_compatible_value(self.inner.borrow().options()).map_err(|err| js_error(&err))
    }

    #[wasm_bindgen(js_name = hasReferenceValues)]
    pub fn has_reference_values_js(&self) -> bool {
        self.inner.borrow().has_reference_values()
    }

    #[wasm_bindgen(getter, js_name = dirtCount)]
    pub fn dirt_count_js(&self) -> f64 {
        self.inner.borrow().dirt_count() as f64
    }

    #[wasm_bindgen(getter, js_name = dirtFactor)]
    pub fn dirt_factor_js(&self) -> f64 {
        self.inner.borrow().dirt_factor()
    }

    #[wasm_bindgen(getter, js_name = documentCount)]
    pub fn document_count_js(&self) -> f64 {
        self.inner.borrow().document_count() as f64
    }

    #[wasm_bindgen(getter, js_name = termCount)]
    pub fn term_count_js(&self) -> f64 {
        self.inner.borrow().term_count() as f64
    }

    /// The special wildcard query value, like `MiniSearch.wildcard`: pass it
    /// to `search` (alone or inside a query tree) to match every document.
    /// A registered symbol, so it is identity-stable across calls.
    #[wasm_bindgen(getter, js_name = wildcard)]
    #[wasm_bindgen(unchecked_return_type = "symbol")]
    pub fn wildcard_js() -> JsValue {
        js_sys::Symbol::for_(WILDCARD_KEY).into()
    }

    /// MiniSearch-compatible `search(query, options?)`. `query` is a string,
    /// the `MiniSearchWasm.wildcard` symbol, or a query-expression tree:
    /// `{ combineWith?, queries: [subquery, …], …optionOverrides }` with
    /// subqueries nesting arbitrarily. Present option keys cascade down the
    /// tree, exactly like JS MiniSearch.
    #[wasm_bindgen(js_name = search, skip_typescript)]
    pub fn search_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Query")] query: JsValue,
        #[wasm_bindgen(unchecked_param_type = "SearchOptions")] options: Option<JsValue>,
    ) -> Result<JsValue, JsValue> {
        let options = options.unwrap_or(JsValue::UNDEFINED);
        // Optional `includeMatch` (default true, for MiniSearch compatibility):
        // callers that never read the per-hit `match` map can set it false to
        // skip building it. Read straight off the options object so neither the
        // engine's option set nor the binary snapshot format is affected.
        let include_match = read_bool_option(&options, "includeMatch", true);
        let per_call = parse_search_options(&options)?;
        let query = parse_query(&query)?;

        // The engine hands over ids, scores and interned term/field tables;
        // the MiniSearch-shaped objects are built by one JavaScript function,
        // where object literals are cheap, instead of one boundary call per
        // property per hit.
        let transfer = if self.exact_dirty() {
            self.inner
                .borrow_mut()
                .search_query_transfer_exact(&query, &per_call, include_match)
        } else {
            self.inner
                .borrow()
                .search_query_transfer(&query, &per_call, include_match)
        };
        build_results(&transfer, include_match)
    }

    /// App-facing fast search and the recommended path for embedding apps. Only
    /// the query string and an `orMode` flag cross the boundary (no options
    /// object to deserialize); the whole search runs in Wasm against the index's
    /// configured search options. The result set crosses back as just three
    /// values — `scores` (a `Float64Array`, one bulk copy) plus `ids` and
    /// `terms` as a newline-joined string; `ids` is a JSON array string — so
    /// there is almost no per-hit object churn at the boundary.
    ///
    /// Shape: `{ count, ids: '["id0","id1"]', scores: Float64Array, terms: "a b\nc" }`.
    /// Decode IDs with `JSON.parse`, preserving their types and escaped delimiters.
    /// where each `terms` row is space-joined. Returns identical rankings to
    /// `search()` (same ids, same BM25 scores).
    #[wasm_bindgen(js_name = searchJoined, unchecked_return_type = "JoinedResults")]
    pub fn search_joined_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] query: JsValue,
        or_mode: bool,
    ) -> Result<JsValue, JsValue> {
        let query = &query_arg(&query)?;
        if self.exact_dirty() {
            let mut inner = self.inner.borrow_mut();
            return Ok(joined_to_js(
                &inner.search_joined_default_exact(query, or_mode),
            ));
        }
        Ok(joined_to_js(
            &self.inner.borrow().search_joined_default(query, or_mode),
        ))
    }

    /// `searchJoined` with per-call option overrides (partial, like `search`):
    /// e.g. `searchJoinedOpts(q, { prefix: false, fuzzy: false })` for exact
    /// whole-token lookups without the rich `search()` result shape.
    #[wasm_bindgen(js_name = searchJoinedOpts, skip_typescript)]
    pub fn search_joined_opts_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] query: JsValue,
        #[wasm_bindgen(unchecked_param_type = "SearchOptions")] options: Option<JsValue>,
    ) -> Result<JsValue, JsValue> {
        let query = &query_arg(&query)?;
        let per_call = parse_search_options(&options.unwrap_or(JsValue::UNDEFINED))?;

        if self.exact_dirty() {
            let mut inner = self.inner.borrow_mut();
            return Ok(joined_to_js(
                &inner.search_joined_opts_exact(query, &per_call),
            ));
        }
        Ok(joined_to_js(
            &self.inner.borrow().search_joined_opts(query, &per_call),
        ))
    }

    /// The most boundary-frugal search: everything numeric. Returns
    /// `{ count, docIds: Uint32Array, scores: Float64Array, termTable: string,
    /// termOffsets: Uint32Array, termIds: Uint32Array }` where `docIds` are
    /// internal short ids resolved against the one-time `docIdTable`, and hit
    /// `i`'s matched terms are `termIds[termOffsets[i]..termOffsets[i+1]]`
    /// indexing the newline-split `termTable`. Each distinct derived term
    /// crosses the boundary once per query, however many hits matched it.
    /// Optional `options` are partial per-call overrides, like `search`.
    #[wasm_bindgen(js_name = searchRaw, skip_typescript)]
    pub fn search_raw_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] query: JsValue,
        #[wasm_bindgen(unchecked_param_type = "SearchOptions")] options: Option<JsValue>,
    ) -> Result<JsValue, JsValue> {
        let query = &query_arg(&query)?;
        let per_call = parse_search_options(&options.unwrap_or(JsValue::UNDEFINED))?;

        let raw = if self.exact_dirty() {
            self.inner.borrow_mut().search_raw_exact(query, &per_call)
        } else {
            self.inner.borrow().search_raw(query, &per_call)
        };

        let doc_ids = js_sys::Uint32Array::new_with_length(raw.doc_ids.len() as u32);
        doc_ids.copy_from(&raw.doc_ids);
        let scores = Float64Array::new_with_length(raw.scores.len() as u32);
        scores.copy_from(&raw.scores);
        let term_offsets = js_sys::Uint32Array::new_with_length(raw.term_offsets.len() as u32);
        term_offsets.copy_from(&raw.term_offsets);
        let term_ids = js_sys::Uint32Array::new_with_length(raw.term_ids.len() as u32);
        term_ids.copy_from(&raw.term_ids);

        let object = Object::new();
        let _ = Reflect::set(
            &object,
            &JsValue::from_str("count"),
            &JsValue::from_f64(raw.doc_ids.len() as f64),
        );
        let _ = Reflect::set(&object, &JsValue::from_str("docIds"), &doc_ids);
        let _ = Reflect::set(&object, &JsValue::from_str("scores"), &scores);
        let _ = Reflect::set(
            &object,
            &JsValue::from_str("termTable"),
            &JsValue::from_str(&raw.term_table),
        );
        let _ = Reflect::set(&object, &JsValue::from_str("termOffsets"), &term_offsets);
        let _ = Reflect::set(&object, &JsValue::from_str("termIds"), &term_ids);
        let _ = Reflect::set(
            &object,
            &JsValue::from_str("idTableVersion"),
            &JsValue::from_str(&raw.id_table_version.to_string()),
        );
        Ok(object.into())
    }

    /// External IDs as a JSON array in short-ID order; removed slots are null.
    /// Decode with JSON.parse. Cache alongside idTableVersion for this instance.
    #[wasm_bindgen(js_name = docIdTable)]
    pub fn doc_id_table_js(&self) -> String {
        self.inner.borrow().doc_id_table()
    }

    /// Instance-local external-ID table generation, encoded as a decimal string.
    /// Changes on add/remove/discard/reset. Raw results carry the same version.
    #[wasm_bindgen(getter, js_name = idTableVersion)]
    pub fn id_table_version_js(&self) -> String {
        self.inner.borrow().id_table_version().to_string()
    }

    /// MiniSearch-compatible `autoSuggest(query, options?)`: suggestions for
    /// search-as-you-type, each as `{ suggestion, terms, score }`, sorted by
    /// descending score. By default query terms are combined with `AND` and
    /// only the last term is prefix-expanded; defaults can be changed with the
    /// constructor's `autoSuggestOptions` or overridden per call (supported
    /// option keys: `fields`, `boost`, `weights`, `prefix`, `fuzzy`,
    /// `maxFuzzy`, `combineWith`, `bm25`).
    #[wasm_bindgen(js_name = autoSuggest, skip_typescript)]
    pub fn auto_suggest_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] query: JsValue,
        #[wasm_bindgen(unchecked_param_type = "SearchOptions")] options: Option<JsValue>,
    ) -> Result<JsValue, JsValue> {
        let query = &query_arg(&query)?;
        let options = options.unwrap_or(JsValue::UNDEFINED);
        let per_call: Option<AutoSuggestOptions> = if options.is_null() || options.is_undefined() {
            None
        } else {
            reject_callbacks(&options, SEARCH_CALLBACKS)?;
            Some(serde_wasm_bindgen::from_value(options).map_err(boundary_error)?)
        };

        let suggestions = if self.exact_dirty() {
            self.inner
                .borrow_mut()
                .auto_suggest_exact(query, per_call.as_ref())
        } else {
            self.inner.borrow().auto_suggest(query, per_call.as_ref())
        };

        let suggestion_key = JsValue::from_str("suggestion");
        let terms_key = JsValue::from_str("terms");
        let score_key = JsValue::from_str("score");
        let array = Array::new_with_length(suggestions.len() as u32);
        for (index, entry) in suggestions.iter().enumerate() {
            let object = Object::new();
            let _ = Reflect::set(
                &object,
                &suggestion_key,
                &JsValue::from_str(&entry.suggestion),
            );
            let terms = Array::new_with_length(entry.terms.len() as u32);
            for (term_index, term) in entry.terms.iter().enumerate() {
                terms.set(term_index as u32, JsValue::from_str(term));
            }
            let _ = Reflect::set(&object, &terms_key, &terms);
            let _ = Reflect::set(&object, &score_key, &JsValue::from_f64(entry.score));
            array.set(index as u32, object.into());
        }
        Ok(array.into())
    }

    /// Boundary-frugal auto-suggest, in the spirit of `searchJoined`: runs with
    /// the index's configured auto-suggest options and returns
    /// `{ count, suggestions: "vita nova\nvita nostra\n…", scores: Float64Array }`.
    /// Each row of `suggestions` is one suggestion phrase; its terms are the
    /// row split on single spaces (a suggestion string *is* its space-joined
    /// terms), so no per-suggestion JS objects or term arrays cross the
    /// boundary.
    #[wasm_bindgen(js_name = autoSuggestJoined, unchecked_return_type = "JoinedSuggestions")]
    pub fn auto_suggest_joined_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] query: JsValue,
    ) -> Result<JsValue, JsValue> {
        let query = &query_arg(&query)?;
        let entries = if self.exact_dirty() {
            self.inner.borrow_mut().auto_suggest_exact(query, None)
        } else {
            self.inner.borrow().auto_suggest(query, None)
        };

        let scores = Float64Array::new_with_length(entries.len() as u32);
        let mut suggestions = String::new();
        for (index, entry) in entries.iter().enumerate() {
            scores.set_index(index as u32, entry.score);
            if index > 0 {
                suggestions.push('\n');
            }
            suggestions.push_str(&entry.suggestion);
        }

        let object = Object::new();
        let _ = Reflect::set(
            &object,
            &JsValue::from_str("count"),
            &JsValue::from_f64(entries.len() as f64),
        );
        let _ = Reflect::set(
            &object,
            &JsValue::from_str("suggestions"),
            &JsValue::from_str(&suggestions),
        );
        let _ = Reflect::set(&object, &JsValue::from_str("scores"), &scores);
        Ok(object.into())
    }

    /// Profiling probe: runs the search but returns only the hit count, so
    /// result materialization/serialization is excluded. Lets the benchmark show
    /// pure engine compute cost separately from the boundary cost.
    #[wasm_bindgen(js_name = searchCountDefault)]
    pub fn search_count_default_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] query: JsValue,
        or_mode: bool,
    ) -> Result<f64, JsValue> {
        let query = &query_arg(&query)?;
        if self.exact_dirty() {
            let mut inner = self.inner.borrow_mut();
            return Ok(inner
                .search_joined_default_exact(query, or_mode)
                .scores
                .len() as f64);
        }
        Ok(self
            .inner
            .borrow()
            .search_packed_default(query, or_mode)
            .ids
            .len() as f64)
    }

    /// Diagnostic probe: hit count for a query with prefix/fuzzy toggled, to
    /// profile where search time goes.
    #[wasm_bindgen(js_name = searchCountOpts)]
    pub fn search_count_opts_js(
        &self,
        #[wasm_bindgen(unchecked_param_type = "string")] query: JsValue,
        prefix: bool,
        fuzzy: bool,
    ) -> Result<f64, JsValue> {
        let query = &query_arg(&query)?;
        if self.exact_dirty() {
            let mut inner = self.inner.borrow_mut();
            return Ok(inner.search_count_opts_exact(query, prefix, fuzzy) as f64);
        }
        Ok(self.inner.borrow().search_count_opts(query, prefix, fuzzy) as f64)
    }

    /// `toJSON()` like the JavaScript library: the plain object that
    /// `JSON.stringify(index)` serializes, loadable with
    /// `MiniSearch.loadJSON(json, options)` there and `loadJSON` here. The
    /// engine's own, more compact JSON is `toNativeJSON()`.
    #[wasm_bindgen(js_name = toJSON, unchecked_return_type = "MiniSearchJSON")]
    pub fn to_json_js(&self) -> Result<JsValue, JsValue> {
        let text = self
            .inner
            .borrow()
            .to_minisearch_json()
            .map_err(|err| js_error(&err))?;
        js_sys::JSON::parse(&text)
    }

    /// `toJSON()` as a string (the same as `JSON.stringify(index)`).
    #[wasm_bindgen(js_name = toJSONString)]
    pub fn to_json_string_js(&self) -> Result<String, JsValue> {
        self.inner
            .borrow()
            .to_minisearch_json()
            .map_err(|err| js_error(&err))
    }

    /// The engine's own JSON snapshot (a Rust-struct layout, versioned and
    /// validated on load). Not readable by the JavaScript library; use
    /// `toJSON()` for that.
    #[wasm_bindgen(js_name = toNativeJSON, unchecked_return_type = "object")]
    pub fn to_native_json_js(&self) -> Result<JsValue, JsValue> {
        let inner = self.inner.borrow();
        inner.check_persistable().map_err(|err| js_error(&err))?;
        to_json_compatible_value(&*inner).map_err(|err| js_error(&err))
    }

    /// The radix tree alone, in the native snapshot's node shape and whatever
    /// its depth: what the facade needs to rebuild the index in JavaScript with
    /// the same key order.
    #[wasm_bindgen(js_name = indexTree, unchecked_return_type = "object")]
    pub fn index_tree_js(&self) -> Result<JsValue, JsValue> {
        to_json_compatible_value(self.inner.borrow().index_root()).map_err(|err| js_error(&err))
    }

    #[wasm_bindgen(js_name = toNativeJSONString)]
    pub fn to_native_json_string_js(&self) -> Result<String, JsValue> {
        self.inner.borrow().to_json().map_err(|err| js_error(&err))
    }

    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes_js(&self) -> Result<Vec<u8>, JsValue> {
        self.inner.borrow().to_bytes().map_err(|err| js_error(&err))
    }

    /// Load the engine's own JSON snapshot (see `toNativeJSON`).
    #[wasm_bindgen(js_name = loadNativeJSON)]
    pub fn load_native_json_js(
        #[wasm_bindgen(unchecked_param_type = "string")] serialized: JsValue,
    ) -> Result<MiniSearchWasm, JsValue> {
        let serialized = string_arg(&serialized, "the snapshot")?;
        let inner = MiniSearch::from_json(&serialized).map_err(|err| js_error(&err))?;

        Ok(MiniSearchWasm::from_inner(inner))
    }

    /// `MiniSearch.loadJSON(json, options)`: load an index serialized by the
    /// JavaScript library (or by `toJSON()` here), with the same constructor
    /// options the index was created with.
    #[wasm_bindgen(js_name = loadJSON)]
    pub fn load_json_js(
        #[wasm_bindgen(unchecked_param_type = "string")] json: JsValue,
        #[wasm_bindgen(unchecked_param_type = "MiniSearchWasmOptions")] options: JsValue,
    ) -> Result<MiniSearchWasm, JsValue> {
        Self::load_minisearch_json_js(json, options)
    }

    /// `MiniSearch.loadJSONAsync(json, options)`: `loadJSON` as a Promise.
    #[wasm_bindgen(js_name = loadJSONAsync, unchecked_return_type = "Promise<MiniSearchWasm>")]
    pub fn load_json_async_js(
        #[wasm_bindgen(unchecked_param_type = "string")] json: JsValue,
        #[wasm_bindgen(unchecked_param_type = "MiniSearchWasmOptions")] options: JsValue,
    ) -> Promise {
        match Self::load_minisearch_json_js(json, options) {
            Ok(index) => Promise::resolve(&JsValue::from(index)),
            Err(error) => Promise::reject(&error),
        }
    }

    #[wasm_bindgen(js_name = loadBytes)]
    pub fn load_bytes_js(
        #[wasm_bindgen(unchecked_param_type = "Uint8Array | ArrayBuffer | ArrayBufferView")]
        bytes: JsValue,
    ) -> Result<MiniSearchWasm, JsValue> {
        let inner = MiniSearch::from_bytes(&bytes_arg(&bytes)?).map_err(|err| js_error(&err))?;

        Ok(MiniSearchWasm::from_inner(inner))
    }

    /// Load an index serialized by JS MiniSearch (`JSON.stringify(miniSearch)`
    /// or `miniSearch.toJSON()`, serialization versions 1 and 2), given the
    /// same constructor options the JavaScript instance used — the counterpart
    /// of `MiniSearch.loadJSON(json, options)`.
    #[wasm_bindgen(js_name = loadMiniSearchJSON)]
    pub fn load_minisearch_json_js(
        #[wasm_bindgen(unchecked_param_type = "string")] json: JsValue,
        #[wasm_bindgen(unchecked_param_type = "MiniSearchWasmOptions")] options: JsValue,
    ) -> Result<MiniSearchWasm, JsValue> {
        let json = string_arg(&json, "the serialized index")?;
        let (options, logger) = parse_constructor_options(&options)?;
        let inner =
            MiniSearch::from_minisearch_json(&json, options).map_err(|err| js_error(&err))?;

        Ok(MiniSearchWasm::from_inner(inner).with_logger(logger))
    }

    /// Serialize in JS MiniSearch's `toJSON()` shape (`serializationVersion`
    /// 2), loadable there with `MiniSearch.loadJSON(json, options)`.
    #[wasm_bindgen(js_name = toMiniSearchJSON)]
    pub fn to_minisearch_json_js(&self) -> Result<String, JsValue> {
        self.inner
            .borrow()
            .to_minisearch_json()
            .map_err(|err| js_error(&err))
    }
}

impl MiniSearchWasm {
    fn from_inner(inner: MiniSearch) -> Self {
        Self {
            inner: Rc::new(RefCell::new(inner)),
            vacuum_runtime: Rc::new(RefCell::new(VacuumRuntime::default())),
            logger: None,
            exact_dirty_queries: Cell::new(false),
        }
    }

    fn exact_dirty(&self) -> bool {
        self.exact_dirty_queries.get() && self.inner.borrow().dirt_count() > 0
    }

    fn with_logger(mut self, logger: Option<Function>) -> Self {
        self.logger = logger;
        self
    }

    /// MiniSearch's `logger(level, message, code)`; without a configured
    /// logger, `console[level](message)` like the JavaScript default.
    fn log(&self, level: &str, message: &str, code: &str) {
        if let Some(logger) = &self.logger {
            let _ = logger.call3(
                &JsValue::UNDEFINED,
                &JsValue::from_str(level),
                &JsValue::from_str(message),
                &JsValue::from_str(code),
            );
            return;
        }
        let Ok(console) = Reflect::get(&js_sys::global(), &JsValue::from_str("console")) else {
            return;
        };
        if let Ok(method) = Reflect::get(&console, &JsValue::from_str(level)) {
            if let Ok(method) = method.dyn_into::<Function>() {
                let _ = method.call1(&console, &JsValue::from_str(message));
            }
        }
    }

    fn log_version_conflicts(&self, warnings: &[String]) {
        for message in warnings {
            self.log("warn", message, "version_conflict");
        }
    }

    fn schema(&self) -> DocumentSchema {
        DocumentSchema::of(self.inner.borrow().options())
    }

    fn maybe_schedule_auto_vacuum(&self) {
        let options = self.inner.borrow().auto_vacuum_request();
        if let Some(options) = options {
            let _ = schedule_vacuum(
                Rc::clone(&self.inner),
                Rc::clone(&self.vacuum_runtime),
                options,
                true,
            );
        }
    }
}

fn schedule_vacuum(
    inner: Rc<RefCell<MiniSearch>>,
    runtime: Rc<RefCell<VacuumRuntime>>,
    options: mini_search::ResolvedVacuumOptions,
    automatic: bool,
) -> Promise {
    {
        let mut state = runtime.borrow_mut();
        if state.active {
            match &mut state.rerun {
                Some((_, all_automatic)) => *all_automatic = *all_automatic && automatic,
                None => state.rerun = Some((options, automatic)),
            }
            return state
                .current
                .as_ref()
                .expect("active vacuum has a promise")
                .clone();
        }
        state.active = true;
    }

    // Like JS MiniSearch, the run starts now: the dirt it will account for is
    // today's, and its first batch is cleaned before this call returns.
    let mut done = {
        let mut index = inner.borrow_mut();
        index.begin_vacuum();
        index.vacuum_step(options.batch_size)
    };

    let future_inner = Rc::clone(&inner);
    let future_runtime = Rc::clone(&runtime);
    let promise = future_to_promise(async move {
        let mut options = options;
        let result = async {
            loop {
                while !done {
                    yield_to_timer(options.batch_wait).await?;
                    done = future_inner.borrow_mut().vacuum_step(options.batch_size);
                }

                let rerun = future_runtime.borrow_mut().rerun.take();
                match rerun {
                    Some((_, true)) if future_inner.borrow().auto_vacuum_request().is_none() => {
                        break
                    }
                    Some((queued, _)) => options = queued,
                    None => break,
                }
                let mut index = future_inner.borrow_mut();
                index.begin_vacuum();
                done = index.vacuum_step(options.batch_size);
            }
            Ok(JsValue::UNDEFINED)
        }
        .await;

        let mut state = future_runtime.borrow_mut();
        state.active = false;
        state.rerun = None;
        state.current = None;
        result
    });
    runtime.borrow_mut().current = Some(promise.clone());
    promise
}

async fn yield_to_timer(milliseconds: u32) -> Result<(), JsValue> {
    let global = js_sys::global();
    let timer: Function = Reflect::get(&global, &JsValue::from_str("setTimeout"))?
        .dyn_into()
        .map_err(|_| JsValue::from_str("global setTimeout is not available"))?;
    let promise = Promise::new(&mut |resolve, reject| {
        if let Err(error) = timer.call2(&global, &resolve, &JsValue::from_f64(milliseconds as f64))
        {
            let _ = reject.call1(&JsValue::UNDEFINED, &error);
        }
    });
    JsFuture::from(promise).await.map(|_| ())
}

fn default_async_chunk_size() -> usize {
    10
}

/// Most boundary-frugal shape: `scores` as a `Float64Array`, plus `ids` and
/// `terms` as a newline-joined string; `ids` is JSON (already built by the engine).
/// Within a `terms` row the individual terms are space-joined. The consumer
/// splits natively in JS, so the entire result set crosses the boundary as
/// just two strings + one typed array.
fn joined_to_js(joined: &JoinedSearchResults) -> JsValue {
    let scores = Float64Array::new_with_length(joined.scores.len() as u32);
    scores.copy_from(&joined.scores);

    let object = Object::new();
    let _ = Reflect::set(
        &object,
        &JsValue::from_str("count"),
        &JsValue::from_f64(joined.scores.len() as f64),
    );
    let _ = Reflect::set(
        &object,
        &JsValue::from_str("ids"),
        &JsValue::from_str(&joined.ids),
    );
    let _ = Reflect::set(&object, &JsValue::from_str("scores"), &scores);
    let _ = Reflect::set(
        &object,
        &JsValue::from_str("terms"),
        &JsValue::from_str(&joined.terms),
    );
    object.into()
}

fn to_json_compatible_value<T: Serialize>(value: &T) -> Result<JsValue, String> {
    let json = serde_json::to_value(value).map_err(|err| err.to_string())?;
    json.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|err| err.to_string())
}

/// A real JS `Error` carrying MiniSearch's message, so `e.message`,
/// `e.stack` and `instanceof Error` behave as with the JavaScript library.
/// A string argument. The generated glue reads a `&str` parameter without
/// checking its type and traps on anything else, so string parameters arrive
/// as values and are checked here.
fn string_arg(value: &JsValue, what: &str) -> Result<String, JsValue> {
    value
        .as_string()
        .ok_or_else(|| js_error(&format!("MiniSearch: {what} must be a string")))
}

fn query_arg(value: &JsValue) -> Result<String, JsValue> {
    string_arg(value, "the query of this method")
}

/// Snapshot bytes: a `Uint8Array`, an `ArrayBuffer` (what `fetch` hands out)
/// or any other view of one.
fn bytes_arg(value: &JsValue) -> Result<Vec<u8>, JsValue> {
    if let Some(buffer) = value.dyn_ref::<js_sys::ArrayBuffer>() {
        return Ok(js_sys::Uint8Array::new(buffer).to_vec());
    }
    if js_sys::ArrayBuffer::is_view(value) {
        let buffer = Reflect::get(value, &JsValue::from_str("buffer"))?;
        let offset = Reflect::get(value, &JsValue::from_str("byteOffset"))?;
        let length = Reflect::get(value, &JsValue::from_str("byteLength"))?;
        let view = js_sys::Uint8Array::new_with_byte_offset_and_length(
            &buffer,
            offset.as_f64().unwrap_or(0.0) as u32,
            length.as_f64().unwrap_or(0.0) as u32,
        );
        return Ok(view.to_vec());
    }
    Err(js_error(
        "MiniSearch: snapshot bytes must be a Uint8Array, an ArrayBuffer or a view of one",
    ))
}

fn js_error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

/// Errors from option/document conversion at the boundary. serde's messages
/// arrive as `Error: …`; strip that and prefix `MiniSearch:` like JS does.
fn boundary_error(err: impl std::fmt::Display) -> JsValue {
    let text = err.to_string();
    let text = text.strip_prefix("Error: ").unwrap_or(&text);
    if text.starts_with("MiniSearch") || text.starts_with("Invalid combination operator") {
        js_error(text)
    } else {
        js_error(&format!("MiniSearch: {text}"))
    }
}

/// Callback options MiniSearch accepts that cannot cross the Wasm boundary,
/// with the supported alternative for each.
const SEARCH_CALLBACKS: &[(&str, &str)] = &[
    (
        "filter",
        "Pass an object of stored-field values to match instead, e.g. filter: { category: \"books\" }.",
    ),
    (
        "boostDocument",
        "Not supported; boost fields with the `boost` option instead.",
    ),
    (
        "boostTerm",
        "Pass an array with one boost per query term instead, e.g. boostTerm: [2, 1].",
    ),
    (
        "prefix",
        "Pass a boolean, or an array with one flag per query term instead, e.g. prefix: [false, true].",
    ),
    (
        "fuzzy",
        "Pass a boolean or a number, or an array with one entry per query term instead, e.g. fuzzy: [false, 0.2].",
    ),
    (
        "tokenize",
        "Use the built-in tokenizer modes instead (constructor option tokenizer: \"default\" | \"jobboard\").",
    ),
    (
        "processTerm",
        "Terms are lower-cased by the built-in tokenizer; custom term processing is not available.",
    ),
];

const CONSTRUCTOR_CALLBACKS: &[(&str, &str)] = &[
    ("extractField", "Fields are read as document[fieldName]."),
    (
        "stringifyField",
        "Field values are converted with String(value), like MiniSearch's default.",
    ),
    (
        "tokenize",
        "Use the built-in tokenizer modes instead (constructor option tokenizer: \"default\" | \"jobboard\").",
    ),
    (
        "processTerm",
        "Terms are lower-cased by the built-in tokenizer; custom term processing is not available.",
    ),
];

/// Throw for function-valued options instead of silently ignoring them.
fn reject_callbacks(options: &JsValue, callbacks: &[(&str, &str)]) -> Result<(), JsValue> {
    if !options.is_object() {
        return Ok(());
    }
    for (key, hint) in callbacks {
        let value = Reflect::get(options, &JsValue::from_str(key))?;
        if value.is_function() {
            return Err(js_error(&format!(
                "MiniSearch: option \"{key}\" cannot be a function in minisearch-wasm: callbacks do not cross the WebAssembly boundary. {hint}"
            )));
        }
    }
    Ok(())
}

/// Constructor options with MiniSearch's own validation message for missing
/// `fields`, rejection of callback options, and the optional `logger`.
fn parse_constructor_options(
    options: &JsValue,
) -> Result<(MiniSearchOptions, Option<Function>), JsValue> {
    let fields = if options.is_object() {
        Reflect::get(options, &JsValue::from_str("fields"))?
    } else {
        JsValue::UNDEFINED
    };
    if fields.is_undefined() || fields.is_null() {
        return Err(js_error("MiniSearch: option \"fields\" must be provided"));
    }
    reject_callbacks(options, CONSTRUCTOR_CALLBACKS)?;
    for nested in ["searchOptions", "autoSuggestOptions"] {
        let value = Reflect::get(options, &JsValue::from_str(nested))?;
        reject_callbacks(&value, SEARCH_CALLBACKS)?;
    }
    let logger = Reflect::get(options, &JsValue::from_str("logger"))?
        .dyn_into::<Function>()
        .ok();
    let parsed: MiniSearchOptions =
        serde_wasm_bindgen::from_value(options.clone()).map_err(boundary_error)?;
    // Refuse at construction what a snapshot could not hold.
    parsed.validate().map_err(|err| js_error(&err))?;
    Ok((parsed, logger))
}

/// Per-call search options: absent → defaults; callbacks rejected.
fn parse_search_options(options: &JsValue) -> Result<PartialSearchOptions, JsValue> {
    if options.is_null() || options.is_undefined() {
        return Ok(PartialSearchOptions::default());
    }
    reject_callbacks(options, SEARCH_CALLBACKS)?;
    serde_wasm_bindgen::from_value(options.clone()).map_err(boundary_error)
}

/// Which document keys the engine reads, and how each converts. Indexed fields
/// follow MiniSearch's default `stringifyField` (`value.toString()`): primitives
/// and arrays of primitives stay JSON values (the engine stringifies those
/// identically), while a `Date`, an object with `toString()` or an array
/// containing objects is converted with `String(value)` on the JavaScript side.
/// The id field and stored-only fields stay JSON values (a `Date` becomes what
/// `JSON.stringify` writes for it).
struct DocumentSchema {
    keys: Vec<(String, bool)>,
}

impl DocumentSchema {
    fn of(options: &MiniSearchOptions) -> Self {
        let mut keys: Vec<(String, bool)> = vec![(options.id_field.clone(), false)];
        for field in &options.fields {
            if keys.iter().any(|(key, _)| key == field) {
                continue;
            }
            keys.push((field.clone(), true));
        }
        for field in &options.store_fields {
            if !keys.iter().any(|(key, _)| key == field) {
                keys.push((field.clone(), false));
            }
        }
        Self { keys }
    }
}

thread_local! {
    static STRING_FN: Function = Reflect::get(&js_sys::global(), &JsValue::from_str("String"))
        .ok()
        .and_then(|value| value.dyn_into::<Function>().ok())
        .expect("global String");
}

/// Whether an indexed value must be stringified on the JavaScript side to
/// match `value.toString()`: objects (including `Date`) and arrays that
/// contain objects. Primitives and arrays of primitives stringify identically
/// in the engine.
fn needs_js_string(value: &JsValue) -> bool {
    if !value.is_object() {
        return false;
    }
    if Array::is_array(value) {
        let array: &Array = value.unchecked_ref();
        return array.iter().any(|item| needs_js_string(&item));
    }
    true
}

/// `String(value)`.
fn js_string_of(value: &JsValue) -> Result<String, JsValue> {
    let text = STRING_FN.with(|string| string.call1(&JsValue::UNDEFINED, value))?;
    Ok(text.as_string().unwrap_or_default())
}

fn normalize_document(document: &JsValue, schema: &DocumentSchema) -> Result<Value, JsValue> {
    if !document.is_object() {
        return Err(js_error("MiniSearch: document must be an object"));
    }
    let mut object = serde_json::Map::new();
    for (key, indexed_only) in &schema.keys {
        let value = Reflect::get(document, &JsValue::from_str(key))?;
        if value.is_undefined() || value.is_function() {
            continue;
        }
        let converted = if *indexed_only && needs_js_string(&value) {
            Value::String(js_string_of(&value)?)
        } else if value.is_instance_of::<js_sys::Date>() {
            // What `JSON.stringify` would store for a Date.
            let json = js_sys::JSON::stringify(&value)?;
            serde_json::from_str(&String::from(json)).unwrap_or(Value::Null)
        } else {
            serde_wasm_bindgen::from_value(value).map_err(boundary_error)?
        };
        object.insert(key.clone(), converted);
    }
    Ok(Value::Object(object))
}

fn normalize_documents(
    documents: &JsValue,
    schema: &DocumentSchema,
) -> Result<Vec<Value>, JsValue> {
    if !Array::is_array(documents) {
        return Err(js_error("MiniSearch: documents must be an array"));
    }
    let array: &Array = documents.unchecked_ref();
    let mut normalized = Vec::with_capacity(array.length() as usize);
    for document in array.iter() {
        normalized.push(normalize_document(&document, schema)?);
    }
    Ok(normalized)
}

/// Builds MiniSearch-shaped result objects from the engine's transfer in one
/// JavaScript call (see `MiniSearch::search_query_transfer`).
#[wasm_bindgen(module = "/src/interop.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = buildResults)]
    fn build_results_module(args: &Array) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = callbackDefault)]
    fn callback_default(name: &str) -> Result<JsValue, JsValue>;
}

fn build_results(transfer: &CompatTransfer, include_match: bool) -> Result<JsValue, JsValue> {
    let args = Array::new();
    args.push(&JsValue::from_str(&transfer.ids));
    args.push(&Float64Array::from(&transfer.scores[..]));
    args.push(&JsValue::from_str(&transfer.table));
    args.push(&Uint32Array::from(&transfer.term_ids[..]));
    args.push(&Uint32Array::from(&transfer.term_offsets[..]));
    args.push(&Uint32Array::from(&transfer.query_term_ids[..]));
    args.push(&Uint32Array::from(&transfer.query_term_offsets[..]));
    args.push(&Uint32Array::from(&transfer.field_ids[..]));
    args.push(&Uint32Array::from(&transfer.field_offsets[..]));
    args.push(&JsValue::from_str(&transfer.field_names));
    args.push(&JsValue::from_str(&transfer.stored));
    args.push(&JsValue::from_bool(include_match));
    build_results_module(&args)
}

#[wasm_bindgen(typescript_custom_section)]
const TYPESCRIPT_TYPES: &'static str = r#"
/** BM25 parameters (MiniSearch defaults: k 1.2, b 0.7, d 0.5). */
export interface Bm25Params { k?: number; b?: number; d?: number }
/** Weights of prefix and fuzzy matches relative to exact matches. */
export interface SearchWeights { fuzzy?: number; prefix?: number }
export type CombineWith = "AND" | "OR" | "AND_NOT" | "and" | "or" | "and_not";
/** `false`, `true` (0.2), a max relative/absolute distance, or one entry per query term. */
export type FuzzyOption = boolean | number | ReadonlyArray<boolean | number>;
/** A flag for every term, or one flag per query term. */
export type PrefixOption = boolean | readonly boolean[];
export interface SearchOptions {
  fields?: string[];
  boost?: Record<string, number>;
  weights?: SearchWeights;
  prefix?: PrefixOption;
  fuzzy?: FuzzyOption;
  maxFuzzy?: number;
  combineWith?: CombineWith;
  bm25?: Bm25Params;
  /** One boost per query term (declarative form of MiniSearch's `boostTerm`). */
  boostTerm?: readonly number[];
  /** Keep only hits whose stored fields (or id) equal these values (declarative form of MiniSearch's `filter`). */
  filter?: Record<string, unknown>;
  /** `search()` only: skip building the per-hit `match` map. */
  includeMatch?: boolean;
}
export interface QueryCombination extends SearchOptions { queries: readonly Query[] }
export type Query = string | QueryCombination | symbol;
export type LogLevel = "debug" | "info" | "warn" | "error";
export interface AutoVacuumOptions { minDirtCount?: number; minDirtFactor?: number; batchSize?: number; batchWait?: number }
export interface VacuumOptions { batchSize?: number; batchWait?: number }
export interface AddAllAsyncOptions { chunkSize?: number }
/**
 * Methods with optional arguments, declared here (merged into the class) so
 * the arguments are optional in TypeScript as they are at runtime.
 */
export interface MiniSearchWasm {
  /** MiniSearch-compatible search: full result objects (`id`, `score`, `terms`, `queryTerms`, `match`, stored fields). */
  search(query: Query, options?: SearchOptions): SearchResult[];
  /** `searchJoined` with per-call option overrides. */
  searchJoinedOpts(query: string, options?: SearchOptions): JoinedResults;
  /** Fully numeric results; resolve `docIds` through `docIdTable()`. */
  searchRaw(query: string, options?: SearchOptions): RawResults;
  /** MiniSearch-compatible auto-suggest (AND, prefix on the last term by default). */
  autoSuggest(query: string, options?: SearchOptions): Suggestion[];
  /** Index in chunks, yielding to the event loop between them. */
  addAllAsync(documents: readonly object[], options?: AddAllAsyncOptions): Promise<void>;
  /** Remove the given documents, or every document when called without arguments. */
  removeAll(documents?: readonly object[]): void;
  /** Remove stale postings of discarded documents, in asynchronous batches. */
  vacuum(options?: VacuumOptions): Promise<void>;
}
export interface MiniSearchWasmOptions {
  fields: string[];
  idField?: string;
  storeFields?: string[];
  tokenizer?: "default" | "jobboard";
  searchOptions?: SearchOptions;
  autoSuggestOptions?: SearchOptions;
  autoVacuum?: boolean | AutoVacuumOptions;
  logger?: (level: LogLevel, message: string, code?: string) => void;
}
export interface SearchResult {
  id: any;
  score: number;
  terms: string[];
  queryTerms: string[];
  match: Record<string, string[]>;
  [storedField: string]: unknown;
}
export interface Suggestion { suggestion: string; terms: string[]; score: number }
/** `searchJoined` result: row i of `scores`, `JSON.parse(ids)` and `terms.split("\n")` is one hit. */
export interface JoinedResults { count: number; ids: string; scores: Float64Array; terms: string }
/** `searchRaw` result: internal doc ids into `docIdTable()`, matched-term ids into `termTable`. */
export interface RawResults {
  count: number;
  idTableVersion: string;
  docIds: Uint32Array;
  scores: Float64Array;
  termTable: string;
  termOffsets: Uint32Array;
  termIds: Uint32Array;
}
export interface JoinedSuggestions { count: number; suggestions: string; scores: Float64Array }
/** MiniSearch's `toJSON()` shape (serialization version 2). */
export interface MiniSearchJSON {
  documentCount: number;
  nextId: number;
  documentIds: Record<string, any>;
  fieldIds: Record<string, number>;
  fieldLength: Record<string, Array<number | null>>;
  averageFieldLength: number[];
  storedFields: Record<string, Record<string, unknown>>;
  dirtCount?: number;
  index: Array<[string, Record<string, Record<string, number>>]>;
  serializationVersion: number;
}
"#;

fn parse_query(value: &JsValue) -> Result<Query, JsValue> {
    if let Some(text) = value.as_string() {
        return Ok(Query::Text(text));
    }

    if *value == MiniSearchWasm::wildcard_js() {
        return Ok(Query::Wildcard);
    }

    if value.is_object() {
        let queries_value = Reflect::get(value, &JsValue::from_str("queries"))?;
        if !Array::is_array(&queries_value) {
            return Err(js_error(
                "MiniSearch: invalid query: a query object must have a 'queries' array",
            ));
        }

        let queries_array = Array::from(&queries_value);
        let mut queries = Vec::with_capacity(queries_array.length() as usize);
        for subquery in queries_array.iter() {
            queries.push(parse_query(&subquery)?);
        }

        // Node options are the object's remaining keys. Deserialize a shallow
        // copy with `queries` removed: its entries may be nested objects or
        // the wildcard symbol, which serde cannot (and must not) consume.
        reject_callbacks(value, SEARCH_CALLBACKS)?;
        let copy = Object::assign(&Object::new(), Object::unchecked_from_js_ref(value));
        Reflect::delete_property(&copy, &JsValue::from_str("queries"))?;
        let options: PartialSearchOptions =
            serde_wasm_bindgen::from_value(copy.into()).map_err(boundary_error)?;

        return Ok(Query::Combination(QueryCombination { queries, options }));
    }

    Err(js_error(
        "MiniSearch: invalid query: expected a string, a query object, or MiniSearchWasm.wildcard",
    ))
}

fn read_bool_option(options: &JsValue, key: &str, default: bool) -> bool {
    if !options.is_object() {
        return default;
    }
    match Reflect::get(options, &JsValue::from_str(key)) {
        Ok(value) if value.is_undefined() || value.is_null() => default,
        Ok(value) => value.as_bool().unwrap_or(default),
        Err(_) => default,
    }
}

/// Get (creating once) the interned JS string for `s`. Cloning the cached
/// `JsValue` shares the same JS string handle instead of re-copying the bytes.
fn json_to_js(value: &Value) -> JsValue {
    match value {
        Value::Null => JsValue::NULL,
        Value::Bool(b) => JsValue::from_bool(*b),
        Value::Number(n) => JsValue::from_f64(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => JsValue::from_str(s),
        Value::Array(items) => {
            let array = Array::new_with_length(items.len() as u32);
            for (index, item) in items.iter().enumerate() {
                array.set(index as u32, json_to_js(item));
            }
            array.into()
        }
        Value::Object(map) => {
            let object = Object::new();
            for (key, val) in map {
                let _ = Reflect::set(&object, &JsValue::from_str(key), &json_to_js(val));
            }
            object.into()
        }
    }
}
