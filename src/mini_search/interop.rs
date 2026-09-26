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
//!
//! An externally identified index (see [`EXTERNAL_ID_FIELD`]) keeps only short
//! ids: the loader then leaves `documentIds` and `storedFields` unparsed and
//! hands their JSON text over, for the facade to `JSON.parse` exactly as
//! MiniSearch's `loadJSON` does.
use super::snapshot::table_slots;
use super::*;
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;

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
    document_ids: Box<RawValue>,
    field_ids: BTreeMap<String, FieldId>,
    /// Sparse per-document arrays; `JSON.stringify` writes interior holes as
    /// null and drops trailing ones.
    field_length: HashMap<String, Vec<Option<u32>>>,
    average_field_length: Box<RawValue>,
    #[serde(default)]
    stored_fields: Option<Box<RawValue>>,
    #[serde(default)]
    dirt_count: usize,
    /// `[term, { fieldId: { docId: frequency } }]` entries in tree order.
    index: Vec<(String, TermEntry)>,
    serialization_version: u64,
}

/// A term's `{ fieldId: postings }`, in the serialized order and read straight
/// into posting lists: no `Value` tree for the bulk of the input. Keys and
/// frequencies stay as read (see [`FieldEntry`]); the loader checks them and
/// words the errors.
struct TermEntry(Vec<(FieldId, FieldEntry)>);

/// One field's postings. Version 2 writes `{ docId: frequency }`, version 1
/// `{ ds: { docId: frequency } }`. A document key that is not a short id reads
/// as `ShortId::MAX` and a frequency that is not a positive `u32` as 0; neither
/// can pass the loader's checks.
enum FieldEntry {
    Postings(Vec<(ShortId, u32)>),
    /// The value of a `ds` key; `None` when it is not an object.
    Nested(Option<Vec<(ShortId, u32)>>),
    Invalid,
}

/// A JSON number read like `Value::as_u64` reads it, then narrowed to `u32`:
/// 0 for anything else (a float, a negative number, another type).
struct Frequency(u32);

impl<'de> Deserialize<'de> for Frequency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Read;
        impl<'de> Visitor<'de> for Read {
            type Value = Frequency;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a term frequency")
            }
            fn visit_u64<E>(self, value: u64) -> Result<Frequency, E> {
                Ok(Frequency(u32::try_from(value).unwrap_or(0)))
            }
            fn visit_i64<E>(self, value: i64) -> Result<Frequency, E> {
                Ok(Frequency(u32::try_from(value).unwrap_or(0)))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Frequency, E> {
                Ok(Frequency(0))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Frequency, E> {
                Ok(Frequency(0))
            }
            fn visit_str<E>(self, _: &str) -> Result<Frequency, E> {
                Ok(Frequency(0))
            }
            fn visit_unit<E>(self) -> Result<Frequency, E> {
                Ok(Frequency(0))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Frequency, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Frequency(0))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Frequency, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Frequency(0))
            }
        }
        deserializer.deserialize_any(Read)
    }
}

/// `{ docId: frequency }`, or `None` for any other JSON value.
struct Frequencies(Option<Vec<(ShortId, u32)>>);

impl<'de> Deserialize<'de> for Frequencies {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(EntryVisitor)
            .map(|entry| match entry {
                FieldEntry::Postings(postings) => Frequencies(Some(postings)),
                _ => Frequencies(None),
            })
    }
}

struct EntryVisitor;

impl<'de> Visitor<'de> for EntryVisitor {
    type Value = FieldEntry;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("term frequencies")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<FieldEntry, A::Error> {
        let mut postings = Vec::with_capacity(map.size_hint().unwrap_or(0));
        let mut nested = None;
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            if key == "ds" {
                nested = Some(map.next_value::<Frequencies>()?.0);
            } else {
                let doc = key.parse::<ShortId>().unwrap_or(ShortId::MAX);
                postings.push((doc, map.next_value::<Frequency>()?.0));
            }
        }
        Ok(match nested {
            Some(nested) => FieldEntry::Nested(nested),
            None => FieldEntry::Postings(postings),
        })
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<FieldEntry, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(FieldEntry::Invalid)
    }
    fn visit_u64<E>(self, _: u64) -> Result<FieldEntry, E> {
        Ok(FieldEntry::Invalid)
    }
    fn visit_i64<E>(self, _: i64) -> Result<FieldEntry, E> {
        Ok(FieldEntry::Invalid)
    }
    fn visit_f64<E>(self, _: f64) -> Result<FieldEntry, E> {
        Ok(FieldEntry::Invalid)
    }
    fn visit_bool<E>(self, _: bool) -> Result<FieldEntry, E> {
        Ok(FieldEntry::Invalid)
    }
    fn visit_str<E>(self, _: &str) -> Result<FieldEntry, E> {
        Ok(FieldEntry::Invalid)
    }
    fn visit_unit<E>(self) -> Result<FieldEntry, E> {
        Ok(FieldEntry::Invalid)
    }
}

impl<'de> Deserialize<'de> for TermEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = TermEntry;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an index entry object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TermEntry, A::Error> {
                let mut fields = Vec::with_capacity(map.size_hint().unwrap_or(1));
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    // An unreadable field id reads as FieldId::MAX, an unknown field.
                    let field = key.parse::<FieldId>().unwrap_or(FieldId::MAX);
                    fields.push((field, map.next_value_seed(EntrySeed)?));
                }
                Ok(TermEntry(fields))
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

struct EntrySeed;

impl<'de> serde::de::DeserializeSeed<'de> for EntrySeed {
    type Value = FieldEntry;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<FieldEntry, D::Error> {
        deserializer.deserialize_any(EntryVisitor)
    }
}

/// The JSON text of MiniSearch JSON's `documentIds`, `storedFields` (`"{}"`
/// when absent) and `averageFieldLength`: the facade reads them with
/// `JSON.parse`, as MiniSearch does, and keeps the ids, the stored fields and
/// which averages are `null`.
pub(crate) struct JsonIdentity {
    pub ids: String,
    pub stored: String,
    pub averages: String,
}

// All partially rebuilt state is owned by the loader. A failed batch drops it;
// no incomplete index escapes through either the synchronous or async API.
pub(crate) struct MiniSearchJsonLoader {
    index: MiniSearch,
    /// Short-id keys with their external id, or `None` for an externally
    /// identified index, whose ids are the short ids themselves.
    documents: std::vec::IntoIter<(String, Option<Value>)>,
    /// `documentIds` and `storedFields` as JSON text, for an externally
    /// identified index.
    identity: Option<JsonIdentity>,
    lengths: std::collections::hash_map::IntoIter<String, Vec<Option<u32>>>,
    stored: std::collections::hash_map::IntoIter<String, BTreeMap<String, Value>>,
    terms: std::vec::IntoIter<(String, TermEntry)>,
    version: u64,
    term: Option<(String, FieldTermData)>,
    term_fields: std::vec::IntoIter<(FieldId, FieldEntry)>,
    postings: Option<LoadingPostings>,
    documents_read: usize,
    lengths_read: usize,
    stored_read: usize,
    terms_read: usize,
}

/// Where MiniSearch's `loadJSAsync` waits for a timer: after every 1000th entry
/// of each map it reads (`documentIds`, `fieldLength`, `storedFields`, each
/// posting list) and after every 1000th term.
const YIELD_EVERY: usize = 1000;

enum Step {
    Continue,
    Yield,
    Done,
}

fn pause(count: usize) -> Step {
    if count.is_multiple_of(YIELD_EVERY) {
        Step::Yield
    } else {
        Step::Continue
    }
}

struct LoadingPostings {
    field: FieldId,
    entries: std::vec::IntoIter<(ShortId, u32)>,
    values: Vec<(ShortId, u32)>,
}

impl MiniSearchJsonLoader {
    /// Reconstruct up to the next point where MiniSearch's async loader yields
    /// (see [`YIELD_EVERY`]); `true` once the index is complete. Even an index
    /// with only one very common term yields within that term.
    pub(crate) fn step(&mut self) -> Result<bool, String> {
        loop {
            match self.advance()? {
                Step::Continue => {}
                Step::Yield => return Ok(false),
                Step::Done => return Ok(true),
            }
        }
    }

    fn advance(&mut self) -> Result<Step, String> {
        let index = &mut self.index;
        let fields = index.options.fields.len();
        if let Some((key, id)) = self.documents.next() {
            let short = short_id(&key)?;
            let id = id.unwrap_or_else(|| Value::from(short));
            if short >= index.next_id || id.is_null() {
                return Err(invalid("invalid document id entry"));
            }
            if index.id_to_short_id.insert(id_key(&id)?, short).is_some() {
                return Err(invalid("duplicate document id"));
            }
            index.document_ids.insert(short, id);
            index.alive[short as usize] = true;
            self.documents_read += 1;
            return Ok(pause(self.documents_read));
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
            self.lengths_read += 1;
            return Ok(pause(self.lengths_read));
        }
        if let Some((key, values)) = self.stored.next() {
            let short = short_id(&key)?;
            if !index.alive.get(short as usize).copied().unwrap_or(false) {
                return Err(invalid("storedFields refers to an unknown document"));
            }
            if !values.is_empty() {
                index.stored_fields.insert(short, values);
            }
            self.stored_read += 1;
            return Ok(pause(self.stored_read));
        }
        if let Some(postings) = &mut self.postings {
            if let Some((doc, frequency)) = postings.entries.next() {
                if doc == ShortId::MAX {
                    return Err(invalid("invalid short document id"));
                }
                if frequency == 0 {
                    return Err(invalid("invalid term frequency"));
                }
                postings.values.push((doc, frequency));
                return Ok(pause(postings.values.len()));
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
            return Ok(Step::Continue);
        }
        if let Some((field, entry)) = self.term_fields.next() {
            if field >= fields {
                return Err(invalid("index entry refers to an unknown field"));
            }
            // Version 1 nested the frequencies inside a `ds` field.
            let entries = match (self.version, entry) {
                (1, FieldEntry::Nested(Some(entries))) | (2, FieldEntry::Postings(entries)) => {
                    entries
                }
                (1, FieldEntry::Postings(_)) => {
                    return Err(invalid("version 1 index entry without ds"))
                }
                // A `ds` key in version 2 is not a document id.
                (2, FieldEntry::Nested(_)) => return Err(invalid("invalid short document id")),
                _ => return Err(invalid("index entry frequencies must be an object")),
            };
            self.postings = Some(LoadingPostings {
                field,
                values: Vec::with_capacity(entries.len()),
                entries: entries.into_iter(),
            });
            return Ok(Step::Continue);
        }
        if let Some((term, data)) = self.term.take() {
            if term.is_empty() || data.is_empty() {
                return Err(invalid("empty index entry"));
            }
            index.index.set(&term, Rc::new(data));
            self.terms_read += 1;
            return Ok(pause(self.terms_read));
        }
        if let Some((term, TermEntry(data))) = self.terms.next() {
            self.term = Some((term, FieldTermData::default()));
            self.term_fields = data.into_iter();
            return Ok(Step::Continue);
        }
        Ok(Step::Done)
    }

    pub(crate) fn finish(self) -> Result<MiniSearch, String> {
        self.index.validate_snapshot()?;
        Ok(self.index)
    }

    /// What MiniSearch keeps as it is read, for an externally identified
    /// index; `None` otherwise.
    pub(crate) fn take_identity(&mut self) -> Option<JsonIdentity> {
        self.identity.take()
    }
}

impl MiniSearch {
    /// Load an index serialized by JS MiniSearch with the options its instance
    /// was created with. Rejects versions other than 1 and 2 with MiniSearch's
    /// own message, options whose `fields` differ from the serialized
    /// `fieldIds`, and any structurally inconsistent state.
    pub fn from_minisearch_json(json: &str, options: MiniSearchOptions) -> Result<Self, String> {
        let mut loader = Self::start_minisearch_json(json, options)?;
        while !loader.step()? {}
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

        let external = index.is_externally_identified();
        let (documents, stored, identity) = if external {
            let keys: HashMap<String, IgnoredAny> = serde_json::from_str(js.document_ids.get())
                .map_err(|err| invalid(&err.to_string()))?;
            let documents: Vec<(String, Option<Value>)> =
                keys.into_keys().map(|key| (key, None)).collect();
            let stored = js
                .stored_fields
                .as_deref()
                .map_or_else(|| "{}".to_owned(), |raw| raw.get().to_owned());
            if !stored.trim_start().starts_with('{') {
                return Err(invalid("storedFields must be an object"));
            }
            let identity = JsonIdentity {
                ids: String::from(Box::<str>::from(js.document_ids)),
                stored,
                averages: js.average_field_length.get().to_owned(),
            };
            (documents, HashMap::new(), Some(identity))
        } else {
            let ids: HashMap<String, Value> = serde_json::from_str(js.document_ids.get())
                .map_err(|err| invalid(&err.to_string()))?;
            let stored: HashMap<String, BTreeMap<String, Value>> = match &js.stored_fields {
                Some(raw) => {
                    serde_json::from_str(raw.get()).map_err(|err| invalid(&err.to_string()))?
                }
                None => HashMap::new(),
            };
            let documents = ids.into_iter().map(|(key, id)| (key, Some(id))).collect();
            (documents, stored, None)
        };

        let averages: Vec<Option<f64>> = serde_json::from_str(js.average_field_length.get())
            .map_err(|err| invalid(&err.to_string()))?;
        if averages.len() > fields {
            return Err(invalid("averageFieldLength has more entries than fields"));
        }
        for (field, average) in averages.into_iter().enumerate() {
            let average = average.unwrap_or(0.0);
            if !average.is_finite() {
                return Err(invalid("averageFieldLength must be finite"));
            }
            index.average_field_length[field] = average;
        }

        Ok(MiniSearchJsonLoader {
            index,
            documents: documents.into_iter(),
            identity,
            lengths: js.field_length.into_iter(),
            stored: stored.into_iter(),
            terms: js.index.into_iter(),
            version: js.serialization_version,
            term: None,
            term_fields: Vec::new().into_iter(),
            postings: None,
            documents_read: 0,
            lengths_read: 0,
            stored_read: 0,
            terms_read: 0,
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
