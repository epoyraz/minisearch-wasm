use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuzzyMatch<T> {
    pub key: String,
    pub value: T,
    pub distance: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RadixNode<T> {
    leaf: Option<T>,
    children: Vec<(String, RadixNode<T>)>,
}

impl<T> Default for RadixNode<T> {
    fn default() -> Self {
        Self {
            leaf: None,
            children: Vec::new(),
        }
    }
}

impl<T> RadixNode<T> {
    fn is_empty(&self) -> bool {
        self.leaf.is_none() && self.children.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchableMap<T> {
    root: RadixNode<T>,
}

impl<T> Default for SearchableMap<T> {
    fn default() -> Self {
        Self {
            root: RadixNode::default(),
        }
    }
}

impl<T> SearchableMap<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.root = RadixNode::default();
    }

    pub fn set(&mut self, key: &str, value: T) {
        let node = create_path(&mut self.root, key);
        node.leaf = Some(value);
    }

    pub fn fetch_with<F>(&mut self, key: &str, initial: F) -> &mut T
    where
        F: FnOnce() -> T,
    {
        let node = create_path(&mut self.root, key);

        if node.leaf.is_none() {
            node.leaf = Some(initial());
        }

        node.leaf.as_mut().expect("leaf was just inserted")
    }

    pub fn update<F>(&mut self, key: &str, updater: F)
    where
        F: FnOnce(Option<T>) -> T,
    {
        let node = create_path(&mut self.root, key);
        let current = node.leaf.take();
        node.leaf = Some(updater(current));
    }

    pub fn delete(&mut self, key: &str) -> bool {
        delete_from(&mut self.root, key)
    }

    pub fn get(&self, key: &str) -> Option<&T> {
        lookup(&self.root, key).and_then(|node| node.leaf.as_ref())
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut T> {
        lookup_mut(&mut self.root, key).and_then(|node| node.leaf.as_mut())
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn len(&self) -> usize {
        count_entries(&self.root)
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_empty()
    }

    pub fn for_each_prefix<F>(&self, prefix: &str, mut visitor: F)
    where
        F: FnMut(&str, &T),
    {
        let mut key = String::new();
        visit_prefix(&self.root, prefix, &mut key, &mut visitor);
    }

    pub fn for_each_fuzzy<F>(&self, query: &str, max_distance: usize, mut visitor: F)
    where
        F: FnMut(&str, &T, usize),
    {
        let query_chars: Vec<char> = query.chars().collect();
        let columns = query_chars.len() + 1;
        let rows = columns + max_distance;
        let sentinel = (max_distance + 1).min(u16::MAX as usize) as u16;
        let mut matrix = vec![sentinel; rows * columns];

        for j in 0..columns {
            matrix[j] = j as u16;
        }

        for i in 1..rows {
            matrix[i * columns] = i as u16;
        }

        let mut key = String::new();
        fuzzy_visit(
            &self.root,
            &query_chars,
            max_distance,
            &mut matrix,
            1,
            columns,
            &mut key,
            &mut visitor,
        );
    }
}

impl<T: Clone> SearchableMap<T> {
    pub fn entries(&self) -> Vec<(String, T)> {
        let mut entries = Vec::new();
        collect_entries(&self.root, String::new(), &mut entries);
        entries
    }

    pub fn keys(&self) -> Vec<String> {
        self.entries().into_iter().map(|(key, _)| key).collect()
    }

    pub fn values(&self) -> Vec<T> {
        self.entries().into_iter().map(|(_, value)| value).collect()
    }

    pub fn prefix_entries(&self, prefix: &str) -> Vec<(String, T)> {
        let mut entries = Vec::new();
        collect_prefix(&self.root, prefix, String::new(), &mut entries);
        entries
    }

    pub fn fuzzy_get(&self, query: &str, max_distance: usize) -> Vec<FuzzyMatch<T>> {
        let query_chars: Vec<char> = query.chars().collect();
        let columns = query_chars.len() + 1;
        let rows = columns + max_distance;
        let sentinel = (max_distance + 1).min(u16::MAX as usize) as u16;
        let mut matrix = vec![sentinel; rows * columns];

        for j in 0..columns {
            matrix[j] = j as u16;
        }

        for i in 1..rows {
            matrix[i * columns] = i as u16;
        }

        let mut results = Vec::new();
        fuzzy_recurse(
            &self.root,
            &query_chars,
            max_distance,
            &mut matrix,
            1,
            columns,
            String::new(),
            &mut results,
        );

        results
    }
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    let mut len = 0;

    for (a, b) in left.chars().zip(right.chars()) {
        if a != b {
            break;
        }

        len += a.len_utf8();
    }

    len
}

fn create_path<'a, T>(node: &'a mut RadixNode<T>, key: &str) -> &'a mut RadixNode<T> {
    if key.is_empty() {
        return node;
    }

    for index in 0..node.children.len() {
        let child_key = node.children[index].0.clone();
        let offset = common_prefix_len(key, &child_key);

        if offset == 0 {
            continue;
        }

        if offset == child_key.len() {
            return create_path(&mut node.children[index].1, &key[offset..]);
        }

        let child = std::mem::take(&mut node.children[index].1);
        let shared = child_key[..offset].to_owned();
        let existing_suffix = child_key[offset..].to_owned();
        let intermediate = RadixNode {
            leaf: None,
            children: vec![(existing_suffix, child)],
        };

        node.children[index] = (shared, intermediate);
        return create_path(&mut node.children[index].1, &key[offset..]);
    }

    node.children.push((key.to_owned(), RadixNode::default()));
    let last = node.children.len() - 1;
    &mut node.children[last].1
}

fn lookup<'a, T>(node: &'a RadixNode<T>, key: &str) -> Option<&'a RadixNode<T>> {
    if key.is_empty() {
        return Some(node);
    }

    for (child_key, child) in &node.children {
        if key.starts_with(child_key) {
            return lookup(child, &key[child_key.len()..]);
        }
    }

    None
}

fn lookup_mut<'a, T>(node: &'a mut RadixNode<T>, key: &str) -> Option<&'a mut RadixNode<T>> {
    if key.is_empty() {
        return Some(node);
    }

    for (child_key, child) in &mut node.children {
        if key.starts_with(child_key.as_str()) {
            return lookup_mut(child, &key[child_key.len()..]);
        }
    }

    None
}

fn delete_from<T>(node: &mut RadixNode<T>, key: &str) -> bool {
    if key.is_empty() {
        return node.leaf.take().is_some();
    }

    for index in 0..node.children.len() {
        let child_key = node.children[index].0.clone();

        if !key.starts_with(&child_key) {
            continue;
        }

        let deleted = delete_from(&mut node.children[index].1, &key[child_key.len()..]);
        if deleted {
            compact_child(node, index);
        }

        return deleted;
    }

    false
}

fn compact_child<T>(node: &mut RadixNode<T>, index: usize) {
    if node.children[index].1.is_empty() {
        node.children.remove(index);
        return;
    }

    if node.children[index].1.leaf.is_none() && node.children[index].1.children.len() == 1 {
        let (suffix, grandchild) = node.children[index].1.children.remove(0);
        node.children[index].0.push_str(&suffix);
        node.children[index].1 = grandchild;
    }
}

fn collect_entries<T: Clone>(node: &RadixNode<T>, prefix: String, entries: &mut Vec<(String, T)>) {
    if let Some(value) = &node.leaf {
        entries.push((prefix.clone(), value.clone()));
    }

    for (child_key, child) in &node.children {
        let mut key = prefix.clone();
        key.push_str(child_key);
        collect_entries(child, key, entries);
    }
}

fn count_entries<T>(node: &RadixNode<T>) -> usize {
    usize::from(node.leaf.is_some())
        + node
            .children
            .iter()
            .map(|(_, child)| count_entries(child))
            .sum::<usize>()
}

fn visit_entries<T, F>(node: &RadixNode<T>, prefix: &mut String, visitor: &mut F)
where
    F: FnMut(&str, &T),
{
    if let Some(value) = &node.leaf {
        visitor(prefix, value);
    }

    for (child_key, child) in &node.children {
        let prefix_len = prefix.len();
        prefix.push_str(child_key);
        visit_entries(child, prefix, visitor);
        prefix.truncate(prefix_len);
    }
}

fn visit_prefix<T, F>(node: &RadixNode<T>, remaining: &str, prefix: &mut String, visitor: &mut F)
where
    F: FnMut(&str, &T),
{
    if remaining.is_empty() {
        visit_entries(node, prefix, visitor);
        return;
    }

    for (child_key, child) in &node.children {
        if remaining.starts_with(child_key) {
            let prefix_len = prefix.len();
            prefix.push_str(child_key);
            visit_prefix(child, &remaining[child_key.len()..], prefix, visitor);
            prefix.truncate(prefix_len);
            return;
        }

        if child_key.starts_with(remaining) {
            let prefix_len = prefix.len();
            prefix.push_str(child_key);
            visit_entries(child, prefix, visitor);
            prefix.truncate(prefix_len);
            return;
        }
    }
}

fn collect_prefix<T: Clone>(
    node: &RadixNode<T>,
    remaining: &str,
    prefix: String,
    entries: &mut Vec<(String, T)>,
) {
    if remaining.is_empty() {
        collect_entries(node, prefix, entries);
        return;
    }

    for (child_key, child) in &node.children {
        if remaining.starts_with(child_key) {
            let mut next_prefix = prefix.clone();
            next_prefix.push_str(child_key);
            collect_prefix(child, &remaining[child_key.len()..], next_prefix, entries);
            return;
        }

        if child_key.starts_with(remaining) {
            let mut next_prefix = prefix.clone();
            next_prefix.push_str(child_key);
            collect_entries(child, next_prefix, entries);
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fuzzy_recurse<T: Clone>(
    node: &RadixNode<T>,
    query: &[char],
    max_distance: usize,
    matrix: &mut [u16],
    row: usize,
    columns: usize,
    prefix: String,
    results: &mut Vec<FuzzyMatch<T>>,
) {
    let offset = row * columns;

    if let Some(value) = &node.leaf {
        let distance = matrix[offset - 1];
        if distance <= max_distance as u16 {
            results.push(FuzzyMatch {
                key: prefix.clone(),
                value: value.clone(),
                distance: distance as usize,
            });
        }
    }

    for (child_key, child) in &node.children {
        let mut i = row;
        let mut skipped = false;

        for character in child_key.chars() {
            if i >= columns + max_distance {
                skipped = true;
                break;
            }

            let this_row_offset = columns * i;
            let prev_row_offset = this_row_offset - columns;
            let mut min_distance = matrix[this_row_offset];
            let jmin = i.saturating_sub(max_distance + 1);
            let jmax = std::cmp::min(columns - 1, i + max_distance);

            for j in jmin..jmax {
                let different = u16::from(character != query[j]);
                let replacement = matrix[prev_row_offset + j] + different;
                let deletion = matrix[prev_row_offset + j + 1] + 1;
                let insertion = matrix[this_row_offset + j] + 1;
                let distance = replacement.min(deletion).min(insertion);

                matrix[this_row_offset + j + 1] = distance;
                min_distance = min_distance.min(distance);
            }

            if min_distance > max_distance as u16 {
                skipped = true;
                break;
            }

            i += 1;
        }

        if !skipped {
            let mut next_prefix = prefix.clone();
            next_prefix.push_str(child_key);
            fuzzy_recurse(
                child,
                query,
                max_distance,
                matrix,
                i,
                columns,
                next_prefix,
                results,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fuzzy_visit<T, F>(
    node: &RadixNode<T>,
    query: &[char],
    max_distance: usize,
    matrix: &mut [u16],
    row: usize,
    columns: usize,
    prefix: &mut String,
    visitor: &mut F,
) where
    F: FnMut(&str, &T, usize),
{
    let offset = row * columns;

    if let Some(value) = &node.leaf {
        let distance = matrix[offset - 1];
        if distance <= max_distance as u16 {
            visitor(prefix, value, distance as usize);
        }
    }

    for (child_key, child) in &node.children {
        let mut i = row;
        let mut skipped = false;

        for character in child_key.chars() {
            if i >= columns + max_distance {
                skipped = true;
                break;
            }

            let this_row_offset = columns * i;
            let prev_row_offset = this_row_offset - columns;
            let mut min_distance = matrix[this_row_offset];
            let jmin = i.saturating_sub(max_distance + 1);
            let jmax = std::cmp::min(columns - 1, i + max_distance);

            for j in jmin..jmax {
                let different = u16::from(character != query[j]);
                let replacement = matrix[prev_row_offset + j] + different;
                let deletion = matrix[prev_row_offset + j + 1] + 1;
                let insertion = matrix[this_row_offset + j] + 1;
                let distance = replacement.min(deletion).min(insertion);

                matrix[this_row_offset + j + 1] = distance;
                min_distance = min_distance.min(distance);
            }

            if min_distance > max_distance as u16 {
                skipped = true;
                break;
            }

            i += 1;
        }

        if !skipped {
            let prefix_len = prefix.len();
            prefix.push_str(child_key);
            fuzzy_visit(
                child,
                query,
                max_distance,
                matrix,
                i,
                columns,
                prefix,
                visitor,
            );
            prefix.truncate(prefix_len);
        }
    }
}
