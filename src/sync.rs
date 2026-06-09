// SPDX-License-Identifier: MIT
use bevy::prelude::*;
use bincode::config;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;

use crate::{
    netres::{NetResource, ReplicationPacket},
    replicated::{EntityIndex, NetworkId, NextNetworkId, Replicated},
};

#[derive(Debug, Clone, Copy)]
pub struct ComponentRegistration {
    pub type_path: &'static str,
    pub wire_id: u64,
    pub register: fn(&mut App),
    pub apply: fn(&mut World, Entity, &[u8]),
    pub snapshot: fn(&mut World) -> Vec<ReplicationPacket>,
}

inventory::collect!(ComponentRegistration);

#[derive(Debug, Clone, Copy)]
pub struct ResourceRegistration {
    pub type_path: &'static str,
    pub wire_id: u64,
    pub register: fn(&mut App),
    pub apply: fn(&mut World, &[u8]),
    pub snapshot: fn(&mut World) -> Vec<ReplicationPacket>,
}

inventory::collect!(ResourceRegistration);

#[derive(Debug, Clone, Copy)]
pub struct PrefabRegistration {
    pub type_path: &'static str,
    pub wire_id: u64,
    pub register: fn(&mut App),
    pub matches: fn(&World, Entity) -> bool,
    pub apply: fn(&mut World, Entity),
}

inventory::collect!(PrefabRegistration);

#[derive(Resource, Default)]
pub struct SyncRegistry {
    by_wire_id: HashMap<u64, ComponentRegistration>,
    by_type_path: HashMap<&'static str, ComponentRegistration>,
}

impl SyncRegistry {
    pub fn register(&mut self, registration: ComponentRegistration) {
        self.by_wire_id.insert(registration.wire_id, registration);
        self.by_type_path.insert(registration.type_path, registration);
    }

    pub fn by_wire_id(&self, wire_id: u64) -> Option<&ComponentRegistration> {
        self.by_wire_id.get(&wire_id)
    }
}

#[derive(Resource, Default)]
pub struct SyncResourceRegistry {
    by_wire_id: HashMap<u64, ResourceRegistration>,
    by_type_path: HashMap<&'static str, ResourceRegistration>,
}

impl SyncResourceRegistry {
    pub fn register(&mut self, registration: ResourceRegistration) {
        self.by_wire_id.insert(registration.wire_id, registration);
        self.by_type_path.insert(registration.type_path, registration);
    }

    pub fn by_wire_id(&self, wire_id: u64) -> Option<&ResourceRegistration> {
        self.by_wire_id.get(&wire_id)
    }
}

pub trait SyncComponent:
    Component + Serialize + DeserializeOwned + Clone + Send + Sync + 'static
{
    const TYPE_PATH: &'static str;
    const WIRE_ID: u64;
}

pub trait SyncResource:
    Resource + Serialize + DeserializeOwned + Clone + Send + Sync + 'static
{
    const TYPE_PATH: &'static str;
    const WIRE_ID: u64;
}

#[derive(Component, Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[doc(hidden)]
pub struct PrefabId(pub u64);

#[derive(Resource, Default)]
pub struct PrefabRegistry {
    by_wire_id: HashMap<u64, PrefabRegistration>,
    by_type_path: HashMap<&'static str, PrefabRegistration>,
}

impl PrefabRegistry {
    pub fn register(&mut self, registration: PrefabRegistration) {
        self.by_wire_id.insert(registration.wire_id, registration);
        self.by_type_path.insert(registration.type_path, registration);
    }

    pub fn by_wire_id(&self, wire_id: u64) -> Option<&PrefabRegistration> {
        self.by_wire_id.get(&wire_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &PrefabRegistration> {
        self.by_wire_id.values()
    }
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

pub fn register_sync_components(app: &mut App) {
    app.init_resource::<SyncRegistry>();
    app.init_resource::<SyncResourceRegistry>();
    app.init_resource::<PrefabRegistry>();

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

pub fn poll_network_incoming(mut net: ResMut<NetResource>) {
    net.poll_incoming();
}

pub fn flush_network_outbox(mut net: ResMut<NetResource>) {
    net.flush_outbox();
}

pub fn sync_component<T: SyncComponent>(
    mut net: ResMut<NetResource>,
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
            bytes,
        });
    }
}

pub fn sync_resource<T: SyncResource>(
    mut net: ResMut<NetResource>,
    resource: Option<Res<T>>,
) {
    let Some(resource) = resource else {
        return;
    };

    if !net.is_server() || !(resource.is_added() || resource.is_changed()) {
        return;
    }

    let bytes = bincode::serde::encode_to_vec(&*resource, config::standard())
        .expect("failed to serialize sync resource");
    net.queue_packet(ReplicationPacket::UpdateResource {
        resource_wire_id: T::WIRE_ID,
        bytes,
    });
}

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
        inventory::iter::<ComponentRegistration>().copied().collect();
    let resource_registrations: Vec<ResourceRegistration> =
        inventory::iter::<ResourceRegistration>().copied().collect();

    for socket in &connections {
        let replicated_entities = {
            let mut query = world.query_filtered::<
                (Entity, &NetworkId, Option<&PrefabId>),
                With<Replicated>,
            >();
            query
                .iter(world)
                .map(|(entity, network_id, prefab_id)| {
                    (entity, *network_id, prefab_id.map(|prefab_id| prefab_id.0).unwrap_or(0))
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

            for packets in component_snapshots.into_iter().chain(resource_snapshots.into_iter()) {
                for packet in packets {
                    net.send_packet_to(socket, packet);
                }
            }
        };
    }
}

pub fn apply_incoming_packets(world: &mut World) {
    let packets = {
        let mut net = world.resource_mut::<NetResource>();
        net.drain_inbox()
    };

    if packets.is_empty() {
        return;
    }

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
                world.resource_mut::<EntityIndex>().insert(NetworkId(network_id), entity);
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
                let entity = world.resource::<EntityIndex>().entity(NetworkId(network_id));
                if let Some(entity) = entity {
                    world.despawn(entity);
                    world
                        .resource_mut::<EntityIndex>()
                        .remove_entity(entity);
                }
            }
            ReplicationPacket::UpdateComponent {
                network_id,
                component_wire_id,
                bytes,
            } => {
                let entity = world.resource::<EntityIndex>().entity(NetworkId(network_id));
                let registration = {
                    world
                        .resource::<SyncRegistry>()
                        .by_wire_id(component_wire_id)
                        .copied()
                };

                if let (Some(entity), Some(registration)) = (entity, registration) {
                    (registration.apply)(world, entity, &bytes);
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
}

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
        world.resource_mut::<NetResource>().queue_packet(
            ReplicationPacket::SpawnEntity {
                network_id: network_id.0,
                prefab_wire_id,
            },
        );
    }
}

pub fn assign_prefab_ids(world: &mut World) {
    let entities = {
        let mut query = world.query_filtered::<Entity, Added<Replicated>>();
        query.iter(world).collect::<Vec<_>>()
    };

    let registrations: Vec<PrefabRegistration> = inventory::iter::<PrefabRegistration>().copied().collect();

    for entity in entities {
        if world.entity(entity).contains::<PrefabId>() {
            continue;
        }

        for registration in &registrations {
            if (registration.matches)(world, entity) {
                world.entity_mut(entity).insert(PrefabId(registration.wire_id));
                break;
            }
        }
    }
}

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
