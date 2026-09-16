use minisearch_wasm::{MiniSearch, MiniSearchOptions, PartialSearchOptions, SearchOptions};
use serde_json::{json, Value};

fn engine() -> MiniSearch {
    MiniSearch::new(
        serde_json::from_value::<MiniSearchOptions>(json!({
            "fields": ["text", "optional"], "storeFields": ["payload"], "autoVacuum": false,
            "searchOptions": {"prefix": true, "fuzzy": 0.2}
        }))
        .unwrap(),
    )
}
fn snapshot(search: &MiniSearch) -> Value {
    serde_json::to_value(search).unwrap()
}
fn reloaded(search: &MiniSearch) -> Vec<MiniSearch> {
    vec![
        MiniSearch::from_bytes(&search.to_bytes().unwrap()).unwrap(),
        MiniSearch::from_json(&serde_json::to_string(search).unwrap()).unwrap(),
    ]
}

#[test]
fn field_presence_and_boundary_empty_tokens_survive_reload_and_discard() {
    let mut search = engine();
    search
        .add_all(vec![
            json!({"id":1,"text":"apple"}),
            json!({"id":2,"text":" apple "}),
            json!({"id":3,"text":null}),
            json!({"id":4}),
            json!({"id":5,"text":""}),
            json!({"id":6,"text":" ... "}),
        ])
        .unwrap();
    assert_eq!(
        snapshot(&search)["field_length"],
        json!([1, 0, 2, 0, 0, 0, 0, 0, 1, 0, 1, 0])
    );
    assert_eq!(
        snapshot(&search)["field_present"],
        json!([true, false, true, false, false, false, false, false, true, false, true, false])
    );
    let averages = snapshot(&search)["average_field_length"].clone();
    for mut index in reloaded(&search) {
        index.discard(&json!(4)).unwrap();
        index.discard(&json!(3)).unwrap();
        assert_eq!(snapshot(&index)["average_field_length"], averages);
        assert_eq!(index.search("apple", SearchOptions::default()).len(), 2);
        index.vacuum();
        assert_eq!(snapshot(&index)["average_field_length"], averages);
    }
}

#[test]
fn long_field_lengths_are_exact_when_discarded_after_reload() {
    let text = (0..70_000)
        .map(|i| format!("t{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut search = engine();
    search
        .add_all(vec![
            json!({"id":1,"text":text}),
            json!({"id":2,"text":"t0"}),
        ])
        .unwrap();
    assert_eq!(snapshot(&search)["field_length"][0], 70_000);
    for mut index in reloaded(&search) {
        index.discard(&json!(1)).unwrap();
        assert_eq!(snapshot(&index)["average_field_length"][0], 1.0);
        assert_eq!(index.search("t0", SearchOptions::default())[0].id, 2);
    }
}

#[test]
fn ordered_prefix_fuzzy_results_and_suggestions_survive_mutations_and_snapshots() {
    for words in [
        "apply application apple",
        "car cat can",
        "dog data day",
        "éclair écran école",
    ] {
        let mut search = engine();
        search
            .add_all(vec![
                json!({"id":1,"text":words}),
                json!({"id":2,"text":"apparatus carrot date"}),
                json!({"id":3,"text":"app car dog"}),
            ])
            .unwrap();
        for phase in 0..4 {
            match phase {
                1 => search
                    .remove(&json!({"id":2,"text":"apparatus carrot date"}))
                    .unwrap(),
                2 => {
                    search.discard(&json!(3)).unwrap();
                    search.add(json!({"id":4,"text":"cat app dog"})).unwrap();
                }
                3 => search.vacuum(),
                _ => {}
            }
            for restored in reloaded(&search) {
                for query in ["a", "c", "d", "é", "appl", "dag", "car app"] {
                    assert_eq!(
                        restored.search_joined_default(query, false),
                        search.search_joined_default(query, false),
                        "phase={phase}, query={query}"
                    );
                    assert_eq!(
                        restored.auto_suggest(query, None),
                        search.auto_suggest(query, None)
                    );
                }
                assert_eq!(restored.to_bytes().unwrap(), search.to_bytes().unwrap());
            }
        }
    }
}

#[test]
fn compact_ids_preserve_json_values_and_generation_changes() {
    let mut search = engine();
    let ids = vec![
        json!("a\nb"),
        json!(1),
        json!("1"),
        json!(""),
        json!(false),
        json!(["x", 1]),
        json!({"a":"b\nc"}),
    ];
    for id in &ids {
        search.add(json!({"id":id,"text":"apple"})).unwrap();
    }
    for mut index in reloaded(&search) {
        let joined: Vec<Value> =
            serde_json::from_str(&index.search_joined_default("apple", false).ids).unwrap();
        assert_eq!(joined, ids);
        let mut version = index.id_table_version();
        let raw = index.search_raw("apple", &PartialSearchOptions::default());
        assert_eq!(raw.id_table_version, version);
        let table: Vec<Value> = serde_json::from_str(&index.doc_id_table()).unwrap();
        assert_eq!(
            raw.doc_ids
                .iter()
                .map(|id| table[*id as usize].clone())
                .collect::<Vec<_>>(),
            ids
        );
        index.discard(&json!(1)).unwrap();
        assert_ne!(index.id_table_version(), version);
        assert_eq!(
            serde_json::from_str::<Value>(&index.doc_id_table()).unwrap()[1],
            Value::Null
        );
        version = index.id_table_version();
        index.add(json!({"id":1,"text":"apple"})).unwrap();
        assert_ne!(index.id_table_version(), version);
        version = index.id_table_version();
        index.remove_all_documents();
        assert_ne!(index.id_table_version(), version);
        assert_eq!(index.doc_id_table(), "[]");
        assert_eq!(index.search_joined_default("apple", false).ids, "[]");
    }
}

fn valid_snapshot() -> Value {
    let mut search = engine();
    search
        .add(json!({"id":"first", "text":"apple", "payload":{"tag": "ok"}}))
        .unwrap();
    snapshot(&search)
}

#[test]
fn json_rejects_inconsistent_state_including_direct_serde_deserialization() {
    let edits = [
        ("/snapshot_version", json!(3)),
        ("/document_count", json!(900)),
        ("/next_id", json!(0)),
        ("/next_id", json!(u32::MAX)),
        ("/dirt_count", json!(2)),
        ("/field_ids/text", json!(100)),
        ("/field_length", json!([])),
        ("/field_present", json!([])),
        ("/average_field_length", json!([])),
        ("/average_field_length/0", json!(-1)),
        ("/field_present/0", json!(false)),
        ("/id_to_short_id/s:first", json!(7)),
        ("/document_ids/0", Value::Null),
        ("/index/root/children/0/0", json!("")),
        ("/index/root/children/0/1/leaf_pos", json!(99)),
        ("/index/root/children/0/1/leaf/0/0", json!(0)),
    ];
    for (path, value) in edits {
        let mut corrupt = valid_snapshot();
        *corrupt
            .pointer_mut(path)
            .unwrap_or_else(|| panic!("missing test target {path}")) = value;
        assert!(
            MiniSearch::from_json(&corrupt.to_string()).is_err(),
            "accepted {path}"
        );
        assert!(
            serde_json::from_value::<MiniSearch>(corrupt).is_err(),
            "serde accepted {path}"
        );
    }
    let mut unknown = valid_snapshot();
    unknown["stored_fields"]["99"] = json!({"payload":1});
    assert!(MiniSearch::from_json(&unknown.to_string()).is_err());
    let mut unversioned = valid_snapshot();
    unversioned
        .as_object_mut()
        .unwrap()
        .remove("snapshot_version");
    assert!(MiniSearch::from_json(&unversioned.to_string()).is_err());
}

#[test]
fn stale_postings_require_dirt_but_valid_dirty_indexes_load() {
    let mut search = engine();
    search
        .add_all(vec![
            json!({"id":1,"text":"apple"}),
            json!({"id":2,"text":"pear"}),
        ])
        .unwrap();
    search.discard(&json!(1)).unwrap();
    assert_eq!(reloaded(&search).len(), 2);
    let mut corrupt = snapshot(&search);
    corrupt["dirt_count"] = json!(0);
    assert!(MiniSearch::from_json(&corrupt.to_string()).is_err());
}

#[test]
fn truncated_mutated_and_oversized_binary_inputs_never_panic() {
    let search = MiniSearch::from_json(&valid_snapshot().to_string()).unwrap();
    let bytes = search.to_bytes().unwrap();
    for length in 0..bytes.len() {
        assert!(
            MiniSearch::from_bytes(&bytes[..length]).is_err(),
            "prefix length {length}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(MiniSearch::from_bytes(&trailing)
        .unwrap_err()
        .contains("trailing"));
    // Every single byte is corrupted with three masks; accepted mutations must
    // still support search and another validated round trip without trapping.
    for offset in 0..bytes.len() {
        for mask in [1, 0x80, 0xff] {
            let mut corrupt = bytes.clone();
            corrupt[offset] ^= mask;
            let attempt = std::panic::catch_unwind(|| {
                if let Ok(loaded) = MiniSearch::from_bytes(&corrupt) {
                    loaded.search_joined_default("apple", false);
                    MiniSearch::from_bytes(&loaded.to_bytes().unwrap()).unwrap();
                }
            });
            assert!(attempt.is_ok(), "panic at byte {offset}, mask {mask}");
        }
    }
    for corrupt in [
        vec![0xff; 10],
        vec![0x84, 0],
        vec![4, 0xff, 0xff, 0xff, 0xff, 0x7f],
    ] {
        assert!(MiniSearch::from_bytes(&corrupt).is_err());
    }
}
