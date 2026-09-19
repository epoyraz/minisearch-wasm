// An index the engine can reach must load again once it has been saved, and a
// state no reader accepts must be refused when saving. Run in release mode as
// well (`cargo test --release --test snapshot_robustness`): release writers
// skip the full structural validation that debug builds perform.
use minisearch_wasm::{MiniSearch, MiniSearchOptions, SearchOptions};
use serde_json::{json, Value};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Tracks live heap bytes and their peak, to bound what a rejected snapshot
/// can make the reader reserve.
struct Tracking;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn grow(bytes: usize) {
    PEAK.fetch_max(
        LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes,
        Ordering::Relaxed,
    );
}
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(pointer, layout)
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        grow(size);
        System.realloc(pointer, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

fn options(value: Value) -> MiniSearchOptions {
    serde_json::from_value(value).unwrap()
}
fn engine(value: Value) -> MiniSearch {
    MiniSearch::new(options(value))
}
fn ids(index: &MiniSearch, query: &str) -> Vec<String> {
    let mut ids: Vec<String> = index
        .search(query, SearchOptions::default())
        .into_iter()
        .map(|row| row.id.to_string())
        .collect();
    ids.sort();
    ids
}
/// Both formats load, and the copies answer like the original.
fn assert_round_trips(index: &MiniSearch, queries: &[&str]) {
    let copies = [
        MiniSearch::from_bytes(&index.to_bytes().unwrap()).unwrap(),
        MiniSearch::from_json(&index.to_json().unwrap()).unwrap(),
    ];
    for copy in &copies {
        assert_eq!(copy.document_count(), index.document_count());
        assert_eq!(copy.term_count(), index.term_count());
        assert_eq!(copy.dirt_count(), index.dirt_count());
        for query in queries {
            assert_eq!(ids(copy, query), ids(index, query), "{query}");
        }
    }
}

#[test]
fn a_negative_field_average_survives_a_save() {
    // Removing documents that lack a field drags that field's running
    // average below zero, in JS MiniSearch as well ([1, -6] for this history).
    let mut index = engine(json!({"fields": ["title", "text"], "autoVacuum": false}));
    for id in 0..8 {
        index.add(json!({"id": id, "title": "only title"})).unwrap();
    }
    index
        .add(json!({"id": 8, "title": "both", "text": "apple pie"}))
        .unwrap();
    index
        .add(json!({"id": 9, "title": "both", "text": "apple tart"}))
        .unwrap();
    for id in 0..8 {
        index
            .remove(&json!({"id": id, "title": "only title"}))
            .unwrap();
    }
    index.discard(&json!(9)).unwrap();
    let snapshot = serde_json::to_value(&index).unwrap();
    assert!(snapshot["average_field_length"][1].as_f64().unwrap() < 0.0);
    assert_round_trips(&index, &["apple", "both"]);

    let imported = MiniSearch::from_minisearch_json(
        &index.to_minisearch_json().unwrap(),
        options(json!({"fields": ["title", "text"], "autoVacuum": false})),
    )
    .unwrap();
    assert_eq!(ids(&imported, "apple"), ids(&index, "apple"));
}

#[test]
fn postings_left_by_a_changed_document_survive_a_save_and_a_compaction() {
    let mut index = engine(json!({"fields": ["title"], "autoVacuum": false}));
    index.add(json!({"id": 1, "title": "foo bar"})).unwrap();
    index.add(json!({"id": 2, "title": "bar baz"})).unwrap();
    // The document changed before removal: `bar` keeps a posting for it, and
    // the dirt count does not know.
    index.remove(&json!({"id": 1, "title": "foo"})).unwrap();
    assert_eq!(index.dirt_count(), 0);
    assert_round_trips(&index, &["foo", "bar", "baz"]);

    index.compact().unwrap();
    assert_eq!(ids(&index, "bar"), ["2"]);
    assert_round_trips(&index, &["bar"]);
    // Nothing stale is left: a vacuum changes nothing.
    let before = index.to_bytes().unwrap();
    index.vacuum();
    assert_eq!(index.to_bytes().unwrap(), before);

    // A term held only by the removed document disappears with it.
    let mut index = engine(json!({"fields": ["title"], "autoVacuum": false}));
    index.add(json!({"id": 1, "title": "foo bar"})).unwrap();
    index.add(json!({"id": 2, "title": "baz"})).unwrap();
    index.remove(&json!({"id": 1, "title": "foo"})).unwrap();
    assert_eq!(index.term_count(), 2);
    index.compact().unwrap();
    assert_eq!(index.term_count(), 1);
}

#[test]
fn nested_prefixes_save_up_to_the_depth_every_reader_accepts() {
    let nested = |depth: usize| {
        let mut index = engine(json!({"fields": ["text"], "autoVacuum": false}));
        let text: Vec<String> = (1..=depth).map(|length| "a".repeat(length)).collect();
        index.add(json!({"id": 1, "text": text.join(" ")})).unwrap();
        index
    };
    assert_round_trips(&nested(128), &["a", &"a".repeat(128)]);
    let too_deep = nested(129);
    for error in [
        too_deep.to_bytes().unwrap_err(),
        too_deep.to_json().unwrap_err(),
    ] {
        assert!(error.contains("128 prefixes deep"), "{error}");
    }
    // The JSON reader bounds nesting itself and never recurses into a bomb.
    let bomb = format!("{}{}", "[".repeat(200_000), "]".repeat(200_000));
    assert!(MiniSearch::from_json(&bomb)
        .unwrap_err()
        .contains("depth limit"));
}

#[test]
fn options_no_snapshot_can_hold_are_refused_when_saving() {
    let mut duplicated = engine(json!({"fields": ["title", "title"]}));
    duplicated.add(json!({"id": 1, "title": "apple"})).unwrap();
    assert!(duplicated
        .to_bytes()
        .unwrap_err()
        .contains("more than once"));
    let mut infinite = options(json!({"fields": ["title"]}));
    infinite
        .search_options
        .boost
        .insert("title".into(), f64::INFINITY);
    assert!(infinite.validate().unwrap_err().contains("finite"));
    let index = MiniSearch::new(infinite);
    assert!(index.to_bytes().unwrap_err().contains("finite"));
    assert!(index.to_json().unwrap_err().contains("finite"));
    assert!(options(json!({"fields": ["a", "a"]}))
        .validate()
        .unwrap_err()
        .contains("more than once"));
}

// Two million add/remove cycles: release builds only.
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn internal_id_churn_is_refused_when_saving_and_cured_by_compaction() {
    let mut index = engine(json!({"fields": ["text"], "autoVacuum": false}));
    let document = json!({"id": 1, "text": "apple"});
    for _ in 0..2_000_001 {
        index.add(document.clone()).unwrap();
        index.remove(&document).unwrap();
    }
    index.add(document).unwrap();
    let error = index.to_bytes().unwrap_err();
    assert!(error.contains("compact()"), "{error}");
    assert!(index.to_json().unwrap_err().contains("compact()"));
    index.compact().unwrap();
    assert_round_trips(&index, &["apple"]);
}

#[test]
fn a_rejected_snapshot_cannot_reserve_more_than_the_decode_budget() {
    // A valid prefix up to the first document id, then an id made of nested
    // arrays that each claim to hold as many values as bytes remain.
    let mut index = engine(json!({"fields": ["text"], "autoVacuum": false}));
    index.add(json!({"id": 1, "text": "apple"})).unwrap();
    let valid = index.to_bytes().unwrap();
    let id_at = valid
        .windows(2)
        .position(|pair| pair == [3, 1]) // TAG_UINT 1: the document id
        .expect("document id value");
    let mut crafted = valid[..id_at].to_vec();
    const PADDING: usize = 1 << 20;
    for level in 0..60usize {
        crafted.push(7); // TAG_ARRAY
        let mut count = (PADDING - level * 4) as u64;
        while count >= 128 {
            crafted.push((count as u8 & 0x7f) | 0x80);
            count >>= 7;
        }
        crafted.push(count as u8);
    }
    crafted.resize(crafted.len() + PADDING, 0);

    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let error = MiniSearch::from_bytes(&crafted).unwrap_err();
    assert!(error.contains("budget"), "{error}");
    // Sixty unchecked reservations of a million values each came to 2 GB; all
    // reservations together now stay within the 256 MiB decode budget. The
    // margin covers what the other tests of this binary allocate meanwhile.
    let reserved = PEAK.load(Ordering::Relaxed).saturating_sub(before);
    assert!(reserved <= 320 * 1024 * 1024, "{reserved} bytes");
}
