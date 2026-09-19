// Removing documents must not shift every posting list once per document:
// emptied postings stay behind as tombstones until a sweep. Nothing outside the
// list may see them, so `remove_all`, one-by-one removal and a freshly built
// index of the survivors have to agree, snapshots included.
use minisearch_wasm::{MiniSearch, MiniSearchOptions};
use serde_json::{json, Value};
use std::time::Instant;

fn engine() -> MiniSearch {
    MiniSearch::new(
        serde_json::from_value::<MiniSearchOptions>(
            json!({"fields": ["title", "text"], "storeFields": ["title"], "autoVacuum": false}),
        )
        .unwrap(),
    )
}

fn corpus(count: usize) -> Vec<Value> {
    let mut seed = 42u64;
    let mut random = move |bound: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % bound
    };
    let words = [
        "ab", "abc", "abcd", "b", "bar", "baz", "common", "common", "x1", "x2", "zeta",
    ];
    (0..count)
        .map(|id| {
            let mut text = |length: u64| {
                (0..1 + random(length))
                    .map(|_| words[random(words.len() as u64) as usize])
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let title = text(3);
            let body = text(8);
            if id % 7 == 0 {
                json!({"id": id, "title": title})
            } else {
                json!({"id": id, "title": title, "text": format!("{body} unique{id}")})
            }
        })
        .collect()
}

#[test]
fn bulk_removal_equals_removing_one_by_one() {
    let documents = corpus(400);
    let mut removed: Vec<Value> = documents.iter().step_by(3).cloned().collect();
    // A document that changed since it was indexed: warnings, leftover postings.
    removed[5]["text"] = json!("changed common text");
    removed.reverse();

    let mut one_by_one = engine();
    let mut in_bulk = engine();
    one_by_one.add_all(documents.clone()).unwrap();
    in_bulk.add_all(documents).unwrap();

    let mut expected_warnings = Vec::new();
    for document in &removed {
        one_by_one
            .remove_with_warnings(document, &mut expected_warnings)
            .unwrap();
    }
    let mut warnings = Vec::new();
    in_bulk
        .remove_all_with_warnings(removed.clone(), &mut warnings)
        .unwrap();

    assert!(!warnings.is_empty());
    assert_eq!(warnings, expected_warnings);
    assert_eq!(
        serde_json::to_value(&in_bulk).unwrap(),
        serde_json::to_value(&one_by_one).unwrap()
    );
    assert_eq!(in_bulk.to_bytes().unwrap(), one_by_one.to_bytes().unwrap());

    // An unknown document stops the removal; what came before it stays removed
    // and no tombstone is left behind.
    let survivors: Vec<Value> = corpus(400)
        .into_iter()
        .skip(1)
        .step_by(3)
        .take(10)
        .collect();
    let mut batch = survivors.clone();
    batch.insert(4, json!({"id": "missing", "title": "ab"}));
    let error = in_bulk.remove_all(batch).unwrap_err();
    assert!(error.contains("not in the index"), "{error}");
    for document in &survivors[..4] {
        one_by_one.remove(document).unwrap();
    }
    assert_eq!(
        serde_json::to_value(&in_bulk).unwrap(),
        serde_json::to_value(&one_by_one).unwrap()
    );
    MiniSearch::from_bytes(&in_bulk.to_bytes().unwrap()).unwrap();
}

// Removing the oldest documents takes postings off the front of every common
// term's list, the worst case for a list that shifts. Set BULK_REMOVAL_DOCS
// (with --release --nocapture) to time it at scale.
#[test]
fn removing_the_oldest_half_in_bulk_gives_the_same_index() {
    let count = std::env::var("BULK_REMOVAL_DOCS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20_000usize);
    let common = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
    let documents: Vec<Value> = (0..count)
        .map(|id| json!({"id": id, "title": common, "text": format!("{common} u{id}")}))
        .collect();
    let oldest: Vec<Value> = documents[..count / 2].to_vec();
    let mut one_by_one = engine();
    let mut in_bulk = engine();
    one_by_one.add_all(documents.clone()).unwrap();
    in_bulk.add_all(documents).unwrap();

    let started = Instant::now();
    for document in &oldest {
        one_by_one.remove(document).unwrap();
    }
    let sequential = started.elapsed();
    let started = Instant::now();
    in_bulk.remove_all(oldest).unwrap();
    println!(
        "{count} documents: one by one {sequential:?}, in bulk {:?}",
        started.elapsed()
    );

    assert_eq!(in_bulk.to_bytes().unwrap(), one_by_one.to_bytes().unwrap());
}
