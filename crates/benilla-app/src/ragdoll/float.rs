//! Fork: a ragdoll in water floats. Each body under the surface takes an upward push against
//! gravity by how deep it sits, and water drag on its fall and spin, so a corpse bobs up and lies
//! at the surface instead of sinking to the bottom. Magma and slime are not water and do not lift.

use avian3d::prelude::*;
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_world::world_point::{Subject, WorldPoint};

use super::life::RagdollBodies;

/// The water's push against gravity on a body all the way under (1 is weightless): a body comes
/// to rest with `1 / BUOYANCY` of it under.
const BUOYANCY: f32 = 1.6;
/// The depth band (yd) over which a body goes from just touching the surface, its middle half a
/// band above it, to all the way under, half a band below.
const BAND: f32 = 0.3;
/// The water's drag on a wet body's velocity and spin, per second.
const WATER_DRAG: f32 = 2.5;
const WATER_SPIN_DRAG: f32 = 2.0;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(FixedUpdate, float_bodies);
}

/// Lifts and slows each ragdoll body in water, keeping it awake while it is.
fn float_bodies(
    mut commands: Commands,
    time: Res<Time>,
    gravity: Option<Res<Gravity>>,
    bodies: Res<RagdollBodies>,
    world: WorldPoint,
    mut rigid: Query<(&Transform, &mut LinearVelocity, &mut AngularVelocity, Has<Sleeping>)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let fall = gravity.map_or(Vec3::NEG_Y * 9.81, |g| g.0);
    for (&unit, ids) in &bodies.0 {
        for &body in ids {
            let Ok((tf, mut linear, mut angular, asleep)) = rigid.get_mut(body) else {
                continue;
            };
            let wow = bevy_to_wow(tf.translation);
            let Some(surface) = world.water_surface_at(Subject::Unit(unit), wow) else {
                continue;
            };
            let under = ((surface - wow[2]) / BAND + 0.5).clamp(0.0, 1.0);
            if under <= 0.0 {
                continue;
            }
            if asleep {
                commands.entity(body).try_remove::<Sleeping>();
            }
            linear.0 -= fall * (BUOYANCY * under * dt);
            linear.0 *= 1.0 - (WATER_DRAG * under * dt).min(1.0);
            angular.0 *= 1.0 - (WATER_SPIN_DRAG * under * dt).min(1.0);
        }
    }
}
