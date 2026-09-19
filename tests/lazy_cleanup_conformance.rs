// JS MiniSearch scores the first query that meets a discarded document's
// posting with a document frequency that still counts the stale entries, and
// removes them (or lowers their frequency by one) as it goes. The expected rows
// below are MiniSearch 7.2.0's own output for this history, first query
// included; `search_query_exact` must reproduce them, mutation and all.
use minisearch_wasm::{MiniSearch, MiniSearchOptions, PartialSearchOptions, Query};
use serde_json::json;

fn dirty_index() -> MiniSearch {
    let mut index = MiniSearch::new(
        serde_json::from_value::<MiniSearchOptions>(
            json!({"fields": ["text"], "autoVacuum": false}),
        )
        .unwrap(),
    );
    for id in 0..12 {
        let apples = if id % 4 == 1 {
            "apple apple apple "
        } else {
            "apple "
        };
        let dish = if id % 3 == 0 { "tart" } else { "pie" };
        index
            .add(json!({"id": id, "text": format!("{apples}{dish} w{id}")}))
            .unwrap();
    }
    for id in [1, 5, 9, 11] {
        index.discard(&json!(id)).unwrap();
    }
    index
}

fn run(index: &mut MiniSearch, query: &str, options: serde_json::Value) -> Vec<(i64, f64)> {
    let per_call: PartialSearchOptions = serde_json::from_value(options).unwrap();
    index
        .search_query_exact(&Query::Text(query.to_owned()), &per_call)
        .into_iter()
        .map(|row| (row.id.as_i64().unwrap(), row.score))
        .collect()
}

fn assert_rows(actual: Vec<(i64, f64)>, expected: &[(i64, f64)]) {
    assert_eq!(
        actual.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        expected.iter().map(|(id, _)| *id).collect::<Vec<_>>()
    );
    for ((_, actual), (_, expected)) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() <= 1e-12 * expected.abs(),
            "{actual} vs {expected}"
        );
    }
}

#[test]
fn dirty_queries_match_minisearch_query_by_query() {
    let mut index = dirty_index();

    // Documents 1, 5 and 9 hold `apple` three times: JS lowers a stale
    // posting's frequency by one per query, so they leave on the third.
    assert_rows(
        run(&mut index, "apple", json!({})),
        &[
            (10, -0.08110083190541356),
            (6, -0.23122601974088736),
            (7, -0.23122601974088736),
            (8, -0.23122601974088736),
            (2, -0.3676836870494775),
            (3, -0.3676836870494775),
            (4, -0.3676836870494775),
            (0, -0.49275610045805424),
        ],
    );
    let second = [
        (10, 0.08573762075992294),
        (6, -0.08110083190541356),
        (7, -0.08110083190541356),
        (8, -0.08110083190541356),
        (2, -0.23122601974088736),
        (3, -0.23122601974088736),
        (4, -0.23122601974088736),
        (0, -0.3676836870494775),
    ];
    assert_rows(run(&mut index, "apple", json!({})), &second);
    assert_rows(run(&mut index, "apple", json!({})), &second);
    assert_eq!(index.term_count(), 15);

    // Prefix expansion removes the emptied `w1`, `w5`, `w9` and `w11` terms.
    assert_rows(
        run(&mut index, "w pi", json!({"prefix": true})),
        &[
            (8, 2.0856265404996193),
            (7, 2.0856265404996193),
            (10, 2.012592866482171),
            (4, 1.9392734049580214),
            (2, 1.9392734049580214),
            (6, 0.8764040882093748),
            (3, 0.8764040882093748),
            (0, 0.8764040882093748),
        ],
    );
    assert_eq!(index.term_count(), 11);
    assert_rows(
        run(&mut index, "w pi", json!({"prefix": true})),
        &[
            (8, 2.25647730890513),
            (7, 2.25647730890513),
            (4, 2.25647730890513),
            (2, 2.25647730890513),
            (10, 2.183443634887682),
            (6, 0.8764040882093748),
            (3, 0.8764040882093748),
            (0, 0.8764040882093748),
        ],
    );
    assert_rows(
        run(
            &mut index,
            "aple tart",
            json!({"fuzzy": 0.3, "combineWith": "AND"}),
        ),
        &[
            (0, 2.1437447572497783),
            (3, 2.1437447572497783),
            (6, 2.1437447572497783),
        ],
    );
}

#[test]
fn queries_without_stale_postings_leave_a_dirty_index_untouched() {
    let mut index = dirty_index();
    let before = serde_json::to_value(&index).unwrap();
    // `w0` belongs to a live document only.
    assert_eq!(run(&mut index, "w0", json!({})).len(), 1);
    assert_eq!(serde_json::to_value(&index).unwrap(), before);
    // The non-mutating API never changes the index, stale postings or not.
    let per_call = PartialSearchOptions::default();
    index.search_query(&Query::Text("apple".to_owned()), &per_call);
    assert_eq!(serde_json::to_value(&index).unwrap(), before);
}
