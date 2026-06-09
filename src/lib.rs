// SPDX-License-Identifier: MIT
//! Library entry point for `bevy-networker-multiplayer`.

pub use bincode;
pub use bevy;
pub use inventory;
pub extern crate networker_rs;
pub use serde;
pub use bevy_networker_multiplayer_macros::{netmsg, sync};

pub mod netres;
pub mod netmsg;
pub mod replicated;
pub mod sync;

pub use netres::NetResource;
pub use netmsg::NetMessage;
pub use replicated::{Replicated, ReplicatedPlugin};

/// Returns the crate name.
pub fn crate_name() -> &'static str {
    "bevy-networker-multiplayer"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_crate_name() {
        assert_eq!(crate_name(), "bevy-networker-multiplayer");
    }
}
