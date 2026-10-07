//! Fork: a ragdoll's draw box follows its body. The exterior cull elects a unit by its
//! `WorldUnit::bound`, a box around the spot the unit stands on, so a body flung or rolled away
//! from that spot vanished whenever the spot left the view. While a unit is a ragdoll its box is
//! stretched each frame over its body parts, and given back when it stands up or goes.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;

use benilla_world::world_unit::WorldUnit;

use super::life::{Ragdoll, RagdollBodies};
use crate::entities::ModelBound;

/// Slack around the body parts' origins, for the limbs' own reach (yd).
const PAD: f32 = 0.8;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, fit_bounds);
}

/// The unit's box has been stretched over its ragdoll and is owed back.
#[derive(Component)]
struct Fitted;

/// What the fit reads and writes of a unit.
type FitUnit = (
    Entity,
    &'static GlobalTransform,
    &'static mut WorldUnit,
    Option<&'static ModelBound>,
    Has<Ragdoll>,
    Has<Fitted>,
);

/// Stretches each ragdoll's box over its bodies, and restores the model's box once it is done.
fn fit_bounds(
    mut commands: Commands,
    mut units: Query<FitUnit>,
    bodies: Res<RagdollBodies>,
    body_tfs: Query<&GlobalTransform, Without<WorldUnit>>,
) {
    for (unit, tf, mut world_unit, model, ragdoll, fitted) in &mut units {
        let Some(bound) = world_unit.bound else {
            continue; // a transport: not the cull's to elect
        };
        let model_box = model.map_or(bound, |m| m.0);
        if !ragdoll {
            if fitted {
                world_unit.bound = Some(model_box);
                commands.entity(unit).remove::<Fitted>();
            }
            continue;
        }
        let Some(ids) = bodies.0.get(&unit) else {
            continue;
        };
        // The bodies in the unit's model space, which the cull's box is measured in.
        let to_model = tf.affine().inverse();
        let pad = Vec3::splat(PAD / tf.scale().max_element().max(1e-3));
        let mut lo = Vec3::from(model_box.min());
        let mut hi = Vec3::from(model_box.max());
        for b in ids.iter().filter_map(|&b| body_tfs.get(b).ok()) {
            let p = to_model.transform_point3(b.translation());
            lo = lo.min(p - pad);
            hi = hi.max(p + pad);
        }
        let fit = Aabb::from_min_max(lo, hi);
        if world_unit.bound != Some(fit) {
            world_unit.bound = Some(fit);
        }
        if !fitted {
            commands.entity(unit).insert(Fitted);
        }
    }
}
