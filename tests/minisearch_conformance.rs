use minisearch_wasm::{
    CombineWith, FuzzySetting, MiniSearch, MiniSearchOptions, SearchOptions, TokenizerMode,
};
use serde_json::json;
use std::collections::BTreeMap;

fn documents() -> Vec<serde_json::Value> {
    vec![
        json!({
            "id": 1,
            "title": "Moby Dick",
            "text": "Call me Ishmael. Some years ago...",
            "category": "fiction"
        }),
        json!({
            "id": 2,
            "title": "Zen and the Art of Motorcycle Maintenance",
            "text": "I can see by my watch...",
            "category": "fiction"
        }),
        json!({
            "id": 3,
            "title": "Neuromancer",
            "text": "The sky above the port was...",
            "category": "fiction"
        }),
        json!({
            "id": 4,
            "title": "Zen and the Art of Archery",
            "text": "At first sight it must seem...",
            "category": "non-fiction"
        }),
    ]
}

fn mini_search() -> MiniSearch {
    let mut search = MiniSearch::new(MiniSearchOptions {
        fields: vec!["title".to_owned(), "text".to_owned()],
        id_field: "id".to_owned(),
        store_fields: vec!["title".to_owned(), "category".to_owned()],
        tokenizer: TokenizerMode::Default,
        search_options: SearchOptions::default(),
    });

    search.add_all(documents()).unwrap();
    search
}

#[test]
fn binary_snapshot_round_trips() {
    let search = mini_search();
    let bytes = search.to_bytes().unwrap();
    let reloaded = MiniSearch::from_bytes(&bytes).unwrap();

    assert_eq!(reloaded.document_count(), search.document_count());
    assert_eq!(reloaded.term_count(), search.term_count());

    for query in ["zen art motorcycle", "ishmael", "neuro", "zaen", "archery"] {
        let mut options = SearchOptions::default();
        options.prefix = true;
        options.fuzzy = Some(FuzzySetting::Distance(0.3));
        assert_eq!(
            reloaded.search(query, options.clone()),
            search.search(query, options),
            "query={query}"
        );
    }
}

#[test]
fn binary_snapshot_round_trips_after_discard() {
    let mut search = mini_search();
    search.discard(&serde_json::json!(2)).unwrap();
    let bytes = search.to_bytes().unwrap();
    let reloaded = MiniSearch::from_bytes(&bytes).unwrap();

    let query = "zen art motorcycle";
    assert_eq!(
        reloaded.search(query, SearchOptions::default()),
        search.search(query, SearchOptions::default()),
    );
}

#[test]
fn adds_documents_and_returns_stored_fields() {
    let search = mini_search();
    let results = search.search("zen art motorcycle", SearchOptions::default());
    let ids = results
        .iter()
        .map(|result| result.id.clone())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec![json!(2), json!(4)]);
    assert_eq!(
        results[0].stored_fields["title"],
        json!("Zen and the Art of Motorcycle Maintenance")
    );
    assert_eq!(results[0].stored_fields["category"], json!("fiction"));
}

#[test]
fn supports_field_boosting_and_field_filtering() {
    let mut search = MiniSearch::new(MiniSearchOptions {
        fields: vec!["title".to_owned(), "text".to_owned()],
        id_field: "id".to_owned(),
        store_fields: vec![],
        tokenizer: TokenizerMode::Default,
        search_options: SearchOptions::default(),
    });
    search
        .add_all(vec![
            json!({ "id": 1, "title": "Divina Commedia", "text": "Nel mezzo del cammin di nostra vita" }),
            json!({ "id": 2, "title": "I Promessi Sposi", "text": "Quel ramo del lago di Como" }),
            json!({ "id": 3, "title": "Vita Nova", "text": "In quella parte del libro della mia memoria" }),
        ])
        .unwrap();

    let mut boost = BTreeMap::new();
    boost.insert("title".to_owned(), 2.0);

    let boosted = search.search(
        "vita",
        SearchOptions {
            boost,
            ..SearchOptions::default()
        },
    );
    assert_eq!(
        boosted
            .iter()
            .map(|result| result.id.clone())
            .collect::<Vec<_>>(),
        vec![json!(3), json!(1)]
    );
    assert!(boosted[0].score > boosted[1].score);

    let title_only = search.search(
        "cammin",
        SearchOptions {
            fields: Some(vec!["title".to_owned()]),
            ..SearchOptions::default()
        },
    );
    assert!(title_only.is_empty());
}

#[test]
fn supports_prefix_and_fuzzy_search() {
    let search = mini_search();

    let prefix = search.search(
        "moto",
        SearchOptions {
            prefix: true,
            ..SearchOptions::default()
        },
    );
    assert_eq!(prefix[0].id, json!(2));

    let fuzzy = search.search(
        "ismael",
        SearchOptions {
            fuzzy: Some(FuzzySetting::Distance(0.2)),
            ..SearchOptions::default()
        },
    );
    assert_eq!(fuzzy[0].id, json!(1));
}

#[test]
fn binary_snapshot_round_trips_results_and_stored_fields() {
    let search = mini_search();
    let bytes = search.to_bytes().unwrap();
    let loaded = MiniSearch::from_bytes(&bytes).unwrap();

    assert_eq!(
        loaded.search("zen art motorcycle", SearchOptions::default()),
        search.search("zen art motorcycle", SearchOptions::default())
    );
    assert_eq!(
        loaded.search("ismael", SearchOptions::default()),
        search.search("ismael", SearchOptions::default())
    );
}

#[test]
fn packed_search_matches_compact_results() {
    let search = mini_search();
    let compact = search.search_compact("zen art motorcycle", SearchOptions::default());
    let packed = search.search_packed("zen art motorcycle", SearchOptions::default());

    assert_eq!(
        packed.ids,
        compact
            .iter()
            .map(|result| result.id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        packed.scores,
        compact
            .iter()
            .map(|result| result.score)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        packed.terms,
        compact
            .iter()
            .map(|result| result.terms.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn combines_results_with_and_and_and_not() {
    let search = mini_search();

    let and_results = search.search(
        "zen archery",
        SearchOptions {
            combine_with: CombineWith::And,
            ..SearchOptions::default()
        },
    );
    assert_eq!(
        and_results
            .iter()
            .map(|result| result.id.clone())
            .collect::<Vec<_>>(),
        vec![json!(4)]
    );

    let and_not_results = search.search(
        "zen archery",
        SearchOptions {
            combine_with: CombineWith::AndNot,
            ..SearchOptions::default()
        },
    );
    assert_eq!(
        and_not_results
            .iter()
            .map(|result| result.id.clone())
            .collect::<Vec<_>>(),
        vec![json!(2)]
    );
}

#[test]
fn remove_discard_and_replace_update_visible_results() {
    let mut search = mini_search();

    search.remove(&documents()[0]).unwrap();
    assert!(search
        .search("ishmael", SearchOptions::default())
        .is_empty());

    search.discard(&json!(2)).unwrap();
    assert!(search
        .search("motorcycle", SearchOptions::default())
        .is_empty());

    search
        .replace(json!({
            "id": 3,
            "title": "Count Zero",
            "text": "Turner woke late",
            "category": "fiction"
        }))
        .unwrap();
    assert!(search
        .search("neuromancer", SearchOptions::default())
        .is_empty());
    assert_eq!(
        search.search("turner", SearchOptions::default())[0].id,
        json!(3)
    );
}

#[test]
fn jobboard_tokenizer_preserves_symbol_terms() {
    let mut search = MiniSearch::new(MiniSearchOptions {
        fields: vec!["title".to_owned(), "description".to_owned()],
        id_field: "id".to_owned(),
        store_fields: vec![],
        tokenizer: TokenizerMode::Jobboard,
        search_options: SearchOptions::default(),
    });
    search
        .add_all(vec![json!({
            "id": "job/1",
            "title": "C++ and .NET Engineer",
            "description": "Build node.js services and maintain C# integrations."
        })])
        .unwrap();

    for query in ["c++", ".net", "node.js", "c#"] {
        assert_eq!(
            search.search(query, SearchOptions::default())[0].id,
            json!("job/1")
        );
    }
}
