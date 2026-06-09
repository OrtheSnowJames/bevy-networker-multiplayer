use bevy::prelude::*;
use bevy_networker_multiplayer::{
    netres::{NetResource, ReplicationPacket},
    sync,
    replicated::{EntityIndex, Replicated},
    ReplicatedPlugin,
};

#[sync]
#[derive(Component)]
struct Health(u32);

#[sync(resource)]
#[derive(Resource)]
struct MatchState {
    score: u32,
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
    server.world_mut().resource_mut::<NetResource>().start_server(0);
    server.world_mut().insert_resource(MatchState { score: 7 });

    server.update();

    let packets = server.world_mut().resource_mut::<NetResource>().drain_outbox();
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
