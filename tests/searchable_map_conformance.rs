use minisearch_rust::SearchableMap;

#[test]
fn set_get_has_and_delete_match_reference_behavior() {
    let mut map = SearchableMap::new();

    map.set("unicorn", 1);
    map.set("universe", 2);
    map.set("university", 3);
    map.set("unique", 4);

    assert_eq!(map.get("unicorn"), Some(&1));
    assert_eq!(map.get("missing"), None);
    assert!(map.has("unique"));
    assert!(!map.has("hello"));
    assert_eq!(map.len(), 4);

    assert!(map.delete("universe"));
    assert!(!map.has("universe"));
    assert_eq!(map.len(), 3);

    map.set("universe", 5);
    assert_eq!(map.get("universe"), Some(&5));
}

#[test]
fn prefix_search_returns_matching_entries() {
    let mut map = SearchableMap::new();
    for (index, word) in ["sum", "summer", "summertime", "sun", "hello"]
        .into_iter()
        .enumerate()
    {
        map.set(word, index);
    }

    let mut keys = map
        .prefix_entries("sum")
        .into_iter()
        .map(|(key, _)| key)
        .collect::<Vec<_>>();
    keys.sort();

    assert_eq!(keys, vec!["sum", "summer", "summertime"]);
}

#[test]
fn fuzzy_search_returns_levenshtein_matches() {
    let mut map = SearchableMap::new();
    for (index, word) in ["hello", "hell", "help", "shell", "winter"]
        .into_iter()
        .enumerate()
    {
        map.set(word, index);
    }

    let mut matches = map
        .fuzzy_get("hallo", 2)
        .into_iter()
        .map(|item| (item.key, item.distance))
        .collect::<Vec<_>>();
    matches.sort();

    assert_eq!(
        matches,
        vec![("hell".to_owned(), 2), ("hello".to_owned(), 1)]
    );
}
