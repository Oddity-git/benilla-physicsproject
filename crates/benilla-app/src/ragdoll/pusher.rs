//! A living player's push capsule: a kinematic body that follows the character and shoves ragdoll
//! bodies (and test boxes) aside, one way only. Nothing else sees it, so movement, the camera and
//! the server never feel a corpse.

use avian3d::prelude::*;
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_world::collision::pusher_layers;

use crate::net::{NetEntity, ObjectStore};

/// The capsule: radius and the straight length between its caps (yd), centred this high above the
/// feet, so it spans the feet to about head height of a human.
const RADIUS: f32 = 0.45;
const LENGTH: f32 = 1.0;
const CENTRE: f32 = 0.95;
/// A move longer than this in one step is a teleport: the capsule jumps there instead of sweeping.
const TELEPORT: f32 = 8.0;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Pushers>()
        .add_systems(FixedUpdate, follow_players);
}

/// Each player's push capsule.
#[derive(Resource, Default)]
struct Pushers(EntityHashMap<Entity>);

/// Spawns a capsule for each living player, and sets its velocity so this physics step carries it
/// to the player: a kinematic body that moves by velocity pushes what it meets, where one moved by
/// teleport only overlaps it.
fn follow_players(
    mut commands: Commands,
    time: Res<Time>,
    players: Query<(Entity, &NetEntity, &ObjectStore, &GlobalTransform)>,
    mut capsules: Query<(&mut Position, &mut LinearVelocity)>,
    mut pushers: ResMut<Pushers>,
) {
    let dt = time.delta_secs().max(1e-4);
    let mut seen = EntityHashSet::default();
    for (player, net, store, tf) in &players {
        if net.kind != EntityKind::Player || store.0.unit_is_dead() {
            continue;
        }
        seen.insert(player);
        let target = tf.translation() + Vec3::Y * CENTRE;
        if let Some(capsule) = pushers.0.get(&player) {
            if let Ok((mut at, mut velocity)) = capsules.get_mut(*capsule) {
                let step = target - at.0;
                if step.length() > TELEPORT {
                    at.0 = target;
                    velocity.0 = Vec3::ZERO;
                } else {
                    velocity.0 = step / dt;
                }
            }
            continue;
        }
        let capsule = commands
            .spawn((
                Name::new("ragdoll pusher"),
                Transform::from_translation(target),
                RigidBody::Kinematic,
                Collider::capsule(RADIUS, LENGTH),
                pusher_layers(),
            ))
            .id();
        pushers.0.insert(player, capsule);
    }
    pushers.0.retain(|player, capsule| {
        let keep = seen.contains(player);
        if !keep {
            if let Ok(mut e) = commands.get_entity(*capsule) {
                e.despawn();
            }
        }
        keep
    });
}
