// SPDX-License-Identifier: MIT
//! Library entry point for `bevy-networker-multiplayer`.
//!
//! This crate keeps the public API intentionally small:
//! - `NetResource` gives access to the underlying socket layer.
//! - `Replicated` marks entities that should exist on every peer.
//! - `#[sync]` marks components and resources that should replicate.
//! - `#[netmsg]` marks typed messages for request/response style traffic.
//! - Optional prediction helpers live behind the `prediction` feature.

pub use bevy;
pub use bincode;
pub use inventory;
pub extern crate networker_rs;
/// Re-export the proc-macros that power sync and messages.
pub use bevy_networker_multiplayer_macros::{PredictLinearMotion, Velocity2d, netmsg, sync};
pub use serde;

/// Typed message support and hashing helpers.
pub mod netmsg;
/// Network transport and connection management.
pub mod netres;
/// Optional client-side prediction support.
#[cfg(feature = "prediction")]
pub mod prediction;
/// Replicated entity marker and plugin wiring.
pub mod replicated;
/// Sync registration, snapshotting, and packet application.
pub mod sync;

/// Trait implemented by messages created with `#[netmsg]`.
pub use netmsg::NetMessage;
/// Resource handle for the networking layer.
pub use netres::NetResource;
/// Prediction plugin and traits, only available with the feature enabled.
#[cfg(feature = "prediction")]
pub use prediction::{LinearMotionPredictionPlugin, PredictLinearMotion, Velocity2d};
/// Marker component and plugin for replicated entities.
pub use replicated::{Replicated, ReplicatedPlugin};

/// Returns the published crate name.
pub fn crate_name() -> &'static str {
    "bevy-networker-multiplayer"
}

/// Basic sanity test for the crate identity helper.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_crate_name() {
        assert_eq!(crate_name(), "bevy-networker-multiplayer");
    }
}
