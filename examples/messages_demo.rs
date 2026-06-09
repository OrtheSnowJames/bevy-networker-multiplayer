use bevy::prelude::*;
use bevy_networker_multiplayer::{netmsg, sync, NetResource, Replicated, ReplicatedPlugin};

const ADDRESS: &str = "127.0.0.1:5002";

#[netmsg]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ChatSend {
    text: String,
}

#[netmsg]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ChatBroadcast {
    text: String,
}

#[netmsg]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ShootProjectile {
    origin: Vec2,
    direction: Vec2,
}

#[sync]
#[derive(Component)]
struct Position(Vec2);

#[sync]
#[derive(Component)]
struct Velocity(Vec2);

#[sync]
#[derive(Component)]
struct Projectile;

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
    if mode == Mode::Client {
        app.add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bevy Netvent Multiplayer".into(),
                    resolution: (960, 540).into(),
                    ..default()
                }),
                ..default()
            }),
        );
    } else {
        app.add_plugins(MinimalPlugins);
    }
    app.add_plugins(ReplicatedPlugin);
    app.insert_resource(DemoMode(mode));
    app.add_systems(Startup, setup);

    match mode {
        Mode::Server => {
            app.add_systems(Update, (server_handle_messages, server_move_projectiles));
        }
        Mode::Client => {
            app.add_systems(
                Startup,
                setup_client_window,
            )
            .add_systems(
                Update,
                (
                    client_send_messages,
                    client_print_broadcasts,
                    client_spawn_projectile_visuals,
                    client_sync_projectile_visuals,
                ),
            );
        }
    }

    app.run();
}

fn parse_mode() -> Mode {
    match std::env::args().nth(1).as_deref() {
        Some("server") => Mode::Server,
        Some("client") => Mode::Client,
        _ => {
            eprintln!("usage: cargo run --example messages_demo -- [server|client]");
            std::process::exit(1);
        }
    }
}

fn setup(mut net: ResMut<NetResource>, mode: Res<DemoMode>) {
    match mode.0 {
        Mode::Server => {
            net.start_server(5002);
            println!("server listening on {ADDRESS}");
        }
        Mode::Client => {
            net.join_server(ADDRESS.to_string());
            println!("client connected to {ADDRESS}");
        }
    }
}

fn setup_client_window(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn server_handle_messages(
    mut commands: Commands,
    mut net: ResMut<NetResource>,
) {
    for chat in net.drain_messages::<ChatSend>() {
        println!("chat: {}", chat.text);
        net.queue_message(ChatBroadcast {
            text: format!("server heard: {}", chat.text),
        });
    }

    for shot in net.drain_messages::<ShootProjectile>() {
        println!("shoot request from client: {:?} -> {:?}", shot.origin, shot.direction);
        commands.spawn((
            Replicated,
            Projectile,
            Position(shot.origin),
            Velocity(shot.direction.normalize_or_zero() * 150.0),
        ));
    }
}

fn server_move_projectiles(
    time: Res<Time>,
    mut query: Query<(&mut Position, &Velocity), With<Projectile>>,
) {
    for (mut position, velocity) in &mut query {
        position.0 += velocity.0 * time.delta_secs();
    }
}

fn client_send_messages(
    time: Res<Time>,
    mut tick: Local<f32>,
    mut net: ResMut<NetResource>,
) {
    *tick += time.delta_secs();
    if *tick < 1.0 {
        return;
    }
    *tick = 0.0;

    net.queue_message(ChatSend {
        text: "hello from client".to_string(),
    });
    net.queue_message(ShootProjectile {
        origin: Vec2::new(0.0, 0.0),
        direction: Vec2::new(1.0, 0.2),
    });
}

fn client_print_broadcasts(mut net: ResMut<NetResource>) {
    for chat in net.drain_messages::<ChatBroadcast>() {
        println!("broadcast: {}", chat.text);
    }
}

fn client_spawn_projectile_visuals(
    mut commands: Commands,
    query: Query<(Entity, &Position), Added<Projectile>>,
) {
    for (entity, position) in &query {
        commands.entity(entity).insert((
            Sprite::from_color(Color::srgb(1.0, 0.25, 0.25), Vec2::splat(20.0)),
            Transform::from_xyz(position.0.x, position.0.y, 0.0),
        ));
    }
}

fn client_sync_projectile_visuals(mut query: Query<(&Position, &mut Transform), With<Projectile>>) {
    for (position, mut transform) in &mut query {
        transform.translation.x = position.0.x;
        transform.translation.y = position.0.y;
    }
}
