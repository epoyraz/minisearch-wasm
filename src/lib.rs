mod mini_search;
mod searchable_map;

pub use mini_search::{
    AutoSuggestOptions, Bm25Params, CombineWith, CompactSearchResult, FuzzySetting,
    JoinedSearchResults, MiniSearch, MiniSearchOptions, PackedSearchResults, SearchOptions,
    SearchResult, Suggestion, TokenizerMode, Weights,
};
pub use searchable_map::{FuzzyMatch, SearchableMap};

use js_sys::{Array, Float64Array, Object, Reflect};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct MiniSearchWasm {
    inner: MiniSearch,
}

#[wasm_bindgen]
impl MiniSearchWasm {
    #[wasm_bindgen(constructor)]
    pub fn new(options: JsValue) -> Result<MiniSearchWasm, JsValue> {
        let options: MiniSearchOptions = serde_wasm_bindgen::from_value(options)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;

        Ok(MiniSearchWasm {
            inner: MiniSearch::new(options),
        })
    }

    #[wasm_bindgen(js_name = add)]
    pub fn add_js(&mut self, document: JsValue) -> Result<(), JsValue> {
        let document: Value = serde_wasm_bindgen::from_value(document)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;

        self.inner
            .add(document)
            .map_err(|err| JsValue::from_str(&err))
    }

    #[wasm_bindgen(js_name = addAll)]
    pub fn add_all_js(&mut self, documents: JsValue) -> Result<(), JsValue> {
        let documents: Vec<Value> = serde_wasm_bindgen::from_value(documents)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;

        self.inner
            .add_all(documents)
            .map_err(|err| JsValue::from_str(&err))
    }

    #[wasm_bindgen(js_name = addAllJSON)]
    pub fn add_all_json_js(&mut self, documents: &str) -> Result<(), JsValue> {
        let documents: Vec<Value> =
            serde_json::from_str(documents).map_err(|err| JsValue::from_str(&err.to_string()))?;

        self.inner
            .add_all(documents)
            .map_err(|err| JsValue::from_str(&err))
    }

    #[wasm_bindgen(js_name = remove)]
    pub fn remove_js(&mut self, document: JsValue) -> Result<(), JsValue> {
        let document: Value = serde_wasm_bindgen::from_value(document)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;

        self.inner
            .remove(&document)
            .map_err(|err| JsValue::from_str(&err))
    }

    #[wasm_bindgen(js_name = discard)]
    pub fn discard_js(&mut self, id: JsValue) -> Result<(), JsValue> {
        let id: Value = serde_wasm_bindgen::from_value(id)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;

        self.inner
            .discard(&id)
            .map_err(|err| JsValue::from_str(&err))
    }

    #[wasm_bindgen(js_name = search)]
    pub fn search_js(&self, query: &str, options: JsValue) -> Result<JsValue, JsValue> {
        // Optional `includeMatch` (default true, for MiniSearch compatibility):
        // callers that never read the per-hit `match` map can set it false to
        // skip building it — the single biggest remaining boundary cost. Read it
        // straight off the options object so neither the engine's option set nor
        // the binary snapshot format is affected.
        let include_match = read_bool_option(&options, "includeMatch", true);

        let search_options: SearchOptions = if options.is_null() || options.is_undefined() {
            SearchOptions::default()
        } else {
            serde_wasm_bindgen::from_value(options)
                .map_err(|err| JsValue::from_str(&err.to_string()))?
        };

        Ok(results_to_js(
            &self.inner.search(query, search_options),
            include_match,
        ))
    }

    /// App-facing fast search and the recommended path for embedding apps. Only
    /// the query string and an `orMode` flag cross the boundary (no options
    /// object to deserialize); the whole search runs in Wasm against the index's
    /// configured search options. The result set crosses back as just three
    /// values — `scores` (a `Float64Array`, one bulk copy) plus `ids` and
    /// `terms` as single newline-joined strings that JS splits natively — so
    /// there is almost no per-hit object churn at the boundary.
    ///
    /// Shape: `{ count, ids: "id0\nid1\n…", scores: Float64Array, terms: "a b\nc\n…" }`
    /// where each `terms` row is space-joined. Returns identical rankings to
    /// `search()` (same ids, same BM25 scores).
    #[wasm_bindgen(js_name = searchJoined)]
    pub fn search_joined_js(&self, query: &str, or_mode: bool) -> JsValue {
        joined_to_js(&self.inner.search_joined_default(query, or_mode))
    }

    /// MiniSearch-compatible `autoSuggest(query, options?)`: suggestions for
    /// search-as-you-type, each as `{ suggestion, terms, score }`, sorted by
    /// descending score. By default query terms are combined with `AND` and
    /// only the last term is prefix-expanded; defaults can be changed with the
    /// constructor's `autoSuggestOptions` or overridden per call (supported
    /// option keys: `fields`, `boost`, `weights`, `prefix`, `fuzzy`,
    /// `maxFuzzy`, `combineWith`, `bm25`).
    #[wasm_bindgen(js_name = autoSuggest)]
    pub fn auto_suggest_js(&self, query: &str, options: JsValue) -> Result<JsValue, JsValue> {
        let per_call: Option<AutoSuggestOptions> = if options.is_null() || options.is_undefined() {
            None
        } else {
            Some(
                serde_wasm_bindgen::from_value(options)
                    .map_err(|err| JsValue::from_str(&err.to_string()))?,
            )
        };

        let suggestions = self.inner.auto_suggest(query, per_call.as_ref());

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
    #[wasm_bindgen(js_name = autoSuggestJoined)]
    pub fn auto_suggest_joined_js(&self, query: &str) -> JsValue {
        let entries = self.inner.auto_suggest(query, None);

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
        object.into()
    }

    /// Profiling probe: runs the search but returns only the hit count, so
    /// result materialization/serialization is excluded. Lets the benchmark show
    /// pure engine compute cost separately from the boundary cost.
    #[wasm_bindgen(js_name = searchCountDefault)]
    pub fn search_count_default_js(&self, query: &str, or_mode: bool) -> f64 {
        self.inner.search_packed_default(query, or_mode).ids.len() as f64
    }

    /// Diagnostic probe: hit count for a query with prefix/fuzzy toggled, to
    /// profile where search time goes.
    #[wasm_bindgen(js_name = searchCountOpts)]
    pub fn search_count_opts_js(&self, query: &str, prefix: bool, fuzzy: bool) -> f64 {
        self.inner.search_count_opts(query, prefix, fuzzy) as f64
    }

    #[wasm_bindgen(js_name = toJSON)]
    pub fn to_json_js(&self) -> Result<JsValue, JsValue> {
        to_json_compatible_value(&self.inner).map_err(|err| JsValue::from_str(&err))
    }

    #[wasm_bindgen(js_name = toJSONString)]
    pub fn to_json_string_js(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes_js(&self) -> Result<Vec<u8>, JsValue> {
        self.inner.to_bytes().map_err(|err| JsValue::from_str(&err))
    }

    #[wasm_bindgen(js_name = loadJSON)]
    pub fn load_json_js(serialized: &str) -> Result<MiniSearchWasm, JsValue> {
        let inner: MiniSearch =
            serde_json::from_str(serialized).map_err(|err| JsValue::from_str(&err.to_string()))?;

        Ok(MiniSearchWasm { inner })
    }

    #[wasm_bindgen(js_name = loadBytes)]
    pub fn load_bytes_js(bytes: &[u8]) -> Result<MiniSearchWasm, JsValue> {
        let inner = MiniSearch::from_bytes(bytes).map_err(|err| JsValue::from_str(&err))?;

        Ok(MiniSearchWasm { inner })
    }
}

/// Most boundary-frugal shape: `scores` as a `Float64Array`, plus `ids` and
/// `terms` as single newline-joined strings (already built by the engine).
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

// Hand-rolled marshaling for full `search()` results, replacing
// serde-wasm-bindgen.
//
// The dominant cost of `search()` is not the engine — it's building one rich JS
// object per hit across the Wasm boundary (hundreds of hits/query). Profiling
// showed serde was not the culprit: a plain js-sys rebuild cost the same. The
// real waste is redundant string copies. Each `JsValue::from_str` copies UTF-8
// out of linear memory and allocates a fresh JS string, and in AND mode *every*
// hit matches the same handful of query terms and the same field names — so we
// were re-copying the same few strings hundreds of times.
//
// So we intern: each distinct term/field string is turned into a `JsValue`
// once and reused via `.clone()` (a cheap handle refcount bump, not a string
// copy). The output shape is byte-for-byte identical — id, score, terms,
// queryTerms, match, then flattened stored fields — but the per-call string
// allocations collapse from O(hits × terms) to O(distinct strings).
fn results_to_js(results: &[SearchResult], include_match: bool) -> JsValue {
    let id_key = JsValue::from_str("id");
    let score_key = JsValue::from_str("score");
    let terms_key = JsValue::from_str("terms");
    let query_terms_key = JsValue::from_str("queryTerms");
    let match_key = JsValue::from_str("match");

    // Interned term/field strings, shared across all hits in this result set.
    let mut interns: HashMap<&str, JsValue> = HashMap::new();

    let array = Array::new_with_length(results.len() as u32);
    for (index, result) in results.iter().enumerate() {
        let object = Object::new();
        let _ = Reflect::set(&object, &id_key, &json_to_js(&result.id));
        let _ = Reflect::set(&object, &score_key, &JsValue::from_f64(result.score));
        let _ = Reflect::set(
            &object,
            &terms_key,
            &interned_str_array(&result.terms, &mut interns),
        );
        let _ = Reflect::set(
            &object,
            &query_terms_key,
            &interned_str_array(&result.query_terms, &mut interns),
        );

        if include_match {
            let matches = Object::new();
            for (term, fields) in &result.matches {
                let key = intern(term, &mut interns);
                let _ = Reflect::set(&matches, &key, &interned_str_array(fields, &mut interns));
            }
            let _ = Reflect::set(&object, &match_key, &matches);
        }

        // `#[serde(flatten)]` stored fields, set straight onto the object.
        for (key, value) in &result.stored_fields {
            let _ = Reflect::set(&object, &JsValue::from_str(key), &json_to_js(value));
        }

        array.set(index as u32, object.into());
    }
    array.into()
}

/// Read an optional boolean flag off a JS options object without disturbing the
/// engine's typed option deserialization (unknown keys to serde are ignored).
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
fn intern<'a>(s: &'a str, cache: &mut HashMap<&'a str, JsValue>) -> JsValue {
    cache
        .entry(s)
        .or_insert_with(|| JsValue::from_str(s))
        .clone()
}

/// Build a JS `Array` of strings, interning each element.
fn interned_str_array<'a>(items: &'a [String], cache: &mut HashMap<&'a str, JsValue>) -> Array {
    let array = Array::new_with_length(items.len() as u32);
    for (index, item) in items.iter().enumerate() {
        array.set(index as u32, intern(item, cache));
    }
    array
}

/// Convert a `serde_json::Value` to a plain JS value (objects become plain
/// objects, matching the previous `json_compatible` serializer). Used for the
/// document id and any stored-field values, which are arbitrary JSON.
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
