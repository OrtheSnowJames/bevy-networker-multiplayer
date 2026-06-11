// SPDX-License-Identifier: MIT
use serde::{Serialize, de::DeserializeOwned};

/// Trait implemented by typed network messages.
///
/// `#[netmsg]` generates this automatically for concrete message structs.
pub trait NetMessage: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    /// Fully qualified type path used to produce a stable wire identifier.
    const TYPE_PATH: &'static str;
    /// Stable wire identifier derived from `TYPE_PATH`.
    const WIRE_ID: u64;
}

/// FNV-1a hash used to derive wire identifiers from type paths.
pub const fn hash_type_path(type_path: &str) -> u64 {
    let bytes = type_path.as_bytes();
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        index += 1;
    }
    hash
}
