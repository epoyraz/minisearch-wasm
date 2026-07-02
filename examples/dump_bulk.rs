//! Bulk differential dump against JS MiniSearch (see scratch `js_bulk.mjs`):
//! reads the synthetic corpus JSON (`argv[1]`), runs the same searches and
//! auto-suggestions on a fresh index and after removes/discards, and writes
//! the same JSON shape to `argv[2]`. Search results come from the packed path
//! (the `searchJoined` engine path), whose per-document term order mirrors the
//! JS `Object.keys(match)` order.

use minisearch_wasm::{
    AutoSuggestOptions, CombineWith, FuzzySetting, MiniSearch, MiniSearchOptions, SearchOptions,
    TokenizerMode,
};
use serde_json::{json, Map, Value};

fn make_index(docs: &[Value]) -> MiniSearch {
    let mut search = MiniSearch::new(MiniSearchOptions {
        fields: vec!["title".to_owned(), "text".to_owned()],
        id_field: "id".to_owned(),
        store_fields: vec![],
        tokenizer: TokenizerMode::Default,
        search_options: SearchOptions::default(),
        auto_suggest_options: None,
    });
    search.add_all(docs.to_vec()).unwrap();
    search
}

fn option_combos() -> Vec<(&'static str, SearchOptions)> {
    let fuzzy = Some(FuzzySetting::Distance(0.2));
    vec![
        ("plain", SearchOptions::default()),
        (
            "and",
            SearchOptions {
                combine_with: CombineWith::And,
                ..SearchOptions::default()
            },
        ),
        (
            "prefix",
            SearchOptions {
                prefix: true,
                ..SearchOptions::default()
            },
        ),
        (
            "fuzzy",
            SearchOptions {
                fuzzy,
                ..SearchOptions::default()
            },
        ),
        (
            "pf_and",
            SearchOptions {
                prefix: true,
                fuzzy,
                combine_with: CombineWith::And,
                ..SearchOptions::default()
            },
        ),
        (
            "pf_or",
            SearchOptions {
                prefix: true,
                fuzzy,
                ..SearchOptions::default()
            },
        ),
    ]
}

fn dump(search: &MiniSearch, queries: &[String]) -> Map<String, Value> {
    let mut out = Map::new();

    for query in queries {
        for (name, options) in option_combos() {
            let packed = search.search_packed(query, options);
            let rows: Vec<Value> = packed
                .ids
                .iter()
                .zip(&packed.scores)
                .zip(&packed.terms)
                .map(|((id, score), terms)| json!({ "id": id, "score": score, "terms": terms }))
                .collect();
            out.insert(format!("s:{name}:{query}"), Value::Array(rows));
        }

        let suggest_combos: Vec<(&str, Option<AutoSuggestOptions>)> = vec![
            ("default", None),
            (
                "fuzzy",
                Some(AutoSuggestOptions {
                    fuzzy: Some(FuzzySetting::Distance(0.2)),
                    ..AutoSuggestOptions::default()
                }),
            ),
            (
                "or",
                Some(AutoSuggestOptions {
                    combine_with: Some(CombineWith::Or),
                    ..AutoSuggestOptions::default()
                }),
            ),
        ];
        for (name, options) in suggest_combos {
            out.insert(
                format!("a:{name}:{query}"),
                serde_json::to_value(search.auto_suggest(query, options.as_ref())).unwrap(),
            );
        }
    }

    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let corpus: Value = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let docs: Vec<Value> = corpus["docs"].as_array().unwrap().clone();
    let queries: Vec<String> = corpus["queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q.as_str().unwrap().to_owned())
        .collect();

    let fresh = dump(&make_index(&docs), &queries);

    let mut mutated = make_index(&docs);
    let mut index = 0usize;
    while index < docs.len() {
        mutated.remove(&docs[index]).unwrap();
        index += 7;
    }
    index = 3;
    while index < docs.len() {
        if index % 7 != 0 {
            mutated.discard(&json!(index)).unwrap();
        }
        index += 11;
    }
    let after_mutation = dump(&mutated, &queries);

    let out = json!({ "fresh": fresh, "afterMutation": after_mutation });
    std::fs::write(&args[2], serde_json::to_string(&out).unwrap()).unwrap();
    eprintln!("rust dump written: {} labels per phase", fresh.len());
}
