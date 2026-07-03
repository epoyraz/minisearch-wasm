//! Bulk differential dump against JS MiniSearch (see scratch `js_bulk.mjs`):
//! reads the synthetic corpus JSON (`argv[1]`), runs the same searches and
//! auto-suggestions on a fresh index and after removes/discards, and writes
//! the same JSON shape to `argv[2]`. Search results come from the packed path
//! (the `searchJoined` engine path), whose per-document term order mirrors the
//! JS `Object.keys(match)` order.

use minisearch_wasm::{
    AutoSuggestOptions, CombineWith, FuzzySetting, MiniSearch, MiniSearchOptions,
    PartialSearchOptions, Query, QueryCombination, SearchOptions, TokenizerMode,
};
use serde_json::{json, Map, Value};

/// Parse the harness's JSON tree representation: strings are text queries,
/// `{"wildcard": true}` is the wildcard, everything else is a combination
/// node whose non-`queries` keys are option overrides.
fn value_to_query(value: &Value) -> Query {
    match value {
        Value::String(text) => Query::Text(text.clone()),
        Value::Object(map) => {
            if map.get("wildcard") == Some(&Value::Bool(true)) {
                return Query::Wildcard;
            }
            let queries = map["queries"]
                .as_array()
                .unwrap()
                .iter()
                .map(value_to_query)
                .collect();
            let mut options_map = map.clone();
            options_map.remove("queries");
            let options: PartialSearchOptions =
                serde_json::from_value(Value::Object(options_map)).unwrap();
            Query::Combination(QueryCombination { queries, options })
        }
        other => panic!("invalid tree query node: {other}"),
    }
}

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

fn dump(
    search: &MiniSearch,
    queries: &[String],
    tree_queries: &[(String, Query)],
) -> Map<String, Value> {
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

    // Query trees: terms dumped SORTED, mirroring js_bulk.mjs (JS match keys
    // are insertion-ordered, this port's full-path match map is sorted — a
    // documented divergence; ids and scores are compared exactly).
    let tree_rows = |results: Vec<minisearch_wasm::SearchResult>| -> Value {
        Value::Array(
            results
                .into_iter()
                .map(|result| {
                    let mut terms = result.terms;
                    terms.sort();
                    json!({ "id": result.id, "score": result.score, "terms": terms })
                })
                .collect(),
        )
    };
    let and_per_call = PartialSearchOptions {
        combine_with: Some(CombineWith::And),
        ..PartialSearchOptions::default()
    };
    for (name, tree) in tree_queries {
        out.insert(
            format!("t:{name}"),
            tree_rows(search.search_query(tree, &PartialSearchOptions::default())),
        );
        out.insert(
            format!("tand:{name}"),
            tree_rows(search.search_query(tree, &and_per_call)),
        );
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

    let tree_queries: Vec<(String, Query)> = corpus["treeQueries"]
        .as_array()
        .map(|trees| {
            trees
                .iter()
                .map(|entry| {
                    (
                        entry["name"].as_str().unwrap().to_owned(),
                        value_to_query(&entry["tree"]),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    let fresh = dump(&make_index(&docs), &queries, &tree_queries);

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
    let after_mutation = dump(&mutated, &queries, &tree_queries);

    let out = json!({ "fresh": fresh, "afterMutation": after_mutation });
    std::fs::write(&args[2], serde_json::to_string(&out).unwrap()).unwrap();
    eprintln!("rust dump written: {} labels per phase", fresh.len());
}
