//! External document identity, for the public facade.
//!
//! MiniSearch keeps document ids and stored fields in JavaScript `Map`s: ids of
//! any type, compared with SameValueZero, and stored values that are live
//! references (`getStoredFields` returns the object search results copy from).
//! The facade keeps them the same way and indexes each document here under its
//! short id: the engine sees an ordinary index whose id field is
//! [`EXTERNAL_ID_FIELD`], holding a number equal to the short id, and which
//! stores no fields. Nothing below changes how an index with its own ids works.

use super::*;

/// The id field of an externally identified index. No document field can be
/// named like this (the facade checks), so it never shadows one.
pub const EXTERNAL_ID_FIELD: &str = "\u{0}";

/// One field of a document the caller tokenized and processed: MiniSearch's
/// field length (its unique tokens) and the terms to index, in order.
type FieldTerms = Option<(u32, Vec<String>)>;

impl MiniSearch {
    /// The short id the next added document receives.
    pub fn next_id(&self) -> ShortId {
        self.next_id
    }

    /// Whether ids and stored fields live outside the engine (see the module
    /// documentation).
    pub fn is_externally_identified(&self) -> bool {
        self.options.id_field == EXTERNAL_ID_FIELD && self.options.store_fields.is_empty()
    }

    /// Index the next document from the text of each field, in `fields`
    /// order (`None`: the field is absent, as for a `null` value), exactly as
    /// [`Self::add`] indexes a document with those string values. Its id is its
    /// short id, which is returned.
    pub fn add_texts(&mut self, texts: &[Option<&str>]) -> Result<ShortId, String> {
        let fields = self.options.fields.len();
        if texts.len() != fields {
            return Err(format!(
                "MiniSearch: expected {fields} field texts, got {}",
                texts.len()
            ));
        }
        let tokenizer = self.options.tokenizer;
        let mut field_tokens: Vec<(FieldId, u32, Vec<&str>)> = Vec::with_capacity(fields);
        for (field_id, text) in texts.iter().enumerate() {
            let Some(text) = text else {
                continue;
            };
            let tokens = tokenize(tokenizer, text);
            let unique_terms = u32::try_from(unique_count(&tokens))
                .map_err(|_| "field has too many unique tokens")?;
            field_tokens.push((field_id, unique_terms, tokens));
        }

        self.invalidate_expansions();
        let id = Value::from(self.next_id);
        let key = id_key(&id)?;
        let short_id = self.add_document_id(id, key)?;
        let count = self.document_count - 1;
        for (field_id, unique_terms, tokens) in field_tokens {
            self.add_field_length(short_id, field_id, count, unique_terms);
            for token in tokens {
                let term = process_term(tokenizer, token);
                if !term.is_empty() {
                    self.add_term(field_id, short_id, &term);
                }
            }
        }
        Ok(short_id)
    }

    /// [`Self::add_texts`] for a batch given as JSON text: one array of field
    /// texts (strings or `null`) per document, the first of which the caller
    /// numbered `first`. Returns how many were added. Nothing is added unless
    /// the whole batch parses and the numbering agrees.
    pub fn add_text_batch(&mut self, json: &str, first: ShortId) -> Result<u32, String> {
        if first != self.next_id {
            return Err(format!(
                "MiniSearch: short id {first} is out of sequence (next is {})",
                self.next_id
            ));
        }
        let documents: Vec<Vec<Option<String>>> =
            serde_json::from_str(json).map_err(|err| format!("MiniSearch: {err}"))?;
        let fields = self.options.fields.len();
        if documents.iter().any(|texts| texts.len() != fields) {
            return Err(format!(
                "MiniSearch: expected {fields} field texts per document"
            ));
        }
        let mut added = 0;
        for texts in &documents {
            let texts: Vec<Option<&str>> = texts.iter().map(Option::as_deref).collect();
            self.add_texts(&texts)?;
            added += 1;
        }
        Ok(added)
    }

    /// [`Self::add_text_batch`] for documents tokenized and processed by the
    /// caller (with JavaScript callbacks): per field, `null` or `[uniqueTokens,
    /// [term, …]]`, MiniSearch's field length and the terms to index in order.
    pub fn add_term_batch(&mut self, json: &str, first: ShortId) -> Result<u32, String> {
        if first != self.next_id {
            return Err(format!(
                "MiniSearch: short id {first} is out of sequence (next is {})",
                self.next_id
            ));
        }
        let documents: Vec<Vec<FieldTerms>> =
            serde_json::from_str(json).map_err(|err| format!("MiniSearch: {err}"))?;
        let fields = self.options.fields.len();
        if documents.iter().any(|terms| terms.len() != fields) {
            return Err(format!(
                "MiniSearch: expected {fields} field terms per document"
            ));
        }
        let mut added = 0;
        for document in documents {
            self.invalidate_expansions();
            let id = Value::from(self.next_id);
            let key = id_key(&id)?;
            let short_id = self.add_document_id(id, key)?;
            let count = self.document_count - 1;
            for (field_id, entry) in document.into_iter().enumerate() {
                let Some((unique_terms, terms)) = entry else {
                    continue;
                };
                self.add_field_length(short_id, field_id, count, unique_terms);
                for term in terms.iter().filter(|term| !term.is_empty()) {
                    self.add_term(field_id, short_id, term);
                }
            }
            added += 1;
        }
        Ok(added)
    }

    /// [`Self::remove_texts`] for a document tokenized and processed by the
    /// caller: per field, `null` or `[uniqueTokens, [term, …]]`.
    pub fn remove_terms(
        &mut self,
        short_id: ShortId,
        json: &str,
        finish: bool,
    ) -> Result<Vec<(String, String)>, String> {
        let fields: Vec<FieldTerms> =
            serde_json::from_str(json).map_err(|err| format!("MiniSearch: {err}"))?;
        if fields.len() != self.options.fields.len() {
            return Err(format!(
                "MiniSearch: expected {} field terms",
                self.options.fields.len()
            ));
        }
        self.remove_fields(
            short_id,
            finish,
            fields
                .into_iter()
                .map(|entry| entry.map(|(unique, terms)| (unique as usize, terms))),
        )
    }

    /// Remove a live document given the text of each field, as
    /// [`Self::remove_with_warnings`] removes a document with those values.
    /// Returns `(term, field)` for every term that was not indexed: MiniSearch
    /// logs these as `version_conflict` warnings, which the caller words with
    /// the document's real id. Without `finish`, only the given fields are
    /// taken out and the document stays: what MiniSearch's `remove` has done
    /// when a callback throws for a later field.
    pub fn remove_texts(
        &mut self,
        short_id: ShortId,
        texts: &[Option<&str>],
        finish: bool,
    ) -> Result<Vec<(String, String)>, String> {
        let fields = self.options.fields.len();
        if texts.len() != fields {
            return Err(format!(
                "MiniSearch: expected {fields} field texts, got {}",
                texts.len()
            ));
        }
        let tokenizer = self.options.tokenizer;
        let fields = texts.iter().map(|text| {
            text.map(|text| {
                let tokens = tokenize(tokenizer, text);
                let unique_terms = unique_count(&tokens);
                let terms = tokens
                    .iter()
                    .map(|token| process_term(tokenizer, token).into_owned())
                    .collect();
                (unique_terms, terms)
            })
        });
        self.remove_fields(short_id, finish, fields)
    }

    /// Take a document's fields out, each given as its field length and terms,
    /// then (with `finish`) the document itself.
    fn remove_fields(
        &mut self,
        short_id: ShortId,
        finish: bool,
        fields: impl Iterator<Item = Option<(usize, Vec<String>)>>,
    ) -> Result<Vec<(String, String)>, String> {
        if !self.alive.get(short_id as usize).copied().unwrap_or(false) {
            return Err(format!(
                "MiniSearch: cannot remove document with short id {short_id}: it is not in the index"
            ));
        }
        self.invalidate_expansions();
        let mut conflicts = Vec::new();
        for (field_id, entry) in fields.enumerate() {
            let Some((unique_terms, terms)) = entry else {
                continue;
            };
            self.remove_field_length(short_id, field_id, self.document_count, unique_terms);
            for term in terms {
                if !term.is_empty() && !self.remove_term(field_id, short_id, &term) {
                    conflicts.push((term, self.options.fields[field_id].clone()));
                }
            }
        }
        if !finish {
            return Ok(conflicts);
        }

        if let Some(id) = self.document_ids.remove(&short_id) {
            self.id_to_short_id.remove(&id_key(&id)?);
        }
        self.stored_fields.remove(&short_id);
        self.clear_field_length_row(short_id);
        self.alive[short_id as usize] = false;
        self.document_count -= 1;
        self.id_table_version += 1;
        Ok(conflicts)
    }

    /// Hand the ids and stored fields over and switch to external identity:
    /// returns both as JSON objects keyed by short id, like MiniSearch's
    /// `documentIds` and `storedFields`. For an index that was loaded (from a
    /// snapshot or MiniSearch's JSON) with its own ids.
    pub fn externalize(&mut self) -> Result<(String, String), String> {
        let ids: BTreeMap<ShortId, &Value> = self
            .document_ids
            .iter()
            .map(|(short_id, id)| (*short_id, id))
            .collect();
        let ids = serde_json::to_string(&ids).map_err(|err| err.to_string())?;
        let stored: BTreeMap<ShortId, &BTreeMap<String, Value>> = self
            .stored_fields
            .iter()
            .map(|(short_id, fields)| (*short_id, fields))
            .collect();
        let stored = serde_json::to_string(&stored).map_err(|err| err.to_string())?;
        self.rekey_short_ids()?;
        self.stored_fields.clear();
        self.options.id_field = EXTERNAL_ID_FIELD.to_owned();
        self.options.store_fields.clear();
        Ok((ids, stored))
    }

    /// [`Self::compact`] for an externally identified index: renumbers the
    /// short ids densely and returns, for each new short id, the old one, so
    /// the caller can renumber its own maps.
    pub fn compact_external(&mut self) -> Result<Vec<ShortId>, String> {
        let mut old: Vec<ShortId> = self.document_ids.keys().copied().collect();
        old.sort_unstable();
        self.compact()?;
        self.rekey_short_ids()?;
        Ok(old)
    }

    /// The short ids of the live documents, ascending.
    pub fn live_short_ids(&self) -> Vec<ShortId> {
        let mut short_ids: Vec<ShortId> = self.document_ids.keys().copied().collect();
        short_ids.sort_unstable();
        short_ids
    }

    /// Make every live document's id its short id.
    fn rekey_short_ids(&mut self) -> Result<(), String> {
        let short_ids: Vec<ShortId> = self.document_ids.keys().copied().collect();
        self.id_to_short_id.clear();
        for short_id in short_ids {
            let id = Value::from(short_id);
            self.id_to_short_id.insert(id_key(&id)?, short_id);
            self.document_ids.insert(short_id, id);
        }
        self.id_table_version += 1;
        Ok(())
    }
}
