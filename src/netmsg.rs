// SPDX-License-Identifier: MIT
use serde::{de::DeserializeOwned, Serialize};

pub trait NetMessage:
    Serialize + DeserializeOwned + Clone + Send + Sync + 'static
{
    const TYPE_PATH: &'static str;
    const WIRE_ID: u64;
}

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
