//! Versioned snapshots. Binary trees retain child order and the leaf slot.
//!
//! The binary reader checks every structural invariant inline while decoding,
//! so a decoded index is operational without a second pass over the data.
//! JSON snapshots, whose fields serde fills without cross-checks, run
//! `validate_snapshot` after deserialization. Debug builds also run the
//! validator after binary loads and before every write, so the test suite
//! catches any drift between the inline checks and the validator.
use super::*;
use crate::searchable_map::RadixNode;

const MAX_INPUT_BYTES: usize = 256 * 1024 * 1024;
const MAX_DECODE_BYTES: usize = 256 * 1024 * 1024;
const MAX_FIELDS: usize = 1024;
const MAX_SHORT_IDS: usize = 2_000_000;
const MAX_FIELD_SLOTS: usize = 16_000_000;
const MAX_TREE_DEPTH: usize = 128;
const MAX_VALUE_DEPTH: usize = 64;
/// Bracket nesting of a JSON snapshot: three levels per radix node (the node,
/// its `children` array and the `[edge, child]` pair), a stored value, and the
/// enclosing objects. Checked on the text before deserializing, because the
/// deserializer's own recursion limit is lifted to reach `MAX_TREE_DEPTH`.
const MAX_JSON_NESTING: usize = 3 * MAX_TREE_DEPTH + MAX_VALUE_DEPTH + 16;

// Tags of the binary JSON-value encoding used for external IDs and stored
// fields. Numbers keep their serde_json variant (unsigned, negative integer,
// float), so `id_key` and JSON equality are unchanged by a round trip.
const TAG_NULL: u8 = 0;
const TAG_FALSE: u8 = 1;
const TAG_TRUE: u8 = 2;
const TAG_UINT: u8 = 3;
const TAG_NEGINT: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_STRING: u8 = 6;
const TAG_ARRAY: u8 = 7;
const TAG_OBJECT: u8 = 8;

// Kept separate from MiniSearch so derive cannot bypass validation. Missing
// version/field-presence data from earlier releases is deliberately rejected.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JsonSnapshot {
    snapshot_version: u64,
    options: MiniSearchOptions,
    index: SearchableMap<Rc<FieldTermData>>,
    document_count: usize,
    next_id: ShortId,
    document_ids: HashMap<ShortId, Value>,
    id_to_short_id: HashMap<String, ShortId>,
    field_ids: BTreeMap<String, FieldId>,
    field_length: Vec<u32>,
    field_present: Vec<bool>,
    average_field_length: Vec<f64>,
    stored_fields: HashMap<ShortId, BTreeMap<String, Value>>,
    dirt_count: usize,
}

impl TryFrom<JsonSnapshot> for MiniSearch {
    type Error = String;

    fn try_from(s: JsonSnapshot) -> Result<Self, String> {
        // Bound `next_id` before sizing the liveness table from it.
        table_slots(s.next_id, s.options.fields.len())?;
        let mut alive = vec![false; s.next_id as usize];
        for &id in s.document_ids.keys() {
            if let Some(slot) = alive.get_mut(id as usize) {
                *slot = true;
            }
        }
        let index = Self {
            snapshot_version: s.snapshot_version,
            options: s.options,
            index: s.index,
            document_count: s.document_count,
            next_id: s.next_id,
            document_ids: s.document_ids,
            id_to_short_id: s.id_to_short_id,
            field_ids: s.field_ids,
            field_length: s.field_length,
            field_present: s.field_present,
            alive,
            average_field_length: s.average_field_length,
            stored_fields: s.stored_fields,
            dirt_count: s.dirt_count,
            vacuum_state: None,
            query_cache: QueryCache::default(),
            stale_hit: StaleFlag::default(),
            id_table_version: 0,
        };
        index.validate_snapshot()?;
        Ok(index)
    }
}

fn invalid(message: &str) -> String {
    format!("invalid minisearch-wasm snapshot: {message}")
}

pub(super) fn table_slots(next_id: ShortId, fields: usize) -> Result<usize, String> {
    let slots = (next_id as usize)
        .checked_mul(fields)
        .ok_or_else(|| invalid("field dimensions overflow"))?;
    if fields > MAX_FIELDS || next_id as usize > MAX_SHORT_IDS || slots > MAX_FIELD_SLOTS {
        return Err(invalid("field dimensions exceed snapshot limits"));
    }
    Ok(slots)
}

/// Deepest bracket nesting of a JSON text, ignoring brackets inside strings.
fn json_nesting(text: &str) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let (mut in_string, mut escaped) = (false, false);
    for byte in text.bytes() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'[' | b'{' => {
                    depth += 1;
                    deepest = deepest.max(depth);
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    deepest
}

fn validate_value(value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_VALUE_DEPTH {
        return Err(invalid("JSON value exceeds depth limit"));
    }
    match value {
        Value::Array(values) => {
            for value in values {
                validate_value(value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_value(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

impl MiniSearch {
    pub fn from_json(serialized: &str) -> Result<Self, String> {
        if serialized.len() > MAX_INPUT_BYTES {
            return Err(invalid("input exceeds snapshot byte limit"));
        }
        if json_nesting(serialized) > MAX_JSON_NESTING {
            return Err(invalid("JSON nesting exceeds depth limit"));
        }
        let mut deserializer = serde_json::Deserializer::from_str(serialized);
        deserializer.disable_recursion_limit();
        let index = Self::deserialize(&mut deserializer).map_err(|err| err.to_string())?;
        deserializer.end().map_err(|err| err.to_string())?;
        Ok(index)
    }

    /// Native JSON snapshot text, refused when [`Self::from_json`] could not
    /// read it back.
    pub fn to_json(&self) -> Result<String, String> {
        self.check_persistable()?;
        serde_json::to_string(self).map_err(|err| err.to_string())
    }

    /// The limits every reader enforces, checked when saving so that an index
    /// that cannot be loaded again fails here, with a way out, and not at the
    /// next startup. Costs one walk over the radix nodes.
    pub fn check_persistable(&self) -> Result<(), String> {
        let fields = self.options.fields.len();
        if fields > MAX_FIELDS
            || self.next_id as usize > MAX_SHORT_IDS
            || (self.next_id as usize).saturating_mul(fields) > MAX_FIELD_SLOTS
        {
            return Err(format!(
                "MiniSearch: the index holds {} internal document slots for {} live documents, more than a snapshot can store; vacuum it and call compact() before saving",
                self.next_id, self.document_count
            ));
        }
        self.options.validate()?;
        let mut pending = vec![(&self.index.root, 0usize)];
        while let Some((node, depth)) = pending.pop() {
            if depth > MAX_TREE_DEPTH {
                return Err(format!(
                    "MiniSearch: the index nests terms more than {MAX_TREE_DEPTH} prefixes deep, more than a snapshot can store"
                ));
            }
            pending.extend(node.children.iter().map(|(_, child)| (child, depth + 1)));
        }
        Ok(())
    }

    /// Full structural check of an in-memory index. The JSON reader runs it
    /// after deserialization; the binary reader enforces the same invariants
    /// inline and only runs this in debug builds as a cross-check.
    pub(super) fn validate_snapshot(&self) -> Result<(), String> {
        if self.snapshot_version != SNAPSHOT_VERSION {
            return Err(invalid(
                "unsupported JSON snapshot version; rebuild from documents",
            ));
        }
        let fields = self.options.fields.len();
        let slots = table_slots(self.next_id, fields)?;
        let expected_fields: BTreeMap<_, _> = self
            .options
            .fields
            .iter()
            .cloned()
            .enumerate()
            .map(|(id, name)| (name, id))
            .collect();
        if expected_fields.len() != fields || self.field_ids != expected_fields {
            return Err(invalid("field IDs do not match configured fields"));
        }
        if self.field_length.len() != slots
            || self.field_present.len() != slots
            || self.average_field_length.len() != fields
        {
            return Err(invalid("inconsistent field dimensions"));
        }
        if self
            .average_field_length
            .iter()
            .any(|average| !average.is_finite())
        {
            // A running average goes negative when documents that lack the
            // field are removed, in JS MiniSearch as well.
            return Err(invalid("field averages must be finite"));
        }
        // JSON serialization rejects nonfinite option values through the round
        // trip below (serde_json otherwise serializes a nonfinite float as null).
        let options_json = serde_json::to_string(&self.options).map_err(|e| e.to_string())?;
        let round_trip: MiniSearchOptions = serde_json::from_str(&options_json)
            .map_err(|_| invalid("invalid or nonfinite options"))?;
        if round_trip != self.options {
            return Err(invalid("nonfinite options"));
        }
        if self.document_count != self.document_ids.len()
            || self.id_to_short_id.len() != self.document_count
            || self
                .document_count
                .checked_add(self.dirt_count)
                .is_none_or(|count| count > self.next_id as usize)
        {
            return Err(invalid("inconsistent document or dirt counts"));
        }
        for (&short_id, id) in &self.document_ids {
            validate_value(id, 0)?;
            if short_id >= self.next_id
                || id.is_null()
                || self.id_to_short_id.get(&id_key(id)?) != Some(&short_id)
            {
                return Err(invalid("inconsistent document ID mappings"));
            }
        }
        for (slot, (&length, &present)) in self
            .field_length
            .iter()
            .zip(&self.field_present)
            .enumerate()
        {
            if (!present && length != 0)
                || (present
                    && !self
                        .document_ids
                        .contains_key(&((slot / fields) as ShortId)))
            {
                return Err(invalid("field data refers to an absent field or document"));
            }
        }
        for (id, values) in &self.stored_fields {
            if !self.document_ids.contains_key(id)
                || values
                    .keys()
                    .any(|key| !self.options.store_fields.contains(key))
            {
                return Err(invalid(
                    "stored fields refer to an unknown document or field",
                ));
            }
            for value in values.values() {
                validate_value(value, 0)?;
            }
        }
        let mut pending = vec![(&self.index.root, 0usize)];
        while let Some((node, depth)) = pending.pop() {
            if depth > MAX_TREE_DEPTH {
                return Err(invalid("radix tree exceeds depth limit"));
            }
            if let Some(data) = &node.leaf {
                if depth == 0 || data.is_empty() || node.leaf_pos as usize > node.children.len() {
                    return Err(invalid("invalid radix leaf"));
                }
                for (field, postings) in data.iter() {
                    if field >= fields || postings.is_empty() {
                        return Err(invalid("invalid posting field"));
                    }
                    let mut previous = None;
                    for (id, freq) in postings.iter() {
                        if id >= self.next_id || freq == 0 || previous.is_some_and(|p| p >= id) {
                            return Err(invalid("invalid or unordered postings"));
                        }
                        // A posting of an absent document is stale, which is
                        // legal: `discard` leaves them, and so does removing
                        // a document whose content changed (outside the
                        // dirt count).
                        if self.document_ids.contains_key(&id) {
                            let slot = id as usize * fields + field;
                            if !self.field_present[slot] || self.field_length[slot] == 0 {
                                return Err(invalid("posting refers to an absent or empty field"));
                            }
                        }
                        previous = Some(id);
                    }
                }
            }
            let mut initials = HashSet::new();
            for (edge, child) in &node.children {
                let first = edge
                    .chars()
                    .next()
                    .ok_or_else(|| invalid("empty radix edge"))?;
                if !initials.insert(first) || (child.leaf.is_none() && child.children.is_empty()) {
                    return Err(invalid("overlapping or empty radix children"));
                }
                pending.push((child, depth + 1));
            }
        }
        Ok(())
    }

    /// Version 4: exact field presence/lengths, ordered radix nodes,
    /// delta-varint postings and tagged binary ID/stored values. Older formats
    /// must be rebuilt from documents.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        // In-memory state was built by this engine or validated when loaded,
        // so release builds check only the limits an engine can outgrow; debug
        // builds keep the full structural pass as a consistency check.
        self.check_persistable()?;
        if cfg!(debug_assertions) {
            self.validate_snapshot()?;
        }
        let mut out = Vec::new();
        write_varint(&mut out, SNAPSHOT_VERSION);
        write_json(&mut out, &self.options)?;
        write_varint(&mut out, self.document_count as u64);
        write_varint(&mut out, self.next_id as u64);
        write_varint(&mut out, self.dirt_count as u64);
        for average in &self.average_field_length {
            out.extend_from_slice(&average.to_le_bytes());
        }
        let mut documents: Vec<_> = self.document_ids.iter().collect();
        documents.sort_by_key(|(id, _)| **id);
        let mut previous = 0;
        let fields = self.options.fields.len();
        for (&id, value) in documents {
            write_varint(&mut out, (id - previous) as u64);
            previous = id;
            write_value(&mut out, value, 0)?;
            for field in 0..fields {
                let slot = id as usize * fields + field;
                write_varint(
                    &mut out,
                    if self.field_present[slot] {
                        self.field_length[slot] as u64 + 1
                    } else {
                        0
                    },
                );
            }
            let stored = self.stored_fields.get(&id);
            write_varint(&mut out, stored.map_or(0, |v| v.len()) as u64);
            if let Some(stored) = stored {
                for (name, value) in stored {
                    write_str(&mut out, name);
                    write_value(&mut out, value, 0)?;
                }
            }
        }
        write_node(&mut out, &self.index.root);
        if out.len() > MAX_INPUT_BYTES {
            return Err(invalid("output exceeds snapshot byte limit"));
        }
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut reader = Reader::new(bytes)?;
        let version = reader.varint()?;
        if version != SNAPSHOT_VERSION {
            return Err(format!(
                "unsupported minisearch-wasm binary snapshot version {version}"
            ));
        }
        let options: MiniSearchOptions = reader.json()?;
        let document_count = reader.count(1)?;
        let next_id = reader.u32()?;
        let dirt_count = reader.usize()?;
        let fields = options.fields.len();
        let slots = table_slots(next_id, fields)?;
        if document_count
            .checked_add(dirt_count)
            .is_none_or(|n| n > next_id as usize)
        {
            return Err(invalid("inconsistent document or dirt counts"));
        }
        let mut index = Self::new(options);
        if index.field_ids.len() != fields {
            return Err(invalid("field IDs do not match configured fields"));
        }
        index.document_count = document_count;
        index.next_id = next_id;
        index.dirt_count = dirt_count;
        for average in &mut index.average_field_length {
            let value = reader.f64()?;
            if !value.is_finite() {
                return Err(invalid("field averages must be finite"));
            }
            *average = value;
        }
        reader.claim(
            slots
                .checked_mul(5)
                .ok_or_else(|| invalid("field dimensions overflow"))?
                + next_id as usize,
        )?;
        index
            .field_length
            .try_reserve_exact(slots)
            .map_err(|_| invalid("field allocation failed"))?;
        index
            .field_present
            .try_reserve_exact(slots)
            .map_err(|_| invalid("field allocation failed"))?;
        index.field_length.resize(slots, 0);
        index.field_present.resize(slots, false);
        index.alive = vec![false; next_id as usize];
        reader.claim(
            document_count
                .checked_mul(256)
                .ok_or_else(|| invalid("document allocation overflow"))?,
        )?;
        index
            .document_ids
            .try_reserve(document_count)
            .map_err(|_| invalid("document allocation failed"))?;
        index
            .id_to_short_id
            .try_reserve(document_count)
            .map_err(|_| invalid("document allocation failed"))?;
        let mut previous: u32 = 0;
        for row in 0..document_count {
            let delta = reader.u32()?;
            let id = previous
                .checked_add(delta)
                .ok_or_else(|| invalid("document ID overflow"))?;
            if id >= next_id || (row > 0 && delta == 0) {
                return Err(invalid("invalid or repeated document ID"));
            }
            previous = id;
            let value = reader.value(0)?;
            if value.is_null() || index.id_to_short_id.insert(id_key(&value)?, id).is_some() {
                return Err(invalid("null or duplicate external ID"));
            }
            index.document_ids.insert(id, value);
            index.alive[id as usize] = true;
            for field in 0..fields {
                let encoded = reader.varint()?;
                let slot = id as usize * fields + field;
                if encoded != 0 {
                    index.field_present[slot] = true;
                    index.field_length[slot] =
                        u32::try_from(encoded - 1).map_err(|_| invalid("field length overflow"))?;
                }
            }
            let count = reader.count(2)?;
            if count > index.options.store_fields.len() {
                return Err(invalid("too many stored fields"));
            }
            let mut stored = BTreeMap::new();
            for _ in 0..count {
                let name = reader.string()?;
                if !index.options.store_fields.contains(&name) {
                    return Err(invalid(
                        "stored fields refer to an unknown document or field",
                    ));
                }
                let value = reader.value(0)?;
                if stored.insert(name, value).is_some() {
                    return Err(invalid("duplicate stored field"));
                }
            }
            if !stored.is_empty() {
                index.stored_fields.insert(id, stored);
            }
        }
        let root = {
            let mut tree = TreeContext {
                fields,
                next_id,
                field_length: &index.field_length,
                field_present: &index.field_present,
                alive: &index.alive,
            };
            reader.node(&mut tree, 0)?
        };
        index.index.root = root;
        if reader.pos != bytes.len() {
            return Err(invalid("trailing bytes"));
        }
        if cfg!(debug_assertions) {
            // Any input the inline checks accept must also satisfy the
            // validator; a panic here means the two have drifted apart.
            index
                .validate_snapshot()
                .expect("binary reader accepted a snapshot the validator rejects");
        }
        Ok(index)
    }
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
fn write_str(out: &mut Vec<u8>, value: &str) {
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}
fn write_json(out: &mut Vec<u8>, value: &impl Serialize) -> Result<(), String> {
    write_str(
        out,
        &serde_json::to_string(value).map_err(|e| e.to_string())?,
    );
    Ok(())
}
fn write_value(out: &mut Vec<u8>, value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_VALUE_DEPTH {
        return Err(invalid("JSON value exceeds depth limit"));
    }
    match value {
        Value::Null => out.push(TAG_NULL),
        Value::Bool(false) => out.push(TAG_FALSE),
        Value::Bool(true) => out.push(TAG_TRUE),
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                out.push(TAG_UINT);
                write_varint(out, unsigned);
            } else if let Some(signed) = number.as_i64() {
                out.push(TAG_NEGINT);
                write_varint(out, signed as u64);
            } else {
                let float = number
                    .as_f64()
                    .filter(|f| f.is_finite())
                    .ok_or_else(|| invalid("nonfinite number"))?;
                out.push(TAG_FLOAT);
                out.extend_from_slice(&float.to_le_bytes());
            }
        }
        Value::String(string) => {
            out.push(TAG_STRING);
            write_str(out, string);
        }
        Value::Array(values) => {
            out.push(TAG_ARRAY);
            write_varint(out, values.len() as u64);
            for value in values {
                write_value(out, value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            out.push(TAG_OBJECT);
            write_varint(out, values.len() as u64);
            for (key, value) in values {
                write_str(out, key);
                write_value(out, value, depth + 1)?;
            }
        }
    }
    Ok(())
}
fn write_node(out: &mut Vec<u8>, node: &RadixNode<Rc<FieldTermData>>) {
    write_varint(out, node.leaf_pos as u64);
    write_varint(
        out,
        node.leaf.as_ref().map_or(0, |fields| fields.len()) as u64,
    );
    if let Some(data) = &node.leaf {
        for (field, postings) in data.iter() {
            write_varint(out, field as u64);
            write_varint(out, postings.len() as u64);
            let mut previous = 0;
            for (id, freq) in postings.iter() {
                write_varint(out, (id - previous) as u64);
                write_varint(out, freq as u64);
                previous = id;
            }
        }
    }
    write_varint(out, node.children.len() as u64);
    for (edge, child) in &node.children {
        write_str(out, edge);
        write_node(out, child);
    }
}

/// Index state the tree reader checks postings against while decoding.
struct TreeContext<'a> {
    fields: usize,
    next_id: u32,
    field_length: &'a [u32],
    field_present: &'a [bool],
    alive: &'a [bool],
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    budget: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(invalid("input exceeds snapshot byte limit"));
        }
        Ok(Self {
            bytes,
            pos: 0,
            budget: MAX_DECODE_BYTES,
        })
    }
    fn claim(&mut self, bytes: usize) -> Result<(), String> {
        self.budget = self
            .budget
            .checked_sub(bytes)
            .ok_or_else(|| invalid("decoded allocation budget exceeded"))?;
        Ok(())
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or_else(|| invalid("length overflow"))?;
        let result = self
            .bytes
            .get(self.pos..end)
            .ok_or("unexpected end of snapshot")?;
        self.pos = end;
        Ok(result)
    }
    /// LEB128 varint, at most ten bytes, canonical (no trailing zero group).
    /// Decodes straight off the remaining slice: one bounds check per varint
    /// instead of one per byte, which is what the posting loop is made of.
    #[inline]
    fn varint(&mut self) -> Result<u64, String> {
        let rest = &self.bytes[self.pos..];
        let mut result = 0u64;
        let mut shift = 0u32;
        for (consumed, &byte) in rest.iter().take(10).enumerate() {
            if shift == 63 && byte > 1 {
                return Err(invalid("varint overflow"));
            }
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                if consumed != 0 && byte == 0 {
                    return Err(invalid("noncanonical varint"));
                }
                self.pos += consumed + 1;
                return Ok(result);
            }
            shift += 7;
        }
        if rest.len() < 10 {
            Err("unexpected end of snapshot".to_owned())
        } else {
            Err(invalid("varint overflow"))
        }
    }
    fn usize(&mut self) -> Result<usize, String> {
        usize::try_from(self.varint()?).map_err(|_| invalid("integer overflow"))
    }
    fn u32(&mut self) -> Result<u32, String> {
        u32::try_from(self.varint()?).map_err(|_| invalid("integer overflow"))
    }
    fn count(&mut self, minimum_bytes: usize) -> Result<usize, String> {
        let count = self.usize()?;
        if count > (self.bytes.len() - self.pos) / minimum_bytes {
            return Err(invalid("count exceeds remaining input"));
        }
        Ok(count)
    }
    fn string(&mut self) -> Result<String, String> {
        let length = self.count(1)?;
        self.claim(length)?;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| invalid("invalid UTF-8"))
    }
    fn json<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, String> {
        let text = self.string()?;
        self.claim(
            text.len()
                .checked_mul(16)
                .ok_or_else(|| invalid("JSON allocation overflow"))?,
        )?;
        serde_json::from_str(&text).map_err(|e| invalid(&format!("invalid JSON value: {e}")))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| invalid("invalid float"))?,
        ))
    }
    /// Tagged JSON value (see the `TAG_*` constants).
    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_VALUE_DEPTH {
            return Err(invalid("JSON value exceeds depth limit"));
        }
        self.claim(32)?;
        let tag = self.take(1)?[0];
        Ok(match tag {
            TAG_NULL => Value::Null,
            TAG_FALSE => Value::Bool(false),
            TAG_TRUE => Value::Bool(true),
            TAG_UINT => Value::from(self.varint()?),
            TAG_NEGINT => {
                let signed = self.varint()? as i64;
                if signed >= 0 {
                    return Err(invalid("noncanonical negative integer"));
                }
                Value::from(signed)
            }
            TAG_FLOAT => Value::Number(
                serde_json::Number::from_f64(self.f64()?)
                    .ok_or_else(|| invalid("nonfinite number"))?,
            ),
            TAG_STRING => Value::String(self.string()?),
            TAG_ARRAY => {
                let count = self.count(1)?;
                // The reservation is the allocation: charge it before making
                // it, or nested counts reserve far more than they ever fill.
                self.claim(
                    count
                        .checked_mul(std::mem::size_of::<Value>())
                        .ok_or_else(|| invalid("value allocation overflow"))?,
                )?;
                let mut values = Vec::new();
                values
                    .try_reserve_exact(count)
                    .map_err(|_| invalid("value allocation failed"))?;
                for _ in 0..count {
                    values.push(self.value(depth + 1)?);
                }
                Value::Array(values)
            }
            TAG_OBJECT => {
                let count = self.count(2)?;
                let mut values = JsonMap::new();
                for _ in 0..count {
                    let key = self.string()?;
                    let value = self.value(depth + 1)?;
                    if values.insert(key, value).is_some() {
                        return Err(invalid("duplicate object key"));
                    }
                }
                Value::Object(values)
            }
            _ => return Err(invalid("invalid value tag")),
        })
    }
    fn node(
        &mut self,
        tree: &mut TreeContext<'_>,
        depth: usize,
    ) -> Result<RadixNode<Rc<FieldTermData>>, String> {
        if depth > MAX_TREE_DEPTH {
            return Err(invalid("radix tree exceeds depth limit"));
        }
        self.claim(128)?;
        let leaf_pos = self.u32()?;
        let field_count = self.count(2)?;
        if field_count > tree.fields || (depth == 0 && field_count != 0) {
            return Err(invalid("invalid radix leaf"));
        }
        let mut data = FieldTermData::default();
        for _ in 0..field_count {
            let field = self.usize()?;
            let count = self.count(2)?;
            if field >= tree.fields || count == 0 || count > tree.next_id as usize {
                return Err(invalid("invalid or duplicate posting field"));
            }
            self.claim(
                count
                    .checked_mul(8)
                    .ok_or_else(|| invalid("posting allocation overflow"))?
                    + 64,
            )?;
            let mut postings = Postings::default();
            postings
                .0
                .try_reserve_exact(count)
                .map_err(|_| invalid("posting allocation failed"))?;
            let mut previous: u32 = 0;
            for row in 0..count {
                let delta = self.u32()?;
                let id = previous
                    .checked_add(delta)
                    .ok_or_else(|| invalid("posting ID overflow"))?;
                let freq = self.u32()?;
                if id >= tree.next_id || freq == 0 || (row > 0 && delta == 0) {
                    return Err(invalid("invalid or unordered posting"));
                }
                // Postings of absent documents are stale, which is legal.
                if tree.alive[id as usize] {
                    let slot = id as usize * tree.fields + field;
                    if !tree.field_present[slot] || tree.field_length[slot] == 0 {
                        return Err(invalid("posting refers to an absent or empty field"));
                    }
                }
                postings.push_sorted(id, freq);
                previous = id;
            }
            if !data.insert(field, postings) {
                return Err(invalid("invalid or duplicate posting field"));
            }
        }
        let count = self.count(4)?;
        if field_count != 0 && leaf_pos as usize > count {
            return Err(invalid("invalid radix leaf position"));
        }
        self.claim(
            count
                .checked_mul(128)
                .ok_or_else(|| invalid("tree allocation overflow"))?,
        )?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(count)
            .map_err(|_| invalid("tree allocation failed"))?;
        let mut initials = HashSet::new();
        for _ in 0..count {
            let edge = self.string()?;
            let first = edge
                .chars()
                .next()
                .ok_or_else(|| invalid("empty radix edge"))?;
            if !initials.insert(first) {
                return Err(invalid("overlapping radix edges"));
            }
            let child = self.node(tree, depth + 1)?;
            if child.leaf.is_none() && child.children.is_empty() {
                return Err(invalid("overlapping or empty radix children"));
            }
            children.push((edge, child));
        }
        Ok(RadixNode {
            leaf: (!data.is_empty()).then_some(Rc::new(data)),
            leaf_pos,
            children,
        })
    }
}
