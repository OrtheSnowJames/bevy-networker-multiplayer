//! Optional client-side prediction helpers.
//!
//! The library keeps prediction generic: you decide which component represents
//! position and which component provides velocity. The plugin only applies the
//! interpolation step on clients.

use bevy::ecs::component::Mutable;
use bevy::prelude::*;

use crate::replicated::Replicated;

/// Component trait for a 2D position-like value that can be predicted locally.
pub trait PredictLinearMotion: Component<Mutability = Mutable> {
    /// Returns the current predicted position.
    fn predicted_position(&self) -> Vec2;
    /// Stores the next predicted position.
    fn set_predicted_position(&mut self, position: Vec2);
}

/// Component trait for a 2D velocity-like value.
pub trait Velocity2d: Component {
    /// Returns the current 2D velocity.
    fn velocity_2d(&self) -> Vec2;
}

/// Plugin that advances predicted motion on clients.
pub struct LinearMotionPredictionPlugin<P, V> {
    marker: std::marker::PhantomData<(P, V)>,
}

impl<P, V> LinearMotionPredictionPlugin<P, V> {
    /// Creates a new prediction plugin for the supplied component pair.
    pub fn new() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

impl<P, V> Plugin for LinearMotionPredictionPlugin<P, V>
where
    P: PredictLinearMotion,
    V: Velocity2d,
{
    /// Registers the per-frame prediction system.
    fn build(&self, app: &mut App) {
        app.add_systems(Update, predict_linear_motion::<P, V>);
    }
}

/// Applies a simple linear position update on clients.
///
/// The server owns the authoritative state; clients use this to make motion feel
/// responsive between network updates.
pub fn predict_linear_motion<P, V>(
    time: Res<Time>,
    net: Res<crate::NetResource>,
    mut query: Query<(&mut P, &V), With<Replicated>>,
) where
    P: PredictLinearMotion,
    V: Velocity2d,
{
    if net.is_server() {
        return;
    }

    let dt = time.delta_secs();

    for (mut position, velocity) in query.iter_mut() {
        let next_position = position.predicted_position() + velocity.velocity_2d() * dt;
        position.set_predicted_position(next_position);
    }
}
