//! Single entry caches. Build versions let downstream caches key on a number
//! instead of comparing whole images.

/// A single cached value whose version counts successful builds.
///
/// Downstream caches use the version as a cheap key instead of comparing values.
pub struct Memo<K, V> {
    entry: Option<(K, V)>,
    version: u64,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self {
            entry: None,
            version: 0,
        }
    }
}

impl<K: PartialEq, V> Memo<K, V> {
    /// Rebuilds when the key differs and returns whether it rebuilt.
    /// Drops the previous value before building.
    pub fn update(&mut self, key: K, build: impl FnOnce() -> V) -> bool {
        if self.entry.as_ref().is_some_and(|(old, _)| *old == key) {
            return false;
        }
        self.clear();
        let value = build();
        self.entry = Some((key, value));
        self.version += 1;
        true
    }

    /// Returns the value, building it only when the key differs.
    pub fn get_or_build(&mut self, key: K, build: impl FnOnce() -> V) -> &mut V {
        self.update(key, build);
        self.get_mut().unwrap()
    }
}

impl<K, V> Memo<K, V> {
    /// Compares in place so the owned key is built only when stale.
    /// Drops the previous value before building.
    pub fn update_by(
        &mut self,
        is_current: impl FnOnce(&K) -> bool,
        key: impl FnOnce() -> K,
        build: impl FnOnce() -> V,
    ) -> bool {
        if self.entry.as_ref().is_some_and(|(old, _)| is_current(old)) {
            return false;
        }
        let key = key();
        self.clear();
        let value = build();
        self.entry = Some((key, value));
        self.version += 1;
        true
    }

    /// The most recently built value, if any.
    pub fn get(&self) -> Option<&V> {
        self.entry.as_ref().map(|(_, value)| value)
    }

    /// The cached value for in place updates, which do not bump the version.
    pub fn get_mut(&mut self) -> Option<&mut V> {
        self.entry.as_mut().map(|(_, value)| value)
    }

    /// Returns the number of successful builds, including those before a clear.
    pub fn version(&self) -> u64 {
        self.version
    }

    fn clear(&mut self) {
        self.entry = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_control_rebuilds_and_versions() {
        let mut memo = Memo::default();
        assert!(memo.get().is_none());
        assert_eq!(memo.version(), 0);
        assert!(memo.update(1, || 10));
        assert_eq!(memo.version(), 1);
        assert!(!memo.update(1, || panic!("same key rebuilt")));
        assert_eq!(*memo.get_or_build(1, || panic!("same key rebuilt")), 10);
        assert_eq!(memo.version(), 1);
        assert!(memo.update(2, || 20));
        assert_eq!(memo.get(), Some(&20));
        assert_eq!(memo.version(), 2);
        *memo.get_mut().unwrap() = 21;
        assert_eq!(memo.get(), Some(&21));
        memo.clear();
        assert!(memo.get().is_none());
        assert_eq!(memo.version(), 2);
        assert_eq!(*memo.get_or_build(2, || 30), 30);
        assert_eq!(memo.version(), 3);
    }

    #[test]
    fn update_by_builds_the_owned_key_only_when_stale() {
        let mut memo = Memo::default();
        assert!(memo.update_by(
            |_: &String| panic!("empty memo compared"),
            || "a".into(),
            || 1
        ));
        assert!(!memo.update_by(
            |key| key == "a",
            || panic!("current key rebuilt"),
            || panic!("current value rebuilt")
        ));
        assert_eq!(memo.version(), 1);
        assert!(memo.update_by(|key| key == "b", || "b".into(), || 2));
        assert_eq!(memo.get(), Some(&2));
        assert_eq!(memo.version(), 2);
        assert!(memo.update_by(|_| false, || "b".into(), || 3));
        assert_eq!(memo.get(), Some(&3));
        assert_eq!(memo.version(), 3);
    }

    #[test]
    fn drops_old_value_before_building() {
        use std::{cell::Cell, rc::Rc};
        struct Value(Rc<Cell<bool>>);
        impl Drop for Value {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let mut memo = Memo::default();
        memo.update(1, || Value(dropped.clone()));
        memo.update(2, || {
            assert!(dropped.get());
            Value(dropped.clone())
        });
    }
}
