//! [`Map`]: an object's entries, in the order they were first inserted.

use std::collections::HashMap;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::Index;
use std::{slice, vec};

use super::Value;

/// Past this many entries, lookups go through a hash index rather than a scan.
const INDEX_THRESHOLD: usize = 8;

/// An object: string keys to values, in the order each key was first
/// inserted. That order is meaningful — a call's arguments resolve in it and
/// `variables` come back in it — so data formats read objects into a `Map`
/// in document order.
///
/// Re-inserting a key replaces its value and keeps its place. Two maps are
/// equal when they hold the same entries, in any order.
#[derive(Clone, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
    /// Key → position in `entries`; built once the map outgrows a scan.
    /// Boxed, since most maps never need one: a `HashMap` inline would more
    /// than double the size of a `Map`, and so of every `Value`.
    #[allow(clippy::box_collection)]
    index: Option<Box<HashMap<String, usize>>>,
}

impl Map {
    /// An empty map.
    pub fn new() -> Self {
        Map::default()
    }

    /// An empty map with room for `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Map {
            entries: Vec::with_capacity(capacity),
            index: None,
        }
    }

    /// A map of `entries`, whose keys are known to be distinct (a call's
    /// arguments): built without checking each key against the others.
    pub(crate) fn from_unique(entries: Vec<(String, Value)>) -> Self {
        let mut map = Map {
            entries,
            index: None,
        };
        if map.entries.len() > INDEX_THRESHOLD {
            map.rebuild_index();
        }
        map
    }

    /// How many entries there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: &str) -> Option<usize> {
        match &self.index {
            Some(index) => index.get(key).copied(),
            // Keys are short, mostly: comparing their bytes in place beats
            // calling `memcmp` for each.
            None => self
                .entries
                .iter()
                .position(|(k, _)| k.len() == key.len() && k.bytes().eq(key.bytes())),
        }
    }

    /// The value for `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.position(key).map(|i| &self.entries[i].1)
    }

    /// The value for `key`, to change.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.position(key).map(move |i| &mut self.entries[i].1)
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    /// Sets `key` to `value`, returning the value it replaces. A new key goes
    /// last; an existing one keeps its place.
    pub fn insert(&mut self, key: String, value: Value) -> Option<Value> {
        if let Some(i) = self.position(&key) {
            return Some(std::mem::replace(&mut self.entries[i].1, value));
        }
        self.push(key, value);
        None
    }

    /// Inserts `key` if it's new; if it isn't, changes nothing and hands
    /// both back.
    pub(crate) fn insert_new(&mut self, key: String, value: Value) -> Result<(), (String, Value)> {
        if self.position(&key).is_some() {
            return Err((key, value));
        }
        self.push(key, value);
        Ok(())
    }

    /// The value for `key`, inserting `null` first if it's missing.
    pub(crate) fn get_or_insert_null(&mut self, key: &str) -> &mut Value {
        let i = match self.position(key) {
            Some(i) => i,
            None => {
                self.push(key.to_owned(), Value::Null);
                self.entries.len() - 1
            }
        };
        &mut self.entries[i].1
    }

    /// Adds an entry last, for a key that isn't there.
    fn push(&mut self, key: String, value: Value) {
        if let Some(index) = &mut self.index {
            index.insert(key.clone(), self.entries.len());
        }
        self.entries.push((key, value));
        if self.index.is_none() && self.entries.len() > INDEX_THRESHOLD {
            self.rebuild_index();
        }
    }

    /// Removes `key`, keeping the others in order; returns its value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.position(key)?;
        let (_, value) = self.entries.remove(i);
        if let Some(index) = &mut self.index {
            // The entries after it moved up one place.
            index.remove(key);
            for position in index.values_mut().filter(|p| **p > i) {
                *position -= 1;
            }
        }
        Some(value)
    }

    /// Removes every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index = None;
    }

    fn rebuild_index(&mut self) {
        let index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (k.clone(), i))
            .collect();
        self.index = Some(Box::new(index));
    }

    /// The entries, in order.
    pub fn iter(&self) -> Iter<'_> {
        Iter(self.entries.iter())
    }

    /// The keys, in order.
    pub fn keys(&self) -> Keys<'_> {
        Keys(self.entries.iter())
    }

    /// The values, in order.
    pub fn values(&self) -> Values<'_> {
        Values(self.entries.iter())
    }
}

impl PartialEq for Map {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl fmt::Debug for Map {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl Index<&str> for Map {
    type Output = Value;

    /// The value for `key`. Panics if it's missing; use [`Map::get`] when it
    /// may be.
    fn index(&self, key: &str) -> &Value {
        match self.get(key) {
            Some(value) => value,
            None => panic!("no entry for key {key:?}"),
        }
    }
}

impl<K: Into<String>, V: Into<Value>> FromIterator<(K, V)> for Map {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Map::new();
        map.extend(iter);
        map
    }
}

impl<K: Into<String>, V: Into<Value>> Extend<(K, V)> for Map {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k.into(), v.into());
        }
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = IntoIter;

    fn into_iter(self) -> IntoIter {
        IntoIter(self.entries.into_iter())
    }
}

impl<'a> IntoIterator for &'a Map {
    type Item = (&'a String, &'a Value);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

/// A [`Map`]'s entries, in order.
#[derive(Clone, Debug)]
pub struct Iter<'a>(slice::Iter<'a, (String, Value)>);

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a String, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(k, v)| (k, v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(k, v)| (k, v))
    }
}

impl ExactSizeIterator for Iter<'_> {}
impl FusedIterator for Iter<'_> {}

/// A [`Map`]'s keys, in order.
#[derive(Clone, Debug)]
pub struct Keys<'a>(slice::Iter<'a, (String, Value)>);

impl<'a> Iterator for Keys<'a> {
    type Item = &'a String;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(k, _)| k)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for Keys<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(k, _)| k)
    }
}

impl ExactSizeIterator for Keys<'_> {}
impl FusedIterator for Keys<'_> {}

/// A [`Map`]'s values, in order.
#[derive(Clone, Debug)]
pub struct Values<'a>(slice::Iter<'a, (String, Value)>);

impl<'a> Iterator for Values<'a> {
    type Item = &'a Value;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(_, v)| v)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for Values<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back().map(|(_, v)| v)
    }
}

impl ExactSizeIterator for Values<'_> {}
impl FusedIterator for Values<'_> {}

/// A [`Map`]'s entries, owned, in order.
#[derive(Debug)]
pub struct IntoIter(vec::IntoIter<(String, Value)>);

impl Iterator for IntoIter {
    type Item = (String, Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl DoubleEndedIterator for IntoIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back()
    }
}

impl ExactSizeIterator for IntoIter {}
impl FusedIterator for IntoIter {}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(map: &Map) -> Vec<&str> {
        map.keys().map(String::as_str).collect()
    }

    #[test]
    fn keeps_insertion_order_and_replaces_in_place() {
        let mut map = Map::new();
        for key in ["z", "a", "m"] {
            map.insert(key.into(), Value::from(1));
        }
        assert_eq!(map.insert("a".into(), Value::from(2)), Some(Value::from(1)));
        assert_eq!(keys(&map), ["z", "a", "m"]);
        assert_eq!(map["a"], Value::from(2));
    }

    #[test]
    fn works_the_same_past_the_index_threshold() {
        let mut map: Map = (0..100).rev().map(|i| (format!("k{i}"), i)).collect();
        assert_eq!(map.len(), 100);
        assert_eq!(map.keys().next().map(String::as_str), Some("k99"));
        assert_eq!(map.get("k7"), Some(&Value::from(7)));
        assert_eq!(map.remove("k50"), Some(Value::from(50)));
        assert_eq!(map.get("k49"), Some(&Value::from(49)));
        assert!(!map.contains_key("k50"));
        assert_eq!(map.len(), 99);
        *map.get_or_insert_null("new") = Value::from(true);
        assert_eq!(map.keys().next_back().map(String::as_str), Some("new"));
        assert_eq!(map.get("new"), Some(&Value::from(true)));
        map.clear();
        assert!(map.is_empty() && map.get("k1").is_none());
    }

    #[test]
    fn removing_past_the_index_threshold_keeps_every_lookup_right() {
        let mut map: Map = (0..20).map(|i| (format!("k{i}"), i)).collect();
        for removed in ["k0", "k19", "k7", "k8"] {
            assert!(map.remove(removed).is_some());
            assert_eq!(map.get(removed), None);
        }
        map.insert("k7".into(), Value::from(70));
        for (i, (key, value)) in map.iter().enumerate() {
            assert_eq!(map.get(key), Some(value), "{key}");
            assert_eq!(map.position(key), Some(i), "{key}");
        }
        assert_eq!(map.len(), 17);
        assert_eq!(map["k7"], Value::from(70));
    }

    #[test]
    fn equality_ignores_order() {
        let a: Map = [("x", 1), ("y", 2)].into_iter().collect();
        let b: Map = [("y", 2), ("x", 1)].into_iter().collect();
        let c: Map = [("x", 1)].into_iter().collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
