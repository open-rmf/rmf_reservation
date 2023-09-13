use std::{collections::{HashMap, HashSet}, hash::Hash};

#[derive(Debug)]
pub struct UniqueMultiHashMap<K: Hash + PartialEq + Eq, V: Hash + PartialEq + Eq> {
    collections: HashMap<K, HashSet<V>>
}

impl<K,V> UniqueMultiHashMap<K, V>
    where K: Hash + PartialEq + Eq,
    V: Hash + PartialEq + Eq
      {
    pub fn new() -> Self {
        Self {
            collections: HashMap::new()
        }
    }

    pub fn insert(&mut self, key: K, value: V) {
        if let Some(bucket) = self.collections.get_mut(&key) {
            bucket.insert(value);
        }
        else {
            self.collections.insert(key, HashSet::from_iter([value]));
        }
    }

    pub fn get(&self, key: &K) -> Option<&HashSet<V>> {
        self.collections.get(key)
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, K, HashSet<V>>  {
       self.collections.iter()
    }

    pub fn len(&self) -> usize {
        self.collections.len()
    }
}
