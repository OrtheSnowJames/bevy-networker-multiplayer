use bevy::prelude::*;
use bevy_networker_multiplayer::{sync, NetResource, Replicated, ReplicatedPlugin};

const ADDRESS: &str = "127.0.0.1:5001";

#[sync]
#[derive(Component)]
struct Position(Vec2);

#[sync]
#[derive(Component)]
struct Health(u32);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Server,
    Client,
}

#[derive(Resource, Clone, Copy)]
struct DemoMode(Mode);

fn main() {
    let mode = parse_mode();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(ReplicatedPlugin);
    app.insert_resource(DemoMode(mode));
    app.add_systems(Startup, setup);

    match mode {
        Mode::Server => {
            app.add_systems(Update, server_motion);
        }
        Mode::Client => {
            app.add_systems(Update, client_report);
        }
    }

    app.run();
}

fn parse_mode() -> Mode {
    match std::env::args().nth(1).as_deref() {
        Some("server") => Mode::Server,
        Some("client") => Mode::Client,
        _ => {
            eprintln!("usage: cargo run --example replication_demo -- [server|client]");
            std::process::exit(1);
        }
    }
}

fn setup(mut commands: Commands, mut net: ResMut<NetResource>, mode: Res<DemoMode>) {
    match mode.0 {
        Mode::Server => {
            net.start_server(5001);
            println!("server listening on {ADDRESS}");
            commands.spawn((
                Replicated,
                Position(Vec2::ZERO),
                Health(100),
            ));
        }
        Mode::Client => {
            net.join_server(ADDRESS.to_string());
            println!("client connected to {ADDRESS}");
        }
    }
}

fn server_motion(
    time: Res<Time>,
    mut tick: Local<f32>,
    mut query: Query<&mut Position, With<Replicated>>,
) {
    *tick += time.delta_secs();
    if *tick < 1.0 {
        return;
    }
    *tick = 0.0;

    if let Ok(mut position) = query.single_mut() {
        position.0.x += 1.0;
        position.0.y += 0.5;
        println!("server position -> {:?}", position.0);
    }
}

fn client_report(
    time: Res<Time>,
    mut tick: Local<f32>,
    query: Query<(Entity, &Position, &Health), With<Replicated>>,
) {
    *tick += time.delta_secs();
    if *tick < 1.0 {
        return;
    }
    *tick = 0.0;

    for (entity, position, health) in &query {
        println!(
            "client entity {:?} => position {:?}, health {}",
            entity, position.0, health.0
        );
    }
}
