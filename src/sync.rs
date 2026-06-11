// SPDX-License-Identifier: MIT
//! Sync registration, snapshots, and packet application.
//!
//! This module is the bridge between Bevy ECS state and the wire format used by
//! `NetResource`. Attribute macros submit metadata into `inventory`; this module
//! collects that metadata, registers systems, and translates packets in both
//! directions.
use bevy::prelude::*;
use bincode::config;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;

use crate::{
    netres::{NetResource, ReplicationPacket},
    replicated::{EntityIndex, NetworkId, NextNetworkId, Replicated},
};

/// Metadata for a syncable component type.
#[derive(Debug, Clone, Copy)]
pub struct ComponentRegistration {
    /// Stable type path for diagnostics and registry lookup.
    pub type_path: &'static str,
    /// Stable wire identifier for the component type.
    pub wire_id: u64,
    /// Registration callback that installs Bevy systems.
    pub register: fn(&mut App),
    /// Applies a decoded component update to an entity.
    pub apply: fn(&mut World, Entity, &[u8]),
    /// Produces full-state snapshots for late joiners.
    pub snapshot: fn(&mut World) -> Vec<ReplicationPacket>,
}

// `inventory` collection of all component registrations.
inventory::collect!(ComponentRegistration);

/// Metadata for a syncable resource type.
#[derive(Debug, Clone, Copy)]
pub struct ResourceRegistration {
    /// Stable type path for diagnostics and registry lookup.
    pub type_path: &'static str,
    /// Stable wire identifier for the resource type.
    pub wire_id: u64,
    /// Registration callback that installs Bevy systems.
    pub register: fn(&mut App),
    /// Applies a decoded resource update to the world.
    pub apply: fn(&mut World, &[u8]),
    /// Produces a snapshot packet for the resource.
    pub snapshot: fn(&mut World) -> Vec<ReplicationPacket>,
}

// `inventory` collection of all resource registrations.
inventory::collect!(ResourceRegistration);

/// Metadata for a prefab definition used to spawn visuals remotely.
#[derive(Debug, Clone, Copy)]
pub struct PrefabRegistration {
    /// Stable type path for diagnostics and registry lookup.
    pub type_path: &'static str,
    /// Stable wire identifier for the prefab.
    pub wire_id: u64,
    /// Registration callback that can install any companion systems.
    pub register: fn(&mut App),
    /// Returns true when an entity should be tagged with this prefab.
    pub matches: fn(&World, Entity) -> bool,
    /// Applies the prefab's visual or structural components.
    pub apply: fn(&mut World, Entity),
}

// `inventory` collection of all prefab registrations.
inventory::collect!(PrefabRegistration);

/// Runtime registry for component sync handlers.
#[derive(Resource, Default)]
pub struct SyncRegistry {
    by_wire_id: HashMap<u64, ComponentRegistration>,
    by_type_path: HashMap<&'static str, ComponentRegistration>,
}

impl SyncRegistry {
    /// Registers one component handler.
    pub fn register(&mut self, registration: ComponentRegistration) {
        self.by_wire_id.insert(registration.wire_id, registration);
        self.by_type_path
            .insert(registration.type_path, registration);
    }

    /// Looks up a component handler by wire ID.
    pub fn by_wire_id(&self, wire_id: u64) -> Option<&ComponentRegistration> {
        self.by_wire_id.get(&wire_id)
    }
}

/// Runtime registry for resource sync handlers.
#[derive(Resource, Default)]
pub struct SyncResourceRegistry {
    by_wire_id: HashMap<u64, ResourceRegistration>,
    by_type_path: HashMap<&'static str, ResourceRegistration>,
}

impl SyncResourceRegistry {
    /// Registers one resource handler.
    pub fn register(&mut self, registration: ResourceRegistration) {
        self.by_wire_id.insert(registration.wire_id, registration);
        self.by_type_path
            .insert(registration.type_path, registration);
    }

    /// Looks up a resource handler by wire ID.
    pub fn by_wire_id(&self, wire_id: u64) -> Option<&ResourceRegistration> {
        self.by_wire_id.get(&wire_id)
    }
}

/// Trait implemented by syncable components.
pub trait SyncComponent:
    Component + Serialize + DeserializeOwned + Clone + Send + Sync + 'static
{
    /// Fully qualified type path.
    const TYPE_PATH: &'static str;
    /// Stable wire identifier.
    const WIRE_ID: u64;
}

/// Trait implemented by syncable resources.
pub trait SyncResource:
    Resource + Serialize + DeserializeOwned + Clone + Send + Sync + 'static
{
    /// Fully qualified type path.
    const TYPE_PATH: &'static str;
    /// Stable wire identifier.
    const WIRE_ID: u64;
}

/// Internal component used to remember which prefab a replicated entity uses.
#[derive(Component, Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[doc(hidden)]
pub struct PrefabId(pub u64);

/// Runtime registry for prefab handlers.
#[derive(Resource, Default)]
pub struct PrefabRegistry {
    by_wire_id: HashMap<u64, PrefabRegistration>,
    by_type_path: HashMap<&'static str, PrefabRegistration>,
}

impl PrefabRegistry {
    /// Registers one prefab handler.
    pub fn register(&mut self, registration: PrefabRegistration) {
        self.by_wire_id.insert(registration.wire_id, registration);
        self.by_type_path
            .insert(registration.type_path, registration);
    }

    /// Looks up a prefab handler by wire ID.
    pub fn by_wire_id(&self, wire_id: u64) -> Option<&PrefabRegistration> {
        self.by_wire_id.get(&wire_id)
    }

    /// Iterates over all prefab registrations.
    pub fn all(&self) -> impl Iterator<Item = &PrefabRegistration> {
        self.by_wire_id.values()
    }
}

/// Component packets that arrived before their entity spawn packet.
#[derive(Resource, Default)]
struct PendingComponentUpdates(Vec<ReplicationPacket>);

/// Sequence tracking for lossy component update streams.
#[derive(Debug, Resource)]
#[doc(hidden)]
pub struct ComponentUpdateSequenceState {
    next_outgoing: u64,
    latest_incoming: HashMap<(u64, u64), u64>,
}

impl Default for ComponentUpdateSequenceState {
    fn default() -> Self {
        Self {
            next_outgoing: 1,
            latest_incoming: HashMap::new(),
        }
    }
}

impl ComponentUpdateSequenceState {
    fn next_outgoing(&mut self) -> u64 {
        let sequence = self.next_outgoing;
        self.next_outgoing = self.next_outgoing.saturating_add(1);
        sequence
    }

    fn accept_incoming(&mut self, network_id: u64, component_wire_id: u64, sequence: u64) -> bool {
        let key = (network_id, component_wire_id);
        if self
            .latest_incoming
            .get(&key)
            .is_some_and(|latest| *latest >= sequence)
        {
            return false;
        }

        self.latest_incoming.insert(key, sequence);
        true
    }

    fn forget_network_id(&mut self, network_id: u64) {
        self.latest_incoming
            .retain(|(stored_network_id, _), _| *stored_network_id != network_id);
    }
}

/// Runtime options for syncing a resource.
#[derive(Clone, Copy, Debug)]
pub struct SyncResourceSettings {
    /// Minimum seconds between sends for this resource type.
    ///
    /// When a resource changes faster than this interval, the latest serialized
    /// value is coalesced and sent once the interval has elapsed.
    pub min_interval_seconds: f32,
    /// Optional unchanged-state heartbeat.
    ///
    /// This is useful for lossy state streams. Reliable resources generally do
    /// not need a heartbeat.
    pub heartbeat_seconds: Option<f32>,
}

impl Default for SyncResourceSettings {
    fn default() -> Self {
        Self {
            min_interval_seconds: 0.0,
            heartbeat_seconds: None,
        }
    }
}

/// Per-system state used to dedupe and coalesce resource snapshots.
#[derive(Debug)]
pub struct SyncResourceSendState {
    last_sent_bytes: Option<Vec<u8>>,
    pending_bytes: Option<Vec<u8>>,
    seconds_since_send: f32,
}

impl Default for SyncResourceSendState {
    fn default() -> Self {
        Self {
            last_sent_bytes: None,
            pending_bytes: None,
            seconds_since_send: f32::INFINITY,
        }
    }
}

/// FNV-1a hash used to derive wire IDs from type paths.
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

/// Collects all inventory registrations and stores them in runtime registries.
pub fn register_sync_components(app: &mut App) {
    app.init_resource::<SyncRegistry>();
    app.init_resource::<SyncResourceRegistry>();
    app.init_resource::<PrefabRegistry>();
    app.init_resource::<PendingComponentUpdates>();
    app.init_resource::<ComponentUpdateSequenceState>();

    let mut registry = SyncRegistry::default();
    for registration in inventory::iter::<ComponentRegistration> {
        registry.register(*registration);
        (registration.register)(app);
    }

    let mut resource_registry = SyncResourceRegistry::default();
    for registration in inventory::iter::<ResourceRegistration> {
        resource_registry.register(*registration);
        (registration.register)(app);
    }

    let mut prefab_registry = PrefabRegistry::default();
    for registration in inventory::iter::<PrefabRegistration> {
        prefab_registry.register(*registration);
        (registration.register)(app);
    }

    app.insert_resource(registry);
    app.insert_resource(resource_registry);
    app.insert_resource(prefab_registry);
}

/// Returns the next outgoing component update sequence.
#[doc(hidden)]
pub fn next_component_update_sequence(world: &mut World) -> u64 {
    world
        .resource_mut::<ComponentUpdateSequenceState>()
        .next_outgoing()
}

/// Poll hook run before the main replication systems.
pub fn poll_network_incoming(mut net: ResMut<NetResource>) {
    net.poll_incoming();
}

/// Flushes queued packets after the frame has finished mutating state.
pub fn flush_network_outbox(mut net: ResMut<NetResource>) {
    net.flush_outbox();
}

/// Sends updated sync components for entities that are added or changed.
pub fn sync_component<T: SyncComponent>(
    mut net: ResMut<NetResource>,
    mut sequence_state: ResMut<ComponentUpdateSequenceState>,
    query: Query<(&NetworkId, &T), (With<Replicated>, Or<(Added<T>, Changed<T>)>)>,
) {
    if !net.is_server() {
        return;
    }

    for (network_id, component) in &query {
        let bytes = bincode::serde::encode_to_vec(component, config::standard())
            .expect("failed to serialize sync component");
        net.queue_packet(ReplicationPacket::UpdateComponent {
            network_id: network_id.0,
            component_wire_id: T::WIRE_ID,
            sequence: sequence_state.next_outgoing(),
            bytes,
        });
    }
}

/// Sends updated resources when they are added or changed.
pub fn sync_resource<T: SyncResource>(
    time: Res<Time>,
    mut net: ResMut<NetResource>,
    resource: Option<Res<T>>,
    mut state: Local<SyncResourceSendState>,
) {
    sync_resource_with_settings::<T>(
        &time,
        &mut net,
        resource,
        &mut state,
        SyncResourceSettings::default(),
    );
}

/// Sends updated resources with byte-level dedupe and optional send coalescing.
pub fn sync_resource_with_settings<T: SyncResource>(
    time: &Time,
    net: &mut NetResource,
    resource: Option<Res<T>>,
    state: &mut SyncResourceSendState,
    settings: SyncResourceSettings,
) {
    let Some(resource) = resource else {
        return;
    };

    if !net.is_server() {
        return;
    }

    state.seconds_since_send += time.delta_secs();

    if resource.is_added() || resource.is_changed() {
        let bytes = bincode::serde::encode_to_vec(&*resource, config::standard())
            .expect("failed to serialize sync resource");

        if state.last_sent_bytes.as_ref() != Some(&bytes) {
            state.pending_bytes = Some(bytes);
        }
    }

    let heartbeat_due = settings
        .heartbeat_seconds
        .map(|seconds| state.seconds_since_send >= seconds.max(0.0))
        .unwrap_or(false);

    if state.pending_bytes.is_none() && heartbeat_due {
        state.pending_bytes = state.last_sent_bytes.clone().or_else(|| {
            Some(
                bincode::serde::encode_to_vec(&*resource, config::standard())
                    .expect("failed to serialize sync resource"),
            )
        });
    }

    let interval_ready = state.seconds_since_send >= settings.min_interval_seconds.max(0.0);
    if state.pending_bytes.is_none() || (!interval_ready && !heartbeat_due) {
        return;
    }

    let bytes = state
        .pending_bytes
        .take()
        .expect("pending resource bytes should exist");
    net.queue_packet(ReplicationPacket::UpdateResource {
        resource_wire_id: T::WIRE_ID,
        bytes: bytes.clone(),
    });
    state.last_sent_bytes = Some(bytes);
    state.seconds_since_send = 0.0;
}

/// Applies a resource snapshot only when the serialized value actually changed.
pub fn apply_resource_update<T: SyncResource>(world: &mut World, bytes: &[u8]) {
    if let Some(existing) = world.get_resource::<T>() {
        let existing_bytes = bincode::serde::encode_to_vec(existing, config::standard())
            .expect("failed to serialize existing sync resource");
        if existing_bytes == bytes {
            return;
        }
    }

    let (resource, _): (T, usize) = bincode::serde::decode_from_slice(bytes, config::standard())
        .expect("failed to deserialize sync resource");
    world.insert_resource(resource);
}

/// Sends the initial world state to newly connected clients.
pub fn sync_new_connections(world: &mut World) {
    let is_server = world.resource::<NetResource>().is_server();
    if !is_server {
        return;
    }

    let connections = {
        let mut net = world.resource_mut::<NetResource>();
        net.drain_new_connections()
    };

    if connections.is_empty() {
        return;
    }

    let component_registrations: Vec<ComponentRegistration> =
        inventory::iter::<ComponentRegistration>()
            .copied()
            .collect();
    let resource_registrations: Vec<ResourceRegistration> =
        inventory::iter::<ResourceRegistration>().copied().collect();

    for socket in &connections {
        let replicated_entities = {
            let mut query =
                world.query_filtered::<(Entity, &NetworkId, Option<&PrefabId>), With<Replicated>>();
            query
                .iter(world)
                .map(|(entity, network_id, prefab_id)| {
                    (
                        entity,
                        *network_id,
                        prefab_id.map(|prefab_id| prefab_id.0).unwrap_or(0),
                    )
                })
                .collect::<Vec<_>>()
        };

        let component_snapshots: Vec<Vec<ReplicationPacket>> = component_registrations
            .iter()
            .map(|registration| (registration.snapshot)(world))
            .collect();
        let resource_snapshots: Vec<Vec<ReplicationPacket>> = resource_registrations
            .iter()
            .map(|registration| (registration.snapshot)(world))
            .collect();

        {
            let net = world.resource::<NetResource>();
            for (_, network_id, prefab_wire_id) in &replicated_entities {
                net.send_packet_to(
                    socket,
                    ReplicationPacket::SpawnEntity {
                        network_id: network_id.0,
                        prefab_wire_id: *prefab_wire_id,
                    },
                );
            }

            for packets in component_snapshots
                .into_iter()
                .chain(resource_snapshots.into_iter())
            {
                for packet in packets {
                    net.send_packet_to(socket, packet);
                }
            }
        };
    }
}

/// Applies queued incoming replication packets to the local world.
pub fn apply_incoming_packets(world: &mut World) {
    let mut packets = {
        let mut pending = world.resource_mut::<PendingComponentUpdates>();
        pending.0.drain(..).collect::<Vec<_>>()
    };

    packets.extend({
        let mut net = world.resource_mut::<NetResource>();
        net.drain_inbox()
    });

    if packets.is_empty() {
        return;
    }

    packets.sort_by_key(|packet| match packet {
        ReplicationPacket::SpawnEntity { .. } => 0,
        ReplicationPacket::UpdateComponent { .. } | ReplicationPacket::UpdateResource { .. } => 1,
        ReplicationPacket::DespawnEntity { .. } => 2,
    });

    let mut deferred = Vec::new();

    for packet in packets {
        match packet {
            ReplicationPacket::SpawnEntity {
                network_id,
                prefab_wire_id,
            } => {
                let entity = world
                    .spawn_empty()
                    .insert(Replicated)
                    .insert(NetworkId(network_id))
                    .id();
                world
                    .resource_mut::<EntityIndex>()
                    .insert(NetworkId(network_id), entity);
                if prefab_wire_id != 0 {
                    if let Some(registration) = world
                        .resource::<PrefabRegistry>()
                        .by_wire_id(prefab_wire_id)
                        .copied()
                    {
                        (registration.apply)(world, entity);
                        world.entity_mut(entity).insert(PrefabId(prefab_wire_id));
                    }
                }
            }
            ReplicationPacket::DespawnEntity { network_id } => {
                let entity = world
                    .resource::<EntityIndex>()
                    .entity(NetworkId(network_id));
                if let Some(entity) = entity {
                    world.despawn(entity);
                    world.resource_mut::<EntityIndex>().remove_entity(entity);
                    world
                        .resource_mut::<ComponentUpdateSequenceState>()
                        .forget_network_id(network_id);
                }
            }
            ReplicationPacket::UpdateComponent {
                network_id,
                component_wire_id,
                sequence,
                bytes,
            } => {
                let entity = world
                    .resource::<EntityIndex>()
                    .entity(NetworkId(network_id));
                let registration = {
                    world
                        .resource::<SyncRegistry>()
                        .by_wire_id(component_wire_id)
                        .copied()
                };

                match (entity, registration) {
                    (Some(entity), Some(registration)) => {
                        let is_fresh = world
                            .resource_mut::<ComponentUpdateSequenceState>()
                            .accept_incoming(network_id, component_wire_id, sequence);
                        if is_fresh {
                            (registration.apply)(world, entity, &bytes);
                        }
                    }
                    (None, Some(_)) => {
                        deferred.push(ReplicationPacket::UpdateComponent {
                            network_id,
                            component_wire_id,
                            sequence,
                            bytes,
                        });
                    }
                    _ => {}
                }
            }
            ReplicationPacket::UpdateResource {
                resource_wire_id,
                bytes,
            } => {
                let registration = {
                    world
                        .resource::<SyncResourceRegistry>()
                        .by_wire_id(resource_wire_id)
                        .copied()
                };

                if let Some(registration) = registration {
                    (registration.apply)(world, &bytes);
                }
            }
        }
    }

    if !deferred.is_empty() {
        world
            .resource_mut::<PendingComponentUpdates>()
            .0
            .extend(deferred);
    }
}

/// Assigns network IDs to newly replicated entities on the server.
pub fn assign_network_ids(world: &mut World) {
    let is_server = world.resource::<NetResource>().is_server();
    if !is_server {
        return;
    }

    let entities = {
        let mut query = world.query_filtered::<Entity, Added<Replicated>>();
        query.iter(world).collect::<Vec<_>>()
    };

    for entity in entities {
        let network_id = {
            let mut next_id = world.resource_mut::<NextNetworkId>();
            let network_id = NetworkId(next_id.0);
            next_id.0 = next_id.0.saturating_add(1);
            network_id
        };

        world.entity_mut(entity).insert(network_id);
        world
            .resource_mut::<EntityIndex>()
            .insert(network_id, entity);
        let prefab_wire_id = world
            .entity(entity)
            .get::<PrefabId>()
            .map(|prefab_id| prefab_id.0)
            .unwrap_or(0);
        world
            .resource_mut::<NetResource>()
            .queue_packet(ReplicationPacket::SpawnEntity {
                network_id: network_id.0,
                prefab_wire_id,
            });
    }
}

/// Detects prefab matches on newly replicated entities.
pub fn assign_prefab_ids(world: &mut World) {
    let entities = {
        let mut query = world.query_filtered::<Entity, Added<Replicated>>();
        query.iter(world).collect::<Vec<_>>()
    };

    let registrations: Vec<PrefabRegistration> =
        inventory::iter::<PrefabRegistration>().copied().collect();

    for entity in entities {
        if world.entity(entity).contains::<PrefabId>() {
            continue;
        }

        for registration in &registrations {
            if (registration.matches)(world, entity) {
                world
                    .entity_mut(entity)
                    .insert(PrefabId(registration.wire_id));
                break;
            }
        }
    }
}

/// Converts despawned replicated entities into network despawn packets.
pub fn replicate_removals(
    mut removed: RemovedComponents<Replicated>,
    mut net: ResMut<NetResource>,
    mut index: ResMut<EntityIndex>,
) {
    if !net.is_server() {
        return;
    }

    for entity in removed.read() {
        if let Some(network_id) = index.remove_entity(entity) {
            net.queue_packet(ReplicationPacket::DespawnEntity {
                network_id: network_id.0,
            });
        }
    }
}
