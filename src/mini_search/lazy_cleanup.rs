//! Queries on an index that still holds postings of discarded documents,
//! answered exactly like JS MiniSearch answers them.
//!
//! JS `termResults` starts a posting list's document frequency at the list's
//! size *including* stale entries, lowers it each time it meets one, and
//! removes that entry (or lowers its frequency by one) on the spot. A live
//! document listed ahead of a stale one is therefore scored with an inflated
//! frequency on the first query, and the index changes as a side effect:
//! later queries, `termCount` and the radix tree's key order all depend on it.
//!
//! The `&self` query paths cannot mutate, and score with the live count
//! instead, which is what JS converges to. They raise [`StaleFlag`] when they
//! meet a stale posting. The `*_exact` entry points below run that fast path
//! first and, only if the flag was raised, discard its result and redo the
//! query through the mutating replica in this module. A dirty index whose
//! queried terms are already clean keeps the fused, cached fast path.

use super::*;

/// Ranked hits of the mutating path: `(doc id, quality score, raw value)`.
type Ranked = Vec<(ShortId, f64, RawResultValue)>;

impl MiniSearch {
    fn take_stale_hit(&self) -> bool {
        self.stale_hit.0.replace(false)
    }

    /// [`Self::search_query_transfer`] with JS MiniSearch's dirty-index
    /// behavior.
    pub fn search_query_transfer_exact(
        &mut self,
        query: &Query,
        per_call: &PartialSearchOptions,
        include_match: bool,
    ) -> CompatTransfer {
        self.search_query_transfer_ordered_exact(query, per_call, include_match, true)
    }

    pub(crate) fn search_query_transfer_ordered_exact(
        &mut self,
        query: &Query,
        per_call: &PartialSearchOptions,
        include_match: bool,
        sort_results: bool,
    ) -> CompatTransfer {
        self.stale_hit.0.set(false);
        let raw_results = self.execute_query_tree(query, per_call);
        let raw_results = if self.take_stale_hit() {
            self.invalidate_expansions();
            self.execute_query_tree_lazy(query, per_call)
        } else {
            raw_results
        };
        self.transfer_from_raw(raw_results, query, per_call, include_match, sort_results)
    }

    /// [`Self::search_query`] with JS MiniSearch's dirty-index behavior.
    pub fn search_query_exact(
        &mut self,
        query: &Query,
        per_call: &PartialSearchOptions,
    ) -> Vec<SearchResult> {
        self.stale_hit.0.set(false);
        let results = self.search_query(query, per_call);
        if !self.take_stale_hit() {
            return results;
        }
        self.invalidate_expansions();
        let raw_results = self.execute_query_tree_lazy(query, per_call);
        let filter = apply_partial_options(&self.options.search_options, per_call).filter;
        let raw_results = self.filtered_raw_results(raw_results, filter.as_ref());
        self.materialize_raw_results(raw_results, !matches!(query, Query::Wildcard))
    }

    /// [`Self::search_joined_default`] with JS MiniSearch's dirty-index behavior.
    pub fn search_joined_default_exact(
        &mut self,
        query: &str,
        or_mode: bool,
    ) -> JoinedSearchResults {
        let mut options = self.options.search_options.clone();
        if or_mode {
            options.combine_with = CombineWith::Or;
        }
        self.search_joined_with_exact(query, &options)
    }

    /// [`Self::search_joined_opts`] with JS MiniSearch's dirty-index behavior.
    pub fn search_joined_opts_exact(
        &mut self,
        query: &str,
        per_call: &PartialSearchOptions,
    ) -> JoinedSearchResults {
        let options = apply_partial_options(&self.options.search_options, per_call);
        self.search_joined_with_exact(query, &options)
    }

    fn search_joined_with_exact(
        &mut self,
        query: &str,
        options: &SearchOptions,
    ) -> JoinedSearchResults {
        use std::fmt::Write;

        self.stale_hit.0.set(false);
        let joined = self.search_joined_with(query, options);
        if !self.take_stale_hit() {
            return joined;
        }

        let specs = self.query_specs(query, options);
        let ranked = self.ranked_lazy(&specs, options);
        let mut ids = String::from("[");
        let mut terms = String::new();
        let mut scores = Vec::with_capacity(ranked.len());
        for (index, (doc_id, score, raw)) in ranked.iter().enumerate() {
            if index > 0 {
                ids.push(',');
                terms.push('\n');
            }
            match self.document_ids.get(doc_id) {
                Some(other) => {
                    let _ = write!(ids, "{other}");
                }
                None => ids.push_str("null"),
            }
            terms.push_str(&raw.matches.js_keys().join(" "));
            scores.push(*score);
        }
        ids.push(']');
        JoinedSearchResults { ids, scores, terms }
    }

    /// [`Self::search_raw`] with JS MiniSearch's dirty-index behavior.
    pub fn search_raw_exact(
        &mut self,
        query: &str,
        per_call: &PartialSearchOptions,
    ) -> RawSearchResults {
        self.stale_hit.0.set(false);
        let results = self.search_raw(query, per_call);
        if !self.take_stale_hit() {
            return results;
        }

        let options = apply_partial_options(&self.options.search_options, per_call);
        let specs = self.query_specs(query, &options);
        let ranked = self.ranked_lazy(&specs, &options);
        let mut interned: FxHashMap<String, u32> = FxHashMap::default();
        let mut table: Vec<String> = Vec::new();
        let mut doc_ids = Vec::with_capacity(ranked.len());
        let mut scores = Vec::with_capacity(ranked.len());
        let mut term_offsets = Vec::with_capacity(ranked.len() + 1);
        let mut term_ids = Vec::new();
        term_offsets.push(0);
        for (doc_id, score, raw) in &ranked {
            doc_ids.push(*doc_id);
            scores.push(*score);
            for term in raw.matches.js_keys() {
                let next = table.len() as u32;
                let id = *interned.entry(term.clone()).or_insert_with(|| {
                    table.push(term);
                    next
                });
                term_ids.push(id);
            }
            term_offsets.push(term_ids.len() as u32);
        }

        RawSearchResults {
            id_table_version: self.id_table_version,
            doc_ids,
            scores,
            term_table: table.join("\n"),
            term_offsets,
            term_ids,
        }
    }

    /// [`Self::search_count_opts`] with JS MiniSearch's dirty-index behavior.
    pub fn search_count_opts_exact(&mut self, query: &str, prefix: bool, fuzzy: bool) -> usize {
        self.stale_hit.0.set(false);
        let count = self.search_count_opts(query, prefix, fuzzy);
        if !self.take_stale_hit() {
            return count;
        }

        let mut options = self.options.search_options.clone();
        options.prefix = prefix.into();
        options.fuzzy = fuzzy.then_some(FuzzySetting::Distance(0.2));
        let specs = self.query_specs(query, &options);
        self.ranked_lazy(&specs, &options).len()
    }

    /// [`Self::auto_suggest`] with JS MiniSearch's dirty-index behavior.
    pub fn auto_suggest_exact(
        &mut self,
        query: &str,
        per_call: Option<&AutoSuggestOptions>,
    ) -> Vec<Suggestion> {
        self.stale_hit.0.set(false);
        let suggestions = self.auto_suggest(query, per_call);
        if !self.take_stale_hit() {
            return suggestions;
        }

        let (specs, options) = self.auto_suggest_specs(query, per_call);
        let ranked = self.ranked_lazy(&specs, &options);

        // JS `autoSuggest`: group ranked hits by their matched-terms phrase in
        // first-appearance order, average the scores, then sort stably.
        let mut phrase_slots: FxHashMap<String, usize> = FxHashMap::default();
        let mut grouped: Vec<(Vec<String>, f64, u32)> = Vec::new();
        for (_, score, raw) in &ranked {
            let terms = raw.matches.js_keys();
            match phrase_slots.entry(terms.join(" ")) {
                Entry::Occupied(slot) => {
                    let (_, total, count) = &mut grouped[*slot.get()];
                    *total += score;
                    *count += 1;
                }
                Entry::Vacant(slot) => {
                    slot.insert(grouped.len());
                    grouped.push((terms, *score, 1));
                }
            }
        }
        let mut suggestions: Vec<Suggestion> = grouped
            .into_iter()
            .map(|(terms, total, count)| Suggestion {
                suggestion: terms.join(" "),
                terms,
                score: total / count as f64,
            })
            .collect();
        suggestions.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
        });
        suggestions
    }

    fn ranked_lazy(&mut self, specs: &[QuerySpec], options: &SearchOptions) -> Ranked {
        self.invalidate_expansions();
        let results = specs
            .iter()
            .map(|spec| self.execute_query_spec_lazy(spec, options))
            .collect::<Vec<_>>();
        let raw_results = combine_results(results, options.combine_with);
        self.ranked_raw_results(raw_results, true, options.filter.as_ref())
    }

    /// Mutating mirror of [`Self::execute_query_tree`].
    fn execute_query_tree_lazy(
        &mut self,
        query: &Query,
        inherited: &PartialSearchOptions,
    ) -> RawResult {
        match query {
            Query::Wildcard => self.execute_wildcard_query(),
            Query::Text(text) => {
                let options = apply_partial_options(&self.options.search_options, inherited);
                let specs = self.query_specs(text, &options);
                let results = specs
                    .iter()
                    .map(|spec| self.execute_query_spec_lazy(spec, &options))
                    .collect::<Vec<_>>();
                combine_results(results, options.combine_with)
            }
            Query::Combination(combination) => {
                let options = overlay_partial_options(inherited, &combination.options);
                let results = combination
                    .queries
                    .iter()
                    .map(|subquery| self.execute_query_tree_lazy(subquery, &options))
                    .collect::<Vec<_>>();
                combine_results(results, options.combine_with.unwrap_or_default())
            }
        }
    }

    /// Mutating mirror of [`Self::execute_query_spec`]. Like JS, the prefix
    /// and fuzzy expansions are read from the tree after the exact term has
    /// been scored (and possibly removed), and before any of them is scored.
    /// Only terms are kept: a term removed earlier in the same query is looked
    /// up again at its turn and found gone, as JS finds its posting map empty.
    fn execute_query_spec_lazy(&mut self, query: &QuerySpec, options: &SearchOptions) -> RawResult {
        let field_boosts = self.field_boosts(options);
        let mut results = RawResult::default();

        self.term_results_lazy(
            &query.term,
            &query.term,
            1.0,
            query.term_boost,
            &field_boosts,
            options.bm25,
            &mut results,
        );

        let query_len = js_len(&query.term);
        let mut prefix_terms: Vec<(String, usize)> = Vec::new();
        if query.prefix {
            self.index.for_each_prefix(&query.term, |derived, _| {
                let term_len = js_len(derived);
                if term_len != query_len {
                    prefix_terms.push((derived.to_owned(), term_len));
                }
            });
        }

        let mut fuzzy_terms: Vec<(String, usize, usize)> = Vec::new();
        if let Some(fuzzy) = query.fuzzy {
            let max_distance = if fuzzy < 1.0 {
                options
                    .max_fuzzy
                    .min((query_len as f64 * fuzzy).round() as usize)
            } else {
                fuzzy as usize
            };
            if max_distance > 0 {
                self.index
                    .for_each_fuzzy(&query.term, max_distance, |derived, _, distance| {
                        // The exact match is scored above, and a term the
                        // prefix pass surfaces is scored there.
                        let is_prefix_match =
                            query.prefix && derived.starts_with(query.term.as_str());
                        if distance > 0 && !is_prefix_match {
                            fuzzy_terms.push((derived.to_owned(), js_len(derived), distance));
                        }
                    });
            }
        }

        for (term, term_len) in prefix_terms {
            let distance = term_len.saturating_sub(query_len);
            let weight = options.weights.prefix * term_len as f64
                / (term_len as f64 + 0.3 * distance as f64);
            self.term_results_lazy(
                &query.term,
                &term,
                weight,
                query.term_boost,
                &field_boosts,
                options.bm25,
                &mut results,
            );
        }

        for (term, term_len, distance) in fuzzy_terms {
            let term_len = term_len as f64;
            let weight = options.weights.fuzzy * term_len / (term_len + distance as f64);
            self.term_results_lazy(
                &query.term,
                &term,
                weight,
                query.term_boost,
                &field_boosts,
                options.bm25,
                &mut results,
            );
        }

        results
    }

    /// Mutating mirror of [`Self::term_results`], replicating JS
    /// `termResults` + `removeTerm` posting by posting.
    #[allow(clippy::too_many_arguments)]
    fn term_results_lazy(
        &mut self,
        source_term: &str,
        derived_term: &str,
        term_weight: f64,
        term_boost: f64,
        field_boosts: &[FieldBoost],
        bm25_params: Bm25Params,
        results: &mut RawResult,
    ) {
        let num_fields = self.options.fields.len();
        let document_count = self.document_count as f64;
        let Self {
            index,
            alive,
            field_length,
            average_field_length,
            ..
        } = self;

        let should_delete_term = {
            let Some(field_term_data) = index.get_mut(derived_term) else {
                return;
            };
            let field_term_data = Rc::make_mut(field_term_data);

            for field_boost in field_boosts {
                let field_id = field_boost.field_id;
                let Some(postings) = field_term_data.get_mut(field_id) else {
                    continue;
                };

                // Starts at the full size and drops by one per stale posting
                // met, so it is only correct for the live documents that
                // follow every stale one. That is the behavior to reproduce.
                let mut matching_fields = postings.len();
                let mut idf = bm25_idf(matching_fields as f64, document_count);
                let avg_field_length = average_field_length[field_id];
                let mut removed_any = false;

                for posting in postings.0.iter_mut() {
                    let (doc_id, term_freq) = *posting;
                    if term_freq == 0 {
                        continue; // a removed document's tombstone
                    }
                    if !alive[doc_id as usize] {
                        // JS `removeTerm`: drop a posting whose frequency is
                        // at most 1, otherwise lower it by one and keep it
                        // (still stale) for the next query.
                        posting.1 = term_freq.saturating_sub(1);
                        removed_any |= posting.1 == 0;
                        matching_fields -= 1;
                        idf = bm25_idf(matching_fields as f64, document_count);
                        continue;
                    }

                    let length = field_length
                        .get(doc_id as usize * num_fields + field_id)
                        .copied()
                        .unwrap_or(0) as usize;
                    if length == 0 || avg_field_length == 0.0 {
                        continue;
                    }

                    let raw_score = idf
                        * bm25_tf_component(
                            term_freq as f64,
                            length as f64,
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

                if removed_any {
                    postings.sweep();
                }
                if postings.is_empty() {
                    field_term_data.remove(field_id);
                }
            }

            field_term_data.is_empty()
        };

        if should_delete_term {
            index.delete(derived_term);
        }
    }
}
