//! Import and export of JS MiniSearch's own `toJSON()` format (serialization
//! versions 1 and 2), so an index persisted by the JavaScript library loads
//! here with the same options — the counterpart of
//! `MiniSearch.loadJSON(json, options)` — and an index built here can be
//! loaded by the JavaScript library.
//!
//! The two engines keep the same state (short ids, per-field lengths, field
//! averages, stored fields, dirt count and per-term postings), so the mapping is
//! one to one. Terms are inserted in the serialized order, which is also the
//! order JS `loadJS` re-inserts them in, so the resulting radix tree — and with
//! it suggestion and matched-term order — is the same on both sides.
use super::snapshot::table_slots;
use super::*;

const MAX_INTEROP_BYTES: usize = 256 * 1024 * 1024;

fn invalid(message: &str) -> String {
    format!("MiniSearch: cannot deserialize index: {message}")
}

fn short_id(key: &str) -> Result<ShortId, String> {
    key.parse::<ShortId>()
        .map_err(|_| invalid("invalid short document id"))
}

/// `MiniSearch#toJSON()` as of MiniSearch 7 (`AsPlainObject`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MiniSearchJson {
    document_count: usize,
    next_id: ShortId,
    document_ids: HashMap<String, Value>,
    field_ids: BTreeMap<String, FieldId>,
    /// Sparse per-document arrays; `JSON.stringify` writes interior holes as
    /// null and drops trailing ones.
    field_length: HashMap<String, Vec<Option<u32>>>,
    average_field_length: Vec<Option<f64>>,
    #[serde(default)]
    stored_fields: HashMap<String, BTreeMap<String, Value>>,
    #[serde(default)]
    dirt_count: usize,
    /// `[term, { fieldId: { docId: frequency } }]` entries in tree order.
    index: Vec<(String, HashMap<String, Value>)>,
    serialization_version: u64,
}

// All partially rebuilt state is owned by the loader. A failed batch drops it;
// no incomplete index escapes through either the synchronous or async API.
pub(crate) struct MiniSearchJsonLoader {
    index: MiniSearch,
    documents: std::collections::hash_map::IntoIter<String, Value>,
    lengths: std::collections::hash_map::IntoIter<String, Vec<Option<u32>>>,
    stored: std::collections::hash_map::IntoIter<String, BTreeMap<String, Value>>,
    terms: std::vec::IntoIter<(String, HashMap<String, Value>)>,
    version: u64,
    term: Option<(String, FieldTermData)>,
    term_fields: std::collections::hash_map::IntoIter<String, Value>,
    postings: Option<LoadingPostings>,
}

struct LoadingPostings {
    field: FieldId,
    entries: serde_json::map::IntoIter,
    values: Vec<(ShortId, u32)>,
}

impl MiniSearchJsonLoader {
    /// At most `budget` reconstruction steps. Posting entries count separately:
    /// even an index with only one very common term yields within that term.
    pub(crate) fn step(&mut self, budget: usize) -> Result<bool, String> {
        for _ in 0..budget.max(1) {
            if self.advance()? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn advance(&mut self) -> Result<bool, String> {
        let index = &mut self.index;
        let fields = index.options.fields.len();
        if let Some((key, id)) = self.documents.next() {
            let short = short_id(&key)?;
            if short >= index.next_id || id.is_null() {
                return Err(invalid("invalid document id entry"));
            }
            if index.id_to_short_id.insert(id_key(&id)?, short).is_some() {
                return Err(invalid("duplicate document id"));
            }
            index.document_ids.insert(short, id);
            index.alive[short as usize] = true;
            return Ok(false);
        }
        if let Some((key, lengths)) = self.lengths.next() {
            let short = short_id(&key)?;
            if !index.alive.get(short as usize).copied().unwrap_or(false) || lengths.len() > fields
            {
                return Err(invalid(
                    "fieldLength refers to an unknown document or field",
                ));
            }
            for (field, length) in lengths.into_iter().enumerate() {
                if let Some(length) = length {
                    let slot = short as usize * fields + field;
                    index.field_present[slot] = true;
                    index.field_length[slot] = length;
                }
            }
            return Ok(false);
        }
        if let Some((key, values)) = self.stored.next() {
            let short = short_id(&key)?;
            if !index.alive.get(short as usize).copied().unwrap_or(false) {
                return Err(invalid("storedFields refers to an unknown document"));
            }
            if !values.is_empty() {
                index.stored_fields.insert(short, values);
            }
            return Ok(false);
        }
        if let Some(postings) = &mut self.postings {
            if let Some((doc_key, frequency)) = postings.entries.next() {
                let doc = short_id(&doc_key)?;
                let frequency = frequency
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or_else(|| invalid("invalid term frequency"))?;
                postings.values.push((doc, frequency));
                return Ok(false);
            }
            let mut postings = self.postings.take().unwrap();
            postings.values.sort_unstable_by_key(|(doc, _)| *doc);
            if !self
                .term
                .as_mut()
                .unwrap()
                .1
                .insert(postings.field, Postings(postings.values, 0))
            {
                return Err(invalid("duplicate field in index entry"));
            }
            return Ok(false);
        }
        if let Some((field_key, mut freqs)) = self.term_fields.next() {
            let field: FieldId = field_key
                .parse()
                .map_err(|_| invalid("invalid field id in index entry"))?;
            if field >= fields {
                return Err(invalid("index entry refers to an unknown field"));
            }
            // Version 1 nested the frequencies inside a `ds` field.
            if self.version == 1 {
                freqs = freqs
                    .as_object_mut()
                    .and_then(|object| object.remove("ds"))
                    .ok_or_else(|| invalid("version 1 index entry without ds"))?;
            }
            let Value::Object(object) = freqs else {
                return Err(invalid("index entry frequencies must be an object"));
            };
            self.postings = Some(LoadingPostings {
                field,
                values: Vec::with_capacity(object.len()),
                entries: object.into_iter(),
            });
            return Ok(false);
        }
        if let Some((term, data)) = self.term.take() {
            if term.is_empty() || data.is_empty() {
                return Err(invalid("empty index entry"));
            }
            index.index.set(&term, Rc::new(data));
            return Ok(false);
        }
        if let Some((term, data)) = self.terms.next() {
            self.term = Some((term, FieldTermData::default()));
            self.term_fields = data.into_iter();
            return Ok(false);
        }
        Ok(true)
    }

    pub(crate) fn finish(self) -> Result<MiniSearch, String> {
        self.index.validate_snapshot()?;
        Ok(self.index)
    }
}

impl MiniSearch {
    /// Load an index serialized by JS MiniSearch with the options its instance
    /// was created with. Rejects versions other than 1 and 2 with MiniSearch's
    /// own message, options whose `fields` differ from the serialized
    /// `fieldIds`, and any structurally inconsistent state.
    pub fn from_minisearch_json(json: &str, options: MiniSearchOptions) -> Result<Self, String> {
        let mut loader = Self::start_minisearch_json(json, options)?;
        while !loader.step(1000)? {}
        loader.finish()
    }

    /// Parse once, then reconstruct the same validated index in bounded batches.
    /// The Wasm async entry yields between batches; the synchronous loader uses
    /// the identical steps without scheduling them.
    pub(crate) fn start_minisearch_json(
        json: &str,
        options: MiniSearchOptions,
    ) -> Result<MiniSearchJsonLoader, String> {
        if json.len() > MAX_INTEROP_BYTES {
            return Err(invalid("input exceeds byte limit"));
        }
        let js: MiniSearchJson =
            serde_json::from_str(json).map_err(|err| invalid(&err.to_string()))?;
        if js.serialization_version != 1 && js.serialization_version != 2 {
            return Err(
                "MiniSearch: cannot deserialize an index created with an incompatible version"
                    .to_owned(),
            );
        }

        let fields = options.fields.len();
        let slots = table_slots(js.next_id, fields)?;
        let mut index = Self::new(options);
        if index.field_ids != js.field_ids {
            return Err(invalid(
                "the serialized fieldIds do not match the configured fields",
            ));
        }
        index.document_count = js.document_count;
        index.next_id = js.next_id;
        index.dirt_count = js.dirt_count;
        index.field_length = vec![0; slots];
        index.field_present = vec![false; slots];
        index.alive = vec![false; js.next_id as usize];

        if js.average_field_length.len() > fields {
            return Err(invalid("averageFieldLength has more entries than fields"));
        }
        for (field, average) in js.average_field_length.into_iter().enumerate() {
            let average = average.unwrap_or(0.0);
            if !average.is_finite() {
                return Err(invalid("averageFieldLength must be finite"));
            }
            index.average_field_length[field] = average;
        }

        Ok(MiniSearchJsonLoader {
            index,
            documents: js.document_ids.into_iter(),
            lengths: js.field_length.into_iter(),
            stored: js.stored_fields.into_iter(),
            terms: js.index.into_iter(),
            version: js.serialization_version,
            term: None,
            term_fields: HashMap::new().into_iter(),
            postings: None,
        })
    }

    /// Serialize in JS MiniSearch's `toJSON()` shape (serialization version 2).
    /// Load it there with `MiniSearch.loadJSON(json, options)`, passing the
    /// same `fields`/`storeFields`/`idField` this index was created with.
    pub fn to_minisearch_json(&self) -> Result<String, String> {
        let fields = self.options.fields.len();

        let mut document_ids = JsonMap::new();
        let mut field_length = JsonMap::new();
        let mut stored_fields = JsonMap::new();
        let mut short_ids: Vec<ShortId> = self.document_ids.keys().copied().collect();
        short_ids.sort_unstable();
        for short in short_ids {
            let key = short.to_string();
            document_ids.insert(key.clone(), self.document_ids[&short].clone());

            // JS keeps a sparse per-document array with entries only for the
            // fields that were present; documents without any indexed field
            // have no entry at all. Mirror `JSON.stringify` of that array.
            let base = short as usize * fields;
            if let Some(last) = (0..fields)
                .rev()
                .find(|field| self.field_present[base + field])
            {
                let lengths: Vec<Value> = (0..=last)
                    .map(|field| {
                        if self.field_present[base + field] {
                            Value::from(self.field_length[base + field])
                        } else {
                            Value::Null
                        }
                    })
                    .collect();
                field_length.insert(key.clone(), Value::Array(lengths));
            }

            if let Some(values) = self.stored_fields.get(&short) {
                stored_fields.insert(
                    key,
                    Value::Object(
                        values
                            .iter()
                            .map(|(name, value)| (name.clone(), value.clone()))
                            .collect(),
                    ),
                );
            }
        }

        let field_ids: JsonMap<String, Value> = self
            .options
            .fields
            .iter()
            .enumerate()
            .map(|(id, name)| (name.clone(), Value::from(id)))
            .collect();

        let index: Vec<Value> = self
            .index
            .entries()
            .into_iter()
            .map(|(term, data)| {
                let mut per_field = JsonMap::new();
                for (field, postings) in data.iter() {
                    let frequencies: JsonMap<String, Value> = postings
                        .iter()
                        .map(|(doc, frequency)| (doc.to_string(), Value::from(frequency)))
                        .collect();
                    per_field.insert(field.to_string(), Value::Object(frequencies));
                }
                Value::Array(vec![Value::String(term), Value::Object(per_field)])
            })
            .collect();

        let mut root = JsonMap::new();
        root.insert("documentCount".to_owned(), Value::from(self.document_count));
        root.insert("nextId".to_owned(), Value::from(self.next_id));
        root.insert("documentIds".to_owned(), Value::Object(document_ids));
        root.insert("fieldIds".to_owned(), Value::Object(field_ids));
        root.insert("fieldLength".to_owned(), Value::Object(field_length));
        root.insert(
            "averageFieldLength".to_owned(),
            Value::Array(
                self.average_field_length
                    .iter()
                    .map(|average| Value::from(*average))
                    .collect(),
            ),
        );
        root.insert("storedFields".to_owned(), Value::Object(stored_fields));
        root.insert("dirtCount".to_owned(), Value::from(self.dirt_count));
        root.insert("index".to_owned(), Value::Array(index));
        root.insert("serializationVersion".to_owned(), Value::from(2));
        serde_json::to_string(&Value::Object(root)).map_err(|err| err.to_string())
    }
}
