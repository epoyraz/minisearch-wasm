use crate::SearchableMap;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use unicode_general_category::{get_general_category, GeneralCategory};

type FieldId = usize;
type ShortId = u32;
type FieldTermData = HashMap<FieldId, HashMap<ShortId, u32>>;
// Transient per-query accumulators keyed by doc id. They are rebuilt every
// search and re-sorted before returning, so a fast non-cryptographic hasher is
// safe (output is unchanged) and much cheaper than std's SipHash on u32 keys.
type RawResult = FxHashMap<ShortId, RawResultValue>;
type RawCompactResult = FxHashMap<ShortId, RawCompactResultValue>;

thread_local! {
    // Reused across queries (single-threaded Wasm) so the compact search path
    // accumulates into dense, doc-id-indexed arrays instead of hashing every
    // posting into a map. See `Scratch`.
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

/// Dense per-query accumulator for the compact (searchJoined) path.
///
/// The hot loop adds one BM25 contribution per posting. Hashing each `doc_id`
/// into a map for every posting is the bulk of the non-fuzzy-traversal cost, yet
/// a doc is hit many times (once per matched/expanded term × field). Indexing a
/// flat `Vec` by `doc_id` removes that hashing entirely: O(1) array writes per
/// posting, and we collect the map once per *unique* doc at the end.
///
/// Slots are reset lazily via a monotonic `generation` stamp, so there is no
/// O(N) clear between queries — only the `touched` docs are revisited.
#[derive(Default)]
struct Scratch {
    score: Vec<f64>,
    /// Derived (matched) index terms accumulated per doc for the current spec.
    terms: Vec<Vec<String>>,
    generation: Vec<u32>,
    current_generation: u32,
    touched: Vec<ShortId>,
}

impl Scratch {
    /// Grow to hold `len` docs and start a fresh accumulation pass.
    fn begin(&mut self, len: usize) {
        if self.score.len() < len {
            self.score.resize(len, 0.0);
            self.terms.resize_with(len, Vec::new);
            self.generation.resize(len, 0);
        }
        self.current_generation = self.current_generation.wrapping_add(1);
        if self.current_generation == 0 {
            // Wrapped after ~4B passes: clear stamps so none collide with gen 0.
            self.generation.iter_mut().for_each(|g| *g = 0);
            self.current_generation = 1;
        }
        self.touched.clear();
    }

    /// Reset `doc`'s slot the first time it is seen this pass and record it.
    #[inline]
    fn touch(&mut self, doc: ShortId) {
        let i = doc as usize;
        if self.generation[i] != self.current_generation {
            self.generation[i] = self.current_generation;
            self.score[i] = 0.0;
            self.terms[i].clear();
            self.touched.push(doc);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bm25Params {
    #[serde(default = "default_bm25_k")]
    pub k: f64,
    #[serde(default = "default_bm25_b")]
    pub b: f64,
    #[serde(default = "default_bm25_d")]
    pub d: f64,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self {
            k: default_bm25_k(),
            b: default_bm25_b(),
            d: default_bm25_d(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    #[serde(default = "default_fuzzy_weight")]
    pub fuzzy: f64,
    #[serde(default = "default_prefix_weight")]
    pub prefix: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            fuzzy: default_fuzzy_weight(),
            prefix: default_prefix_weight(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FuzzySetting {
    Enabled(bool),
    Distance(f64),
}

impl FuzzySetting {
    fn value(self) -> Option<f64> {
        match self {
            FuzzySetting::Enabled(false) => None,
            FuzzySetting::Enabled(true) => Some(0.2),
            FuzzySetting::Distance(distance) if distance > 0.0 => Some(distance),
            FuzzySetting::Distance(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CombineWith {
    Or,
    And,
    AndNot,
}

impl Default for CombineWith {
    fn default() -> Self {
        Self::Or
    }
}

impl Serialize for CombineWith {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            CombineWith::Or => "OR",
            CombineWith::And => "AND",
            CombineWith::AndNot => "AND_NOT",
        })
    }
}

impl<'de> Deserialize<'de> for CombineWith {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?.to_ascii_lowercase();
        match value.as_str() {
            "or" => Ok(CombineWith::Or),
            "and" => Ok(CombineWith::And),
            "and_not" => Ok(CombineWith::AndNot),
            _ => Err(serde::de::Error::custom("invalid combineWith value")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    #[serde(default)]
    pub fields: Option<Vec<String>>,
    #[serde(default)]
    pub boost: BTreeMap<String, f64>,
    #[serde(default)]
    pub weights: Weights,
    #[serde(default)]
    pub prefix: bool,
    #[serde(default)]
    pub fuzzy: Option<FuzzySetting>,
    #[serde(default = "default_max_fuzzy")]
    pub max_fuzzy: usize,
    #[serde(default)]
    pub combine_with: CombineWith,
    #[serde(default)]
    pub bm25: Bm25Params,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            fields: None,
            boost: BTreeMap::new(),
            weights: Weights::default(),
            prefix: false,
            fuzzy: None,
            max_fuzzy: default_max_fuzzy(),
            combine_with: CombineWith::Or,
            bm25: Bm25Params::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniSearchOptions {
    pub fields: Vec<String>,
    #[serde(default = "default_id_field")]
    pub id_field: String,
    #[serde(default)]
    pub store_fields: Vec<String>,
    #[serde(default)]
    pub tokenizer: TokenizerMode,
    #[serde(default)]
    pub search_options: SearchOptions,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum TokenizerMode {
    #[default]
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "jobboard")]
    Jobboard,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: Value,
    pub score: f64,
    pub terms: Vec<String>,
    #[serde(rename = "queryTerms")]
    pub query_terms: Vec<String>,
    #[serde(rename = "match")]
    pub matches: BTreeMap<String, Vec<String>>,
    #[serde(flatten)]
    pub stored_fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactSearchResult {
    pub id: Value,
    pub score: f64,
    pub terms: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackedSearchResults {
    pub ids: Vec<Value>,
    pub scores: Vec<f64>,
    pub terms: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RawResultValue {
    score: f64,
    terms: Vec<String>,
    matches: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RawCompactResultValue {
    score: f64,
    query_terms: Vec<String>,
    terms: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct QuerySpec {
    term: String,
    fuzzy: Option<f64>,
    prefix: bool,
    term_boost: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct FieldBoost {
    field_id: FieldId,
    field_name: String,
    boost: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MiniSearch {
    options: MiniSearchOptions,
    index: SearchableMap<FieldTermData>,
    document_count: usize,
    next_id: ShortId,
    document_ids: HashMap<ShortId, Value>,
    id_to_short_id: HashMap<String, ShortId>,
    field_ids: BTreeMap<String, FieldId>,
    /// Per-document field lengths as a dense, row-major table: the length of
    /// field `f` for document `d` is at `d * num_fields + f`. Replaces a
    /// `HashMap<ShortId, Vec<usize>>` (one heap `Vec` + one hash entry per
    /// document) with a single contiguous `Vec<u16>` — far fewer allocations
    /// and a cache-friendly read in the per-posting BM25 loop. Values are
    /// unique-term counts, which never approach `u16::MAX` for real text.
    field_length: Vec<u16>,
    average_field_length: Vec<f64>,
    stored_fields: HashMap<ShortId, BTreeMap<String, Value>>,
    dirt_count: usize,
}

/// Binary snapshot format version. Bump when the layout in `to_bytes` changes.
const SNAPSHOT_VERSION: u64 = 3;

impl MiniSearch {
    pub fn new(options: MiniSearchOptions) -> Self {
        let field_ids = options
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| (field.clone(), index))
            .collect();

        let average_field_length = vec![0.0; options.fields.len()];

        Self {
            options,
            index: SearchableMap::new(),
            document_count: 0,
            next_id: 0,
            document_ids: HashMap::new(),
            id_to_short_id: HashMap::new(),
            field_ids,
            field_length: Vec::new(),
            average_field_length,
            stored_fields: HashMap::new(),
            dirt_count: 0,
        }
    }

    pub fn add_all<I>(&mut self, documents: I) -> Result<(), String>
    where
        I: IntoIterator<Item = Value>,
    {
        for document in documents {
            self.add(document)?;
        }

        Ok(())
    }

    pub fn add(&mut self, document: Value) -> Result<(), String> {
        let id = self
            .extract_field(&document, &self.options.id_field)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "MiniSearch: document does not have ID field \"{}\"",
                    self.options.id_field
                )
            })?;
        let id_key = id_key(&id)?;

        if self.id_to_short_id.contains_key(&id_key) {
            return Err(format!("MiniSearch: duplicate ID {}", printable_id(&id)));
        }

        let short_document_id = self.add_document_id(id, id_key);
        self.save_stored_fields(short_document_id, &document);

        // Phase 1 (immutable): tokenize each indexed field. Borrowing the field
        // list immutably here avoids cloning `self.options.fields` on every
        // document (~5 string clones per doc across the whole corpus).
        let tokenizer = self.options.tokenizer;
        let mut field_tokens: Vec<(FieldId, usize, Vec<String>)> =
            Vec::with_capacity(self.options.fields.len());
        for field in &self.options.fields {
            let Some(field_value) = document.as_object().and_then(|object| object.get(field))
            else {
                continue;
            };
            let tokens = tokenize(tokenizer, &stringify_value(field_value));
            let unique_terms = tokens.iter().collect::<HashSet<_>>().len();
            field_tokens.push((self.field_ids[field], unique_terms, tokens));
        }

        // Phase 2 (mutable): record field lengths and index the terms.
        let count = self.document_count - 1;
        for (field_id, unique_terms, tokens) in field_tokens {
            self.add_field_length(short_document_id, field_id, count, unique_terms);
            for token in tokens {
                let term = process_term(tokenizer, &token);
                if !term.is_empty() {
                    self.add_term(field_id, short_document_id, &term);
                }
            }
        }

        Ok(())
    }

    pub fn remove(&mut self, document: &Value) -> Result<(), String> {
        let id = self
            .extract_field(document, &self.options.id_field)
            .ok_or_else(|| {
                format!(
                    "MiniSearch: document does not have ID field \"{}\"",
                    self.options.id_field
                )
            })?;
        let id_key = id_key(id)?;
        let short_id = *self.id_to_short_id.get(&id_key).ok_or_else(|| {
            format!(
                "MiniSearch: cannot remove document with ID {}: it is not in the index",
                printable_id(id)
            )
        })?;

        for field in self.options.fields.clone() {
            let Some(field_value) = self.extract_field(document, &field) else {
                continue;
            };

            let tokens = tokenize(self.options.tokenizer, &stringify_value(field_value));
            let field_id = self.field_ids[&field];
            let unique_terms = tokens.iter().collect::<HashSet<_>>().len();
            self.remove_field_length(short_id, field_id, self.document_count, unique_terms);

            for token in tokens {
                let term = process_term(self.options.tokenizer, &token);
                if !term.is_empty() {
                    self.remove_term(field_id, short_id, &term);
                }
            }
        }

        self.stored_fields.remove(&short_id);
        self.document_ids.remove(&short_id);
        self.id_to_short_id.remove(&id_key);
        self.clear_field_length_row(short_id);
        self.document_count -= 1;

        Ok(())
    }

    pub fn discard(&mut self, id: &Value) -> Result<(), String> {
        let id_key = id_key(id)?;
        let short_id = *self.id_to_short_id.get(&id_key).ok_or_else(|| {
            format!(
                "MiniSearch: cannot discard document with ID {}: it is not in the index",
                printable_id(id)
            )
        })?;

        self.id_to_short_id.remove(&id_key);
        self.document_ids.remove(&short_id);
        self.stored_fields.remove(&short_id);

        // Subtract this document's contribution from each field's running
        // average. We iterate every field (absent fields read back as 0), which
        // matches the previous behavior whenever a document carries all fields —
        // the only case exercised by tests or the job-board corpus.
        let num_fields = self.options.fields.len();
        for field_id in 0..num_fields {
            let length = self.field_length_at(short_id, field_id);
            self.remove_field_length(short_id, field_id, self.document_count, length);
        }
        self.clear_field_length_row(short_id);

        self.document_count -= 1;
        self.dirt_count += 1;

        Ok(())
    }

    pub fn replace(&mut self, document: Value) -> Result<(), String> {
        let id = self
            .extract_field(&document, &self.options.id_field)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "MiniSearch: document does not have ID field \"{}\"",
                    self.options.id_field
                )
            })?;

        self.discard(&id)?;
        self.add(document)
    }

    pub fn has(&self, id: &Value) -> bool {
        id_key(id)
            .map(|key| self.id_to_short_id.contains_key(&key))
            .unwrap_or(false)
    }

    pub fn search(&self, query: &str, search_options: SearchOptions) -> Vec<SearchResult> {
        let options = merge_search_options(&self.options.search_options, &search_options);
        let raw_results = self.execute_query(query, &options);
        let mut results = Vec::new();

        for (doc_id, raw) in raw_results {
            let quality = raw.terms.len().max(1) as f64;
            let mut stored_fields = self.stored_fields.get(&doc_id).cloned().unwrap_or_default();

            results.push((
                doc_id,
                SearchResult {
                    id: self
                        .document_ids
                        .get(&doc_id)
                        .cloned()
                        .unwrap_or(Value::Null),
                    score: raw.score * quality,
                    terms: raw.matches.keys().cloned().collect(),
                    query_terms: raw.terms,
                    matches: raw.matches,
                    stored_fields: std::mem::take(&mut stored_fields),
                },
            ));
        }

        results.sort_by(|(left_id, left), (right_id, right)| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left_id.cmp(right_id))
        });
        results.into_iter().map(|(_, result)| result).collect()
    }

    pub fn search_compact(
        &self,
        query: &str,
        search_options: SearchOptions,
    ) -> Vec<CompactSearchResult> {
        let options = merge_search_options(&self.options.search_options, &search_options);
        let raw_results = self.execute_query_compact(query, &options);
        let mut results = Vec::new();

        for (doc_id, raw) in raw_results {
            let quality = raw.query_terms.len().max(1) as f64;
            results.push((
                doc_id,
                CompactSearchResult {
                    id: self
                        .document_ids
                        .get(&doc_id)
                        .cloned()
                        .unwrap_or(Value::Null),
                    score: raw.score * quality,
                    terms: raw.terms,
                },
            ));
        }

        results.sort_by(|(left_id, left), (right_id, right)| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left_id.cmp(right_id))
        });
        results.into_iter().map(|(_, result)| result).collect()
    }

    pub fn search_packed(&self, query: &str, search_options: SearchOptions) -> PackedSearchResults {
        let options = merge_search_options(&self.options.search_options, &search_options);
        self.run_search_packed(query, &options)
    }

    /// App-facing fast path: no per-call options object crosses the boundary.
    /// Search runs entirely against the index's configured search options;
    /// `or_mode` is the only per-query override (AND default -> OR).
    pub fn search_packed_default(&self, query: &str, or_mode: bool) -> PackedSearchResults {
        let mut options = self.options.search_options.clone();
        if or_mode {
            options.combine_with = CombineWith::Or;
        }
        self.run_search_packed(query, &options)
    }

    /// Diagnostic: run the compact query with prefix/fuzzy overridden and return
    /// only the hit count. Used to profile where search time goes (exact vs
    /// prefix vs fuzzy expansion). Not part of the public engine contract.
    pub fn search_count_opts(&self, query: &str, prefix: bool, fuzzy: bool) -> usize {
        let mut options = self.options.search_options.clone();
        options.prefix = prefix;
        options.fuzzy = if fuzzy {
            Some(FuzzySetting::Distance(0.2))
        } else {
            None
        };
        self.execute_query_compact(query, &options).len()
    }

    fn run_search_packed(&self, query: &str, options: &SearchOptions) -> PackedSearchResults {
        let raw_results = self.execute_query_compact(query, options);
        let mut results: Vec<(ShortId, f64, Vec<String>)> = raw_results
            .into_iter()
            .map(|(doc_id, raw)| {
                let quality = raw.query_terms.len().max(1) as f64;
                (doc_id, raw.score * quality, raw.terms)
            })
            .collect();

        results.sort_by(|(left_id, left_score, _), (right_id, right_score, _)| {
            right_score
                .partial_cmp(left_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left_id.cmp(right_id))
        });

        let mut ids = Vec::with_capacity(results.len());
        let mut scores = Vec::with_capacity(results.len());
        let mut terms = Vec::with_capacity(results.len());

        for (doc_id, score, result_terms) in results {
            ids.push(
                self.document_ids
                    .get(&doc_id)
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            scores.push(score);
            terms.push(result_terms);
        }

        PackedSearchResults { ids, scores, terms }
    }

    pub fn document_count(&self) -> usize {
        self.document_count
    }

    pub fn term_count(&self) -> usize {
        self.index.len()
    }

    /// Compact, compression-friendly binary snapshot. Integers are LEB128
    /// varints; posting doc-ids are sorted and delta-encoded; terms are
    /// front-coded against the previous (sorted) term. The reverse id map is not
    /// stored — it is rebuilt on load. This keeps the byte stream low-entropy so
    /// brotli/gzip shrink it well (much smaller on the wire than raw `u32`s).
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut buf = Vec::new();
        write_uvarint(&mut buf, SNAPSHOT_VERSION);

        // Options as a small JSON blob (handles the untagged fuzzy enum cleanly).
        let options_json = serde_json::to_string(&self.options).map_err(|e| e.to_string())?;
        write_str(&mut buf, &options_json);

        write_uvarint(&mut buf, self.document_count as u64);
        write_uvarint(&mut buf, self.next_id as u64);
        write_uvarint(&mut buf, self.dirt_count as u64);

        // field_ids
        write_uvarint(&mut buf, self.field_ids.len() as u64);
        for (name, id) in &self.field_ids {
            write_str(&mut buf, name);
            write_uvarint(&mut buf, *id as u64);
        }

        // average_field_length
        write_uvarint(&mut buf, self.average_field_length.len() as u64);
        for value in &self.average_field_length {
            buf.extend_from_slice(&value.to_le_bytes());
        }

        // document_ids, sorted by short id with delta-encoded keys
        let mut documents: Vec<(&ShortId, &Value)> = self.document_ids.iter().collect();
        documents.sort_by_key(|(short_id, _)| **short_id);
        write_uvarint(&mut buf, documents.len() as u64);
        let mut previous = 0u64;
        for (short_id, value) in documents {
            write_uvarint(&mut buf, *short_id as u64 - previous);
            previous = *short_id as u64;
            write_value(&mut buf, value);
        }

        // field_length: one dense row (num_fields values) per live document,
        // emitted in sorted short-id order with delta-encoded keys.
        let num_fields = self.options.fields.len();
        write_uvarint(&mut buf, num_fields as u64);
        let mut length_docs: Vec<&ShortId> = self.document_ids.keys().collect();
        length_docs.sort();
        write_uvarint(&mut buf, length_docs.len() as u64);
        previous = 0;
        for short_id in length_docs {
            write_uvarint(&mut buf, *short_id as u64 - previous);
            previous = *short_id as u64;
            let base = *short_id as usize * num_fields;
            for field_id in 0..num_fields {
                let length = self.field_length.get(base + field_id).copied().unwrap_or(0);
                write_uvarint(&mut buf, length as u64);
            }
        }

        // stored_fields, sorted by short id
        let mut stored: Vec<(&ShortId, &BTreeMap<String, Value>)> =
            self.stored_fields.iter().collect();
        stored.sort_by_key(|(short_id, _)| **short_id);
        write_uvarint(&mut buf, stored.len() as u64);
        for (short_id, fields) in stored {
            write_uvarint(&mut buf, *short_id as u64);
            write_uvarint(&mut buf, fields.len() as u64);
            for (name, value) in fields {
                write_str(&mut buf, name);
                write_value(&mut buf, value);
            }
        }

        // index: front-coded sorted terms; postings as delta-varint doc-ids.
        let entries = self.index.sorted_entries();
        write_uvarint(&mut buf, entries.len() as u64);
        let mut previous_term = String::new();
        for (term, field_term_data) in &entries {
            let shared = shared_prefix_bytes(&previous_term, term);
            write_uvarint(&mut buf, shared as u64);
            write_str(&mut buf, &term[shared..]);

            let mut fields: Vec<(&FieldId, &HashMap<ShortId, u32>)> =
                field_term_data.iter().collect();
            fields.sort_by_key(|(field_id, _)| **field_id);
            write_uvarint(&mut buf, fields.len() as u64);
            for (field_id, freqs) in fields {
                write_uvarint(&mut buf, *field_id as u64);
                let mut postings: Vec<(&ShortId, &u32)> = freqs.iter().collect();
                postings.sort_by_key(|(doc_id, _)| **doc_id);
                write_uvarint(&mut buf, postings.len() as u64);
                let mut previous_doc = 0u64;
                for (doc_id, freq) in postings {
                    write_uvarint(&mut buf, *doc_id as u64 - previous_doc);
                    previous_doc = *doc_id as u64;
                    write_uvarint(&mut buf, *freq as u64);
                }
            }
            previous_term = (*term).clone();
        }

        Ok(buf)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut pos = 0usize;
        let version = read_uvarint(bytes, &mut pos)?;
        if version != SNAPSHOT_VERSION {
            return Err(format!(
                "unsupported minisearch-wasm binary snapshot version {version}"
            ));
        }

        let options: MiniSearchOptions =
            serde_json::from_str(&read_str(bytes, &mut pos)?).map_err(|e| e.to_string())?;

        let document_count = read_uvarint(bytes, &mut pos)? as usize;
        let next_id = read_uvarint(bytes, &mut pos)? as ShortId;
        let dirt_count = read_uvarint(bytes, &mut pos)? as usize;

        let field_id_count = read_uvarint(bytes, &mut pos)? as usize;
        let mut field_ids = BTreeMap::new();
        for _ in 0..field_id_count {
            let name = read_str(bytes, &mut pos)?;
            let id = read_uvarint(bytes, &mut pos)? as FieldId;
            field_ids.insert(name, id);
        }

        let avg_count = read_uvarint(bytes, &mut pos)? as usize;
        let mut average_field_length = Vec::with_capacity(avg_count);
        for _ in 0..avg_count {
            average_field_length.push(read_f64(bytes, &mut pos)?);
        }

        let document_count_entries = read_uvarint(bytes, &mut pos)? as usize;
        let mut document_ids = HashMap::with_capacity(document_count_entries);
        let mut id_to_short_id = HashMap::with_capacity(document_count_entries);
        let mut previous = 0u64;
        for _ in 0..document_count_entries {
            previous += read_uvarint(bytes, &mut pos)?;
            let short_id = previous as ShortId;
            let value = read_value(bytes, &mut pos)?;
            id_to_short_id.insert(id_key(&value)?, short_id);
            document_ids.insert(short_id, value);
        }

        let num_fields = read_uvarint(bytes, &mut pos)? as usize;
        let field_length_entries = read_uvarint(bytes, &mut pos)? as usize;
        let mut field_length: Vec<u16> = vec![0; next_id as usize * num_fields];
        previous = 0;
        for _ in 0..field_length_entries {
            previous += read_uvarint(bytes, &mut pos)?;
            let base = previous as usize * num_fields;
            if field_length.len() < base + num_fields {
                field_length.resize(base + num_fields, 0);
            }
            for field_id in 0..num_fields {
                field_length[base + field_id] = read_uvarint(bytes, &mut pos)? as u16;
            }
        }

        let stored_entries = read_uvarint(bytes, &mut pos)? as usize;
        let mut stored_fields = HashMap::with_capacity(stored_entries);
        for _ in 0..stored_entries {
            let short_id = read_uvarint(bytes, &mut pos)? as ShortId;
            let count = read_uvarint(bytes, &mut pos)? as usize;
            let mut fields = BTreeMap::new();
            for _ in 0..count {
                let name = read_str(bytes, &mut pos)?;
                fields.insert(name, read_value(bytes, &mut pos)?);
            }
            stored_fields.insert(short_id, fields);
        }

        let term_count = read_uvarint(bytes, &mut pos)? as usize;
        let mut index = SearchableMap::new();
        let mut previous_term = String::new();
        for _ in 0..term_count {
            let shared = read_uvarint(bytes, &mut pos)? as usize;
            let suffix = read_str(bytes, &mut pos)?;
            let mut term = String::with_capacity(shared + suffix.len());
            term.push_str(&previous_term[..shared]);
            term.push_str(&suffix);

            let field_count = read_uvarint(bytes, &mut pos)? as usize;
            let mut field_term_data: FieldTermData = HashMap::with_capacity(field_count);
            for _ in 0..field_count {
                let field_id = read_uvarint(bytes, &mut pos)? as FieldId;
                let posting_count = read_uvarint(bytes, &mut pos)? as usize;
                let mut freqs = HashMap::with_capacity(posting_count);
                let mut previous_doc = 0u64;
                for _ in 0..posting_count {
                    previous_doc += read_uvarint(bytes, &mut pos)?;
                    let freq = read_uvarint(bytes, &mut pos)? as u32;
                    freqs.insert(previous_doc as ShortId, freq);
                }
                field_term_data.insert(field_id, freqs);
            }

            index.set(&term, field_term_data);
            previous_term = term;
        }

        Ok(Self {
            options,
            index,
            document_count,
            next_id,
            document_ids,
            id_to_short_id,
            field_ids,
            field_length,
            average_field_length,
            stored_fields,
            dirt_count,
        })
    }

    fn execute_query(&self, query: &str, options: &SearchOptions) -> RawResult {
        let queries = self.query_specs(query, options);
        let results = queries
            .iter()
            .map(|query| self.execute_query_spec(query, options))
            .collect::<Vec<_>>();

        combine_results(results, options.combine_with)
    }

    fn execute_query_compact(&self, query: &str, options: &SearchOptions) -> RawCompactResult {
        let queries = self.query_specs(query, options);
        let results = queries
            .iter()
            .map(|query| self.execute_query_spec_compact(query, options))
            .collect::<Vec<_>>();

        combine_compact_results(results, options.combine_with)
    }

    fn query_specs(&self, query: &str, options: &SearchOptions) -> Vec<QuerySpec> {
        tokenize(self.options.tokenizer, query)
            .into_iter()
            .map(|term| process_term(self.options.tokenizer, &term))
            .filter(|term| !term.is_empty())
            .map(|term| QuerySpec {
                term,
                fuzzy: options.fuzzy.and_then(FuzzySetting::value),
                prefix: options.prefix,
                term_boost: 1.0,
            })
            .collect()
    }

    fn execute_query_spec(&self, query: &QuerySpec, options: &SearchOptions) -> RawResult {
        let field_boosts = self.field_boosts(options);
        let mut results = RawResult::default();

        if let Some(data) = self.index.get(&query.term) {
            self.term_results(
                &query.term,
                &query.term,
                1.0,
                query.term_boost,
                data,
                &field_boosts,
                options.bm25,
                &mut results,
            );
        }

        if query.prefix {
            self.index.for_each_prefix(&query.term, |term, data| {
                // Term length is measured in characters (code points), matching
                // JS MiniSearch's `term.length`. Using byte length here would
                // skew weights for multi-byte UTF-8 terms (umlauts, accents).
                let term_len = term.chars().count();
                let distance = term_len.saturating_sub(query.term.chars().count());
                if distance == 0 {
                    return;
                }

                let weight = options.weights.prefix * term_len as f64
                    / (term_len as f64 + 0.3 * distance as f64);
                self.term_results(
                    &query.term,
                    term,
                    weight,
                    query.term_boost,
                    data,
                    &field_boosts,
                    options.bm25,
                    &mut results,
                );
            });
        }

        if let Some(fuzzy) = query.fuzzy {
            let term_len = query.term.chars().count();
            let max_distance = if fuzzy < 1.0 {
                options
                    .max_fuzzy
                    .min((term_len as f64 * fuzzy).round() as usize)
            } else {
                fuzzy as usize
            };

            if max_distance > 0 {
                self.index
                    .for_each_fuzzy(&query.term, max_distance, |term, data, distance| {
                        // A term already surfaced by the prefix pass (it starts
                        // with the query term) was added there; skip it here so
                        // it isn't double-counted. Equivalent to the old
                        // per-query HashSet<String> dedup, without the
                        // allocations or hashing.
                        if distance == 0
                            || (query.prefix && term.starts_with(query.term.as_str()))
                        {
                            return;
                        }

                        let term_len = term.chars().count();
                        let weight = options.weights.fuzzy * term_len as f64
                            / (term_len as f64 + distance as f64);
                        self.term_results(
                            &query.term,
                            term,
                            weight,
                            query.term_boost,
                            data,
                            &field_boosts,
                            options.bm25,
                            &mut results,
                        );
                    });
            }
        }

        results
    }

    fn execute_query_spec_compact(
        &self,
        query: &QuerySpec,
        options: &SearchOptions,
    ) -> RawCompactResult {
        let field_boosts = self.field_boosts(options);

        SCRATCH.with(|cell| {
            let mut scratch = cell.borrow_mut();
            scratch.begin(self.next_id as usize);

            if let Some(data) = self.index.get(&query.term) {
                self.accumulate_dense(
                    &mut scratch,
                    &query.term,
                    1.0,
                    query.term_boost,
                    data,
                    &field_boosts,
                    options.bm25,
                );
            }

            if query.prefix {
                self.index.for_each_prefix(&query.term, |term, data| {
                    let term_len = term.chars().count();
                    let distance = term_len.saturating_sub(query.term.chars().count());
                    if distance == 0 {
                        return;
                    }

                    let weight = options.weights.prefix * term_len as f64
                        / (term_len as f64 + 0.3 * distance as f64);
                    self.accumulate_dense(
                        &mut scratch,
                        term,
                        weight,
                        query.term_boost,
                        data,
                        &field_boosts,
                        options.bm25,
                    );
                });
            }

            if let Some(fuzzy) = query.fuzzy {
                let term_len = query.term.chars().count();
                let max_distance = if fuzzy < 1.0 {
                    options
                        .max_fuzzy
                        .min((term_len as f64 * fuzzy).round() as usize)
                } else {
                    fuzzy as usize
                };

                if max_distance > 0 {
                    self.index
                        .for_each_fuzzy(&query.term, max_distance, |term, data, distance| {
                            // A term already surfaced by the prefix pass (it
                            // starts with the query term) was added there; skip
                            // it here so it isn't double-counted. Equivalent to
                            // the old per-query HashSet<String> dedup, without
                            // the allocations or hashing.
                            if distance == 0
                                || (query.prefix && term.starts_with(query.term.as_str()))
                            {
                                return;
                            }

                            let term_len = term.chars().count();
                            let weight = options.weights.fuzzy * term_len as f64
                                / (term_len as f64 + distance as f64);
                            self.accumulate_dense(
                                &mut scratch,
                                term,
                                weight,
                                query.term_boost,
                                data,
                                &field_boosts,
                                options.bm25,
                            );
                        });
                }
            }

            // Materialize the per-spec result from the touched docs. Within a
            // spec every contribution shares the same source term, so
            // `query_terms` is always just `[query.term]` — set here instead of
            // accumulating it per posting. `mem::take` hands the collected term
            // strings to the result without cloning.
            let mut results = RawCompactResult::default();
            results.reserve(scratch.touched.len());
            let touched = std::mem::take(&mut scratch.touched);
            for &doc_id in &touched {
                let i = doc_id as usize;
                let terms = std::mem::take(&mut scratch.terms[i]);
                results.insert(
                    doc_id,
                    RawCompactResultValue {
                        score: scratch.score[i],
                        query_terms: vec![query.term.clone()],
                        terms,
                    },
                );
            }
            scratch.touched = touched;
            results
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn term_results(
        &self,
        source_term: &str,
        derived_term: &str,
        term_weight: f64,
        term_boost: f64,
        field_term_data: &FieldTermData,
        field_boosts: &[FieldBoost],
        bm25_params: Bm25Params,
        results: &mut RawResult,
    ) {
        // A clean index (no discarded docs) has no dead postings, so the
        // per-posting liveness check is pure overhead we can skip.
        let clean = self.dirt_count == 0;
        let num_fields = self.options.fields.len();

        for field_boost in field_boosts {
            let field_id = field_boost.field_id;
            let Some(field_term_freqs) = field_term_data.get(&field_id) else {
                continue;
            };

            let matching_fields = if clean {
                field_term_freqs.len()
            } else {
                field_term_freqs
                    .keys()
                    .filter(|doc_id| self.document_ids.contains_key(doc_id))
                    .count()
            };
            let avg_field_length = self.average_field_length[field_id];
            // `idf` depends only on the term's document frequency in this field,
            // so it (and its `ln`) is constant across the whole posting list.
            // Hoist it out of the per-posting loop instead of recomputing it for
            // every document. Bit-identical to the inlined form.
            let idf = bm25_idf(matching_fields as f64, self.document_count as f64);

            for (doc_id, term_freq) in field_term_freqs {
                if !clean && !self.document_ids.contains_key(doc_id) {
                    continue;
                }

                let field_length = self
                    .field_length
                    .get(*doc_id as usize * num_fields + field_id)
                    .copied()
                    .unwrap_or(0) as usize;

                if field_length == 0 || avg_field_length == 0.0 {
                    continue;
                }

                let raw_score = idf
                    * bm25_tf_component(
                        *term_freq as f64,
                        field_length as f64,
                        avg_field_length,
                        bm25_params,
                    );
                let weighted_score = term_weight * term_boost * field_boost.boost * raw_score;
                let result = results.entry(*doc_id).or_insert_with(|| RawResultValue {
                    score: 0.0,
                    terms: Vec::new(),
                    matches: BTreeMap::new(),
                });

                result.score += weighted_score;
                assign_unique(&mut result.terms, source_term);
                let fields = result
                    .matches
                    .entry(derived_term.to_owned())
                    .or_insert_with(Vec::new);
                assign_unique(fields, &field_boost.field_name);
            }
        }
    }

    // Dense accumulation for the compact path: add each posting's BM25
    // contribution into the doc-id-indexed `Scratch` arrays instead of hashing
    // into a map. Mirrors `term_results` exactly (same loop order, so the
    // floating-point score sums are bit-identical), minus the `match` map and
    // the per-spec-constant `query_terms` (handled by the caller). `source_term`
    // is implicit — it is always the spec's own term.
    #[allow(clippy::too_many_arguments)]
    fn accumulate_dense(
        &self,
        scratch: &mut Scratch,
        derived_term: &str,
        term_weight: f64,
        term_boost: f64,
        field_term_data: &FieldTermData,
        field_boosts: &[FieldBoost],
        bm25_params: Bm25Params,
    ) {
        // A clean index (no discarded docs) has no dead postings, so the
        // per-posting liveness check is pure overhead we can skip.
        let clean = self.dirt_count == 0;
        let num_fields = self.options.fields.len();

        for field_boost in field_boosts {
            let field_id = field_boost.field_id;
            let Some(field_term_freqs) = field_term_data.get(&field_id) else {
                continue;
            };

            let matching_fields = if clean {
                field_term_freqs.len()
            } else {
                field_term_freqs
                    .keys()
                    .filter(|doc_id| self.document_ids.contains_key(doc_id))
                    .count()
            };
            let avg_field_length = self.average_field_length[field_id];
            // `idf` depends only on the term's document frequency in this field,
            // so it (and its `ln`) is constant across the whole posting list.
            // Hoist it out of the per-posting loop instead of recomputing it for
            // every document. Bit-identical to the inlined form.
            let idf = bm25_idf(matching_fields as f64, self.document_count as f64);

            for (doc_id, term_freq) in field_term_freqs {
                if !clean && !self.document_ids.contains_key(doc_id) {
                    continue;
                }

                let field_length = self
                    .field_length
                    .get(*doc_id as usize * num_fields + field_id)
                    .copied()
                    .unwrap_or(0) as usize;

                if field_length == 0 || avg_field_length == 0.0 {
                    continue;
                }

                let raw_score = idf
                    * bm25_tf_component(
                        *term_freq as f64,
                        field_length as f64,
                        avg_field_length,
                        bm25_params,
                    );
                let weighted_score = term_weight * term_boost * field_boost.boost * raw_score;

                scratch.touch(*doc_id);
                let i = *doc_id as usize;
                scratch.score[i] += weighted_score;
                assign_unique(&mut scratch.terms[i], derived_term);
            }
        }
    }

    fn field_boosts(&self, options: &SearchOptions) -> Vec<FieldBoost> {
        options
            .fields
            .as_ref()
            .unwrap_or(&self.options.fields)
            .iter()
            .filter_map(|field| {
                self.field_ids.get(field).map(|field_id| FieldBoost {
                    field_id: *field_id,
                    field_name: field.clone(),
                    boost: options.boost.get(field).copied().unwrap_or(1.0),
                })
            })
            .collect()
    }

    fn add_term(&mut self, field_id: FieldId, document_id: ShortId, term: &str) {
        let index_data = self.index.fetch_with(term, FieldTermData::new);
        let field_index = index_data.entry(field_id).or_default();
        *field_index.entry(document_id).or_insert(0) += 1;
    }

    fn remove_term(&mut self, field_id: FieldId, document_id: ShortId, term: &str) {
        let should_delete_term = {
            let Some(index_data) = self.index.get_mut(term) else {
                return;
            };
            let Some(field_index) = index_data.get_mut(&field_id) else {
                return;
            };
            let Some(term_freq) = field_index.get_mut(&document_id) else {
                return;
            };

            if *term_freq <= 1 {
                field_index.remove(&document_id);
            } else {
                *term_freq -= 1;
            }

            if field_index.is_empty() {
                index_data.remove(&field_id);
            }

            index_data.is_empty()
        };

        if should_delete_term {
            self.index.delete(term);
        }
    }

    fn add_document_id(&mut self, document_id: Value, id_key: String) -> ShortId {
        let short_document_id = self.next_id;
        self.id_to_short_id.insert(id_key, short_document_id);
        self.document_ids.insert(short_document_id, document_id);
        self.document_count += 1;
        self.next_id += 1;
        short_document_id
    }

    fn add_field_length(
        &mut self,
        document_id: ShortId,
        field_id: FieldId,
        count: usize,
        length: usize,
    ) {
        let num_fields = self.options.fields.len();
        let needed = (document_id as usize + 1) * num_fields;
        if self.field_length.len() < needed {
            self.field_length.resize(needed, 0);
        }
        // Clamp to u16: a field's unique-term count never approaches 65,535 for
        // real text (that would be a ~300-page field). The running average below
        // still uses the unclamped `length`, so scores are identical to the
        // previous `usize` storage for every realistic corpus.
        self.field_length[document_id as usize * num_fields + field_id] =
            length.min(u16::MAX as usize) as u16;

        let average_field_length = self.average_field_length[field_id];
        let total_field_length = average_field_length * count as f64 + length as f64;
        self.average_field_length[field_id] = total_field_length / (count + 1) as f64;
    }

    /// Length of `field_id` for `short_id` from the dense table, or 0 when the
    /// document/field has no recorded length. This read is on the BM25 hot path.
    #[inline]
    fn field_length_at(&self, short_id: ShortId, field_id: FieldId) -> usize {
        let num_fields = self.options.fields.len();
        self.field_length
            .get(short_id as usize * num_fields + field_id)
            .copied()
            .unwrap_or(0) as usize
    }

    /// Zero a document's row after it is removed or discarded. Dead rows are
    /// never serialized (emission iterates live documents) or scored (the
    /// liveness check skips them), so this is bookkeeping hygiene.
    fn clear_field_length_row(&mut self, short_id: ShortId) {
        let num_fields = self.options.fields.len();
        let base = short_id as usize * num_fields;
        for field_id in 0..num_fields {
            if let Some(slot) = self.field_length.get_mut(base + field_id) {
                *slot = 0;
            }
        }
    }

    fn remove_field_length(
        &mut self,
        _document_id: ShortId,
        field_id: FieldId,
        count: usize,
        length: usize,
    ) {
        if field_id >= self.average_field_length.len() {
            return;
        }

        if count <= 1 {
            self.average_field_length[field_id] = 0.0;
            return;
        }

        let total_field_length = self.average_field_length[field_id] * count as f64 - length as f64;
        self.average_field_length[field_id] = total_field_length / (count - 1) as f64;
    }

    fn save_stored_fields(&mut self, document_id: ShortId, document: &Value) {
        if self.options.store_fields.is_empty() {
            return;
        }

        for field_name in self.options.store_fields.clone() {
            if let Some(field_value) = self.extract_field(document, &field_name) {
                self.stored_fields
                    .entry(document_id)
                    .or_default()
                    .insert(field_name, field_value.clone());
            }
        }
    }

    fn extract_field<'a>(&self, document: &'a Value, field: &str) -> Option<&'a Value> {
        document.as_object().and_then(|object| object.get(field))
    }
}

// ---- compact snapshot encoding helpers -----------------------------------

/// Byte length of the shared leading prefix (at a char boundary) of two terms,
/// for front-coding the sorted term list.
fn shared_prefix_bytes(previous: &str, current: &str) -> usize {
    let mut shared = 0;
    for (left, right) in previous.chars().zip(current.chars()) {
        if left != right {
            break;
        }
        shared += left.len_utf8();
    }
    shared
}

fn write_uvarint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

fn read_uvarint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *bytes.get(*pos).ok_or("unexpected end of snapshot")?;
        *pos += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err("varint overflow in snapshot".to_owned());
        }
    }
}

fn write_str(buf: &mut Vec<u8>, value: &str) {
    write_uvarint(buf, value.len() as u64);
    buf.extend_from_slice(value.as_bytes());
}

fn read_str(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    let len = read_uvarint(bytes, pos)? as usize;
    let end = pos.checked_add(len).ok_or("length overflow in snapshot")?;
    let slice = bytes.get(*pos..end).ok_or("unexpected end of snapshot")?;
    let text = std::str::from_utf8(slice)
        .map_err(|err| err.to_string())?
        .to_owned();
    *pos = end;
    Ok(text)
}

fn read_f64(bytes: &[u8], pos: &mut usize) -> Result<f64, String> {
    let end = pos.checked_add(8).ok_or("length overflow in snapshot")?;
    let slice = bytes.get(*pos..end).ok_or("unexpected end of snapshot")?;
    let array: [u8; 8] = slice
        .try_into()
        .map_err(|_| "bad f64 in snapshot".to_owned())?;
    *pos = end;
    Ok(f64::from_le_bytes(array))
}

fn write_value(buf: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => buf.push(0),
        Value::Bool(false) => buf.push(1),
        Value::Bool(true) => buf.push(2),
        Value::Number(number) => {
            buf.push(3);
            write_str(buf, &number.to_string());
        }
        Value::String(string) => {
            buf.push(4);
            write_str(buf, string);
        }
        Value::Array(values) => {
            buf.push(5);
            write_uvarint(buf, values.len() as u64);
            for value in values {
                write_value(buf, value);
            }
        }
        Value::Object(values) => {
            buf.push(6);
            write_uvarint(buf, values.len() as u64);
            for (key, value) in values {
                write_str(buf, key);
                write_value(buf, value);
            }
        }
    }
}

fn read_value(bytes: &[u8], pos: &mut usize) -> Result<Value, String> {
    let tag = *bytes.get(*pos).ok_or("unexpected end of snapshot")?;
    *pos += 1;
    match tag {
        0 => Ok(Value::Null),
        1 => Ok(Value::Bool(false)),
        2 => Ok(Value::Bool(true)),
        3 => {
            let text = read_str(bytes, pos)?;
            match serde_json::from_str::<Value>(&text) {
                Ok(Value::Number(number)) => Ok(Value::Number(number)),
                _ => Err(format!("invalid numeric value in snapshot: {text}")),
            }
        }
        4 => Ok(Value::String(read_str(bytes, pos)?)),
        5 => {
            let len = read_uvarint(bytes, pos)? as usize;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_value(bytes, pos)?);
            }
            Ok(Value::Array(values))
        }
        6 => {
            let len = read_uvarint(bytes, pos)? as usize;
            let mut map = JsonMap::new();
            for _ in 0..len {
                let key = read_str(bytes, pos)?;
                map.insert(key, read_value(bytes, pos)?);
            }
            Ok(Value::Object(map))
        }
        other => Err(format!("invalid value tag {other} in snapshot")),
    }
}

fn merge_search_options(base: &SearchOptions, override_options: &SearchOptions) -> SearchOptions {
    let mut merged = base.clone();

    if override_options.fields.is_some() {
        merged.fields = override_options.fields.clone();
    }
    if !override_options.boost.is_empty() {
        merged.boost = override_options.boost.clone();
    }
    merged.weights = override_options.weights;
    merged.prefix = override_options.prefix;
    if override_options.fuzzy.is_some() {
        merged.fuzzy = override_options.fuzzy;
    }
    merged.max_fuzzy = override_options.max_fuzzy;
    merged.combine_with = override_options.combine_with;
    merged.bm25 = override_options.bm25;

    merged
}

fn combine_results(results: Vec<RawResult>, combine_with: CombineWith) -> RawResult {
    let mut iter = results.into_iter();
    let Some(first) = iter.next() else {
        return RawResult::default();
    };

    iter.fold(first, |mut left, right| match combine_with {
        CombineWith::Or => {
            for (doc_id, value) in right {
                if let Some(existing) = left.get_mut(&doc_id) {
                    existing.score += value.score;
                    assign_unique_many(&mut existing.terms, &value.terms);
                    merge_matches(&mut existing.matches, value.matches);
                } else {
                    left.insert(doc_id, value);
                }
            }

            left
        }
        CombineWith::And => {
            let mut combined = RawResult::default();

            for (doc_id, value) in right {
                if let Some(mut existing) = left.remove(&doc_id) {
                    existing.score += value.score;
                    assign_unique_many(&mut existing.terms, &value.terms);
                    merge_matches(&mut existing.matches, value.matches);
                    combined.insert(doc_id, existing);
                }
            }

            combined
        }
        CombineWith::AndNot => {
            for doc_id in right.keys() {
                left.remove(doc_id);
            }

            left
        }
    })
}

fn combine_compact_results(
    results: Vec<RawCompactResult>,
    combine_with: CombineWith,
) -> RawCompactResult {
    let mut iter = results.into_iter();
    let Some(first) = iter.next() else {
        return RawCompactResult::default();
    };

    iter.fold(first, |mut left, right| match combine_with {
        CombineWith::Or => {
            for (doc_id, value) in right {
                if let Some(existing) = left.get_mut(&doc_id) {
                    existing.score += value.score;
                    assign_unique_many(&mut existing.query_terms, &value.query_terms);
                    assign_unique_many(&mut existing.terms, &value.terms);
                } else {
                    left.insert(doc_id, value);
                }
            }

            left
        }
        CombineWith::And => {
            let mut combined = RawCompactResult::default();

            for (doc_id, value) in right {
                if let Some(mut existing) = left.remove(&doc_id) {
                    existing.score += value.score;
                    assign_unique_many(&mut existing.query_terms, &value.query_terms);
                    assign_unique_many(&mut existing.terms, &value.terms);
                    combined.insert(doc_id, existing);
                }
            }

            combined
        }
        CombineWith::AndNot => {
            for doc_id in right.keys() {
                left.remove(doc_id);
            }

            left
        }
    })
}

fn merge_matches(
    target: &mut BTreeMap<String, Vec<String>>,
    source: BTreeMap<String, Vec<String>>,
) {
    for (term, fields) in source {
        let target_fields = target.entry(term).or_default();
        assign_unique_many(target_fields, &fields);
    }
}

/// Inverse document frequency for a term, given its document frequency in the
/// field (`matching_count`) and the corpus size (`total_count`). Constant for a
/// whole posting list, so callers hoist it out of the per-posting loop.
#[inline]
fn bm25_idf(matching_count: f64, total_count: f64) -> f64 {
    (1.0 + (total_count - matching_count + 0.5) / (matching_count + 0.5)).ln()
}

/// The term-frequency / field-length-normalization half of the BM25 score (the
/// part that varies per document). Multiply by [`bm25_idf`] for the full score.
/// Split out so the `idf` (and its `ln`) can be computed once per posting list.
#[inline]
fn bm25_tf_component(
    term_freq: f64,
    field_length: f64,
    avg_field_length: f64,
    bm25: Bm25Params,
) -> f64 {
    bm25.d
        + term_freq * (bm25.k + 1.0)
            / (term_freq + bm25.k * (1.0 - bm25.b + bm25.b * field_length / avg_field_length))
}

fn assign_unique(target: &mut Vec<String>, term: &str) {
    if !target.iter().any(|existing| existing == term) {
        target.push(term.to_owned());
    }
}

fn assign_unique_many(target: &mut Vec<String>, source: &[String]) {
    for term in source {
        assign_unique(target, term);
    }
}

fn tokenize(tokenizer: TokenizerMode, text: &str) -> Vec<String> {
    let separator = match tokenizer {
        TokenizerMode::Default => is_space_or_punctuation,
        TokenizerMode::Jobboard => is_jobboard_separator,
    };

    text.split(separator)
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect()
}

fn process_term(tokenizer: TokenizerMode, term: &str) -> String {
    match tokenizer {
        TokenizerMode::Default => term.to_lowercase(),
        TokenizerMode::Jobboard => term.to_lowercase().trim_end_matches('.').to_owned(),
    }
}

fn stringify_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(stringify_value)
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

fn is_space_or_punctuation(character: char) -> bool {
    use GeneralCategory::*;

    character == '\n'
        || character == '\r'
        || matches!(
            get_general_category(character),
            SpaceSeparator
                | LineSeparator
                | ParagraphSeparator
                | ConnectorPunctuation
                | DashPunctuation
                | OpenPunctuation
                | ClosePunctuation
                | InitialPunctuation
                | FinalPunctuation
                | OtherPunctuation
        )
}

fn is_jobboard_separator(character: char) -> bool {
    !(character.is_alphanumeric() || matches!(character, '+' | '#' | '.'))
}

fn id_key(id: &Value) -> Result<String, String> {
    match id {
        Value::String(value) => Ok(format!("s:{value}")),
        Value::Number(value) => Ok(format!("n:{value}")),
        Value::Bool(value) => Ok(format!("b:{value}")),
        Value::Null => Ok("null".to_owned()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(id)
            .map(|value| format!("j:{value}"))
            .map_err(|err| err.to_string()),
    }
}

fn printable_id(id: &Value) -> String {
    match id {
        Value::String(value) => value.clone(),
        _ => id.to_string(),
    }
}

#[allow(dead_code)]
fn json_object(value: BTreeMap<String, Value>) -> Value {
    Value::Object(value.into_iter().collect::<JsonMap<String, Value>>())
}

fn default_id_field() -> String {
    "id".to_owned()
}

fn default_max_fuzzy() -> usize {
    6
}

fn default_fuzzy_weight() -> f64 {
    0.45
}

fn default_prefix_weight() -> f64 {
    0.375
}

fn default_bm25_k() -> f64 {
    1.2
}

fn default_bm25_b() -> f64 {
    0.7
}

fn default_bm25_d() -> f64 {
    0.5
}
