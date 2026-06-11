# bevy-networker-multiplayer

Little multiplayer plugin on top of [networker-rs](https://github.com/OrtheSnowJames/networker-rs).
Tired of big net libraries with big boilerplate? Here's the small solution!

## How it works
The API stays small:

- `Replicated` = entity should exist on the network
- `#[sync]` = component should sync over the wire
- `#[sync(prefab(...))]` = client-side visual prefab for that component
- `#[sync(resource)]` = sync a Bevy resource
- `#[netmsg]` = typed message / RPC-style traffic

`NetResource` is inserted automatically and is the bridge to `networker-rs`: start or join from it, then the plugin uses it to queue packets, flush them, and apply incoming replication back into Bevy.

Resources are synced as whole snapshots instead of using network ids. That makes them a good fit for match state, lobby state, money, chat history, and timers.

Example:

```rust
use bevy::prelude::*;
use bevy_networker_multiplayer::{sync, ReplicatedPlugin};

#[sync(resource)]
#[derive(Resource)]
struct MatchState {
    round: u32,
    time_left: f32,
}

fn setup(mut commands: Commands) {
    commands.insert_resource(MatchState {
        round: 1,
        time_left: 60.0,
    });
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(ReplicatedPlugin)
        .add_systems(Startup, setup)
        .run();
}
```

Basic server/client split:

```rust
use bevy::prelude::*;
use bevy_networker_multiplayer::{NetResource, ReplicatedPlugin};

const ADDRESS: &str = "127.0.0.1:5001";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Server,
    Client,
}

#[derive(Resource, Clone, Copy)]
struct DemoMode(Mode);

fn main() {
    let mode = if std::env::args().nth(1).as_deref() == Some("server") {
        Mode::Server
    } else {
        Mode::Client
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(ReplicatedPlugin);
    app.insert_resource(DemoMode(mode));
    app.add_systems(Startup, setup);
    app.run();
}

fn setup(mut net: ResMut<NetResource>, mode: Res<DemoMode>) {
    match mode.0 {
        Mode::Server => {
            net.start_server(5001);
            println!("server listening on {ADDRESS}");
        }
        Mode::Client => {
            net.join_server(ADDRESS.to_string());
            println!("client connected to {ADDRESS}");
        }
    }
}
```

That pattern is enough to get a server running, connect a client, and let the plugin handle replication, resource sync, and typed messages.

Basic moving-cubes demo without prediction:

```bash
cargo run --example cubes_demo -- server
cargo run --example cubes_demo -- client
```

The server starts a shared world of replicated cubes and moves them every frame. Each client opens its own window, receives the cube state, and renders the same motion locally. The cube visuals come from `#[sync(prefab(...))]`, and the `Position` component automatically drives the spawned `Transform` on the client, so you do not need a separate visual sync system. New clients also receive a snapshot of the current state when they connect.

Client-side movement prediction lives in a separate example:

```bash
cargo run --example cubes_demo_prediction --features prediction -- server
cargo run --example cubes_demo_prediction --features prediction -- client
```

The prediction API is derived on tuple structs:

```rust
#[cfg(feature = "prediction")]
#[derive(Component, PredictLinearMotion)]
struct Position(Vec2);

#[cfg(feature = "prediction")]
#[derive(Component, Velocity2d)]
struct Velocity(Vec2);
```

## Example

Run the replication demo in two terminals:

```bash
cargo run --example replication_demo -- server
cargo run --example replication_demo -- client
```

The server spawns a replicated entity and moves it once per second.
The client connects over UDP, receives spawn/update packets, and prints the replicated state it sees.

Run the message demo in two terminals:

```bash
cargo run --example messages_demo -- server
cargo run --example messages_demo -- client
```

The client sends chat and shoot messages. The server prints chat, broadcasts a reply, and turns shoot requests into replicated projectile entities.

## Notes

- Uses UDP via `networker-rs`
- I found out [lightyear](https://github.com/cBournhonesque/lightyear/) existed just after making this
- Latest update: made sure entities don't snap back by dropping late packets