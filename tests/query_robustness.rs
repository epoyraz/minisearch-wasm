// Queries no search box should send must still come back: on wasm32 a panic or
// a failed allocation aborts the module for every index in it.
use minisearch_wasm::{MiniSearch, MiniSearchOptions, PartialSearchOptions, Query, SearchableMap};
use serde_json::{json, Value};

fn engine(settings: Value) -> MiniSearch {
    MiniSearch::new(serde_json::from_value::<MiniSearchOptions>(settings).unwrap())
}
fn per_call(options: Value) -> PartialSearchOptions {
    serde_json::from_value(options).unwrap()
}
fn rows(index: &MiniSearch, query: &str, options: Value) -> Vec<(String, f64)> {
    index
        .search_query(&Query::Text(query.to_owned()), &per_call(options))
        .into_iter()
        .map(|row| (row.id.to_string(), row.score))
        .collect()
}

#[test]
fn an_absurd_fuzzy_distance_matches_everything_and_allocates_nothing_absurd() {
    let mut index = engine(json!({"fields": ["text"]}));
    index
        .add_all(vec![
            json!({"id": 1, "text": "apple"}),
            json!({"id": 2, "text": "pear"}),
        ])
        .unwrap();
    // Longer than 64 UTF-16 units: the banded-matrix fallback, whose size
    // depends on the distance.
    let long = "a".repeat(70);
    let everything = rows(&index, &long, json!({"fuzzy": 1000}));
    assert_eq!(everything.len(), 2);
    for distance in [6e7, 1e15, f64::MAX] {
        assert_eq!(rows(&index, &long, json!({"fuzzy": distance})), everything);
        let joined = index.search_joined_opts(&long, &per_call(json!({"fuzzy": distance})));
        assert_eq!(joined.scores.len(), 2);
    }
    // The bit-parallel path (up to 64 units) takes the same distances.
    assert_eq!(rows(&index, "appel", json!({"fuzzy": 6e7})).len(), 2);

    let mut map = SearchableMap::new();
    map.set("apple", 1);
    assert_eq!(map.fuzzy_get(&long, usize::MAX).len(), 1);
}

#[test]
fn a_query_term_too_long_for_a_fuzzy_matrix_still_finds_its_exact_match() {
    let mut index = engine(json!({"fields": ["text"]}));
    let long = "b".repeat(20_000);
    index.add(json!({"id": 1, "text": long})).unwrap();
    index.add(json!({"id": 2, "text": "pear"})).unwrap();
    assert_eq!(rows(&index, &long, json!({"fuzzy": 0.2})).len(), 1);
    assert_eq!(
        rows(&index, &long, json!({"fuzzy": 0.2, "prefix": true})).len(),
        1
    );
}

// A query term that occurs twice can expand differently each time. A document
// reached only by the second occurrence still matched that term, and its
// quality factor (matched distinct query terms) must say so on every path.
#[test]
fn a_repeated_query_term_is_credited_whichever_occurrence_matches() {
    let mut index = engine(json!({"fields": ["text"]}));
    for (id, text) in [
        (1, "foobar bar"),
        (2, "foo bar"),
        (3, "foobar"),
        (4, "bar baz"),
    ] {
        index.add(json!({"id": id, "text": text})).unwrap();
    }
    let options = json!({"prefix": [false, false, true]});
    let full = rows(&index, "foo bar foo", options.clone());
    let joined = index.search_joined_opts("foo bar foo", &per_call(options.clone()));
    let raw = index.search_raw("foo bar foo", &per_call(options));
    assert_eq!(
        joined.scores,
        full.iter().map(|row| row.1).collect::<Vec<_>>()
    );
    assert_eq!(raw.scores, joined.scores);
    assert_eq!(
        serde_json::from_str::<Vec<Value>>(&joined.ids).unwrap(),
        full.iter()
            .map(|row| serde_json::from_str(&row.0).unwrap())
            .collect::<Vec<Value>>()
    );

    // MiniSearch 7.2.0: autoSuggest('foo bar foo', { combineWith: 'OR' }).
    let suggest: minisearch_wasm::AutoSuggestOptions =
        serde_json::from_value(json!({"combineWith": "OR"})).unwrap();
    let suggestions = index.auto_suggest("foo bar foo", Some(&suggest));
    let bar_foobar = suggestions
        .iter()
        .find(|suggestion| suggestion.suggestion == "bar foobar")
        .expect("suggestion");
    assert!(
        (bar_foobar.score - 1.687824161083053).abs() < 1e-12,
        "{}",
        bar_foobar.score
    );

    // Past the 64 distinct terms a bit mask holds.
    let mut wide = engine(json!({"fields": ["text"]}));
    let terms: Vec<String> = (0..70).map(|n| format!("term{n}")).collect();
    wide.add(json!({"id": 1, "text": format!("{} lastly", terms.join(" "))}))
        .unwrap();
    let query = format!("{} last last", terms.join(" "));
    let mut prefix = vec![false; 72];
    prefix[71] = true;
    let options = json!({"prefix": prefix});
    let full = rows(&wide, &query, options.clone());
    assert_eq!(
        wide.search_joined_opts(&query, &per_call(options)).scores,
        vec![full[0].1]
    );
}

// JSON text keeps `4.0` a float and `4` an integer; to a filter they are the
// same number, as they are to the JavaScript callback it stands in for.
#[test]
fn the_declarative_filter_compares_numbers_by_value() {
    let mut index = engine(json!({"fields": ["text"], "storeFields": ["rating", "tags"]}));
    let documents: Vec<Value> = serde_json::from_str(
        r#"[{"id": 1.0, "text": "foo", "rating": 4.0, "tags": [1, 2.0]},
            {"id": 2, "text": "foo", "rating": 1E2, "tags": [1, 3]},
            {"id": 3, "text": "foo", "rating": 4.5, "tags": []}]"#,
    )
    .unwrap();
    index.add_all(documents).unwrap();
    let matching = |filter: Value| -> Vec<String> {
        rows(&index, "foo", json!({"filter": filter}))
            .into_iter()
            .map(|row| row.0)
            .collect()
    };
    assert_eq!(matching(json!({"rating": 4})), ["1.0"]);
    assert_eq!(matching(json!({"rating": 100})), ["2"]);
    assert_eq!(matching(json!({"rating": 4.5})), ["3"]);
    assert_eq!(matching(json!({"id": 1})), ["1.0"]);
    assert_eq!(matching(json!({"tags": [1.0, 2]})), ["1.0"]);
    assert!(matching(json!({"rating": "4"})).is_empty());
}
