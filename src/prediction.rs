use bevy::prelude::*;
use bevy::ecs::component::Mutable;

use crate::replicated::Replicated;

pub trait PredictLinearMotion: Component<Mutability = Mutable> {
    fn predicted_position(&self) -> Vec2;
    fn set_predicted_position(&mut self, position: Vec2);
}

pub trait Velocity2d: Component {
    fn velocity_2d(&self) -> Vec2;
}

pub struct LinearMotionPredictionPlugin<P, V> {
    marker: std::marker::PhantomData<(P, V)>,
}

impl<P, V> LinearMotionPredictionPlugin<P, V> {
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
    fn build(&self, app: &mut App) {
        app.add_systems(Update, predict_linear_motion::<P, V>);
    }
}

pub fn predict_linear_motion<P, V>(
    time: Res<Time>,
    net: Res<crate::NetResource>,
    mut query: Query<(&mut P, &V), With<Replicated>>,
)
where
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
