//! What the mod keeps with the world, by key in its world store
//! (`yog_api::storage`): read as the server starts, written as the world is saved.

use yog_api::{Schema, storage};

/// What's kept at `key`; its default with nothing kept (or no world open).
pub fn load<T: Schema + Default>(key: &str) -> T {
    storage::world()
        .map(|s| s.get_or_default(key))
        .unwrap_or_default()
}

/// `value` kept at `key` (written with the world's next save).
pub fn save<T: Schema>(key: &str, value: &T) {
    if let Some(s) = storage::world() {
        s.set(key, value);
    }
}
