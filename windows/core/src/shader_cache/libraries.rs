//! Share one compiled library across state keys that emit identical MSL.
//!
//! A fixed-function key includes state the emitter can ignore, such as unused texture
//! arguments. The generated entry name contains that key's hash, so Metal sees a different
//! source even when every instruction is identical. Compare the full source with only that
//! entry name normalized, on a state-key miss. Existing disk keys and files remain valid.

use rustc_hash::FxHashMap;

#[derive(Eq, PartialEq, Hash)]
struct SourceIdentity {
    msl: String,
    entry: String,
}

impl SourceIdentity {
    fn new(msl: &str, entry: &str) -> Self {
        // Generated shaders declare exactly one entry and never refer to it elsewhere.
        // If a cache record doesn't have that shape, preserve its complete identity.
        // This also prevents replacing an entry's spelling inside another identifier.
        let declaration = format!(" {entry}(");
        if !entry.is_empty() && msl.matches(entry).count() == 1 && msl.contains(&declaration) {
            Self {
                msl: msl.replacen(&declaration, " mtld3d_shared_entry(", 1),
                entry: String::new(),
            }
        } else {
            Self {
                msl: msl.to_owned(),
                entry: entry.to_owned(),
            }
        }
    }
}

/// Per-device library ownership and its non-owning state/source indices.
///
/// `values()` visits each compilation once, so shader aliases neither leak handles nor
/// release them twice. The same cache moves from prewarm to the live encoder before draws.
pub struct CompiledShaders<T> {
    keys: FxHashMap<u64, usize>,
    sources: FxHashMap<SourceIdentity, usize>,
    libraries: Vec<T>,
    reused: usize,
}

impl<T> Default for CompiledShaders<T> {
    fn default() -> Self {
        Self {
            keys: FxHashMap::default(),
            sources: FxHashMap::default(),
            libraries: Vec::new(),
            reused: 0,
        }
    }
}

impl<T> CompiledShaders<T> {
    #[must_use]
    pub fn get(&self, key: &u64) -> Option<&T> {
        self.keys.get(key).map(|&index| &self.libraries[index])
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.libraries.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.libraries.is_empty()
    }

    #[must_use]
    pub const fn reused(&self) -> usize {
        self.reused
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.libraries.iter()
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.sources.clear();
        self.libraries.clear();
        self.reused = 0;
    }

    /// Resolve a state miss, compiling only a new source. The bool reports an actual compile.
    /// Failed compiles are not cached, and can be retried. Source comparison uses full Eq,
    /// so a hash collision cannot make distinct shader bodies share a handle.
    pub fn resolve(
        &mut self,
        key: u64,
        msl: &str,
        entry: &str,
        compile: impl FnOnce() -> Option<T>,
    ) -> Option<(&T, bool)> {
        if let Some(&index) = self.keys.get(&key) {
            return Some((&self.libraries[index], false));
        }
        let source = SourceIdentity::new(msl, entry);
        if let Some(&index) = self.sources.get(&source) {
            self.keys.insert(key, index);
            self.reused += 1;
            return Some((&self.libraries[index], false));
        }
        let library = compile()?;
        let index = self.libraries.len();
        self.libraries.push(library);
        self.keys.insert(key, index);
        self.sources.insert(source, index);
        Some((&self.libraries[index], true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shader(entry: &str, value: u8) -> String {
        format!("fragment float4 {entry}() {{ return float4({value}); }}")
    }

    #[test]
    fn different_state_names_share_one_compilation_and_owner() {
        let mut cache = CompiledShaders::default();
        assert_eq!(
            cache.resolve(1, &shader("first", 1), "first", || Some(42)),
            Some((&42, true))
        );
        assert_eq!(
            cache.resolve(2, &shader("second", 1), "second", || panic!(
                "duplicate compile"
            )),
            Some((&42, false))
        );
        assert_eq!(cache.get(&1), cache.get(&2));
        assert_eq!(cache.values().copied().collect::<Vec<_>>(), [42]);
        assert_eq!(cache.reused(), 1);
        // Moving the prewarm cache preserves source reuse for a never-before-seen state key.
        let mut live = cache;
        assert_eq!(
            live.resolve(3, &shader("third", 1), "third", || panic!(
                "warm source lost"
            )),
            Some((&42, false))
        );
        assert_eq!(live.len(), 1);
        live.clear();
        assert!(live.is_empty());
        assert_eq!(live.get(&1), None);
        assert_eq!(
            live.resolve(2, &shader("second", 1), "second", || Some(43)),
            Some((&43, true))
        );
    }

    #[test]
    fn different_instructions_and_stages_do_not_alias() {
        let mut cache = CompiledShaders::default();
        assert!(
            cache
                .resolve(1, &shader("first", 1), "first", || Some(1))
                .unwrap()
                .1
        );
        assert!(
            cache
                .resolve(2, &shader("second", 0), "second", || Some(2))
                .unwrap()
                .1
        );
        let vertex = shader("third", 1).replace("fragment", "vertex");
        assert!(cache.resolve(3, &vertex, "third", || Some(3)).unwrap().1);
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn failed_compiles_retry_and_existing_keys_skip_the_compiler() {
        let mut cache = CompiledShaders::default();
        let source = shader("entry", 1);
        assert_eq!(cache.resolve(1, &source, "entry", || None::<u8>), None);
        assert!(cache.is_empty());
        assert_eq!(
            cache.resolve(1, &source, "entry", || Some(1)),
            Some((&1, true))
        );
        assert_eq!(
            cache.resolve(1, &source, "entry", || panic!("recompiled a known key")),
            Some((&1, false))
        );
        assert_eq!(cache.reused(), 0);
    }

    #[test]
    fn entry_references_and_unknown_source_shapes_keep_exact_identity() {
        let mut cache = CompiledShaders::default();
        for (key, entry, source) in [
            (1, "entry", "fragment float4 entry() { return entry(); }"),
            (2, "other", "fragment float4 other() { return entry(); }"),
            (
                3,
                "entry",
                "fragment float4 entry_suffix() { return float4(0); }",
            ),
            (
                4,
                "other",
                "fragment float4 entry_suffix() { return float4(0); }",
            ),
        ] {
            assert!(cache.resolve(key, source, entry, || Some(key)).unwrap().1);
        }
        assert_eq!(cache.len(), 4);
    }
}
