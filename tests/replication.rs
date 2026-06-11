use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
#[cfg(feature = "prediction")]
use bevy_networker_multiplayer::{LinearMotionPredictionPlugin, PredictLinearMotion, Velocity2d};
use bevy_networker_multiplayer::{
    ReplicatedPlugin,
    netres::{NetResource, ReplicationPacket},
    replicated::{EntityIndex, NetworkId, Replicated},
    sync,
};
use std::time::Duration;

#[sync]
#[derive(Component)]
struct Health(u32);

#[sync(prefab(
    Sprite::from_color(Color::srgb(0.2, 0.8, 1.0), Vec2::splat(32.0)),
    Transform::from_xyz(0.0, 0.0, 0.0)
))]
#[derive(Component)]
struct VisualPosition(Vec2);

#[cfg(feature = "prediction")]
#[sync(prefab(
    Sprite::from_color(Color::srgb(0.2, 0.8, 1.0), Vec2::splat(32.0)),
    Transform::from_xyz(0.0, 0.0, 0.0)
))]
#[derive(Component, PredictLinearMotion)]
struct PredictedPosition(Vec2);

#[cfg(feature = "prediction")]
#[sync]
#[derive(Component, Velocity2d)]
struct PredictedVelocity(Vec2);

#[sync(resource)]
#[derive(Resource)]
struct MatchState {
    score: u32,
}

#[sync(resource, interval = 0.5)]
#[derive(Resource)]
struct SlowMatchState {
    score: u32,
}

#[derive(Resource, Default)]
struct ChangedReads(u32);

fn count_match_state_changes(state: Option<Res<MatchState>>, mut reads: ResMut<ChangedReads>) {
    if state.map(|state| state.is_changed()).unwrap_or(false) {
        reads.0 += 1;
    }
}

#[test]
fn replicated_entities_get_ids_and_updates() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(ReplicatedPlugin);

    app.world_mut()
        .resource_mut::<NetResource>()
        .start_server(0);

    let entity = app.world_mut().spawn((Replicated, Health(100))).id();
    app.update();

    let network_id = app
        .world()
        .resource::<EntityIndex>()
        .network_id(entity)
        .expect("replicated entity should have a network id");
    assert_eq!(network_id.0, 0);

    let packets = app.world_mut().resource_mut::<NetResource>().drain_outbox();
    assert_eq!(packets.len(), 2);
    assert!(matches!(
        packets[0],
        ReplicationPacket::SpawnEntity {
            network_id: 0,
            prefab_wire_id: 0
        }
    ));
    assert!(matches!(
        packets[1],
        ReplicationPacket::UpdateComponent {
            network_id: 0,
            component_wire_id: _,
            ..
        }
    ));
}

#[test]
fn replicated_resources_queue_and_apply_updates() {
    let mut server = App::new();
    server.add_plugins(MinimalPlugins);
    server.add_plugins(ReplicatedPlugin);
    server
        .world_mut()
        .resource_mut::<NetResource>()
        .start_server(0);
    server.world_mut().insert_resource(MatchState { score: 7 });

    server.update();

    let packets = server
        .world_mut()
        .resource_mut::<NetResource>()
        .drain_outbox();
    assert_eq!(packets.len(), 1);

    let bytes = match &packets[0] {
        ReplicationPacket::UpdateResource {
            resource_wire_id,
            bytes,
        } => {
            assert_eq!(
                *resource_wire_id,
                <MatchState as sync::SyncResource>::WIRE_ID
            );
            bytes.clone()
        }
        other => panic!("unexpected packet: {other:?}"),
    };

    let mut client = App::new();
    client.add_plugins(MinimalPlugins);
    client.add_plugins(ReplicatedPlugin);
    client
        .world_mut()
        .resource_mut::<NetResource>()
        .inject_packet(ReplicationPacket::UpdateResource {
            resource_wire_id: <MatchState as sync::SyncResource>::WIRE_ID,
            bytes,
        });

    sync::apply_incoming_packets(client.world_mut());

    let resource = client.world().resource::<MatchState>();
    assert_eq!(resource.score, 7);
}

#[test]
fn replicated_resources_skip_semantically_identical_changes() {
    let mut server = App::new();
    server.add_plugins(MinimalPlugins);
    server.add_plugins(ReplicatedPlugin);
    server
        .world_mut()
        .resource_mut::<NetResource>()
        .start_server(0);
    server.world_mut().insert_resource(MatchState { score: 7 });

    server.update();
    assert_eq!(
        server
            .world_mut()
            .resource_mut::<NetResource>()
            .drain_outbox()
            .len(),
        1
    );

    server.world_mut().resource_mut::<MatchState>().score = 7;
    server.update();
    assert!(
        server
            .world_mut()
            .resource_mut::<NetResource>()
            .drain_outbox()
            .is_empty()
    );

    server.world_mut().resource_mut::<MatchState>().score = 8;
    server.update();
    assert_eq!(
        server
            .world_mut()
            .resource_mut::<NetResource>()
            .drain_outbox()
            .len(),
        1
    );
}

#[test]
fn replicated_resources_coalesce_fast_changes() {
    let mut server = App::new();
    server.add_plugins(MinimalPlugins);
    server.add_plugins(ReplicatedPlugin);
    server.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    server
        .world_mut()
        .resource_mut::<NetResource>()
        .start_server(0);
    server
        .world_mut()
        .insert_resource(SlowMatchState { score: 1 });

    server.update();
    assert_eq!(
        server
            .world_mut()
            .resource_mut::<NetResource>()
            .drain_outbox()
            .len(),
        1
    );

    server.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        100,
    )));
    server.world_mut().resource_mut::<SlowMatchState>().score = 2;
    server.update();
    assert!(
        server
            .world_mut()
            .resource_mut::<NetResource>()
            .drain_outbox()
            .is_empty()
    );

    server.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(1)));
    server.update();
    server.update();
    let packets = server
        .world_mut()
        .resource_mut::<NetResource>()
        .drain_outbox();
    assert_eq!(packets.len(), 1);

    let bytes = match &packets[0] {
        ReplicationPacket::UpdateResource { bytes, .. } => bytes,
        other => panic!("unexpected packet: {other:?}"),
    };
    let (state, _): (SlowMatchState, usize) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard())
            .expect("slow state should deserialize");
    assert_eq!(state.score, 2);
}

#[test]
fn duplicate_resource_packets_do_not_mark_resource_changed() {
    let bytes = bincode::serde::encode_to_vec(MatchState { score: 7 }, bincode::config::standard())
        .expect("match state should serialize");

    let mut client = App::new();
    client.add_plugins(MinimalPlugins);
    client.add_plugins(ReplicatedPlugin);
    client.init_resource::<ChangedReads>();
    client.add_systems(Update, count_match_state_changes);
    client.world_mut().insert_resource(MatchState { score: 7 });

    client.update();
    client.world_mut().resource_mut::<ChangedReads>().0 = 0;

    client
        .world_mut()
        .resource_mut::<NetResource>()
        .inject_packet(ReplicationPacket::UpdateResource {
            resource_wire_id: <MatchState as sync::SyncResource>::WIRE_ID,
            bytes,
        });
    sync::apply_incoming_packets(client.world_mut());
    client.update();

    assert_eq!(client.world().resource::<ChangedReads>().0, 0);
}

#[test]
fn component_updates_wait_for_spawn_entity() {
    let mut client = App::new();
    client.add_plugins(MinimalPlugins);
    client.add_plugins(ReplicatedPlugin);

    let bytes = bincode::serde::encode_to_vec(Health(42), bincode::config::standard())
        .expect("health should serialize");

    client
        .world_mut()
        .resource_mut::<NetResource>()
        .inject_packet(ReplicationPacket::UpdateComponent {
            network_id: 9,
            component_wire_id: <Health as sync::SyncComponent>::WIRE_ID,
            bytes,
        });

    sync::apply_incoming_packets(client.world_mut());
    assert!(
        client
            .world()
            .resource::<EntityIndex>()
            .entity(NetworkId(9))
            .is_none()
    );

    client
        .world_mut()
        .resource_mut::<NetResource>()
        .inject_packet(ReplicationPacket::SpawnEntity {
            network_id: 9,
            prefab_wire_id: 0,
        });

    sync::apply_incoming_packets(client.world_mut());

    let entity = client
        .world()
        .resource::<EntityIndex>()
        .entity(NetworkId(9))
        .expect("spawn packet should create the replicated entity");
    assert_eq!(client.world().entity(entity).get::<Health>().unwrap().0, 42);
}

#[test]
fn prefab_spawn_packets_create_client_visuals() {
    let mut server = App::new();
    server.add_plugins(MinimalPlugins);
    server.add_plugins(ReplicatedPlugin);
    server
        .world_mut()
        .resource_mut::<NetResource>()
        .start_server(0);
    server
        .world_mut()
        .spawn((Replicated, VisualPosition(Vec2::new(12.0, 34.0))));

    server.update();

    let packets = server
        .world_mut()
        .resource_mut::<NetResource>()
        .drain_outbox();
    assert!(packets.iter().any(|packet| matches!(
        packet,
        ReplicationPacket::SpawnEntity {
            network_id: 0,
            prefab_wire_id
        } if *prefab_wire_id == <VisualPosition as sync::SyncComponent>::WIRE_ID
    )));

    let mut client = App::new();
    client.add_plugins(MinimalPlugins);
    client.add_plugins(ReplicatedPlugin);

    for packet in packets {
        client
            .world_mut()
            .resource_mut::<NetResource>()
            .inject_packet(packet);
    }

    sync::apply_incoming_packets(client.world_mut());

    let entity = client
        .world()
        .resource::<EntityIndex>()
        .entity(NetworkId(0))
        .expect("spawn packet should create the replicated entity");
    let transform = client
        .world()
        .entity(entity)
        .get::<Transform>()
        .expect("prefab should insert a transform");

    assert!(client.world().entity(entity).contains::<Sprite>());
    assert_eq!(transform.translation.x, 12.0);
    assert_eq!(transform.translation.y, 34.0);
}

#[cfg(feature = "prediction")]
#[test]
fn prediction_updates_prefab_visual_transform() {
    let mut client = App::new();
    client.add_plugins(MinimalPlugins);
    client.add_plugins(ReplicatedPlugin);
    client.add_plugins(LinearMotionPredictionPlugin::<
        PredictedPosition,
        PredictedVelocity,
    >::new());
    client.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));

    let position_bytes = bincode::serde::encode_to_vec(
        PredictedPosition(Vec2::new(10.0, 20.0)),
        bincode::config::standard(),
    )
    .expect("position should serialize");
    let velocity_bytes = bincode::serde::encode_to_vec(
        PredictedVelocity(Vec2::new(4.0, 6.0)),
        bincode::config::standard(),
    )
    .expect("velocity should serialize");

    for packet in [
        ReplicationPacket::SpawnEntity {
            network_id: 11,
            prefab_wire_id: <PredictedPosition as sync::SyncComponent>::WIRE_ID,
        },
        ReplicationPacket::UpdateComponent {
            network_id: 11,
            component_wire_id: <PredictedPosition as sync::SyncComponent>::WIRE_ID,
            bytes: position_bytes,
        },
        ReplicationPacket::UpdateComponent {
            network_id: 11,
            component_wire_id: <PredictedVelocity as sync::SyncComponent>::WIRE_ID,
            bytes: velocity_bytes,
        },
    ] {
        client
            .world_mut()
            .resource_mut::<NetResource>()
            .inject_packet(packet);
    }

    client.update();
    client.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(1)));
    client.update();

    let entity = client
        .world()
        .resource::<EntityIndex>()
        .entity(NetworkId(11))
        .expect("spawn packet should create the replicated entity");
    let transform = client
        .world()
        .entity(entity)
        .get::<Transform>()
        .expect("prefab should insert a transform");

    assert_eq!(transform.translation.x, 11.0);
    assert_eq!(transform.translation.y, 21.5);
}
