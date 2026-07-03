use minisearch_wasm::{MiniSearch, MiniSearchOptions, SearchOptions, TokenizerMode};
use serde_json::{json, Value};

fn index(field: &str, documents: Vec<Value>) -> MiniSearch {
    let mut search = MiniSearch::new(MiniSearchOptions {
        fields: vec![field.to_owned()],
        id_field: "id".to_owned(),
        store_fields: vec![],
        tokenizer: TokenizerMode::Default,
        search_options: SearchOptions::default(),
        auto_suggest_options: None,
        auto_vacuum: None,
    });
    search.add_all(documents).unwrap();
    search
}

fn ids(search: &MiniSearch, query: &str) -> Vec<Value> {
    search
        .search(query, SearchOptions::default())
        .into_iter()
        .map(|result| result.id)
        .collect()
}

#[test]
fn default_tokenizer_splits_punctuation_without_dropping_diacritics() {
    let search = index(
        "text",
        vec![
            json!({
                "id": 1,
                "text": "Se la vita è sventura,\nperché da noi si dura?\nIntatta luna, tale\nè lo stato mortale."
            }),
            json!({
                "id": 2,
                "text": "The estimates range from roughly 1 in 100 to 1 in 100,000. A Shuttle could fly for 300 years. What is management's fantastic faith in the machinery?"
            }),
        ],
    );

    assert_eq!(ids(&search, "perché"), vec![json!(1)]);
    assert!(ids(&search, "perch").is_empty());
    assert_eq!(ids(&search, "luna"), vec![json!(1)]);
    assert_eq!(ids(&search, "300"), vec![json!(2)]);
    assert_eq!(ids(&search, "machinery"), vec![json!(2)]);
}

#[test]
fn default_tokenizer_supports_non_latin_alphabets() {
    let search = index(
        "title",
        vec![
            json!({ "id": 1, "title": "София София" }),
            json!({ "id": 2, "title": "アネモネ" }),
            json!({ "id": 3, "title": "«τέχνη»" }),
            json!({ "id": 4, "title": "سمت  الرأس" }),
            json!({ "id": 5, "title": "123 45" }),
        ],
    );

    for (query, id) in [
        ("софия", 1),
        ("アネモネ", 2),
        ("τέχνη", 3),
        ("الرأس", 4),
        ("123", 5),
    ] {
        assert_eq!(ids(&search, query), vec![json!(id)], "query={query}");
    }
}

#[test]
fn default_tokenizer_collapses_contiguous_separators() {
    let search = index("text", vec![json!({ "id": 1, "text": "a  b...c ? d" })]);

    for term in ["a", "b", "c", "d"] {
        assert_eq!(ids(&search, term), vec![json!(1)], "term={term}");
    }
    assert_eq!(search.term_count(), 4);
}
