// Declarative forms of MiniSearch's callback options, the version-conflict
// warnings JS sends to its logger, and the tokenizer's Unicode tables.
use minisearch_wasm::{
    AutoSuggestOptions, FuzzySetting, MiniSearch, MiniSearchOptions, PartialSearchOptions,
    PrefixSetting, SearchOptions,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn engine(fields: &[&str], store: &[&str]) -> MiniSearch {
    MiniSearch::new(
        serde_json::from_value::<MiniSearchOptions>(json!({
            "fields": fields, "storeFields": store, "autoVacuum": false
        }))
        .unwrap(),
    )
}

fn ids(results: &[minisearch_wasm::SearchResult]) -> Vec<Value> {
    results.iter().map(|result| result.id.clone()).collect()
}

#[test]
fn per_term_prefix_fuzzy_and_boost_follow_javascript_callback_semantics() {
    let mut search = engine(&["text"], &[]);
    search
        .add_all(vec![
            json!({"id": 1, "text": "apple pie"}),
            json!({"id": 2, "text": "apply now"}),
            json!({"id": 3, "text": "pear pie"}),
        ])
        .unwrap();

    // `prefix: (term, i) => i === 1`: "app" stays exact (no hits), "pi" expands.
    let results = search.search(
        "app pi",
        SearchOptions {
            prefix: PrefixSetting::PerTerm(vec![false, true]),
            ..SearchOptions::default()
        },
    );
    assert_eq!(ids(&results), vec![json!(1), json!(3)]);
    let all = search.search(
        "app pi",
        SearchOptions {
            prefix: true.into(),
            ..SearchOptions::default()
        },
    );
    assert_eq!(all.len(), 3);

    // `fuzzy: (term, i) => i === 0 ? 0.4 : false`: only the first term is fuzzy.
    let results = search.search(
        "appel pie",
        SearchOptions {
            fuzzy: Some(FuzzySetting::PerTerm(vec![
                FuzzySetting::Distance(0.4),
                FuzzySetting::Enabled(false),
            ])),
            ..SearchOptions::default()
        },
    );
    assert_eq!(ids(&results)[0], json!(1));
    assert_eq!(results.len(), 3);
    let missing_entries = search.search(
        "appel pei",
        SearchOptions {
            fuzzy: Some(FuzzySetting::PerTerm(vec![FuzzySetting::Distance(0.4)])),
            ..SearchOptions::default()
        },
    );
    assert_eq!(
        missing_entries.len(),
        2,
        "a missing entry means no fuzzy matching for that term"
    );

    // `boostTerm: (term, i) => i === 0 ? 2 : 1` scales the term's contribution.
    let plain = search.search("pie", SearchOptions::default());
    let boosted = search.search(
        "pie",
        SearchOptions {
            boost_term: Some(vec![2.0]),
            ..SearchOptions::default()
        },
    );
    assert_eq!(boosted[0].score, plain[0].score * 2.0);
    assert_eq!(ids(&boosted), ids(&plain));
}

#[test]
fn declarative_filter_keeps_only_matching_stored_fields() {
    let mut search = engine(&["text"], &["category"]);
    search
        .add_all(vec![
            json!({"id": 1, "text": "apple", "category": "books"}),
            json!({"id": 2, "text": "apple", "category": "toys"}),
            json!({"id": 3, "text": "apple"}),
        ])
        .unwrap();
    let books: BTreeMap<String, Value> = [("category".to_owned(), json!("books"))].into();
    let by_id: BTreeMap<String, Value> = [("id".to_owned(), json!(2))].into();
    let none: BTreeMap<String, Value> = [("category".to_owned(), json!("food"))].into();

    let filtered = search.search(
        "apple",
        SearchOptions {
            filter: Some(books.clone()),
            ..SearchOptions::default()
        },
    );
    assert_eq!(ids(&filtered), vec![json!(1)]);
    let by_id_results = search.search(
        "apple",
        SearchOptions {
            filter: Some(by_id),
            ..SearchOptions::default()
        },
    );
    assert_eq!(ids(&by_id_results), vec![json!(2)]);
    assert!(search
        .search(
            "apple",
            SearchOptions {
                filter: Some(none),
                ..SearchOptions::default()
            }
        )
        .is_empty());

    // The fused paths and suggestions honor the filter too.
    let joined = search.search_joined_opts(
        "apple",
        &PartialSearchOptions {
            filter: Some(books.clone()),
            ..PartialSearchOptions::default()
        },
    );
    assert_eq!(joined.ids, "[1]");
    let suggestions = search.auto_suggest(
        "app",
        Some(&AutoSuggestOptions {
            filter: Some(books),
            ..AutoSuggestOptions::default()
        }),
    );
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].suggestion, "apple");
}

#[test]
fn remove_reports_version_conflicts_like_the_javascript_logger() {
    let mut search = engine(&["title"], &[]);
    search.add(json!({"id": 1, "title": "a b"})).unwrap();
    let mut warnings = Vec::new();
    search
        .remove_with_warnings(&json!({"id": 1, "title": "a c"}), &mut warnings)
        .unwrap();
    assert_eq!(
        warnings,
        vec![
            "MiniSearch: document with ID 1 has changed before removal: term \"c\" was not present in field \"title\". Removing a document after it has changed can corrupt the index!"
        ]
    );
    assert_eq!(search.document_count(), 0);
    assert!(search.search("b", SearchOptions::default()).is_empty());
}

#[test]
fn tokenizer_separators_come_from_the_javascript_engines_unicode_tables() {
    assert_eq!(minisearch_wasm::UNICODE_VERSION, "17.0");
    let mut search = engine(&["text"], &[]);
    // U+1B7F and U+10D6E are punctuation since Unicode 16/17, U+2E5D since 14;
    // the soft hyphen (a format character) is not a separator, as in JS.
    search
        .add(json!({"id": 1, "text": "a\u{1B7F}b e\u{10D6E}f c\u{2E5D}d g\u{00AD}h"}))
        .unwrap();
    for term in ["a", "b", "e", "f", "c", "d", "g\u{00AD}h"] {
        assert_eq!(
            search.search(term, SearchOptions::default()).len(),
            1,
            "{term:?}"
        );
    }
    assert!(search.search("g", SearchOptions::default()).is_empty());
}
