// Behaviors that bring the port closer to JS MiniSearch at no runtime cost:
// JS `Map` tie order, `String(number)` field formatting, UTF-16 term lengths,
// `Object.keys` order for matched terms, and MiniSearch's own JSON format.
use minisearch_wasm::{FuzzySetting, MiniSearch, MiniSearchOptions, SearchOptions};
use serde_json::{json, Value};

fn options(fields: &[&str], store: &[&str]) -> MiniSearchOptions {
    serde_json::from_value(json!({
        "fields": fields, "storeFields": store, "autoVacuum": false
    }))
    .unwrap()
}

fn engine(fields: &[&str], store: &[&str]) -> MiniSearch {
    MiniSearch::new(options(fields, store))
}

fn ids(results: &[minisearch_wasm::SearchResult]) -> Vec<Value> {
    results.iter().map(|result| result.id.clone()).collect()
}

#[test]
fn equal_scores_keep_javascript_map_order() {
    // JS builds the result Map by folding the per-term maps in query order:
    // "a" touches docs 2 and 3, then "b" adds doc 1. Docs 1 and 2 tie, and the
    // stable score sort leaves them in that insertion order: 3, 2, 1. The old
    // doc-id tiebreak produced 3, 1, 2.
    let mut search = engine(&["text"], &[]);
    search
        .add_all(vec![
            json!({"id": 1, "text": "b"}),
            json!({"id": 2, "text": "a"}),
            json!({"id": 3, "text": "a b"}),
        ])
        .unwrap();
    let results = search.search("a b", SearchOptions::default());
    assert_eq!(ids(&results), vec![json!(3), json!(2), json!(1)]);
    assert_eq!(results[1].score, results[2].score);

    let joined: Vec<Value> =
        serde_json::from_str(&search.search_joined_default("a b", true).ids).unwrap();
    assert_eq!(joined, vec![json!(3), json!(2), json!(1)]);
}

#[test]
fn matched_terms_follow_javascript_object_key_order() {
    // `Object.keys(match)` lists array-index-like keys first, ascending, then
    // the others in insertion order.
    let mut search = engine(&["text"], &[]);
    search
        .add(json!({"id": 1, "text": "report 2024 zeta alpha 7"}))
        .unwrap();
    let results = search.search("zeta report 2024 alpha 7", SearchOptions::default());
    assert_eq!(
        results[0].terms,
        vec!["7", "2024", "zeta", "report", "alpha"]
    );
    assert_eq!(
        results[0].matches.js_keys(),
        vec!["7", "2024", "zeta", "report", "alpha"]
    );
    assert_eq!(
        results[0].query_terms,
        vec!["zeta", "report", "2024", "alpha", "7"]
    );
    let joined = search.search_joined_default("zeta report 2024 alpha 7", false);
    assert_eq!(joined.terms, "7 2024 zeta report alpha");
    let suggestions = search.auto_suggest("zeta report 2024 alpha 7", None);
    assert_eq!(suggestions[0].suggestion, "7 2024 zeta report alpha");

    // Non-numeric terms keep first-match order.
    let plain = search.search("zeta alpha", SearchOptions::default());
    assert_eq!(plain[0].terms, vec!["zeta", "alpha"]);
}

#[test]
fn numeric_field_values_stringify_like_javascript() {
    // JS indexes `fieldValue.toString()`, so 10.0 is "10", not "10.0".
    let mut floats = engine(&["text"], &[]);
    floats.add(json!({"id": 1, "text": 10.0})).unwrap();
    floats.add(json!({"id": 2, "text": "10 20"})).unwrap();
    let mut strings = engine(&["text"], &[]);
    strings.add(json!({"id": 1, "text": "10"})).unwrap();
    strings.add(json!({"id": 2, "text": "10 20"})).unwrap();
    assert_eq!(
        floats.search("10", SearchOptions::default()),
        strings.search("10", SearchOptions::default())
    );
    assert!(floats.search("0", SearchOptions::default()).is_empty());

    // Number.prototype.toString formatting: exponent form outside 1e-6..1e21,
    // integers beyond 2^53 rounded to doubles, arrays joined with commas.
    let mut formatted = engine(&["text"], &[]);
    formatted
        .add(json!({"id": 1, "text": [1e21, 1.5, 0.000001, 1e-7, -2.0, 12345678901234567890u64]}))
        .unwrap();
    // `+` is a math symbol, not punctuation, so "1e+21" stays one token; the
    // `-` in "1e-7" splits it into "1e" and "7".
    for present in [
        "1e+21",
        "1e",
        "1",
        "5",
        "0",
        "000001",
        "7",
        "2",
        "12345678901234567000",
    ] {
        assert_eq!(
            formatted.search(present, SearchOptions::default()).len(),
            1,
            "token {present}"
        );
    }
    for absent in [
        "21",
        "1000000000000000000000",
        "0000001",
        "12345678901234567890",
    ] {
        assert!(
            formatted
                .search(absent, SearchOptions::default())
                .is_empty(),
            "token {absent}"
        );
    }
}

#[test]
fn numeric_ids_compare_as_javascript_numbers() {
    // 1 and 1.0 are the same JS number, so they are the same document id.
    let mut search = engine(&["text"], &[]);
    search.add(json!({"id": 1.0, "text": "apple"})).unwrap();
    assert!(search.add(json!({"id": 1, "text": "pear"})).is_err());
    assert!(search.has(&json!(1)));
    search.discard(&json!(1)).unwrap();
    assert_eq!(search.document_count(), 0);
}

#[test]
fn term_lengths_and_fuzzy_distances_use_utf16_units() {
    let fuzzy = SearchOptions {
        fuzzy: Some(FuzzySetting::Distance(0.2)),
        ..SearchOptions::default()
    };
    let mut search = engine(&["text"], &[]);
    search
        .add_all(vec![
            json!({"id": 1, "text": "😀😀"}),
            json!({"id": 2, "text": "😀😀xy"}),
        ])
        .unwrap();
    // "😀😀😀".length is 6 in JS, so maxFuzzy is round(1.2) = 1, and the distance
    // to "😀😀" is 2 code units: no match. Counting code points (3 and 1) it
    // would have matched.
    assert!(search.search("😀😀😀", fuzzy.clone()).is_empty());
    // "😀😀x" is 5 units (maxFuzzy 1) and one edit away from both terms.
    assert_eq!(search.search("😀😀x", fuzzy).len(), 2);
}

#[test]
fn stored_fields_lookup_matches_javascript() {
    let mut search = engine(&["text"], &["tag"]);
    search
        .add(json!({"id": 1, "text": "apple", "tag": "fruit"}))
        .unwrap();
    search.add(json!({"id": 2, "text": "pear"})).unwrap();
    assert_eq!(
        search.stored_fields_of(&json!(1)).unwrap()["tag"],
        json!("fruit")
    );
    assert!(search.stored_fields_of(&json!(2)).unwrap().is_empty());
    assert!(search.stored_fields_of(&json!(3)).is_none());
    assert!(engine(&["text"], &[]).stored_fields_of(&json!(1)).is_none());
}

fn populated() -> MiniSearch {
    let mut search = engine(&["title", "text"], &["tag"]);
    search
        .add_all(vec![
            json!({"id": "a", "title": "apple pie", "text": "sweet apple", "tag": 1}),
            json!({"id": 2, "title": "pear", "tag": {"nested": true}}),
            json!({"id": 3, "text": " apply application "}),
            json!({"id": 4}),
            json!({"id": 5, "title": "apricot", "text": "apple"}),
        ])
        .unwrap();
    search.discard(&json!(5)).unwrap();
    search
}

#[test]
fn minisearch_json_round_trips_through_the_javascript_format() {
    let search = populated();
    let json = search.to_minisearch_json().unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["serializationVersion"], json!(2));
    assert_eq!(parsed["documentCount"], json!(4));
    assert_eq!(parsed["dirtCount"], json!(1));
    assert_eq!(parsed["fieldIds"], json!({"title": 0, "text": 1}));
    // Sparse JS arrays: interior holes are null, trailing ones are dropped and
    // documents without indexed fields have no entry.
    assert_eq!(parsed["fieldLength"]["1"], json!([1]));
    // " apply application " has three unique tokens in JS: the boundary empty
    // token counts.
    assert_eq!(parsed["fieldLength"]["2"], json!([null, 3]));
    assert!(parsed["fieldLength"].get("3").is_none());
    assert_eq!(parsed["storedFields"]["0"], json!({"tag": 1}));

    let loaded =
        MiniSearch::from_minisearch_json(&json, options(&["title", "text"], &["tag"])).unwrap();
    assert_eq!(loaded.document_count(), 4);
    assert_eq!(loaded.dirt_count(), 1);
    // Like JS `loadJSON`, the import re-inserts terms in serialized order, so
    // the tree — and with it matched-term order — can differ from the live
    // index; result sets, scores and per-document term sets are identical.
    for query in ["apple", "app", "pear apple", "apl", ""] {
        let options = SearchOptions {
            prefix: true.into(),
            fuzzy: Some(FuzzySetting::Distance(0.2)),
            ..SearchOptions::default()
        };
        let mut expected = search.search(query, options.clone());
        let mut actual = loaded.search(query, options);
        for results in [&mut expected, &mut actual] {
            results.sort_by_key(|result| result.id.to_string());
            for result in results.iter_mut() {
                result.terms.sort();
            }
        }
        assert_eq!(ids(&actual), ids(&expected), "query {query}");
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.score, expected.score, "query {query}");
            assert_eq!(actual.terms, expected.terms, "query {query}");
            assert_eq!(actual.stored_fields, expected.stored_fields);
        }
    }
    // Re-exporting a reloaded index lists the same entries (in a different
    // tree order, as in JS), and a second import is identical to the first.
    let reexported: Value = serde_json::from_str(&loaded.to_minisearch_json().unwrap()).unwrap();
    let sorted_index = |value: &Value| {
        let mut entries = value["index"].as_array().unwrap().clone();
        entries.sort_by_key(|entry| entry[0].to_string());
        entries
    };
    assert_eq!(sorted_index(&reexported), sorted_index(&parsed));
    for field in [
        "documentIds",
        "fieldLength",
        "storedFields",
        "averageFieldLength",
        "dirtCount",
    ] {
        assert_eq!(reexported[field], parsed[field], "{field}");
    }
    let again =
        MiniSearch::from_minisearch_json(&json, options(&["title", "text"], &["tag"])).unwrap();
    assert_eq!(again.to_bytes().unwrap(), loaded.to_bytes().unwrap());
    assert_eq!(
        again.auto_suggest("app", None),
        loaded.auto_suggest("app", None)
    );
}

#[test]
fn loads_javascript_serialized_indexes_of_both_versions() {
    // Hand-written output of `JSON.stringify(new MiniSearch({fields: ["title"],
    // storeFields: ["tag"]}))` after adding {id: "x", title: "hello world",
    // tag: "t"} — as version 2 and in the older version 1 shape.
    let v2 = r#"{"documentCount":1,"nextId":1,"documentIds":{"0":"x"},"fieldIds":{"title":0},
        "fieldLength":{"0":[2]},"averageFieldLength":[2],"storedFields":{"0":{"tag":"t"}},
        "dirtCount":0,"index":[["hello",{"0":{"0":1}}],["world",{"0":{"0":1}}]],
        "serializationVersion":2}"#;
    let v1 = r#"{"documentCount":1,"nextId":1,"documentIds":{"0":"x"},"fieldIds":{"title":0},
        "fieldLength":{"0":[2]},"averageFieldLength":[2],"storedFields":{"0":{"tag":"t"}},
        "index":[["hello",{"0":{"ds":{"0":1}}}],["world",{"0":{"ds":{"0":1}}}]],
        "serializationVersion":1}"#;
    for serialized in [v2, v1] {
        let loaded =
            MiniSearch::from_minisearch_json(serialized, options(&["title"], &["tag"])).unwrap();
        let results = loaded.search("hello", SearchOptions::default());
        assert_eq!(ids(&results), vec![json!("x")]);
        assert_eq!(results[0].stored_fields["tag"], json!("t"));
        assert_eq!(loaded.term_count(), 2);
    }

    let wrong_fields = MiniSearch::from_minisearch_json(v2, options(&["body"], &[]));
    assert!(wrong_fields.is_err());
    let bad_version = v2.replace("\"serializationVersion\":2", "\"serializationVersion\":3");
    assert_eq!(
        MiniSearch::from_minisearch_json(&bad_version, options(&["title"], &["tag"])).unwrap_err(),
        "MiniSearch: cannot deserialize an index created with an incompatible version"
    );
    let inconsistent = v2.replace("\"documentCount\":1", "\"documentCount\":7");
    assert!(
        MiniSearch::from_minisearch_json(&inconsistent, options(&["title"], &["tag"])).is_err()
    );
    let stale_without_dirt = v2.replace("\"documentIds\":{\"0\":\"x\"}", "\"documentIds\":{}");
    assert!(
        MiniSearch::from_minisearch_json(&stale_without_dirt, options(&["title"], &["tag"]))
            .is_err()
    );
}
