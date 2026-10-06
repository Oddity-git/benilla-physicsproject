//! A ragdoll's life: a unit seen alive that dies gets bodies at its current bone pose, the bodies
//! drive the bones until they settle, and then the settled pose is frozen and the bodies go.
//! A unit that streams in already dead keeps its Death pose: nothing was seen to fall.

use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashMap;
use bevy::math::Affine3A;
use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_world::collision::ragdoll_layers;
use benilla_world::rig_anim::RigPose;

use super::rig::{build_profile, Profile};
use crate::creature_anim::AnimDriver;
use crate::entities::mount::MountBody;
use crate::net::{NetEntity, ObjectStore, SelfPlayer};

/// Joint limits about the bind pose: how far a limb may swing off its bind direction, and twist.
const SWING_LIMIT: f32 = 70.0_f32.to_radians();
const TWIST_LIMIT: f32 = 25.0_f32.to_radians();
/// A ragdoll freezes once every body sleeps, or after this long regardless.
const MAX_SIM_SECS: f32 = 12.0;
/// Most ragdolls simulating at once; a death past it freezes the oldest.
const MAX_ACTIVE: usize = 12;
/// A small push at death so the body folds instead of dropping straight down (yd/s).
const DEATH_NUDGE: f32 = 1.2;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<RagdollBodies>()
        .add_systems(Update, (track_unit_motion, start_ragdolls).chain())
        .add_systems(
            PostUpdate,
            (drive_bones_from_bodies, freeze_settled, cleanup)
                .chain()
                .in_set(benilla_world::rig_anim::PosePost),
        );
}

/// The unit was seen alive, so its death is witnessed and ragdolls.
#[derive(Component)]
pub(super) struct SeenAlive;

/// Where the unit was last frame, for the velocity its bodies inherit.
#[derive(Component, Default)]
pub(super) struct LastSpot(Vec3);

/// The unit is a ragdoll: simulating, or frozen in its settled pose.
#[derive(Component)]
pub(super) struct Ragdoll {
    profile: Profile,
    started: f32,
    /// Frozen bone locals once settled, by profile body; `None` while the bodies simulate.
    frozen: Option<Vec<Transform>>,
}

/// The body entities of each ragdolled unit, profile order, so a unit that vanishes or stands up
/// takes its bodies with it.
#[derive(Resource, Default)]
struct RagdollBodies(EntityHashMap<Vec<Entity>>);

fn ragdolls_off() -> bool {
    std::env::var_os("WOW_NO_RAGDOLL").is_some()
}

/// Records each living unit's position, and marks it seen alive.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn track_unit_motion(
    mut commands: Commands,
    mut units: Query<
        (
            Entity,
            &GlobalTransform,
            &ObjectStore,
            Option<&mut LastSpot>,
            Has<SeenAlive>,
        ),
        (With<AnimDriver>, Without<Ragdoll>),
    >,
) {
    for (e, tf, store, spot, seen) in &mut units {
        if store.0.unit_is_dead() {
            continue;
        }
        match spot {
            Some(mut s) => s.0 = tf.translation(),
            None => {
                commands.entity(e).insert(LastSpot(tf.translation()));
            }
        }
        if !seen {
            commands.entity(e).insert(SeenAlive);
        }
    }
}

/// A unit seen alive that is now really dead (not feigning) becomes a ragdoll: one body a profile
/// bone at its current pose, jointed to its parent body at the bone's pivot.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn start_ragdolls(
    mut commands: Commands,
    time: Res<Time>,
    dying: Query<
        (
            Entity,
            &ObjectStore,
            &NetEntity,
            &RigPose,
            &GlobalTransform,
            Option<&LastSpot>,
        ),
        (
            With<SeenAlive>,
            With<AnimDriver>,
            Without<Ragdoll>,
            Without<SelfPlayer>,
            Without<MountBody>,
        ),
    >,
    frames: Query<&GlobalTransform>,
    mut active: Query<(Entity, &mut Ragdoll)>,
    mut bodies: ResMut<RagdollBodies>,
) {
    if ragdolls_off() {
        return;
    }
    let dt = time.delta_secs().max(1e-3);
    for (unit, store, net, rig, unit_tf, spot) in &dying {
        if net.kind != EntityKind::Unit || !store.0.unit_is_dead() {
            continue;
        }
        commands.entity(unit).remove::<SeenAlive>();
        let Some(profile) = build_profile(&rig.binds, &rig.parents) else {
            continue;
        };
        let Ok(frame) = frames.get(rig.joints_root) else {
            continue;
        };
        let world = frame.affine();
        let (_, frame_rot, _) = world.to_scale_rotation_translation();
        let scale = frame.compute_transform().scale.x.abs().max(1e-3);
        let unit_velocity = spot.map_or(Vec3::ZERO, |s| (unit_tf.translation() - s.0) / dt);
        let facing = (unit_tf.rotation() * Vec3::NEG_Z)
            .with_y(0.0)
            .normalize_or_zero();
        let nudge = (-facing + Vec3::Y * 0.3) * DEATH_NUDGE;

        let mut spawned = Vec::with_capacity(profile.bodies.len());
        let mut isos = Vec::with_capacity(profile.bodies.len());
        for body in &profile.bodies {
            let Some(model) = rig.model.get(body.bone as usize) else {
                continue;
            };
            let (_, bone_rot, bone_at) = model.to_scale_rotation_translation();
            let at = world.transform_point3(bone_at);
            let rot = (frame_rot * bone_rot).normalize();
            let len = body.segment.length() * scale;
            let radius = body.radius * scale;
            // The capsule along the segment, in the body's frame (bind rotations are identity, so
            // the bind segment is already bone-local).
            let axis = Quat::from_rotation_arc(Vec3::Y, body.segment / body.segment.length());
            let shape = Collider::compound(vec![(
                Position(body.segment * scale * 0.5),
                Rotation(axis),
                Collider::capsule(radius, (len - 2.0 * radius).max(0.01)),
            )]);
            let id = commands
                .spawn((
                    Name::new("ragdoll body"),
                    Transform::from_translation(at).with_rotation(rot),
                    RigidBody::Dynamic,
                    shape,
                    ragdoll_layers(),
                    LinearVelocity(unit_velocity + nudge),
                    TransformInterpolation,
                ))
                .id();
            spawned.push(id);
            isos.push((at, rot));
        }
        if spawned.len() != profile.bodies.len() {
            for id in spawned {
                commands.entity(id).despawn();
            }
            continue;
        }
        for (k, body) in profile.bodies.iter().enumerate() {
            let Some(p) = body.parent else {
                continue;
            };
            // The joint sits on this body's pivot (its origin), and in the parent body's frame at
            // the same world point. Both bases put +Y down this body's bind segment, so the limits
            // are measured from the bind pose.
            let (parent_at, parent_rot) = isos[p];
            let anchor1 = parent_rot.inverse() * (isos[k].0 - parent_at);
            let basis = Quat::from_rotation_arc(Vec3::Y, body.segment / body.segment.length());
            commands.spawn((
                Name::new("ragdoll joint"),
                SphericalJoint::new(spawned[p], spawned[k])
                    .with_local_anchor1(anchor1)
                    .with_local_anchor2(Vec3::ZERO)
                    .with_local_basis1(basis)
                    .with_local_basis2(basis)
                    .with_swing_limits(-SWING_LIMIT, SWING_LIMIT)
                    .with_twist_limits(-TWIST_LIMIT, TWIST_LIMIT),
            ));
        }
        info!(
            "ragdoll: unit {unit} falls with {} bodies (scale {scale:.2})",
            spawned.len()
        );
        bodies.0.insert(unit, spawned);
        commands.entity(unit).insert(Ragdoll {
            profile,
            started: time.elapsed_secs(),
            frozen: None,
        });
    }

    // Past the cap, the oldest simulating ragdoll settles where it is.
    let mut live: Vec<(Entity, f32)> = active
        .iter()
        .filter(|(_, r)| r.frozen.is_none())
        .map(|(e, r)| (e, r.started))
        .collect();
    if live.len() > MAX_ACTIVE {
        live.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (e, _) in live.iter().take(live.len() - MAX_ACTIVE) {
            if let Ok((_, mut r)) = active.get_mut(*e) {
                r.started = f32::NEG_INFINITY; // `freeze_settled` takes it this frame
            }
        }
    }
}

/// Writes each simulating ragdoll's bodies into its bone locals, after the evaluator and before
/// the compose: a body's bone takes its world pose, every other bone keeps its local and so rides
/// its parent. A frozen ragdoll re-applies its settled locals.
fn drive_bones_from_bodies(
    mut rigs: Query<(Entity, &Ragdoll, &mut RigPose)>,
    frames: Query<&GlobalTransform>,
    body_tfs: Query<&Transform, With<RigidBody>>,
    bodies: Res<RagdollBodies>,
) {
    for (unit, rag, rig) in &mut rigs {
        let rig = rig.into_inner();
        if let Some(frozen) = &rag.frozen {
            for (body, local) in rag.profile.bodies.iter().zip(frozen) {
                if let Some(slot) = rig.locals.get_mut(body.bone as usize) {
                    *slot = *local;
                }
            }
            rig.pose_dirty = true;
            continue;
        }
        let (Ok(frame), Some(ids)) = (frames.get(rig.joints_root), bodies.0.get(&unit)) else {
            continue;
        };
        write_pose(rig, &rag.profile, frame, ids, &body_tfs);
    }
}

/// The body-to-bone write for one rig; see [`drive_bones_from_bodies`].
fn write_pose(
    rig: &mut RigPose,
    profile: &Profile,
    frame: &GlobalTransform,
    ids: &[Entity],
    body_tfs: &Query<&Transform, With<RigidBody>>,
) {
    let world_inv = frame.affine().inverse();
    let (_, frame_rot, _) = frame.affine().to_scale_rotation_translation();
    let frame_rot_inv = frame_rot.inverse();
    let n = rig.locals.len().min(rig.parents.len());
    let mut model = vec![Affine3A::IDENTITY; n];
    let mut body_k = 0;
    for i in 0..n {
        let parent = usize::try_from(rig.parents[i])
            .ok()
            .filter(|&p| p < i)
            .map(|p| model[p]);
        let is_body = profile.physical.get(i).copied().unwrap_or(false);
        if is_body {
            let tf = ids.get(body_k).and_then(|id| body_tfs.get(*id).ok());
            body_k += 1;
            if let Some(tf) = tf {
                let m = Affine3A::from_rotation_translation(
                    (frame_rot_inv * tf.rotation).normalize(),
                    world_inv.transform_point3(tf.translation),
                );
                let local = parent.map_or(m, |p| p.inverse() * m);
                let (_, r, t) = local.to_scale_rotation_translation();
                rig.locals[i] = Transform::from_translation(t).with_rotation(r);
                model[i] = m;
                continue;
            }
        }
        let local = rig.locals[i].compute_affine();
        model[i] = parent.map_or(local, |p| p * local);
    }
    rig.pose_dirty = true;
}

/// Freezes a ragdoll once all its bodies sleep (or [`MAX_SIM_SECS`] passes): its body bones keep
/// their last locals and the bodies and joints go.
fn freeze_settled(
    mut commands: Commands,
    time: Res<Time>,
    mut rigs: Query<(Entity, &mut Ragdoll, &RigPose)>,
    sleeping: Query<Has<Sleeping>>,
    joints: Query<(Entity, &SphericalJoint)>,
    mut bodies: ResMut<RagdollBodies>,
) {
    let now = time.elapsed_secs();
    for (unit, mut rag, rig) in &mut rigs {
        if rag.frozen.is_some() {
            continue;
        }
        let Some(ids) = bodies.0.get(&unit) else {
            continue;
        };
        let asleep = ids.iter().all(|id| sleeping.get(*id).unwrap_or(true));
        if !asleep && now - rag.started < MAX_SIM_SECS {
            continue;
        }
        let frozen = rag
            .profile
            .bodies
            .iter()
            .map(|b| rig.locals.get(b.bone as usize).copied().unwrap_or_default())
            .collect();
        rag.frozen = Some(frozen);
        let ids = bodies.0.remove(&unit).unwrap_or_default();
        despawn_bodies(&mut commands, &ids, &joints);
    }
}

/// A ragdoll whose unit is gone or alive again (a resurrection, a respawn on the same guid) loses
/// its bodies, and a living unit its ragdoll, so its animation takes back over.
fn cleanup(
    mut commands: Commands,
    units: Query<&ObjectStore>,
    rags: Query<(Entity, &ObjectStore), With<Ragdoll>>,
    joints: Query<(Entity, &SphericalJoint)>,
    mut bodies: ResMut<RagdollBodies>,
) {
    for (unit, store) in &rags {
        if !store.0.unit_is_dead() {
            commands.entity(unit).remove::<Ragdoll>();
            if let Some(ids) = bodies.0.remove(&unit) {
                despawn_bodies(&mut commands, &ids, &joints);
            }
        }
    }
    let gone: Vec<Entity> = bodies
        .0
        .keys()
        .copied()
        .filter(|u| units.get(*u).is_err())
        .collect();
    for unit in gone {
        if let Some(ids) = bodies.0.remove(&unit) {
            despawn_bodies(&mut commands, &ids, &joints);
        }
    }
}

fn despawn_bodies(
    commands: &mut Commands,
    ids: &[Entity],
    joints: &Query<(Entity, &SphericalJoint)>,
) {
    for (joint, j) in joints {
        if ids.contains(&j.body1) || ids.contains(&j.body2) {
            commands.entity(joint).despawn();
        }
    }
    for id in ids {
        if let Ok(mut e) = commands.get_entity(*id) {
            e.despawn();
        }
    }
}
