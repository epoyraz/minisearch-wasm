mod mini_search;
mod searchable_map;

pub use mini_search::{
    Bm25Params, CombineWith, CompactSearchResult, FuzzySetting, MiniSearch, MiniSearchOptions,
    PackedSearchResults, SearchOptions, SearchResult, TokenizerMode, Weights,
};
pub use searchable_map::{FuzzyMatch, SearchableMap};

use js_sys::{Float64Array, Object, Reflect};
use serde::Serialize;
use serde_json::Value;
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
        let options: SearchOptions = if options.is_null() || options.is_undefined() {
            SearchOptions::default()
        } else {
            serde_wasm_bindgen::from_value(options)
                .map_err(|err| JsValue::from_str(&err.to_string()))?
        };

        to_js_value(&self.inner.search(query, options)).map_err(|err| JsValue::from_str(&err))
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
        packed_to_joined_js(&self.inner.search_packed_default(query, or_mode))
    }

    /// Profiling probe: runs the search but returns only the hit count, so
    /// result materialization/serialization is excluded. Lets the benchmark show
    /// pure engine compute cost separately from the boundary cost.
    #[wasm_bindgen(js_name = searchCountDefault)]
    pub fn search_count_default_js(&self, query: &str, or_mode: bool) -> f64 {
        self.inner.search_packed_default(query, or_mode).ids.len() as f64
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

/// Most boundary-frugal packed shape: `scores` as a `Float64Array`, plus
/// `ids` and `terms` as single newline-joined strings. Within a `terms` row the
/// individual terms are space-joined. The consumer splits natively in JS, so the
/// entire result set crosses the boundary as just two strings + one typed array.
fn packed_to_joined_js(packed: &PackedSearchResults) -> JsValue {
    let len = packed.ids.len();

    let scores = Float64Array::new_with_length(len as u32);
    scores.copy_from(&packed.scores);

    let mut ids = String::new();
    for (index, id) in packed.ids.iter().enumerate() {
        if index > 0 {
            ids.push('\n');
        }
        match id {
            Value::String(string) => ids.push_str(string),
            other => ids.push_str(&other.to_string()),
        }
    }

    let mut terms = String::new();
    for (index, result_terms) in packed.terms.iter().enumerate() {
        if index > 0 {
            terms.push('\n');
        }
        for (term_index, term) in result_terms.iter().enumerate() {
            if term_index > 0 {
                terms.push(' ');
            }
            terms.push_str(term);
        }
    }

    let object = Object::new();
    let _ = Reflect::set(
        &object,
        &JsValue::from_str("count"),
        &JsValue::from_f64(len as f64),
    );
    let _ = Reflect::set(&object, &JsValue::from_str("ids"), &JsValue::from_str(&ids));
    let _ = Reflect::set(&object, &JsValue::from_str("scores"), &scores);
    let _ = Reflect::set(
        &object,
        &JsValue::from_str("terms"),
        &JsValue::from_str(&terms),
    );
    object.into()
}

fn to_json_compatible_value<T: Serialize>(value: &T) -> Result<JsValue, String> {
    let json = serde_json::to_value(value).map_err(|err| err.to_string())?;
    json.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|err| err.to_string())
}

// Serialize straight to a JsValue without the intermediate serde_json::Value
// tree. Safe for result types that contain only arrays/numbers/strings/plain
// structs (no `#[serde(flatten)]`, no Rust maps that must become JS objects).
fn to_js_value<T: Serialize>(value: &T) -> Result<JsValue, String> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|err| err.to_string())
}
