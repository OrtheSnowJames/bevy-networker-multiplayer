use bevy::prelude::*;
use bevy_networker_multiplayer::{sync, NetResource, Replicated, ReplicatedPlugin};

const ADDRESS: &str = "127.0.0.1:5003";

#[sync(prefab(Sprite::from_color(Color::srgb(0.2, 0.8, 1.0), Vec2::splat(32.0)), Transform::from_xyz(0.0, 0.0, 0.0)))]
#[derive(Component)]
struct Position(Vec2);

#[sync]
#[derive(Component)]
struct Velocity(Vec2);

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
        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bevy Networker Multiplayer - Cubes".into(),
                resolution: (960, 540).into(),
                ..default()
            }),
            ..default()
        }));
    } else {
        app.add_plugins(MinimalPlugins);
    }

    app.add_plugins(ReplicatedPlugin);
    app.insert_resource(DemoMode(mode));
    app.add_systems(Startup, setup);

    match mode {
        Mode::Server => {
            app.add_systems(Update, server_move_cubes);
        }
        Mode::Client => {
            app.add_systems(Startup, setup_client_window);
        }
    }

    app.run();
}

fn parse_mode() -> Mode {
    match std::env::args().nth(1).as_deref() {
        Some("server") => Mode::Server,
        Some("client") => Mode::Client,
        _ => {
            eprintln!("usage: cargo run --example cubes_demo -- [server|client]");
            std::process::exit(1);
        }
    }
}

fn setup(mut commands: Commands, mut net: ResMut<NetResource>, mode: Res<DemoMode>) {
    match mode.0 {
        Mode::Server => {
            net.start_server(5003);
            println!("server listening on {ADDRESS}");

            for index in 0..5 {
                commands.spawn((
                    Replicated,
                    Position(Vec2::new(index as f32 * 80.0 - 160.0, 0.0)),
                    Velocity(Vec2::new(70.0 + index as f32 * 15.0, 40.0 + index as f32 * 10.0)),
                ));
            }
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

fn server_move_cubes(time: Res<Time>, mut query: Query<(&mut Position, &mut Velocity)>) {
    let dt = time.delta_secs();

    for (mut position, mut velocity) in &mut query {
        position.0 += velocity.0 * dt;

        if position.0.x > 360.0 || position.0.x < -360.0 {
            velocity.0.x = -velocity.0.x;
        }

        if position.0.y > 200.0 || position.0.y < -200.0 {
            velocity.0.y = -velocity.0.y;
        }
    }
}
