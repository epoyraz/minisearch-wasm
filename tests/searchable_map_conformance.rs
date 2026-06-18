use minisearch_rust::SearchableMap;

#[test]
fn fuzzy_bit_parallel_matches_dp_reference() {
    // The bit-parallel (Myers) fuzzy path in `for_each_fuzzy` must return exactly
    // the same matches and distances as the banded-DP reference (`fuzzy_get`).
    let words = [
        "software",
        "softer",
        "soften",
        "engineer",
        "engine",
        "engineering",
        "java",
        "javascript",
        "lava",
        "guava",
        "kava",
        "python",
        "pylon",
        "react",
        "reactor",
        "reaction",
        "kubernetes",
        "kubernet",
        "cloud",
        "cloudy",
        "zürich",
        "zurich",
        "münchen",
        "muenchen",
        "café",
        "cafe",
        "naïve",
        "naive",
        "straße",
        "strasse",
        "manager",
        "managed",
        "manage",
        "data",
        "date",
        "dato",
        "database",
        "developer",
        "develop",
        "devops",
    ];
    let mut map = SearchableMap::new();
    for (index, word) in words.iter().enumerate() {
        map.set(word, index);
    }

    let queries = [
        "software",
        "enginer",
        "javasript",
        "jaba",
        "reactor",
        "kubernates",
        "cloud",
        "zurich",
        "zürich",
        "munchen",
        "cafe",
        "naive",
        "strasse",
        "manger",
        "databse",
        "devloper",
        "x",
        "softwarexyz",
    ];

    for query in queries {
        for max_distance in 0..=3 {
            let mut myers: Vec<(String, usize)> = Vec::new();
            map.for_each_fuzzy(query, max_distance, |term, _value, distance| {
                myers.push((term.to_owned(), distance));
            });
            myers.sort();

            let mut reference: Vec<(String, usize)> = map
                .fuzzy_get(query, max_distance)
                .into_iter()
                .map(|hit| (hit.key, hit.distance))
                .collect();
            reference.sort();

            assert_eq!(
                myers, reference,
                "query={query:?} max_distance={max_distance}"
            );
        }
    }
}

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
