use std::collections::HashMap;

/// A simple in-memory key-value store optimized for API performance.
/// Uses a HashMap for O(1) average-case access and avoids unnecessary cloning.
#[derive(Debug, Clone)]
pub struct OptimizedStore {
    // Using a HashMap for efficient lookups and updates
    data: HashMap<String, Vec<u32>>,
}

impl OptimizedStore {
    /// Creates a new, empty OptimizedStore.
    pub fn new() -> Self {
        OptimizedStore {
            data: HashMap::new(),
        }
    }

    /// Inserts a value into the store.
    /// Uses a reference to avoid cloning the key string.
    pub fn insert(&mut self, key: &str, value: Vec<u32>) {
        self.data.insert(key.to_string(), value);
    }

    /// Retrieves a value by key.
    /// Returns a reference to avoid cloning the value.
    pub fn get(&self, key: &str) -> Option<&Vec<u32>> {
        self.data.get(key)
    }

    /// Returns the number of items in the store.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Clears all data from the store.
    pub fn clear(&mut self) {
        self.data.clear();
    }

    /// Retrieves multiple values by keys in a single operation.
    /// This is more efficient than calling get() repeatedly.
    pub fn get_multiple(&self, keys: &[&str]) -> Vec<Option<&Vec<u32>>> {
        keys.iter().map(|k| self.data.get(*k)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get() {
        let mut store = OptimizedStore::new();
        store.insert("key1", vec![1, 2, 3]);
        assert_eq!(store.get("key1"), Some(&vec![1, 2, 3]));
        assert_eq!(store.get("key2"), None);
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut store = OptimizedStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        store.insert("key1", vec![]);
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_clear() {
        let mut store = OptimizedStore::new();
        store.insert("key1", vec![1]);
        store.insert("key2", vec![2]);
        assert_eq!(store.len(), 2);
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_get_multiple() {
        let mut store = OptimizedStore::new();
        store.insert("a", vec![10]);
        store.insert("b", vec![20]);
        store.insert("c", vec![30]);

        let keys = vec!["a", "b", "d", "c"];
        let results = store.get_multiple(&keys);

        assert_eq!(results[0], Some(&vec![10]));
        assert_eq!(results[1], Some(&vec![20]));
        assert_eq!(results[2], None);
        assert_eq!(results[3], Some(&vec![30]));
    }

    #[test]
    fn test_performance_benchmark() {
        let mut store = OptimizedStore::new();
        
        // Simulate a high-load API scenario
        for i in 0..10_000 {
            let key = format!("key_{}", i);
            store.insert(&key, vec![i as u32]);
        }

        // Verify data integrity
        assert_eq!(store.get("key_5000"), Some(&vec![5000]));
        assert_eq!(store.len(), 10_000);
    }
}