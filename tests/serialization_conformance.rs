use minisearch_wasm::{
    AutoVacuumOptions, AutoVacuumSetting, MiniSearch, MiniSearchOptions, SearchOptions,
    TokenizerMode,
};
use serde_json::json;

fn options() -> MiniSearchOptions {
    MiniSearchOptions {
        fields: vec!["title".to_owned(), "text".to_owned()],
        id_field: "id".to_owned(),
        store_fields: vec!["category".to_owned()],
        tokenizer: TokenizerMode::Default,
        search_options: SearchOptions::default(),
        auto_suggest_options: None,
        auto_vacuum: Some(AutoVacuumSetting::Options(AutoVacuumOptions {
            min_dirt_count: Some(2),
            min_dirt_factor: Some(0.2),
            batch_size: Some(3),
            batch_wait: Some(4),
        })),
    }
}

fn populated() -> MiniSearch {
    let mut search = MiniSearch::new(options());
    search
        .add_all(vec![
            json!({ "id": 1, "title": "Vita Nova", "text": "In quella parte", "category": "poetry" }),
            json!({ "id": 2, "title": "I Promessi Sposi", "text": "Quel ramo del lago", "category": "fiction" }),
            json!({ "id": 3, "title": "Divina Commedia", "text": "Nel mezzo del cammin", "category": "poetry" }),
        ])
        .unwrap();
    search.discard(&json!(2)).unwrap();
    search
}

#[test]
fn rust_json_round_trip_preserves_dirty_state_options_and_results() {
    let search = populated();
    let serialized = serde_json::to_string(&search).unwrap();
    let restored: MiniSearch = serde_json::from_str(&serialized).unwrap();

    assert_eq!(restored.to_bytes().unwrap(), search.to_bytes().unwrap());
    for query in ["vita", "lago", "mezzo", "promessi"] {
        assert_eq!(
            restored.search(query, SearchOptions::default()),
            search.search(query, SearchOptions::default()),
            "query={query}"
        );
    }
}

#[test]
fn binary_round_trip_preserves_dirty_state_options_and_results() {
    let search = populated();
    let restored = MiniSearch::from_bytes(&search.to_bytes().unwrap()).unwrap();

    assert_eq!(restored.to_bytes().unwrap(), search.to_bytes().unwrap());
    assert_eq!(restored.dirt_count(), 1);
    assert!(restored.search("lago", SearchOptions::default()).is_empty());
}

#[test]
fn malformed_json_and_binary_snapshots_are_rejected() {
    assert!(serde_json::from_str::<MiniSearch>("{").is_err());

    let empty_error = MiniSearch::from_bytes(&[]).unwrap_err();
    assert!(empty_error.contains("unexpected end"));

    let mut incompatible = populated().to_bytes().unwrap();
    incompatible[0] = 99;
    assert_eq!(
        MiniSearch::from_bytes(&incompatible).unwrap_err(),
        "unsupported minisearch-wasm binary snapshot version 99"
    );

    let mut truncated = populated().to_bytes().unwrap();
    truncated.truncate(truncated.len() / 2);
    assert!(MiniSearch::from_bytes(&truncated).is_err());
}
