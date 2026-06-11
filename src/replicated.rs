// SPDX-License-Identifier: MIT
//! Replicated entity marker and plugin wiring.
use bevy::prelude::*;
use std::collections::HashMap;

use crate::{netres::NetResource, sync};

/// Marker component for entities that should exist on every peer.
#[derive(Component, Default)]
pub struct Replicated;

/// Internal mapping between a Bevy entity and its network identity.
#[derive(Component, Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[doc(hidden)]
pub struct NetworkId(pub u64);

/// Monotonic counter used to assign fresh network IDs on the server.
#[derive(Resource, Default)]
#[doc(hidden)]
pub struct NextNetworkId(pub u64);

/// Bidirectional index between network IDs and local entities.
#[derive(Resource, Default)]
#[doc(hidden)]
pub struct EntityIndex {
    by_network: HashMap<NetworkId, Entity>,
    by_entity: HashMap<Entity, NetworkId>,
}

impl EntityIndex {
    /// Stores the mapping in both directions.
    pub fn insert(&mut self, network_id: NetworkId, entity: Entity) {
        self.by_network.insert(network_id, entity);
        self.by_entity.insert(entity, network_id);
    }

    /// Looks up the local entity for a network ID.
    pub fn entity(&self, network_id: NetworkId) -> Option<Entity> {
        self.by_network.get(&network_id).copied()
    }

    /// Looks up the network ID for a local entity.
    pub fn network_id(&self, entity: Entity) -> Option<NetworkId> {
        self.by_entity.get(&entity).copied()
    }

    /// Removes an entity from the index and returns its previous network ID.
    pub fn remove_entity(&mut self, entity: Entity) -> Option<NetworkId> {
        let network_id = self.by_entity.remove(&entity)?;
        self.by_network.remove(&network_id);
        Some(network_id)
    }
}

/// Bevy plugin that wires replication resources and systems together.
pub struct ReplicatedPlugin;

impl Plugin for ReplicatedPlugin {
    /// Initializes replication state and installs the replication pipeline.
    fn build(&self, app: &mut App) {
        app.init_resource::<NetResource>()
            .init_resource::<NextNetworkId>()
            .init_resource::<EntityIndex>();

        sync::register_sync_components(app);

        app.add_systems(PreUpdate, sync::poll_network_incoming)
            .add_systems(
                Update,
                (
                    sync::assign_prefab_ids,
                    sync::assign_network_ids,
                    sync::replicate_removals,
                    sync::sync_new_connections,
                    sync::apply_incoming_packets,
                )
                    .chain(),
            )
            .add_systems(PostUpdate, sync::flush_network_outbox);
    }
}
