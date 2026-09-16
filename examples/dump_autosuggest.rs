//! Dumps `auto_suggest` output for the differential check against JS
//! MiniSearch (see the scratch script `js_autosuggest.mjs`). Prints one JSON
//! object keyed by test label.

use minisearch_wasm::{
    AutoSuggestOptions, CombineWith, FuzzySetting, MiniSearch, MiniSearchOptions, SearchOptions,
    TokenizerMode, Weights,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

fn italian() -> Vec<Value> {
    vec![
        json!({ "id": 1, "title": "Divina Commedia", "text": "Nel mezzo del cammin di nostra vita", "category": "poetry" }),
        json!({ "id": 2, "title": "I Promessi Sposi", "text": "Quel ramo del lago di Como", "category": "fiction" }),
        json!({ "id": 3, "title": "Vita Nova", "text": "In quella parte del libro della mia memoria", "category": "poetry" }),
    ]
}

fn books() -> Vec<Value> {
    vec![
        json!({ "id": 1, "title": "Moby Dick", "text": "Call me Ishmael. Some years ago...", "category": "fiction" }),
        json!({ "id": 2, "title": "Zen and the Art of Motorcycle Maintenance", "text": "I can see by my watch...", "category": "fiction" }),
        json!({ "id": 3, "title": "Neuromancer", "text": "The sky above the port was...", "category": "fiction" }),
        json!({ "id": 4, "title": "Zen and the Art of Archery", "text": "At first sight it must seem...", "category": "non-fiction" }),
    ]
}

fn index(
    docs: Vec<Value>,
    auto_suggest_options: Option<AutoSuggestOptions>,
    search_options: SearchOptions,
) -> MiniSearch {
    let mut search = MiniSearch::new(MiniSearchOptions {
        fields: vec!["title".to_owned(), "text".to_owned()],
        id_field: "id".to_owned(),
        store_fields: vec!["category".to_owned()],
        tokenizer: TokenizerMode::Default,
        search_options,
        auto_suggest_options,
        auto_vacuum: None,
    });
    search.add_all(docs).unwrap();
    search
}

fn main() {
    let mut out = Map::new();
    let mut run =
        |label: &str, search: &MiniSearch, query: &str, options: Option<&AutoSuggestOptions>| {
            out.insert(
                label.to_owned(),
                serde_json::to_value(search.auto_suggest(query, options)).unwrap(),
            );
        };

    let it = index(italian(), None, SearchOptions::default());
    run("it:com", &it, "com", None);
    run("it:vita no", &it, "vita no", None);
    run("it:nostra vi", &it, "nostra vi", None);
    run("it:vita", &it, "vita", None);
    run("it:del", &it, "del", None);
    run("it:de", &it, "de", None);
    run("it:d", &it, "d", None);
    run("it:quel", &it, "quel", None);
    run(
        "it:vita-fuzzy-prefix",
        &it,
        "vita",
        Some(&AutoSuggestOptions {
            fuzzy: Some(FuzzySetting::Enabled(true)),
            prefix: Some(true.into()),
            ..AutoSuggestOptions::default()
        }),
    );
    run(
        "it:del la-OR",
        &it,
        "del la",
        Some(&AutoSuggestOptions {
            combine_with: Some(CombineWith::Or),
            ..AutoSuggestOptions::default()
        }),
    );
    run("it:della memoria", &it, "della memoria", None);
    run(
        "it:memoria-fuzzy02",
        &it,
        "memoia",
        Some(&AutoSuggestOptions {
            fuzzy: Some(FuzzySetting::Distance(0.2)),
            ..AutoSuggestOptions::default()
        }),
    );
    let mut boost = BTreeMap::new();
    boost.insert("title".to_owned(), 2.0);
    run(
        "it:boost-title",
        &it,
        "vita",
        Some(&AutoSuggestOptions {
            boost,
            ..AutoSuggestOptions::default()
        }),
    );
    run(
        "it:weights",
        &it,
        "com",
        Some(&AutoSuggestOptions {
            weights: Some(Weights {
                prefix: 0.2,
                fuzzy: 0.9,
            }),
            ..AutoSuggestOptions::default()
        }),
    );
    run(
        "it:fields-title",
        &it,
        "vita",
        Some(&AutoSuggestOptions {
            fields: Some(vec!["title".to_owned()]),
            ..AutoSuggestOptions::default()
        }),
    );

    let ctor_suggest = index(
        italian(),
        Some(AutoSuggestOptions {
            combine_with: Some(CombineWith::Or),
            fuzzy: Some(FuzzySetting::Enabled(true)),
            ..AutoSuggestOptions::default()
        }),
        SearchOptions::default(),
    );
    run("ctor-suggest:nosta vi", &ctor_suggest, "nosta vi", None);
    run("ctor-suggest:com", &ctor_suggest, "com", None);

    let ctor_search = index(
        italian(),
        None,
        SearchOptions {
            combine_with: CombineWith::Or,
            fuzzy: Some(FuzzySetting::Enabled(true)),
            ..SearchOptions::default()
        },
    );
    run("ctor-search:nosta vi", &ctor_search, "nosta vi", None);

    let bk = index(books(), None, SearchOptions::default());
    run("bk:zen ar", &bk, "zen ar", None);
    run(
        "bk:zen ar-OR",
        &bk,
        "zen ar",
        Some(&AutoSuggestOptions {
            combine_with: Some(CombineWith::Or),
            ..AutoSuggestOptions::default()
        }),
    );
    run("bk:moto", &bk, "moto", None);
    run(
        "bk:nromancer-fuzzy",
        &bk,
        "nromancer",
        Some(&AutoSuggestOptions {
            fuzzy: Some(FuzzySetting::Distance(0.2)),
            ..AutoSuggestOptions::default()
        }),
    );
    run("bk:the", &bk, "the", None);
    run("bk:zen art mot", &bk, "zen art mot", None);
    run("bk:a", &bk, "a", None);

    println!(
        "{}",
        serde_json::to_string_pretty(&Value::Object(out)).unwrap()
    );
}
