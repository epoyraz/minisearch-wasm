use crate::SearchableMap;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

mod interop;
mod lazy_cleanup;
mod snapshot;

type FieldId = usize;
type ShortId = u32;
/// One term's posting lists, keyed by field id and kept sorted by it. Indexes
/// have a handful of fields, so a sorted `Vec` beats a hash table: one
/// allocation per term instead of a hash table, a one-to-three element scan
/// per lookup in the BM25 loop, and deterministic iteration for snapshots.
///
/// Serializes as a `{fieldId: postings}` JSON map, the same shape the previous
/// `HashMap<FieldId, Postings>` produced, so `toJSON`/`loadJSON` are unchanged.
///
/// The radix tree stores it behind an `Rc` so the expansion cache can hold a
/// handle to the postings of every cached derived term and replay a query
/// without a tree lookup per term. Mutations go through `Rc::make_mut`; every
/// mutation path clears the cache first, so the `Rc` is unique and mutation
/// happens in place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldTermData(Vec<(FieldId, Postings)>);

impl FieldTermData {
    fn position(&self, field: FieldId) -> Result<usize, usize> {
        self.0.binary_search_by_key(&field, |(id, _)| *id)
    }

    #[inline]
    fn get(&self, field: FieldId) -> Option<&Postings> {
        self.0
            .iter()
            .find(|(id, _)| *id == field)
            .map(|(_, postings)| postings)
    }

    fn get_mut(&mut self, field: FieldId) -> Option<&mut Postings> {
        self.0
            .iter_mut()
            .find(|(id, _)| *id == field)
            .map(|(_, postings)| postings)
    }

    fn entry_or_default(&mut self, field: FieldId) -> &mut Postings {
        let index = match self.position(field) {
            Ok(index) => index,
            Err(index) => {
                self.0.insert(index, (field, Postings::default()));
                index
            }
        };
        &mut self.0[index].1
    }

    /// Inserts a field's postings; `false` if the field is already present.
    fn insert(&mut self, field: FieldId, postings: Postings) -> bool {
        match self.position(field) {
            Ok(_) => false,
            Err(index) => {
                self.0.insert(index, (field, postings));
                true
            }
        }
    }

    fn remove(&mut self, field: FieldId) {
        if let Ok(index) = self.position(field) {
            self.0.remove(index);
        }
    }

    fn retain(&mut self, mut keep: impl FnMut(FieldId, &mut Postings) -> bool) {
        self.0
            .retain_mut(|(field, postings)| keep(*field, postings));
    }

    fn iter(&self) -> impl Iterator<Item = (FieldId, &Postings)> + '_ {
        self.0.iter().map(|(field, postings)| (*field, postings))
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for FieldTermData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.0.iter().map(|(field, postings)| (field, postings)))
    }
}

impl<'de> Deserialize<'de> for FieldTermData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FieldTermDataVisitor;

        impl<'de> serde::de::Visitor<'de> for FieldTermDataVisitor {
            type Value = FieldTermData;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map from field id to postings")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut data = FieldTermData::default();
                while let Some((field, postings)) = map.next_entry::<FieldId, Postings>()? {
                    if !data.insert(field, postings) {
                        return Err(serde::de::Error::custom("duplicate posting field"));
                    }
                }
                Ok(data)
            }
        }

        deserializer.deserialize_map(FieldTermDataVisitor)
    }
}

/// One field's posting list for a term: `(doc id, term frequency)` pairs
/// sorted by doc id, flat and contiguous. The BM25 loop iterates it linearly
/// (no hash-bucket hopping), snapshots write it without re-sorting and read it
/// with a straight push loop, and it costs one allocation instead of a hash
/// table per (term, field). `addAll` assigns doc ids monotonically, so
/// build-time inserts append at the end.
///
/// Removal does not shift the list: an entry whose frequency reaches zero stays
/// in place as a tombstone (the second field counts them) until they outnumber
/// the live entries, and then one pass drops them all. Removing documents is
/// therefore amortized constant per posting, however long the list and
/// wherever in it the document sits. Every reader goes through `len` and
/// `iter`, which do not see tombstones.
///
/// Serializes as a `{docId: freq}` JSON map, the same shape the previous
/// `HashMap<ShortId, u32>` produced, so `toJSON`/`loadJSON` output is
/// unchanged.
#[derive(Clone, Debug, Default)]
pub struct Postings(Vec<(ShortId, u32)>, usize);

impl PartialEq for Postings {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl Postings {
    fn len(&self) -> usize {
        self.0.len() - self.1
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn iter(&self) -> impl Iterator<Item = (ShortId, u32)> + '_ {
        self.0
            .iter()
            .copied()
            .filter(|(_, frequency)| *frequency > 0)
    }

    fn increment(&mut self, doc_id: ShortId) {
        match self.0.binary_search_by_key(&doc_id, |(doc, _)| *doc) {
            Ok(index) => {
                if self.0[index].1 == 0 {
                    self.1 -= 1;
                }
                self.0[index].1 += 1;
            }
            Err(index) => self.0.insert(index, (doc_id, 1)),
        }
    }

    /// Decrement the doc's frequency; at zero the entry no longer exists.
    /// Returns `false` (changing nothing) when the doc has no entry.
    fn decrement(&mut self, doc_id: ShortId) -> bool {
        let Ok(index) = self.0.binary_search_by_key(&doc_id, |(doc, _)| *doc) else {
            return false;
        };
        let frequency = &mut self.0[index].1;
        if *frequency == 0 {
            return false;
        }
        *frequency -= 1;
        if *frequency == 0 {
            self.1 += 1;
            if self.1 * 2 > self.0.len() {
                self.sweep();
            }
        }
        true
    }

    /// Drop the tombstones.
    fn sweep(&mut self) {
        self.0.retain(|(_, frequency)| *frequency > 0);
        self.1 = 0;
    }

    /// Append a posting known to have the largest doc id so far (snapshot
    /// load: doc ids are delta-decoded in ascending order).
    fn push_sorted(&mut self, doc_id: ShortId, freq: u32) {
        debug_assert!(self.0.last().is_none_or(|(last, _)| *last < doc_id));
        self.0.push((doc_id, freq));
    }

    fn retain(&mut self, mut keep: impl FnMut(ShortId) -> bool) {
        self.0
            .retain(|(doc_id, frequency)| *frequency > 0 && keep(*doc_id));
        self.1 = 0;
    }
}

impl Serialize for Postings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.iter())
    }
}

impl<'de> Deserialize<'de> for Postings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let entries = HashMap::<ShortId, u32>::deserialize(deserializer)?;
        let mut postings: Vec<(ShortId, u32)> = entries.into_iter().collect();
        // A zero frequency is how a removed entry looks in memory; it is never
        // serialized.
        if postings.iter().any(|(_, frequency)| *frequency == 0) {
            return Err(serde::de::Error::custom("posting with zero frequency"));
        }
        postings.sort_unstable_by_key(|(doc_id, _)| *doc_id);
        Ok(Postings(postings, 0))
    }
}
// Transient per-query accumulator keyed by doc id (full `search()` path). It
// is rebuilt every search and re-sorted before returning, so a fast
// non-cryptographic hasher is safe (output is unchanged) and much cheaper than
// std's SipHash on u32 keys.
/// Insertion-ordered result map for the compatibility path, mirroring the JS
/// `Map` so equal-score ties come out in the same order. A removed entry
/// leaves a tombstone and a later re-insert appends, like `Map.delete` followed
/// by `Map.set`.
#[derive(Default)]
struct RawResult {
    entries: Vec<(ShortId, Option<RawResultValue>)>,
    index: FxHashMap<ShortId, usize>,
}

impl RawResult {
    fn len(&self) -> usize {
        self.index.len()
    }

    fn get_mut(&mut self, doc_id: ShortId) -> Option<&mut RawResultValue> {
        let slot = *self.index.get(&doc_id)?;
        self.entries[slot].1.as_mut()
    }

    fn insert(&mut self, doc_id: ShortId, value: RawResultValue) {
        match self.index.get(&doc_id) {
            Some(&slot) => self.entries[slot].1 = Some(value),
            None => {
                self.index.insert(doc_id, self.entries.len());
                self.entries.push((doc_id, Some(value)));
            }
        }
    }

    fn entry_or_insert_with(
        &mut self,
        doc_id: ShortId,
        default: impl FnOnce() -> RawResultValue,
    ) -> &mut RawResultValue {
        let slot = match self.index.get(&doc_id) {
            Some(&slot) => slot,
            None => {
                self.index.insert(doc_id, self.entries.len());
                self.entries.push((doc_id, Some(default())));
                self.entries.len() - 1
            }
        };
        self.entries[slot].1.as_mut().expect("live result entry")
    }

    fn remove(&mut self, doc_id: ShortId) -> Option<RawResultValue> {
        let slot = self.index.remove(&doc_id)?;
        self.entries[slot].1.take()
    }

    fn doc_ids(&self) -> impl Iterator<Item = ShortId> + '_ {
        self.entries
            .iter()
            .filter(|(_, value)| value.is_some())
            .map(|(doc_id, _)| *doc_id)
    }

    fn into_entries(self) -> impl Iterator<Item = (ShortId, RawResultValue)> {
        self.entries
            .into_iter()
            .filter_map(|(doc_id, value)| value.map(|value| (doc_id, value)))
    }
}

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
/// posting.
///
/// A whole multi-term query runs in ONE scratch pass — there are no per-spec
/// intermediate maps and no merge step. Per doc the pass tracks the running
/// score, the matched derived terms (as ids into a per-query interned `table`,
/// so no per-(doc, term) `String`s), and three counters that replace the old
/// combinator: `spec_count` (`AND` keeps a doc iff it equals the spec count),
/// `first_spec` (`AND_NOT` keeps a doc iff it is 0 and `spec_count` is 1), and
/// `term_count` (matched distinct query terms — the score's quality factor).
/// A query term can occur more than once and expand differently each time
/// (per-term `prefix`/`fuzzy`, or auto-suggest's prefix on the last term only),
/// so a document is credited once per distinct term, whichever occurrence
/// reaches it: `term_mask` remembers which, reproducing the old string dedup.
///
/// Scores stay bit-identical to the old per-spec-map + merge design: each
/// spec accumulates into `spec_score` and is flushed into `score` as one
/// addition per doc per spec, the same float summation order as the old
/// `existing.score += value.score` merge.
///
/// Slots are reset lazily via monotonic generation stamps, so there is no
/// O(N) clear between queries — only touched docs are revisited.
#[derive(Default)]
struct Scratch {
    /// Per-doc score across the whole query (spec subtotals flushed in).
    score: Vec<f64>,
    /// Per-doc subtotal for the spec currently accumulating.
    spec_score: Vec<f64>,
    /// Per-doc matched derived terms, as `table` ids, in first-match order.
    terms: Vec<Vec<u32>>,
    /// Per-doc number of specs that matched.
    spec_count: Vec<u32>,
    /// Per-doc number of distinct query terms that matched (quality factor).
    term_count: Vec<u32>,
    /// Per-doc set of the distinct query terms already counted, by slot; slots
    /// past the mask's width are kept in `wide_terms`.
    term_mask: Vec<u64>,
    wide_terms: HashSet<(ShortId, u32)>,
    /// Per-doc index of the first spec that touched the doc (for `AND_NOT`).
    first_spec: Vec<u32>,
    /// Per-doc id of the derived term most recently accumulated into the
    /// doc. A term's fields are processed back to back, so comparing against
    /// this skips the `terms` membership scan for all but the first posting
    /// of each (doc, term) pair.
    last_term: Vec<u32>,
    generation: Vec<u32>,
    spec_generation: Vec<u32>,
    query_counter: u32,
    spec_counter: u32,
    touched: Vec<ShortId>,
    spec_touched: Vec<ShortId>,
    /// Interned derived terms for the current query.
    table: Vec<String>,
    /// Per interned term, its value when it is a JS array-index key (all
    /// digits, canonical); `Object.keys` lists such terms first.
    table_index: Vec<Option<u32>>,
    lookup: FxHashMap<String, u32>,
}

impl Scratch {
    /// Grow to hold `len` docs and start a fresh query pass.
    fn begin_query(&mut self, len: usize) {
        if self.score.len() < len {
            self.score.resize(len, 0.0);
            self.spec_score.resize(len, 0.0);
            self.terms.resize_with(len, Vec::new);
            self.spec_count.resize(len, 0);
            self.term_count.resize(len, 0);
            self.term_mask.resize(len, 0);
            self.first_spec.resize(len, 0);
            self.last_term.resize(len, u32::MAX);
            self.generation.resize(len, 0);
            self.spec_generation.resize(len, 0);
        }
        self.query_counter = self.query_counter.wrapping_add(1);
        if self.query_counter == 0 {
            // Wrapped after ~4B passes: clear stamps so none collide with gen 0.
            self.generation.iter_mut().for_each(|g| *g = 0);
            self.query_counter = 1;
        }
        self.touched.clear();
        self.wide_terms.clear();
        self.table.clear();
        self.table_index.clear();
        self.lookup.clear();
    }

    /// Start accumulating the next query spec.
    fn begin_spec(&mut self) {
        self.spec_counter = self.spec_counter.wrapping_add(1);
        if self.spec_counter == 0 {
            self.spec_generation.iter_mut().for_each(|g| *g = 0);
            self.spec_counter = 1;
        }
        self.spec_touched.clear();
    }

    /// Add each spec-touched doc's subtotal into its query total — one
    /// addition per doc per spec, preserving the old merge's float order.
    fn flush_spec(&mut self) {
        for &doc in &self.spec_touched {
            let i = doc as usize;
            self.score[i] += self.spec_score[i];
        }
    }

    /// Id of `term` in the per-query intern table, creating it on first use.
    fn intern(&mut self, term: &str) -> u32 {
        if let Some(&id) = self.lookup.get(term) {
            return id;
        }
        let id = self.table.len() as u32;
        self.table.push(term.to_owned());
        self.table_index.push(js_array_index(term));
        self.lookup.insert(term.to_owned(), id);
        id
    }

    /// Reset `doc`'s slots the first time it is seen this query/spec and
    /// record the spec-membership counters. `term_slot` numbers the spec's
    /// query term among the query's distinct terms.
    #[inline]
    fn touch(&mut self, doc: ShortId, spec_index: u32, term_slot: u32) -> usize {
        let i = doc as usize;
        if self.generation[i] != self.query_counter {
            self.generation[i] = self.query_counter;
            self.score[i] = 0.0;
            self.terms[i].clear();
            self.spec_count[i] = 0;
            self.term_count[i] = 0;
            self.term_mask[i] = 0;
            self.first_spec[i] = spec_index;
            self.last_term[i] = u32::MAX;
            self.touched.push(doc);
        }
        if self.spec_generation[i] != self.spec_counter {
            self.spec_generation[i] = self.spec_counter;
            self.spec_score[i] = 0.0;
            self.spec_count[i] += 1;
            let first_match = match 1u64.checked_shl(term_slot) {
                Some(bit) => {
                    let unseen = self.term_mask[i] & bit == 0;
                    self.term_mask[i] |= bit;
                    unseen
                }
                None => self.wide_terms.insert((doc, term_slot)),
            };
            self.term_count[i] += u32::from(first_match);
            self.spec_touched.push(doc);
        }
        i
    }
}

/// View of a finished fused query, borrowed from the thread-local scratch:
/// ranked `(doc, score)` hits plus each doc's matched terms as ids into the
/// interned `table`. Only valid inside `run_fused_query`'s `finish` closure.
struct FusedHits<'a> {
    /// (doc id, quality-multiplied score), sorted by score; ties keep the JS
    /// `Map` order (see `run_fused_query`).
    hits: Vec<(ShortId, f64)>,
    /// Per-doc matched derived-term ids, indexed by doc id.
    terms: &'a [Vec<u32>],
    /// Interned derived terms.
    table: &'a [String],
    /// Array-index value per interned term (see `Scratch::table_index`).
    table_index: &'a [Option<u32>],
    /// Whether any interned term is array-index-like; when false every
    /// `term_ids` call is a plain borrow.
    has_index_terms: bool,
}

impl FusedHits<'_> {
    /// `doc`'s matched term ids in JS `Object.keys(match)` order: array-index
    /// terms first in ascending numeric order, then the rest in first-match
    /// order. Borrows the scratch list unless a reorder is actually needed.
    fn term_ids(&self, doc: ShortId) -> Cow<'_, [u32]> {
        let ids = &self.terms[doc as usize];
        if !self.has_index_terms
            || ids
                .iter()
                .all(|&id| self.table_index[id as usize].is_none())
        {
            return Cow::Borrowed(ids);
        }
        let mut ordered = ids.clone();
        ordered.sort_by_key(|&id| match self.table_index[id as usize] {
            Some(index) => (0u8, index),
            None => (1, 0),
        });
        Cow::Owned(ordered)
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

/// MiniSearch's `fuzzy` option: `false`, `true` (0.2), a number, or — the
/// declarative form of its per-term callback — one entry per query term.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FuzzySetting {
    Enabled(bool),
    Distance(f64),
    PerTerm(Vec<FuzzySetting>),
}

impl FuzzySetting {
    fn scalar(&self) -> Option<f64> {
        match self {
            FuzzySetting::Enabled(false) => None,
            FuzzySetting::Enabled(true) => Some(0.2),
            FuzzySetting::Distance(distance) if *distance > 0.0 => Some(*distance),
            FuzzySetting::Distance(_) | FuzzySetting::PerTerm(_) => None,
        }
    }

    /// Fuzziness for the query term at `index`: like JS
    /// `fuzzy(term, index, terms)` for the per-term form; missing entries mean
    /// no fuzzy matching for that term.
    fn for_term(&self, index: usize) -> Option<f64> {
        match self {
            FuzzySetting::PerTerm(terms) => terms.get(index).and_then(FuzzySetting::scalar),
            scalar => scalar.scalar(),
        }
    }
}

/// MiniSearch's `prefix` option: a flag for every term or — the declarative
/// form of its per-term callback — one flag per query term.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PrefixSetting {
    All(bool),
    PerTerm(Vec<bool>),
}

impl Default for PrefixSetting {
    fn default() -> Self {
        PrefixSetting::All(false)
    }
}

impl From<bool> for PrefixSetting {
    fn from(enabled: bool) -> Self {
        PrefixSetting::All(enabled)
    }
}

impl PrefixSetting {
    /// Whether the query term at `index` is prefix-expanded; missing per-term
    /// entries mean no expansion, like an undefined callback result in JS.
    fn for_term(&self, index: usize) -> bool {
        match self {
            PrefixSetting::All(enabled) => *enabled,
            PrefixSetting::PerTerm(terms) => terms.get(index).copied().unwrap_or(false),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CombineWith {
    #[default]
    Or,
    And,
    AndNot,
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
        let value = String::deserialize(deserializer)?;
        match value.to_ascii_lowercase().as_str() {
            "or" => Ok(CombineWith::Or),
            "and" => Ok(CombineWith::And),
            "and_not" => Ok(CombineWith::AndNot),
            // MiniSearch's own message for an unknown operator.
            _ => Err(serde::de::Error::custom(format!(
                "Invalid combination operator: {value}"
            ))),
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
    pub prefix: PrefixSetting,
    #[serde(default)]
    pub fuzzy: Option<FuzzySetting>,
    #[serde(default = "default_max_fuzzy")]
    pub max_fuzzy: usize,
    #[serde(default)]
    pub combine_with: CombineWith,
    #[serde(default)]
    pub bm25: Bm25Params,
    /// Declarative form of MiniSearch's `boostTerm` callback: one boost per
    /// query term (missing entries boost by 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_term: Option<Vec<f64>>,
    /// Declarative form of MiniSearch's `filter` callback: a hit is kept only
    /// if every listed stored field (or the id field) equals the given value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<BTreeMap<String, Value>>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            fields: None,
            boost: BTreeMap::new(),
            weights: Weights::default(),
            prefix: PrefixSetting::default(),
            fuzzy: None,
            max_fuzzy: default_max_fuzzy(),
            combine_with: CombineWith::Or,
            bm25: Bm25Params::default(),
            boost_term: None,
            filter: None,
        }
    }
}

/// Options for [`MiniSearch::auto_suggest`]. Unlike [`SearchOptions`], every
/// field is optional so that unset fields fall back through the same chain as
/// JS MiniSearch: per-call options → constructor `autoSuggestOptions` →
/// constructor `searchOptions` → auto-suggest defaults (`combineWith: AND`,
/// prefix search on the last query term only).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSuggestOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub boost: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<Weights>,
    /// `Some(All(true))`: prefix-expand every query term; `Some(All(false))`:
    /// none; `Some(PerTerm(..))`: per term. `None` (default): prefix-expand
    /// only the last term, matching the JS default of
    /// `(term, i, terms) => i === terms.length - 1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<PrefixSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuzzy: Option<FuzzySetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fuzzy: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combine_with: Option<CombineWith>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bm25: Option<Bm25Params>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_term: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<BTreeMap<String, Value>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VacuumOptions {
    #[serde(default)]
    pub batch_size: Option<usize>,
    #[serde(default)]
    pub batch_wait: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoVacuumOptions {
    #[serde(default)]
    pub min_dirt_count: Option<usize>,
    #[serde(default)]
    pub min_dirt_factor: Option<f64>,
    #[serde(default)]
    pub batch_size: Option<usize>,
    #[serde(default)]
    pub batch_wait: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoVacuumSetting {
    Enabled(bool),
    Options(AutoVacuumOptions),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedVacuumOptions {
    pub batch_size: usize,
    pub batch_wait: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResolvedAutoVacuumOptions {
    min_dirt_count: usize,
    min_dirt_factor: f64,
    vacuum: ResolvedVacuumOptions,
}

impl VacuumOptions {
    pub(crate) fn resolved(self) -> ResolvedVacuumOptions {
        ResolvedVacuumOptions {
            batch_size: self
                .batch_size
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_VACUUM_BATCH_SIZE),
            batch_wait: self
                .batch_wait
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_VACUUM_BATCH_WAIT),
        }
    }
}

impl AutoVacuumOptions {
    fn resolved(self) -> ResolvedAutoVacuumOptions {
        ResolvedAutoVacuumOptions {
            min_dirt_count: self
                .min_dirt_count
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_AUTO_VACUUM_MIN_DIRT_COUNT),
            min_dirt_factor: self
                .min_dirt_factor
                .filter(|value| *value > 0.0)
                .unwrap_or(DEFAULT_AUTO_VACUUM_MIN_DIRT_FACTOR),
            vacuum: VacuumOptions {
                batch_size: self.batch_size,
                batch_wait: self.batch_wait,
            }
            .resolved(),
        }
    }
}

/// Search options where every field is optional, mirroring how JS MiniSearch
/// treats per-call options and query-tree node options: a plain object whose
/// *present* keys override the inherited options (`{...inherited, ...node}`),
/// while absent keys fall through — ultimately to the constructor's
/// `searchOptions` at the leaves. [`SearchOptions`] cannot express "absent"
/// (its fields carry defaults), so merging it would clobber inherited values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialSearchOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost: Option<BTreeMap<String, f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<Weights>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<PrefixSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuzzy: Option<FuzzySetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fuzzy: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combine_with: Option<CombineWith>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bm25: Option<Bm25Params>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_term: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<BTreeMap<String, Value>>,
}

/// A search query, like JS MiniSearch's `Query` type: either a plain query
/// string, the special wildcard (matching every document), or a combination
/// of subqueries — each itself a full `Query` — merged with an `AND` / `OR` /
/// `AND_NOT` operator and optional per-node option overrides.
#[derive(Clone, Debug, PartialEq)]
// A query tree node is transient (built per call), so the size difference
// between the string and combination variants is irrelevant.
#[allow(clippy::large_enum_variant)]
pub enum Query {
    Text(String),
    Wildcard,
    Combination(QueryCombination),
}

/// A query-tree node: subqueries plus the node's option overrides. Present
/// option keys cascade down to the node's subtree, exactly like the JS
/// `{...searchOptions, ...query}` spread in `executeQuery`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryCombination {
    pub queries: Vec<Query>,
    pub options: PartialSearchOptions,
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
    /// Default options for `auto_suggest`, like the JS `autoSuggestOptions`
    /// constructor option. `None` keeps snapshots byte-identical to engines
    /// built before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_suggest_options: Option<AutoSuggestOptions>,
    /// Automatic cleanup of stale postings left by `discard`. Missing, `null`,
    /// and `true` use MiniSearch's defaults; `false` disables it; an object
    /// overrides individual thresholds and batching settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_vacuum: Option<AutoVacuumSetting>,
}

impl MiniSearchOptions {
    /// Options an index can be saved with: distinct field names, and numbers
    /// that survive JSON (a nonfinite boost or weight serializes as `null`).
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = HashSet::new();
        if let Some(duplicate) = self
            .fields
            .iter()
            .find(|field| !seen.insert(field.as_str()))
        {
            return Err(format!(
                "MiniSearch: field \"{duplicate}\" is listed more than once in option \"fields\""
            ));
        }
        let round_trip = serde_json::to_string(self)
            .ok()
            .and_then(|json| serde_json::from_str::<MiniSearchOptions>(&json).ok());
        if round_trip.as_ref() != Some(self) {
            return Err("MiniSearch: numeric options must be finite".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum TokenizerMode {
    #[default]
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "jobboard")]
    Jobboard,
}

/// Per-hit `match` map (derived term -> matched fields), kept in insertion
/// order like the JS object it mirrors. Serializes as a JSON object whose keys
/// follow JS `Object.keys` order (array-index-like terms first, ascending).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MatchInfo(Vec<(String, Vec<String>)>);

impl MatchInfo {
    /// Entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> + '_ {
        self.0
            .iter()
            .map(|(term, fields)| (term.as_str(), fields.as_slice()))
    }

    pub fn get(&self, term: &str) -> Option<&[String]> {
        self.0
            .iter()
            .find(|(existing, _)| existing == term)
            .map(|(_, fields)| fields.as_slice())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Keys in JS `Object.keys` order, which is what MiniSearch returns as a
    /// result's `terms`.
    pub fn js_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.0.iter().map(|(term, _)| term.clone()).collect();
        js_sort_object_keys(&mut keys);
        keys
    }

    /// Entries in JS `Object.keys` order.
    pub fn js_ordered(&self) -> impl Iterator<Item = (&str, &[String])> + '_ {
        let mut order: Vec<usize> = (0..self.0.len()).collect();
        order.sort_by_key(|&index| js_key_rank(&self.0[index].0));
        order.into_iter().map(move |index| {
            let (term, fields) = &self.0[index];
            (term.as_str(), fields.as_slice())
        })
    }

    fn push_field(&mut self, term: &str, field: &str) {
        match self.0.iter_mut().find(|(existing, _)| existing == term) {
            Some((_, fields)) => assign_unique(fields, field),
            None => self.0.push((term.to_owned(), vec![field.to_owned()])),
        }
    }

    /// JS `Object.assign(existing.match, match)`: incoming terms replace an
    /// existing entry's fields in place; new terms append.
    fn assign(&mut self, source: MatchInfo) {
        for (term, fields) in source.0 {
            match self.0.iter_mut().find(|(existing, _)| *existing == term) {
                Some((_, existing)) => *existing = fields,
                None => self.0.push((term, fields)),
            }
        }
    }
}

impl Serialize for MatchInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut order: Vec<usize> = (0..self.0.len()).collect();
        order.sort_by_key(|&index| js_key_rank(&self.0[index].0));
        serializer.collect_map(order.into_iter().map(|index| {
            let (term, fields) = &self.0[index];
            (term, fields)
        }))
    }
}

impl<'de> Deserialize<'de> for MatchInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MatchInfoVisitor;

        impl<'de> serde::de::Visitor<'de> for MatchInfoVisitor {
            type Value = MatchInfo;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map from matched term to fields")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut info = MatchInfo::default();
                while let Some((term, fields)) = map.next_entry::<String, Vec<String>>()? {
                    info.assign(MatchInfo(vec![(term, fields)]));
                }
                Ok(info)
            }
        }

        deserializer.deserialize_map(MatchInfoVisitor)
    }
}

/// JS array-index property key: a canonical decimal in `0..=u32::MAX - 1`.
/// `Object.keys` lists these first, in ascending numeric order, before string
/// keys in insertion order.
fn js_array_index(term: &str) -> Option<u32> {
    let bytes = term.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 10
        || !bytes.iter().all(u8::is_ascii_digit)
        || (bytes.len() > 1 && bytes[0] == b'0')
    {
        return None;
    }
    let value: u64 = term.parse().ok()?;
    (value < u64::from(u32::MAX)).then_some(value as u32)
}

fn js_key_rank(key: &str) -> (u8, u32) {
    match js_array_index(key) {
        Some(index) => (0, index),
        None => (1, 0),
    }
}

/// Reorder keys into JS `Object.keys` order (stable, so string keys keep
/// their insertion order).
fn js_sort_object_keys(keys: &mut [String]) {
    keys.sort_by_key(|key| js_key_rank(key));
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: Value,
    pub score: f64,
    pub terms: Vec<String>,
    #[serde(rename = "queryTerms")]
    pub query_terms: Vec<String>,
    #[serde(rename = "match")]
    pub matches: MatchInfo,
    #[serde(flatten)]
    pub stored_fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompactSearchResult {
    pub id: Value,
    pub score: f64,
    pub terms: Vec<String>,
}

/// One auto-suggest entry: a completed/corrected version of the query, as in JS
/// MiniSearch's `Suggestion` type. `suggestion` is always `terms` joined with a
/// single space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub suggestion: String,
    pub terms: Vec<String>,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackedSearchResults {
    pub ids: Vec<Value>,
    pub scores: Vec<f64>,
    pub terms: Vec<Vec<String>>,
}

/// Boundary transfer for the Wasm `search()` path (see
/// [`MiniSearch::search_query_transfer`]). Row `i` of `scores`, `ids` and the
/// offset arrays describes the same hit; `table` is newline-joined and shared
/// by `term_ids` and `query_term_ids`; `field_offsets` is aligned with
/// `term_ids` (one entry per matched term) and `field_names` is newline-joined
/// in field-id order. `stored` is a JSON array with one object or `null` per
/// hit, or empty when no hit has stored fields.
#[derive(Clone, Debug, PartialEq)]
pub struct CompatTransfer {
    pub ids: String,
    pub scores: Vec<f64>,
    pub table: String,
    pub term_ids: Vec<u32>,
    pub term_offsets: Vec<u32>,
    pub query_term_ids: Vec<u32>,
    pub query_term_offsets: Vec<u32>,
    pub field_ids: Vec<u32>,
    pub field_offsets: Vec<u32>,
    pub field_names: String,
    pub stored: String,
}

fn intern_term<'a>(
    interned: &mut FxHashMap<&'a str, u32>,
    table: &mut Vec<&'a str>,
    term: &'a str,
) -> u32 {
    if let Some(&id) = interned.get(term) {
        return id;
    }
    let id = table.len() as u32;
    table.push(term);
    interned.insert(term, id);
    id
}

/// Result set already materialized in the `searchJoined` boundary shape:
/// `ids` is a JSON array preserving external ID types and escaping; `terms`
/// is newline-joined (terms space-joined within a row). Scores are one vector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JoinedSearchResults {
    pub ids: String,
    pub scores: Vec<f64>,
    pub terms: String,
}

/// Result set in the most boundary-frugal shape possible: everything numeric.
/// `doc_ids` are internal short ids — the caller resolves them against the
/// one-time [`MiniSearch::doc_id_table`] — and hit `i`'s matched terms are
/// `term_ids[term_offsets[i]..term_offsets[i + 1]]`, indexing into
/// `term_table` (the query's distinct derived terms, newline-joined). Unlike
/// [`JoinedSearchResults`] no per-hit strings are built at all: each distinct
/// term crosses the boundary once, however many hits matched it.
#[derive(Clone, Debug, PartialEq)]
pub struct RawSearchResults {
    pub id_table_version: u64,
    pub doc_ids: Vec<ShortId>,
    pub scores: Vec<f64>,
    pub term_table: String,
    pub term_offsets: Vec<u32>,
    pub term_ids: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RawResultValue {
    score: f64,
    terms: Vec<String>,
    matches: MatchInfo,
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

#[derive(Clone, Debug, PartialEq)]
struct VacuumState {
    terms: Vec<String>,
    next_term: usize,
    initial_dirt_count: usize,
}

/// One prefix-expanded term: the derived term plus its precomputed length in
/// chars (JS `term.length`), from which the prefix weight is recomputed at use
/// time. Exact matches (distance 0) are never stored.
#[derive(Clone, Debug)]
struct PrefixExpansion {
    term: String,
    term_len: u32,
    /// Handle to the term's postings, so replaying the cached list needs no
    /// tree lookup. Valid until the next mutation, which clears the cache.
    data: Rc<FieldTermData>,
}

/// One fuzzy-expanded term with its precomputed char length and edit
/// distance. Exact matches (distance 0) are never stored; the "already a
/// prefix match" skip depends on the query's prefix flag, so it is applied at
/// use time, not here.
#[derive(Clone, Debug)]
struct FuzzyExpansion {
    term: String,
    term_len: u32,
    distance: u32,
    /// See [`PrefixExpansion::data`].
    data: Rc<FieldTermData>,
}

/// Memo of radix-tree expansions, keyed by query term (and max distance for
/// fuzzy). Fuzzy traversal dominates query cost (~86% on the jobboard corpus)
/// and search-as-you-type repeats the same committed terms every keystroke, so
/// replaying a cached expansion list (each entry carrying a handle to its
/// postings) replaces the whole tree walk. Entries are stored in traversal order and
/// weights are recomputed from the stored lengths, so scores and per-document
/// term order stay bit-identical to the uncached path.
///
/// Correctness: any index mutation clears the cache (see
/// `invalidate_expansions` callers) *before* touching the tree, so the
/// posting handles are unique again and `Rc::make_mut` mutates in place.
#[derive(Default)]
struct ExpansionCache {
    prefix: FxHashMap<String, std::rc::Rc<Vec<PrefixExpansion>>>,
    fuzzy: FxHashMap<(String, usize), std::rc::Rc<Vec<FuzzyExpansion>>>,
}

/// Cap on entries per cache map; on overflow the map is cleared (crude, but
/// query vocabularies are tiny compared to this bound).
const EXPANSION_CACHE_CAP: usize = 4096;

/// `RefCell`-wrapped [`ExpansionCache`] as a `MiniSearch` field: pure memo
/// state, so clones start cold and any two caches compare equal.
#[derive(Default)]
struct QueryCache(RefCell<ExpansionCache>);

impl Clone for QueryCache {
    fn clone(&self) -> Self {
        QueryCache::default()
    }
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QueryCache")
    }
}

impl PartialEq for QueryCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Set when a query meets the posting of a discarded document. JS MiniSearch
/// scores such a first query with a posting count that still includes the
/// stale entries and removes them as it goes; the `&self` query paths report
/// the encounter here so the `*_exact` entry points can redo the query through
/// the mutating replica of that behavior. Pure per-query state, like
/// [`QueryCache`]: clones start unset and any two flags compare equal.
#[derive(Default)]
struct StaleFlag(Cell<bool>);

impl Clone for StaleFlag {
    fn clone(&self) -> Self {
        StaleFlag::default()
    }
}

impl std::fmt::Debug for StaleFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StaleFlag")
    }
}

impl PartialEq for StaleFlag {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "snapshot::JsonSnapshot")]
pub struct MiniSearch {
    snapshot_version: u64,
    options: MiniSearchOptions,
    index: SearchableMap<Rc<FieldTermData>>,
    document_count: usize,
    next_id: ShortId,
    document_ids: HashMap<ShortId, Value>,
    id_to_short_id: HashMap<String, ShortId>,
    field_ids: BTreeMap<String, FieldId>,
    /// Exact unique-token counts, indexed by `doc * num_fields + field`.
    field_length: Vec<u32>,
    /// Absent/null fields must not contribute to discard's running averages.
    field_present: Vec<bool>,
    /// Liveness by short id (`false` for removed/discarded slots). A dense
    /// lookup for the per-posting check on dirty indexes; rebuilt on load.
    #[serde(skip)]
    alive: Vec<bool>,
    average_field_length: Vec<f64>,
    stored_fields: HashMap<ShortId, BTreeMap<String, Value>>,
    dirt_count: usize,
    #[serde(skip)]
    vacuum_state: Option<VacuumState>,
    #[serde(skip)]
    query_cache: QueryCache,
    #[serde(skip)]
    stale_hit: StaleFlag,
    /// Instance-local generation of the external ID table, never persisted.
    #[serde(skip)]
    id_table_version: u64,
}

/// Binary snapshot format version. Bump when the layout in `to_bytes` changes.
const SNAPSHOT_VERSION: u64 = 4;
const DEFAULT_VACUUM_BATCH_SIZE: usize = 1000;
const DEFAULT_VACUUM_BATCH_WAIT: u32 = 10;
const DEFAULT_AUTO_VACUUM_MIN_DIRT_COUNT: usize = 20;
const DEFAULT_AUTO_VACUUM_MIN_DIRT_FACTOR: f64 = 0.1;

impl MiniSearch {
    pub(crate) fn set_auto_vacuum(&mut self, setting: Option<AutoVacuumSetting>) {
        self.options.auto_vacuum = setting;
    }

    pub fn new(options: MiniSearchOptions) -> Self {
        let field_ids = options
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| (field.clone(), index))
            .collect();

        let average_field_length = vec![0.0; options.fields.len()];

        Self {
            snapshot_version: SNAPSHOT_VERSION,
            options,
            index: SearchableMap::new(),
            document_count: 0,
            next_id: 0,
            document_ids: HashMap::new(),
            id_to_short_id: HashMap::new(),
            field_ids,
            field_length: Vec::new(),
            field_present: Vec::new(),
            alive: Vec::new(),
            average_field_length,
            stored_fields: HashMap::new(),
            dirt_count: 0,
            vacuum_state: None,
            query_cache: QueryCache::default(),
            stale_hit: StaleFlag::default(),
            id_table_version: 0,
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
            .filter(|id| !id.is_null())
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

        // Phase 1 (immutable): tokenize each indexed field. Borrowing the field
        // list immutably here avoids cloning `self.options.fields` on every
        // document (~5 string clones per doc across the whole corpus).
        let tokenizer = self.options.tokenizer;
        let mut field_tokens: Vec<(FieldId, u32, Vec<String>)> =
            Vec::with_capacity(self.options.fields.len());
        for field in &self.options.fields {
            let Some(field_value) = self
                .extract_field(&document, field)
                .filter(|v| !v.is_null())
            else {
                continue;
            };
            let tokens = tokenize(tokenizer, &stringify_value(field_value));
            let unique_terms = u32::try_from(tokens.iter().collect::<HashSet<_>>().len())
                .map_err(|_| "field has too many unique tokens")?;
            field_tokens.push((self.field_ids[field], unique_terms, tokens));
        }

        // Phase 2 (mutable): record field lengths and index the terms. The
        // expansion cache is cleared first so its posting handles are unique
        // and `add_term` mutates in place.
        self.invalidate_expansions();
        let short_document_id = self.add_document_id(id, id_key)?;
        self.save_stored_fields(short_document_id, &document);
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
        let mut warnings = Vec::new();
        self.remove_with_warnings(document, &mut warnings)
    }

    /// `remove` that also reports MiniSearch's `version_conflict` warnings: a
    /// term of the given document that was not in the index, meaning the
    /// document changed after it was added. JS passes these to `logger`.
    pub fn remove_with_warnings(
        &mut self,
        document: &Value,
        warnings: &mut Vec<String>,
    ) -> Result<(), String> {
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

        self.invalidate_expansions();
        for field in self.options.fields.clone() {
            let Some(field_value) = self
                .extract_field(document, &field)
                .filter(|v| !v.is_null())
            else {
                continue;
            };

            let tokens = tokenize(self.options.tokenizer, &stringify_value(field_value));
            let field_id = self.field_ids[&field];
            let unique_terms = tokens.iter().collect::<HashSet<_>>().len();
            self.remove_field_length(short_id, field_id, self.document_count, unique_terms);

            for token in tokens {
                let term = process_term(self.options.tokenizer, &token);
                if !term.is_empty() && !self.remove_term(field_id, short_id, &term) {
                    warnings.push(format!(
                        "MiniSearch: document with ID {} has changed before removal: term \"{term}\" was not present in field \"{field}\". Removing a document after it has changed can corrupt the index!",
                        printable_id(id)
                    ));
                }
            }
        }

        self.stored_fields.remove(&short_id);
        self.document_ids.remove(&short_id);
        self.id_to_short_id.remove(&id_key);
        self.clear_field_length_row(short_id);
        self.alive[short_id as usize] = false;
        self.document_count -= 1;
        self.id_table_version += 1;

        Ok(())
    }

    /// `remove_all` collecting `version_conflict` warnings (see
    /// [`Self::remove_with_warnings`]).
    pub fn remove_all_with_warnings(
        &mut self,
        documents: Vec<Value>,
        warnings: &mut Vec<String>,
    ) -> Result<(), String> {
        for document in documents {
            self.remove_with_warnings(&document, warnings)?;
        }
        Ok(())
    }

    pub fn remove_all(&mut self, documents: Vec<Value>) -> Result<(), String> {
        self.remove_all_with_warnings(documents, &mut Vec::new())
    }

    pub fn remove_all_documents(&mut self) {
        let options = self.options.clone();
        let version = self.id_table_version + 1;
        *self = Self::new(options);
        self.id_table_version = version;
    }

    pub fn discard(&mut self, id: &Value) -> Result<(), String> {
        self.discard_without_auto_vacuum(id)?;
        self.maybe_auto_vacuum();
        Ok(())
    }

    pub(crate) fn discard_deferred(&mut self, id: &Value) -> Result<(), String> {
        self.discard_without_auto_vacuum(id)
    }

    fn discard_without_auto_vacuum(&mut self, id: &Value) -> Result<(), String> {
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

        let num_fields = self.options.fields.len();
        for field_id in 0..num_fields {
            if self.field_present[short_id as usize * num_fields + field_id] {
                let length = self.field_length_at(short_id, field_id);
                self.remove_field_length(short_id, field_id, self.document_count, length);
            }
        }
        self.clear_field_length_row(short_id);
        self.alive[short_id as usize] = false;

        self.document_count -= 1;
        self.dirt_count += 1;
        self.id_table_version += 1;

        Ok(())
    }

    pub fn discard_all(&mut self, ids: &[Value]) -> Result<(), String> {
        for id in ids {
            self.discard_without_auto_vacuum(id)?;
        }

        self.maybe_auto_vacuum();
        Ok(())
    }

    pub(crate) fn discard_all_deferred(&mut self, ids: &[Value]) -> Result<(), String> {
        for id in ids {
            self.discard_without_auto_vacuum(id)?;
        }

        Ok(())
    }

    /// `replace` without native auto-vacuum, for the Wasm wrapper, which
    /// schedules vacuuming incrementally itself.
    pub(crate) fn replace_deferred(&mut self, document: Value) -> Result<(), String> {
        let id = self
            .extract_field(&document, &self.options.id_field)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "MiniSearch: document does not have ID field \"{}\"",
                    self.options.id_field
                )
            })?;

        self.discard_without_auto_vacuum(&id)?;
        self.add(document)
    }

    /// JS `getStoredFields(id)`: the stored fields of a live document, or
    /// `None` when the id is unknown or no fields are stored.
    pub fn stored_fields_of(&self, id: &Value) -> Option<BTreeMap<String, Value>> {
        let short_id = *self.id_to_short_id.get(&id_key(id).ok()?)?;
        if self.options.store_fields.is_empty() {
            return None;
        }
        Some(
            self.stored_fields
                .get(&short_id)
                .cloned()
                .unwrap_or_default(),
        )
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

    pub fn vacuum(&mut self) {
        self.begin_vacuum();
        while !self.vacuum_step(usize::MAX) {}
    }

    pub fn begin_vacuum(&mut self) {
        if self.vacuum_state.is_some() {
            return;
        }

        // JS vacuums in tree-iteration order. The order in which emptied
        // terms leave the tree decides the key order of the merged nodes.
        let terms = self.index.keys();
        self.vacuum_state = Some(VacuumState {
            terms,
            next_term: 0,
            initial_dirt_count: self.dirt_count,
        });
    }

    /// Cleans at most `max_terms` terms from the current vacuum run. Returns
    /// `true` when the run is complete. A run is started automatically when
    /// needed, making this suitable for a JS timer-driven chunking loop.
    pub fn vacuum_step(&mut self, max_terms: usize) -> bool {
        self.begin_vacuum();

        let (start, end, terms_len) = {
            let state = self.vacuum_state.as_ref().expect("vacuum was just started");
            let start = state.next_term;
            let end = start.saturating_add(max_terms).min(state.terms.len());
            (start, end, state.terms.len())
        };

        let terms: Vec<String> = self
            .vacuum_state
            .as_ref()
            .expect("vacuum state exists")
            .terms[start..end]
            .to_vec();

        // Clear cached posting handles before mutating so `Rc::make_mut`
        // works in place.
        self.invalidate_expansions();
        for term in terms {
            let should_delete_term = {
                let alive = &self.alive;
                let Some(fields_data) = self.index.get_mut(&term) else {
                    continue;
                };
                let fields_data = Rc::make_mut(fields_data);

                fields_data.retain(|_, field_index| {
                    field_index.retain(|short_id| alive[short_id as usize]);
                    !field_index.is_empty()
                });
                fields_data.is_empty()
            };

            if should_delete_term {
                self.index.delete(&term);
            }
        }

        let state = self.vacuum_state.as_mut().expect("vacuum state exists");
        state.next_term = end;
        if end < terms_len {
            return false;
        }

        let initial_dirt_count = state.initial_dirt_count;
        self.dirt_count = self.dirt_count.saturating_sub(initial_dirt_count);
        self.vacuum_state = None;
        true
    }

    pub fn is_vacuuming(&self) -> bool {
        self.vacuum_state.is_some()
    }

    /// Reclaim dense ID/field tables after churn, preserving insertion and
    /// radix traversal order. Raw-result consumers must refresh their ID table.
    /// An active/dirty index cannot be remapped while stale postings exist.
    pub fn compact(&mut self) -> Result<(), String> {
        if self.is_vacuuming() || self.dirt_count != 0 {
            return Err("MiniSearch: vacuum must finish before compacting the index".into());
        }
        if self.next_id as usize == self.document_count {
            return Ok(());
        }
        self.invalidate_expansions();
        let mut ids: Vec<_> = self.document_ids.keys().copied().collect();
        ids.sort_unstable();
        let mapping: HashMap<_, _> = ids
            .iter()
            .enumerate()
            .map(|(new, old)| (*old, new as ShortId))
            .collect();
        let fields = self.options.fields.len();
        let mut lengths = Vec::with_capacity(ids.len() * fields);
        let mut present = Vec::with_capacity(ids.len() * fields);
        for old in &ids {
            let start = *old as usize * fields;
            lengths.extend_from_slice(&self.field_length[start..start + fields]);
            present.extend_from_slice(&self.field_present[start..start + fields]);
        }
        for value in self.id_to_short_id.values_mut() {
            *value = mapping[value];
        }
        self.document_ids = std::mem::take(&mut self.document_ids)
            .into_iter()
            .map(|(id, value)| (mapping[&id], value))
            .collect();
        self.stored_fields = std::mem::take(&mut self.stored_fields)
            .into_iter()
            .map(|(id, value)| (mapping[&id], value))
            .collect();
        // Mutate postings in place: rebuilding the radix tree would change ties.
        // Removing a document whose content changed leaves postings outside the
        // dirt count (the original API warns about this); they have no new id,
        // so they are dropped here, like a vacuum would drop them.
        let mut emptied = Vec::new();
        for term in self.index.keys() {
            let data = Rc::make_mut(self.index.get_mut(&term).expect("existing term"));
            data.retain(|_, postings| {
                postings
                    .0
                    .retain_mut(|(id, frequency)| match mapping.get(id) {
                        Some(new) if *frequency > 0 => {
                            *id = *new;
                            true
                        }
                        _ => false,
                    });
                postings.1 = 0;
                !postings.is_empty()
            });
            if data.is_empty() {
                emptied.push(term);
            }
        }
        for term in emptied {
            self.index.delete(&term);
        }
        self.field_length = lengths;
        self.field_present = present;
        self.alive = vec![true; ids.len()];
        self.next_id = ids.len() as ShortId;
        self.id_table_version += 1;
        // Scratch is thread-local rather than index-owned. Release its high-water
        // allocation at this explicit maintenance boundary, between queries.
        SCRATCH.with(|scratch| *scratch.borrow_mut() = Scratch::default());
        Ok(())
    }

    pub fn dirt_count(&self) -> usize {
        self.dirt_count
    }

    pub fn dirt_factor(&self) -> f64 {
        self.dirt_count as f64 / (1 + self.document_count + self.dirt_count) as f64
    }

    pub(crate) fn auto_vacuum_request(&self) -> Option<ResolvedVacuumOptions> {
        let options = self.resolved_auto_vacuum_options()?;
        (self.dirt_count >= options.min_dirt_count && self.dirt_factor() >= options.min_dirt_factor)
            .then_some(options.vacuum)
    }

    fn maybe_auto_vacuum(&mut self) {
        if self.auto_vacuum_request().is_some() {
            self.vacuum();
        }
    }

    fn resolved_auto_vacuum_options(&self) -> Option<ResolvedAutoVacuumOptions> {
        match self.options.auto_vacuum {
            Some(AutoVacuumSetting::Enabled(false)) => None,
            Some(AutoVacuumSetting::Options(options)) => Some(options.resolved()),
            Some(AutoVacuumSetting::Enabled(true)) | None => {
                Some(AutoVacuumOptions::default().resolved())
            }
        }
    }

    pub fn has(&self, id: &Value) -> bool {
        id_key(id)
            .map(|key| self.id_to_short_id.contains_key(&key))
            .unwrap_or(false)
    }

    pub fn search(&self, query: &str, search_options: SearchOptions) -> Vec<SearchResult> {
        let options = merge_search_options(&self.options.search_options, &search_options);
        let raw_results = self.execute_query(query, &options);
        let raw_results = self.filtered_raw_results(raw_results, options.filter.as_ref());
        self.materialize_raw_results(raw_results, true)
    }

    fn filtered_raw_results(
        &self,
        raw_results: RawResult,
        filter: Option<&BTreeMap<String, Value>>,
    ) -> RawResult {
        let Some(filter) = filter else {
            return raw_results;
        };
        let mut filtered = RawResult::default();
        for (doc_id, raw) in raw_results.into_entries() {
            if self.matches_filter(doc_id, filter) {
                filtered.insert(doc_id, raw);
            }
        }
        filtered
    }

    /// Search with a full query expression — a plain string, the wildcard, or
    /// an `AND`/`OR`/`AND_NOT` tree of subqueries — like passing a `Query` to
    /// JS MiniSearch's `search`. `per_call` carries the second-argument
    /// options; only its present keys override the constructor's
    /// `searchOptions`, and tree nodes overlay their own present keys on top,
    /// cascading down to the leaves (the JS `{...searchOptions, ...query}`
    /// spread).
    pub fn search_query(
        &self,
        query: &Query,
        per_call: &PartialSearchOptions,
    ) -> Vec<SearchResult> {
        let raw_results = self.execute_query_tree(query, per_call);
        let filter = apply_partial_options(&self.options.search_options, per_call).filter;
        let raw_results = self.filtered_raw_results(raw_results, filter.as_ref());
        // JS skips sorting a top-level wildcard (every score is 1) and returns
        // document-insertion order, which is ascending short id: ids are
        // assigned monotonically, and snapshots reload them in that order.
        let sort_by_score = !matches!(query, Query::Wildcard);
        self.materialize_raw_results(raw_results, sort_by_score)
    }

    /// Whether `doc_id` passes a declarative `filter`: every listed stored
    /// field (or the id field) equals the given value.
    fn matches_filter(&self, doc_id: ShortId, filter: &BTreeMap<String, Value>) -> bool {
        let stored = self.stored_fields.get(&doc_id);
        filter.iter().all(|(key, expected)| {
            let actual = if *key == self.options.id_field {
                self.document_ids.get(&doc_id)
            } else {
                stored.and_then(|fields| fields.get(key))
            };
            actual.is_some_and(|actual| json_equal(actual, expected))
        })
    }

    /// Ranked compatibility-path results in JS order: `(doc id, quality
    /// score, raw value)`, filtered by a declarative `filter` when set.
    fn ranked_raw_results(
        &self,
        raw_results: RawResult,
        sort_by_score: bool,
        filter: Option<&BTreeMap<String, Value>>,
    ) -> Vec<(ShortId, f64, RawResultValue)> {
        let mut ranked: Vec<(ShortId, f64, RawResultValue)> = raw_results
            .into_entries()
            .filter(|(doc_id, _)| filter.is_none_or(|filter| self.matches_filter(*doc_id, filter)))
            .map(|(doc_id, raw)| {
                let quality = raw.terms.len().max(1) as f64;
                (doc_id, raw.score * quality, raw)
            })
            .collect();
        if sort_by_score {
            // Stable and by score only, like JS `results.sort(byScore)`: equal
            // scores keep the `Map` order `RawResult` preserved.
            ranked.sort_by(|(_, left, _), (_, right, _)| {
                right.partial_cmp(left).unwrap_or(Ordering::Equal)
            });
        }
        ranked
    }

    /// Everything the Wasm `search()` needs to build MiniSearch-shaped result
    /// objects on the JavaScript side in one call: ids as a JSON array,
    /// scores, and per-hit `terms` / `queryTerms` / `match` as ids into one
    /// interned term table plus offsets, and stored fields as one JSON array.
    pub fn search_query_transfer(
        &self,
        query: &Query,
        per_call: &PartialSearchOptions,
        include_match: bool,
    ) -> CompatTransfer {
        let raw_results = self.execute_query_tree(query, per_call);
        self.transfer_from_raw(raw_results, query, per_call, include_match)
    }

    fn transfer_from_raw(
        &self,
        raw_results: RawResult,
        query: &Query,
        per_call: &PartialSearchOptions,
        include_match: bool,
    ) -> CompatTransfer {
        use std::fmt::Write;

        let sort_by_score = !matches!(query, Query::Wildcard);
        let filter = apply_partial_options(&self.options.search_options, per_call).filter;
        let ranked = self.ranked_raw_results(raw_results, sort_by_score, filter.as_ref());

        let mut transfer = CompatTransfer {
            ids: String::from("["),
            scores: Vec::with_capacity(ranked.len()),
            table: String::new(),
            term_ids: Vec::new(),
            term_offsets: vec![0],
            query_term_ids: Vec::new(),
            query_term_offsets: vec![0],
            field_ids: Vec::new(),
            field_offsets: vec![0],
            field_names: self.options.fields.join("\n"),
            stored: String::from("["),
        };
        let mut interned: FxHashMap<&str, u32> = FxHashMap::default();
        let mut table: Vec<&str> = Vec::new();
        let mut any_stored = false;

        for (index, (doc_id, score, raw)) in ranked.iter().enumerate() {
            if index > 0 {
                transfer.ids.push(',');
                transfer.stored.push(',');
            }
            match self.document_ids.get(doc_id) {
                Some(id) => {
                    let _ = write!(transfer.ids, "{id}");
                }
                None => transfer.ids.push_str("null"),
            }
            transfer.scores.push(*score);
            for (term, fields) in raw.matches.js_ordered() {
                let term_id = intern_term(&mut interned, &mut table, term);
                transfer.term_ids.push(term_id);
                if include_match {
                    for field in fields {
                        if let Some(&field_id) = self.field_ids.get(field) {
                            transfer.field_ids.push(field_id as u32);
                        }
                    }
                    transfer.field_offsets.push(transfer.field_ids.len() as u32);
                }
            }
            transfer.term_offsets.push(transfer.term_ids.len() as u32);
            for term in &raw.terms {
                let term_id = intern_term(&mut interned, &mut table, term);
                transfer.query_term_ids.push(term_id);
            }
            transfer
                .query_term_offsets
                .push(transfer.query_term_ids.len() as u32);
            match self.stored_fields.get(doc_id) {
                Some(fields) if !fields.is_empty() => {
                    any_stored = true;
                    transfer
                        .stored
                        .push_str(&serde_json::to_string(fields).unwrap_or_else(|_| "null".into()));
                }
                _ => transfer.stored.push_str("null"),
            }
        }
        transfer.ids.push(']');
        transfer.stored.push(']');
        if !any_stored {
            transfer.stored.clear();
        }
        transfer.table = table.join("\n");
        transfer
    }

    fn materialize_raw_results(
        &self,
        raw_results: RawResult,
        sort_by_score: bool,
    ) -> Vec<SearchResult> {
        let mut results = Vec::with_capacity(raw_results.len());

        for (doc_id, raw) in raw_results.into_entries() {
            let quality = raw.terms.len().max(1) as f64;
            let stored_fields = self.stored_fields.get(&doc_id).cloned().unwrap_or_default();

            results.push(SearchResult {
                id: self
                    .document_ids
                    .get(&doc_id)
                    .cloned()
                    .unwrap_or(Value::Null),
                score: raw.score * quality,
                terms: raw.matches.js_keys(),
                query_terms: raw.terms,
                matches: raw.matches,
                stored_fields,
            });
        }

        if sort_by_score {
            // Stable and by score only, like JS `results.sort(byScore)`: equal
            // scores keep the `Map` order `RawResult` preserved.
            results.sort_by(|left, right| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(Ordering::Equal)
            });
        }
        results
    }

    pub fn search_compact(
        &self,
        query: &str,
        search_options: SearchOptions,
    ) -> Vec<CompactSearchResult> {
        let options = merge_search_options(&self.options.search_options, &search_options);
        let specs = self.query_specs(query, &options);
        self.run_fused_query(&specs, &options, |fused| {
            fused
                .hits
                .iter()
                .map(|&(doc_id, score)| CompactSearchResult {
                    id: self
                        .document_ids
                        .get(&doc_id)
                        .cloned()
                        .unwrap_or(Value::Null),
                    score,
                    terms: fused
                        .term_ids(doc_id)
                        .iter()
                        .map(|&id| fused.table[id as usize].clone())
                        .collect(),
                })
                .collect()
        })
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

    /// Suggest completed/corrected versions of `query`, like JS MiniSearch's
    /// `autoSuggest`: run the query (by default combining terms with `AND` and
    /// prefix-expanding only the last term), then group the ranked results by
    /// their matched-terms phrase, averaging the scores of the documents that
    /// share a phrase.
    ///
    /// Option fallback follows JS MiniSearch: `per_call` options override the
    /// constructor's `autoSuggestOptions`, which override the constructor's
    /// `searchOptions`; `combineWith` and `prefix` skip the `searchOptions`
    /// layer and default to `AND` / last-term-only instead.
    pub fn auto_suggest(
        &self,
        query: &str,
        per_call: Option<&AutoSuggestOptions>,
    ) -> Vec<Suggestion> {
        let (specs, options) = self.auto_suggest_specs(query, per_call);
        self.group_suggestions(&specs, &options)
    }

    fn auto_suggest_specs(
        &self,
        query: &str,
        per_call: Option<&AutoSuggestOptions>,
    ) -> (Vec<QuerySpec>, SearchOptions) {
        let (options, prefix) = self.resolve_auto_suggest_options(per_call);

        // Like `query_specs`, but with per-term prefix expansion: `None`
        // prefix-expands only the last term (the JS auto-suggest default).
        let terms: Vec<String> = tokenize(self.options.tokenizer, query)
            .into_iter()
            .map(|term| process_term(self.options.tokenizer, &term))
            .filter(|term| !term.is_empty())
            .collect();
        let last_index = terms.len().saturating_sub(1);
        let specs: Vec<QuerySpec> = terms
            .into_iter()
            .enumerate()
            .map(|(index, term)| QuerySpec {
                term,
                fuzzy: options
                    .fuzzy
                    .as_ref()
                    .and_then(|fuzzy| fuzzy.for_term(index)),
                prefix: prefix
                    .as_ref()
                    .map_or(index == last_index, |prefix| prefix.for_term(index)),
                term_boost: term_boost(&options, index),
            })
            .collect();
        (specs, options)
    }

    fn group_suggestions(&self, specs: &[QuerySpec], options: &SearchOptions) -> Vec<Suggestion> {
        self.run_fused_query(specs, options, |fused| {
            // Group ranked hits by matched-terms phrase — keyed by the term-id
            // sequence, which is equivalent to the joined phrase (terms cannot
            // contain spaces) without building a string per document. A doc's
            // terms are in first-match order (query-spec order, tree-traversal
            // order within a spec), matching JS `Object.keys(match)`, so the
            // phrase is identical to the JS one. First-appearance grouping
            // order plus the stable sort below reproduce JS's tie order.
            let mut phrase_slots: FxHashMap<Cow<'_, [u32]>, usize> = FxHashMap::default();
            let mut grouped: Vec<(Cow<'_, [u32]>, f64, u32)> = Vec::new();
            for &(doc_id, score) in &fused.hits {
                let term_ids = fused.term_ids(doc_id);
                match phrase_slots.entry(term_ids) {
                    Entry::Occupied(slot) => {
                        let (_, total, count) = &mut grouped[*slot.get()];
                        *total += score;
                        *count += 1;
                    }
                    Entry::Vacant(slot) => {
                        let key = slot.key().clone();
                        slot.insert(grouped.len());
                        grouped.push((key, score, 1));
                    }
                }
            }

            let mut suggestions: Vec<Suggestion> = grouped
                .into_iter()
                .map(|(term_ids, total, count)| {
                    let terms: Vec<String> = term_ids
                        .iter()
                        .map(|&id| fused.table[id as usize].clone())
                        .collect();
                    Suggestion {
                        suggestion: terms.join(" "),
                        terms,
                        score: total / count as f64,
                    }
                })
                .collect();
            suggestions.sort_by(|left, right| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(Ordering::Equal)
            });
            suggestions
        })
    }

    /// Resolve the effective search options for `auto_suggest`, returning the
    /// merged options plus the tri-state prefix setting (`None` = expand only
    /// the last term).
    fn resolve_auto_suggest_options(
        &self,
        per_call: Option<&AutoSuggestOptions>,
    ) -> (SearchOptions, Option<PrefixSetting>) {
        let mut options = self.options.search_options.clone();
        let mut combine_with: Option<CombineWith> = None;
        let mut prefix: Option<PrefixSetting> = None;

        let layers = [self.options.auto_suggest_options.as_ref(), per_call];
        for layer in layers.into_iter().flatten() {
            if layer.fields.is_some() {
                options.fields = layer.fields.clone();
            }
            if !layer.boost.is_empty() {
                options.boost = layer.boost.clone();
            }
            if let Some(weights) = layer.weights {
                options.weights = weights;
            }
            if let Some(fuzzy) = &layer.fuzzy {
                options.fuzzy = Some(fuzzy.clone());
            }
            if let Some(max_fuzzy) = layer.max_fuzzy {
                options.max_fuzzy = max_fuzzy;
            }
            if let Some(bm25) = layer.bm25 {
                options.bm25 = bm25;
            }
            if let Some(boost_term) = &layer.boost_term {
                options.boost_term = Some(boost_term.clone());
            }
            if let Some(filter) = &layer.filter {
                options.filter = Some(filter.clone());
            }
            if layer.combine_with.is_some() {
                combine_with = layer.combine_with;
            }
            if layer.prefix.is_some() {
                prefix = layer.prefix.clone();
            }
        }

        // The auto-suggest default is AND; the constructor's searchOptions
        // combineWith intentionally does not apply here (in JS the auto-suggest
        // defaults always shadow it).
        options.combine_with = combine_with.unwrap_or(CombineWith::And);
        options.prefix = PrefixSetting::All(false);

        (options, prefix)
    }

    /// Diagnostic: run the compact query with prefix/fuzzy overridden and return
    /// only the hit count. Used to profile where search time goes (exact vs
    /// prefix vs fuzzy expansion). Not part of the public engine contract.
    pub fn search_count_opts(&self, query: &str, prefix: bool, fuzzy: bool) -> usize {
        let mut options = self.options.search_options.clone();
        options.prefix = prefix.into();
        options.fuzzy = if fuzzy {
            Some(FuzzySetting::Distance(0.2))
        } else {
            None
        };
        let specs = self.query_specs(query, &options);
        self.run_fused_query(&specs, &options, |fused| fused.hits.len())
    }

    /// Engine side of the Wasm `searchJoined`: the ranked result set already
    /// materialized as the boundary shape — `ids` as a JSON array string and
    /// `terms` newline-joined (space-joined within a row), plus a scores
    /// vector. Building the joined strings here skips the per-hit id `Value`
    /// clones and per-(doc, term) `String`s a `PackedSearchResults` would
    /// allocate just to be concatenated and thrown away at the boundary.
    pub fn search_joined_default(&self, query: &str, or_mode: bool) -> JoinedSearchResults {
        let mut options = self.options.search_options.clone();
        if or_mode {
            options.combine_with = CombineWith::Or;
        }
        self.search_joined_with(query, &options)
    }

    /// `searchJoined` with per-call option overrides: present keys of
    /// `per_call` override the index's configured search options (the same
    /// partial semantics as `search`). Lets boundary-frugal callers run e.g.
    /// exact whole-token lookups (`{prefix: false, fuzzy: false}`) without
    /// paying for the rich `search()` result shape.
    pub fn search_joined_opts(
        &self,
        query: &str,
        per_call: &PartialSearchOptions,
    ) -> JoinedSearchResults {
        let options = apply_partial_options(&self.options.search_options, per_call);
        self.search_joined_with(query, &options)
    }

    /// Engine side of the Wasm `searchRaw`: ranked hits as short doc ids +
    /// scores, matched terms as ids into a per-query interned term table.
    /// Present keys of `per_call` override the configured search options.
    pub fn search_raw(&self, query: &str, per_call: &PartialSearchOptions) -> RawSearchResults {
        let options = apply_partial_options(&self.options.search_options, per_call);
        let specs = self.query_specs(query, &options);
        self.run_fused_query(&specs, &options, |fused| {
            let mut doc_ids = Vec::with_capacity(fused.hits.len());
            let mut scores = Vec::with_capacity(fused.hits.len());
            let mut term_offsets = Vec::with_capacity(fused.hits.len() + 1);
            let mut term_ids = Vec::new();
            term_offsets.push(0);

            for &(doc_id, score) in &fused.hits {
                doc_ids.push(doc_id);
                scores.push(score);
                term_ids.extend_from_slice(&fused.term_ids(doc_id));
                term_offsets.push(term_ids.len() as u32);
            }

            RawSearchResults {
                id_table_version: self.id_table_version,
                doc_ids,
                scores,
                term_table: fused.table.join("\n"),
                term_offsets,
                term_ids,
            }
        })
    }

    /// External IDs as a JSON array in short-ID order; deleted slots are null.
    /// Cache with [`Self::id_table_version`], scoped to this engine instance.
    pub fn doc_id_table(&self) -> String {
        use std::fmt::Write;

        let mut table = String::from("[");
        for short_id in 0..self.next_id {
            if short_id > 0 {
                table.push(',');
            }
            match self.document_ids.get(&short_id) {
                Some(other) => {
                    let _ = write!(table, "{other}");
                }
                None => table.push_str("null"),
            }
        }
        table.push(']');
        table
    }

    pub fn id_table_version(&self) -> u64 {
        self.id_table_version
    }

    fn search_joined_with(&self, query: &str, options: &SearchOptions) -> JoinedSearchResults {
        use std::fmt::Write;

        let specs = self.query_specs(query, options);
        self.run_fused_query(&specs, options, |fused| {
            let mut ids = String::from("[");
            let mut terms = String::new();
            let mut scores = Vec::with_capacity(fused.hits.len());

            for (index, &(doc_id, score)) in fused.hits.iter().enumerate() {
                if index > 0 {
                    ids.push(',');
                    terms.push('\n');
                }
                match self.document_ids.get(&doc_id) {
                    Some(other) => {
                        let _ = write!(ids, "{other}");
                    }
                    None => ids.push_str("null"),
                }
                for (term_index, &id) in fused.term_ids(doc_id).iter().enumerate() {
                    if term_index > 0 {
                        terms.push(' ');
                    }
                    terms.push_str(&fused.table[id as usize]);
                }
                scores.push(score);
            }

            ids.push(']');
            JoinedSearchResults { ids, scores, terms }
        })
    }

    fn run_search_packed(&self, query: &str, options: &SearchOptions) -> PackedSearchResults {
        let specs = self.query_specs(query, options);
        self.run_fused_query(&specs, options, |fused| {
            let mut ids = Vec::with_capacity(fused.hits.len());
            let mut scores = Vec::with_capacity(fused.hits.len());
            let mut terms = Vec::with_capacity(fused.hits.len());

            for &(doc_id, score) in &fused.hits {
                ids.push(
                    self.document_ids
                        .get(&doc_id)
                        .cloned()
                        .unwrap_or(Value::Null),
                );
                scores.push(score);
                terms.push(
                    fused
                        .term_ids(doc_id)
                        .iter()
                        .map(|&id| fused.table[id as usize].clone())
                        .collect(),
                );
            }

            PackedSearchResults { ids, scores, terms }
        })
    }

    /// Run every query spec through one shared dense scratch pass, filter by
    /// the combine mode, rank, and hand the borrowed result view to `finish`.
    ///
    /// One scratch pass replaces the old per-spec map materialization and
    /// pairwise merge: `AND` keeps a doc iff its matched-spec count equals the
    /// spec count, `AND_NOT` iff only spec 0 touched it, and the quality
    /// factor is the matched distinct-term count, which equals the old merge's
    /// string dedup. Scores are bit-identical to the old design; see `Scratch`.
    fn run_fused_query<R>(
        &self,
        specs: &[QuerySpec],
        options: &SearchOptions,
        finish: impl FnOnce(FusedHits<'_>) -> R,
    ) -> R {
        let field_boosts = self.field_boosts(options);

        SCRATCH.with(|cell| {
            // Taken out for the query, not borrowed: should the query abort
            // (on wasm32 a panic does not unwind), the next one finds an empty
            // scratch instead of a borrow that is never released.
            let mut taken = cell.take();
            let scratch = &mut taken;
            scratch.begin_query(self.next_id as usize);

            let mut term_slots: FxHashMap<&str, u32> = FxHashMap::default();
            for (spec_index, spec) in specs.iter().enumerate() {
                let next_slot = term_slots.len() as u32;
                let term_slot = *term_slots.entry(spec.term.as_str()).or_insert(next_slot);
                scratch.begin_spec();
                self.execute_spec_fused(
                    scratch,
                    spec,
                    spec_index as u32,
                    term_slot,
                    options,
                    &field_boosts,
                );
                scratch.flush_spec();
            }

            let spec_count = specs.len() as u32;
            // JS folds the per-spec maps into one `Map`: `OR` and `AND_NOT`
            // keep the first spec's insertion order (first touch), `AND`
            // rebuilds the map in the *last* spec's order. The stable score
            // sort below then leaves equal scores in that order, exactly like
            // `results.sort(byScore)` in JS.
            let candidates: &[ShortId] = match options.combine_with {
                CombineWith::And if specs.len() > 1 => &scratch.spec_touched,
                _ => &scratch.touched,
            };
            let mut hits: Vec<(ShortId, f64)> = Vec::with_capacity(candidates.len());
            for &doc_id in candidates {
                let i = doc_id as usize;
                let keep = match options.combine_with {
                    CombineWith::Or => true,
                    CombineWith::And => scratch.spec_count[i] == spec_count,
                    CombineWith::AndNot => scratch.first_spec[i] == 0 && scratch.spec_count[i] == 1,
                };
                if keep {
                    let quality = scratch.term_count[i].max(1) as f64;
                    hits.push((doc_id, scratch.score[i] * quality));
                }
            }

            if let Some(filter) = &options.filter {
                hits.retain(|(doc_id, _)| self.matches_filter(*doc_id, filter));
            }

            hits.sort_by(|(_, left_score), (_, right_score)| {
                right_score
                    .partial_cmp(left_score)
                    .unwrap_or(Ordering::Equal)
            });

            let result = finish(FusedHits {
                hits,
                terms: &scratch.terms,
                table: &scratch.table,
                table_index: &scratch.table_index,
                has_index_terms: scratch.table_index.iter().any(Option::is_some),
            });
            cell.replace(taken);
            result
        })
    }

    pub fn document_count(&self) -> usize {
        self.document_count
    }

    pub fn options(&self) -> &MiniSearchOptions {
        &self.options
    }

    /// Legacy native snapshots can contain JSON objects/arrays. The public
    /// facade restores those into JS once, so subsequent reads retain identity.
    pub(crate) fn has_reference_values(&self) -> bool {
        self.document_ids
            .values()
            .chain(
                self.stored_fields
                    .values()
                    .flat_map(|fields| fields.values()),
            )
            .any(|value| matches!(value, Value::Array(_) | Value::Object(_)))
    }

    pub(crate) fn index_root(&self) -> &crate::searchable_map::RadixNode<Rc<FieldTermData>> {
        &self.index.root
    }

    pub fn term_count(&self) -> usize {
        self.index.len()
    }

    fn execute_query(&self, query: &str, options: &SearchOptions) -> RawResult {
        let queries = self.query_specs(query, options);
        let results = queries
            .iter()
            .map(|query| self.execute_query_spec(query, options))
            .collect::<Vec<_>>();

        combine_results(results, options.combine_with)
    }

    /// Recursive query-tree executor, mirroring JS `executeQuery`. `inherited`
    /// is the accumulated partial options from the per-call argument and any
    /// ancestor nodes; the constructor's `searchOptions` are merged in only at
    /// the string leaves, exactly like JS. Note the asymmetry this implies, as
    /// in JS: a combination node without its own `combineWith` combines its
    /// subqueries with `OR` (the `combineResults` default) even when the
    /// constructor's `searchOptions` say `AND` — the constructor default
    /// reaches only the term combination inside string leaves.
    fn execute_query_tree(&self, query: &Query, inherited: &PartialSearchOptions) -> RawResult {
        match query {
            Query::Wildcard => self.execute_wildcard_query(),
            Query::Text(text) => {
                let options = apply_partial_options(&self.options.search_options, inherited);
                self.execute_query(text, &options)
            }
            Query::Combination(combination) => {
                let options = overlay_partial_options(inherited, &combination.options);
                let results = combination
                    .queries
                    .iter()
                    .map(|subquery| self.execute_query_tree(subquery, &options))
                    .collect::<Vec<_>>();

                combine_results(results, options.combine_with.unwrap_or_default())
            }
        }
    }

    /// Match every live document with score 1, no matched terms — JS
    /// `executeWildcardQuery` minus the `boostDocument` callback (not ported,
    /// per the no-callbacks rule).
    fn execute_wildcard_query(&self) -> RawResult {
        // JS iterates `_documentIds` in insertion order, which is short-id order.
        let mut doc_ids: Vec<ShortId> = self.document_ids.keys().copied().collect();
        doc_ids.sort_unstable();
        let mut results = RawResult::default();
        for doc_id in doc_ids {
            results.insert(
                doc_id,
                RawResultValue {
                    score: 1.0,
                    terms: Vec::new(),
                    matches: MatchInfo::default(),
                },
            );
        }
        results
    }

    /// The processed, non-empty terms of a query string, in query order.
    pub fn query_terms(&self, query: &str) -> Vec<String> {
        tokenize(self.options.tokenizer, query)
            .into_iter()
            .map(|term| process_term(self.options.tokenizer, &term))
            .filter(|term| !term.is_empty())
            .collect()
    }

    fn query_specs(&self, query: &str, options: &SearchOptions) -> Vec<QuerySpec> {
        tokenize(self.options.tokenizer, query)
            .into_iter()
            .map(|term| process_term(self.options.tokenizer, &term))
            .filter(|term| !term.is_empty())
            .enumerate()
            .map(|(index, term)| QuerySpec {
                term,
                fuzzy: options
                    .fuzzy
                    .as_ref()
                    .and_then(|fuzzy| fuzzy.for_term(index)),
                prefix: options.prefix.for_term(index),
                term_boost: term_boost(options, index),
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
            let query_len = js_len(&query.term);
            for expansion in self.prefix_expansions(&query.term).iter() {
                let term_len = expansion.term_len as usize;
                let distance = term_len.saturating_sub(query_len);
                let weight = options.weights.prefix * term_len as f64
                    / (term_len as f64 + 0.3 * distance as f64);
                self.term_results(
                    &query.term,
                    &expansion.term,
                    weight,
                    query.term_boost,
                    &expansion.data,
                    &field_boosts,
                    options.bm25,
                    &mut results,
                );
            }
        }

        if let Some(fuzzy) = query.fuzzy {
            let term_len = js_len(&query.term);
            let max_distance = if fuzzy < 1.0 {
                options
                    .max_fuzzy
                    .min((term_len as f64 * fuzzy).round() as usize)
            } else {
                fuzzy as usize
            };

            if max_distance > 0 {
                for expansion in self.fuzzy_expansions(&query.term, max_distance).iter() {
                    // A term already surfaced by the prefix pass (it starts
                    // with the query term) was added there; skip it here so
                    // it isn't double-counted.
                    if query.prefix && expansion.term.starts_with(query.term.as_str()) {
                        continue;
                    }

                    let term_len = expansion.term_len as f64;
                    let weight =
                        options.weights.fuzzy * term_len / (term_len + expansion.distance as f64);
                    self.term_results(
                        &query.term,
                        &expansion.term,
                        weight,
                        query.term_boost,
                        &expansion.data,
                        &field_boosts,
                        options.bm25,
                        &mut results,
                    );
                }
            }
        }

        results
    }

    /// Accumulate one query spec (exact + optional prefix/fuzzy expansions)
    /// into the shared per-query scratch. Caller brackets this with
    /// `begin_spec`/`flush_spec`.
    fn execute_spec_fused(
        &self,
        scratch: &mut Scratch,
        query: &QuerySpec,
        spec_index: u32,
        term_slot: u32,
        options: &SearchOptions,
        field_boosts: &[FieldBoost],
    ) {
        if let Some(data) = self.index.get(&query.term) {
            let term_id = scratch.intern(&query.term);
            self.accumulate_dense(
                scratch,
                term_id,
                spec_index,
                term_slot,
                1.0,
                query.term_boost,
                data,
                field_boosts,
                options.bm25,
            );
        }

        if query.prefix {
            let query_len = js_len(&query.term);
            for expansion in self.prefix_expansions(&query.term).iter() {
                let term_len = expansion.term_len as usize;
                let distance = term_len.saturating_sub(query_len);
                let weight = options.weights.prefix * term_len as f64
                    / (term_len as f64 + 0.3 * distance as f64);
                let term_id = scratch.intern(&expansion.term);
                self.accumulate_dense(
                    scratch,
                    term_id,
                    spec_index,
                    term_slot,
                    weight,
                    query.term_boost,
                    &expansion.data,
                    field_boosts,
                    options.bm25,
                );
            }
        }

        if let Some(fuzzy) = query.fuzzy {
            let term_len = js_len(&query.term);
            let max_distance = if fuzzy < 1.0 {
                options
                    .max_fuzzy
                    .min((term_len as f64 * fuzzy).round() as usize)
            } else {
                fuzzy as usize
            };

            if max_distance > 0 {
                for expansion in self.fuzzy_expansions(&query.term, max_distance).iter() {
                    // A term already surfaced by the prefix pass (it starts
                    // with the query term) was added there; skip it here so
                    // it isn't double-counted.
                    if query.prefix && expansion.term.starts_with(query.term.as_str()) {
                        continue;
                    }

                    let term_len = expansion.term_len as f64;
                    let weight =
                        options.weights.fuzzy * term_len / (term_len + expansion.distance as f64);
                    let term_id = scratch.intern(&expansion.term);
                    self.accumulate_dense(
                        scratch,
                        term_id,
                        spec_index,
                        term_slot,
                        weight,
                        query.term_boost,
                        &expansion.data,
                        field_boosts,
                        options.bm25,
                    );
                }
            }
        }
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
            let Some(field_term_freqs) = field_term_data.get(field_id) else {
                continue;
            };

            let matching_fields = if clean {
                field_term_freqs.len()
            } else {
                field_term_freqs
                    .iter()
                    .filter(|(doc_id, _)| self.alive[*doc_id as usize])
                    .count()
            };
            let avg_field_length = self.average_field_length[field_id];
            // `idf` depends only on the term's document frequency in this field,
            // so it (and its `ln`) is constant across the whole posting list.
            // Hoist it out of the per-posting loop instead of recomputing it for
            // every document. Bit-identical to the inlined form.
            let idf = bm25_idf(matching_fields as f64, self.document_count as f64);

            for (doc_id, term_freq) in field_term_freqs.iter() {
                if !clean && !self.alive[doc_id as usize] {
                    self.stale_hit.0.set(true);
                    continue;
                }

                let field_length = self
                    .field_length
                    .get(doc_id as usize * num_fields + field_id)
                    .copied()
                    .unwrap_or(0) as usize;

                if field_length == 0 || avg_field_length == 0.0 {
                    continue;
                }

                let raw_score = idf
                    * bm25_tf_component(
                        term_freq as f64,
                        field_length as f64,
                        avg_field_length,
                        bm25_params,
                    );
                let weighted_score = term_weight * term_boost * field_boost.boost * raw_score;
                let result = results.entry_or_insert_with(doc_id, || RawResultValue {
                    score: 0.0,
                    terms: Vec::new(),
                    matches: MatchInfo::default(),
                });

                result.score += weighted_score;
                assign_unique(&mut result.terms, source_term);
                result
                    .matches
                    .push_field(derived_term, &field_boost.field_name);
            }
        }
    }

    // Dense accumulation for the fused compact path: add each posting's BM25
    // contribution into the doc-id-indexed `Scratch` arrays instead of hashing
    // into a map. Mirrors `term_results` exactly (same loop order, so the
    // floating-point score sums are bit-identical), minus the `match` map;
    // the matched-query-term bookkeeping is the `spec_index`/`term_slot`
    // counters and the derived term is an id into the per-query intern table.
    #[allow(clippy::too_many_arguments)]
    fn accumulate_dense(
        &self,
        scratch: &mut Scratch,
        term_id: u32,
        spec_index: u32,
        term_slot: u32,
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
            let Some(field_term_freqs) = field_term_data.get(field_id) else {
                continue;
            };

            let matching_fields = if clean {
                field_term_freqs.len()
            } else {
                field_term_freqs
                    .iter()
                    .filter(|(doc_id, _)| self.alive[*doc_id as usize])
                    .count()
            };
            let avg_field_length = self.average_field_length[field_id];
            // `idf` depends only on the term's document frequency in this field,
            // so it (and its `ln`) is constant across the whole posting list.
            // Hoist it out of the per-posting loop instead of recomputing it for
            // every document. Bit-identical to the inlined form.
            let idf = bm25_idf(matching_fields as f64, self.document_count as f64);

            for (doc_id, term_freq) in field_term_freqs.iter() {
                if !clean && !self.alive[doc_id as usize] {
                    self.stale_hit.0.set(true);
                    continue;
                }

                let field_length = self
                    .field_length
                    .get(doc_id as usize * num_fields + field_id)
                    .copied()
                    .unwrap_or(0) as usize;

                if field_length == 0 || avg_field_length == 0.0 {
                    continue;
                }

                let raw_score = idf
                    * bm25_tf_component(
                        term_freq as f64,
                        field_length as f64,
                        avg_field_length,
                        bm25_params,
                    );
                let weighted_score = term_weight * term_boost * field_boost.boost * raw_score;

                let i = scratch.touch(doc_id, spec_index, term_slot);
                scratch.spec_score[i] += weighted_score;
                if scratch.last_term[i] != term_id {
                    scratch.last_term[i] = term_id;
                    if !scratch.terms[i].contains(&term_id) {
                        scratch.terms[i].push(term_id);
                    }
                }
            }
        }
    }

    fn invalidate_expansions(&self) {
        let mut cache = self.query_cache.0.borrow_mut();
        cache.prefix.clear();
        cache.fuzzy.clear();
    }

    /// Prefix expansions of `term` (excluding the exact match), memoized in
    /// traversal order. See [`ExpansionCache`].
    fn prefix_expansions(&self, term: &str) -> std::rc::Rc<Vec<PrefixExpansion>> {
        if let Some(hit) = self.query_cache.0.borrow().prefix.get(term) {
            return std::rc::Rc::clone(hit);
        }

        let query_len = js_len(term);
        let mut expansions = Vec::new();
        self.index.for_each_prefix(term, |derived, data| {
            // Term length is measured in characters (code points), matching
            // JS MiniSearch's `term.length`. Using byte length here would
            // skew weights for multi-byte UTF-8 terms (umlauts, accents).
            let term_len = js_len(derived);
            if term_len == query_len {
                return; // Skip exact match.
            }
            expansions.push(PrefixExpansion {
                term: derived.to_owned(),
                term_len: term_len as u32,
                data: Rc::clone(data),
            });
        });

        let expansions = std::rc::Rc::new(expansions);
        let mut cache = self.query_cache.0.borrow_mut();
        if cache.prefix.len() >= EXPANSION_CACHE_CAP {
            cache.prefix.clear();
        }
        cache
            .prefix
            .insert(term.to_owned(), std::rc::Rc::clone(&expansions));
        expansions
    }

    /// Fuzzy expansions of `term` within `max_distance` (excluding the exact
    /// match), memoized in traversal order. See [`ExpansionCache`].
    fn fuzzy_expansions(
        &self,
        term: &str,
        max_distance: usize,
    ) -> std::rc::Rc<Vec<FuzzyExpansion>> {
        let key = (term.to_owned(), max_distance);
        if let Some(hit) = self.query_cache.0.borrow().fuzzy.get(&key) {
            return std::rc::Rc::clone(hit);
        }

        let mut expansions = Vec::new();
        self.index
            .for_each_fuzzy(term, max_distance, |derived, data, distance| {
                if distance == 0 {
                    return; // Skip exact match.
                }
                expansions.push(FuzzyExpansion {
                    term: derived.to_owned(),
                    term_len: js_len(derived) as u32,
                    distance: distance as u32,
                    data: Rc::clone(data),
                });
            });

        let expansions = std::rc::Rc::new(expansions);
        let mut cache = self.query_cache.0.borrow_mut();
        if cache.fuzzy.len() >= EXPANSION_CACHE_CAP {
            cache.fuzzy.clear();
        }
        cache.fuzzy.insert(key, std::rc::Rc::clone(&expansions));
        expansions
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
                    // JS `boost[field] || 1`: a zero (or NaN) boost means none.
                    boost: options
                        .boost
                        .get(field)
                        .copied()
                        .filter(|boost| *boost != 0.0 && !boost.is_nan())
                        .unwrap_or(1.0),
                })
            })
            .collect()
    }

    fn add_term(&mut self, field_id: FieldId, document_id: ShortId, term: &str) {
        let index_data = self
            .index
            .fetch_with(term, || Rc::new(FieldTermData::default()));
        Rc::make_mut(index_data)
            .entry_or_default(field_id)
            .increment(document_id);
    }

    /// Removes one posting; `false` when the term was not indexed for the
    /// document (JS logs this as a `version_conflict`).
    fn remove_term(&mut self, field_id: FieldId, document_id: ShortId, term: &str) -> bool {
        let should_delete_term = {
            let Some(index_data) = self.index.get_mut(term) else {
                return false;
            };
            let index_data = Rc::make_mut(index_data);
            let Some(field_index) = index_data.get_mut(field_id) else {
                return false;
            };
            if !field_index.decrement(document_id) {
                return false;
            }

            if field_index.is_empty() {
                index_data.remove(field_id);
            }

            index_data.is_empty()
        };

        if should_delete_term {
            self.index.delete(term);
        }
        true
    }

    fn add_document_id(&mut self, document_id: Value, id_key: String) -> Result<ShortId, String> {
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or("document ID capacity exhausted")?;
        let slots = (next_id as usize)
            .checked_mul(self.options.fields.len())
            .ok_or("field table capacity exhausted")?;
        self.field_length.resize(slots, 0);
        self.field_present.resize(slots, false);
        self.alive.resize(next_id as usize, false);
        let short_document_id = self.next_id;
        self.alive[short_document_id as usize] = true;
        self.id_to_short_id.insert(id_key, short_document_id);
        self.document_ids.insert(short_document_id, document_id);
        self.document_count += 1;
        self.next_id = next_id;
        self.id_table_version += 1;
        Ok(short_document_id)
    }

    fn add_field_length(
        &mut self,
        document_id: ShortId,
        field_id: FieldId,
        count: usize,
        length: u32,
    ) {
        let num_fields = self.options.fields.len();
        let slot = document_id as usize * num_fields + field_id;
        self.field_length[slot] = length;
        self.field_present[slot] = true;

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
            if let Some(slot) = self.field_present.get_mut(base + field_id) {
                *slot = false;
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

fn merge_search_options(base: &SearchOptions, override_options: &SearchOptions) -> SearchOptions {
    let mut merged = base.clone();

    if override_options.fields.is_some() {
        merged.fields = override_options.fields.clone();
    }
    if !override_options.boost.is_empty() {
        merged.boost = override_options.boost.clone();
    }
    merged.weights = override_options.weights;
    merged.prefix = override_options.prefix.clone();
    if override_options.fuzzy.is_some() {
        merged.fuzzy = override_options.fuzzy.clone();
    }
    merged.max_fuzzy = override_options.max_fuzzy;
    merged.combine_with = override_options.combine_with;
    merged.bm25 = override_options.bm25;
    if override_options.boost_term.is_some() {
        merged.boost_term = override_options.boost_term.clone();
    }
    if override_options.filter.is_some() {
        merged.filter = override_options.filter.clone();
    }

    merged
}

/// Apply a partial's present keys over concrete options — the leaf-level
/// `{...globalSearchOptions, ...accumulated}` merge in JS `executeQuery`.
fn apply_partial_options(base: &SearchOptions, partial: &PartialSearchOptions) -> SearchOptions {
    let mut merged = base.clone();

    if let Some(fields) = &partial.fields {
        merged.fields = Some(fields.clone());
    }
    if let Some(boost) = &partial.boost {
        merged.boost = boost.clone();
    }
    if let Some(weights) = partial.weights {
        merged.weights = weights;
    }
    if let Some(prefix) = &partial.prefix {
        merged.prefix = prefix.clone();
    }
    if let Some(fuzzy) = &partial.fuzzy {
        merged.fuzzy = Some(fuzzy.clone());
    }
    if let Some(max_fuzzy) = partial.max_fuzzy {
        merged.max_fuzzy = max_fuzzy;
    }
    if let Some(combine_with) = partial.combine_with {
        merged.combine_with = combine_with;
    }
    if let Some(bm25) = partial.bm25 {
        merged.bm25 = bm25;
    }
    if let Some(boost_term) = &partial.boost_term {
        merged.boost_term = Some(boost_term.clone());
    }
    if let Some(filter) = &partial.filter {
        merged.filter = Some(filter.clone());
    }

    merged
}

/// Overlay one partial on another — the per-node `{...inherited, ...node}`
/// spread in JS `executeQuery`: `over`'s present keys win, absent keys fall
/// through to `base`.
fn overlay_partial_options(
    base: &PartialSearchOptions,
    over: &PartialSearchOptions,
) -> PartialSearchOptions {
    PartialSearchOptions {
        fields: over.fields.clone().or_else(|| base.fields.clone()),
        boost: over.boost.clone().or_else(|| base.boost.clone()),
        weights: over.weights.or(base.weights),
        prefix: over.prefix.clone().or_else(|| base.prefix.clone()),
        fuzzy: over.fuzzy.clone().or_else(|| base.fuzzy.clone()),
        max_fuzzy: over.max_fuzzy.or(base.max_fuzzy),
        combine_with: over.combine_with.or(base.combine_with),
        bm25: over.bm25.or(base.bm25),
        boost_term: over.boost_term.clone().or_else(|| base.boost_term.clone()),
        filter: over.filter.clone().or_else(|| base.filter.clone()),
    }
}

/// Boost of the query term at `index` (JS `boostTerm(term, index, terms)`).
fn term_boost(options: &SearchOptions, index: usize) -> f64 {
    options
        .boost_term
        .as_ref()
        .and_then(|boosts| boosts.get(index).copied())
        .unwrap_or(1.0)
}

/// Fold per-spec (or per-subquery) results left to right like JS
/// `combineResults`, including its `Map` ordering: `OR` keeps the
/// accumulator's order and appends new documents, `AND` rebuilds the map in
/// the right operand's order, `AND_NOT` deletes from the accumulator.
/// Equality of JSON values the way JavaScript sees them: a number is its
/// value, however it was written (`4`, `4.0` and `4e0` are the same number,
/// while `serde_json` keeps integers and floats apart).
fn json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            left == right || ((left.is_f64() || right.is_f64()) && left.as_f64() == right.as_f64())
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, left)| right.get(key).is_some_and(|right| json_equal(left, right)))
        }
        _ => left == right,
    }
}

fn combine_results(results: Vec<RawResult>, combine_with: CombineWith) -> RawResult {
    let mut iter = results.into_iter();
    let Some(first) = iter.next() else {
        return RawResult::default();
    };

    iter.fold(first, |mut left, right| match combine_with {
        CombineWith::Or => {
            for (doc_id, value) in right.into_entries() {
                if let Some(existing) = left.get_mut(doc_id) {
                    existing.score += value.score;
                    assign_unique_many(&mut existing.terms, &value.terms);
                    existing.matches.assign(value.matches);
                } else {
                    left.insert(doc_id, value);
                }
            }

            left
        }
        CombineWith::And => {
            let mut combined = RawResult::default();

            for (doc_id, value) in right.into_entries() {
                if let Some(mut existing) = left.remove(doc_id) {
                    existing.score += value.score;
                    assign_unique_many(&mut existing.terms, &value.terms);
                    existing.matches.assign(value.matches);
                    combined.insert(doc_id, existing);
                }
            }

            combined
        }
        CombineWith::AndNot => {
            for doc_id in right.doc_ids().collect::<Vec<_>>() {
                left.remove(doc_id);
            }

            left
        }
    })
}

/// Inverse document frequency for a term, given its document frequency in the
/// field (`matching_count`) and the corpus size (`total_count`). Constant for a
/// whole posting list, so callers hoist it out of the per-posting loop.
#[inline]
fn bm25_idf(matching_count: f64, total_count: f64) -> f64 {
    crate::js_math::log(1.0 + (total_count - matching_count + 0.5) / (matching_count + 0.5))
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

    let mut tokens: Vec<String> = text
        .split(separator)
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect();
    if tokenizer == TokenizerMode::Default {
        // JS splits on a *run* of separators. Interior empties disappear,
        // but the boundary empty token participates in field-length counting.
        if text.is_empty() {
            tokens.push(String::new());
        } else {
            if text.chars().next().is_some_and(separator) {
                tokens.insert(0, String::new());
            }
            if text.chars().next_back().is_some_and(separator) {
                tokens.push(String::new());
            }
        }
    }
    tokens
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
        Value::Number(value) => js_number_string(value),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(stringify_value)
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// MiniSearch's `SPACE_OR_PUNCTUATION` (`/[\n\r\p{Z}\p{P}]+/u`), with the
/// `\p{Z}`/`\p{P}` classification generated from the JavaScript engine's
/// Unicode tables (see `src/separators.rs`).
fn is_space_or_punctuation(character: char) -> bool {
    character == '\n' || character == '\r' || crate::separators::is_separator(character)
}

fn is_jobboard_separator(character: char) -> bool {
    !(character.is_alphanumeric() || matches!(character, '+' | '#' | '.'))
}

fn id_key(id: &Value) -> Result<String, String> {
    match id {
        Value::String(value) => Ok(format!("s:{value}")),
        Value::Number(value) => Ok(format!("n:{}", js_number_string(value))),
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
        Value::Number(value) => js_number_string(value),
        _ => id.to_string(),
    }
}

/// `String(number)` as JavaScript computes it. Integers beyond 2^53 first
/// round to a double, as they would when JS parses the JSON.
fn js_number_string(number: &serde_json::Number) -> String {
    const SAFE: u64 = 1 << 53;
    if let Some(unsigned) = number.as_u64() {
        if unsigned <= SAFE {
            return unsigned.to_string();
        }
        return js_double_string(unsigned as f64);
    }
    if let Some(signed) = number.as_i64() {
        if signed >= -(SAFE as i64) {
            return signed.to_string();
        }
        return js_double_string(signed as f64);
    }
    js_double_string(number.as_f64().unwrap_or(0.0))
}

/// ECMA-262 `Number::toString(x)` for radix 10: shortest round-trip digits,
/// plain notation for decimal exponents in `-6..=21`, exponent notation
/// otherwise (`1e+21`, `1e-7`).
fn js_double_string(x: f64) -> String {
    if x == 0.0 {
        return "0".to_owned();
    }
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let scientific = format!("{:e}", x.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("LowerExp always writes an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n: i32 = exponent.parse::<i32>().expect("LowerExp exponent") + 1;

    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        let e = n - 1;
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.abs().to_string());
    }
    out
}

/// JS `String.length`: UTF-16 code units. Prefix/fuzzy weights and fuzzy
/// distances use this so astral characters (emoji) count as two, like JS.
#[inline]
fn js_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
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
