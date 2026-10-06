//! A ragdoll's life: a unit seen alive that dies gets bodies at its current bone pose and the
//! bodies drive the bones. Settled bodies sleep but stay, so a passing player can shove the corpse;
//! a ragdoll that never settles, or the oldest past the cap, freezes its pose and the bodies go.
//! A unit that streams in already dead keeps its Death pose: nothing was seen to fall.

use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashMap;
use bevy::math::Affine3A;
use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_world::collision::{ragdoll_layers, RagdollIgnore};
use benilla_world::rig_anim::RigPose;

use super::blow::LastHit;
use super::rig::{build_profile, Profile};
use crate::creature_anim::AnimDriver;
use crate::entities::mount::MountBody;
use crate::net::{GuidIndex, NetEntity, ObjectStore};

/// Joint limits about the bind pose: how far a limb may swing off its bind direction, and twist.
const SWING_LIMIT: f32 = 55.0_f32.to_radians();
const TWIST_LIMIT: f32 = 20.0_f32.to_radians();
/// Damping of each joint's relative spin, so limbs stop swinging instead of sloshing.
const JOINT_DAMPING: f32 = 4.0;
/// Each body's own damping: a little air drag on its spin and fall.
const BODY_ANGULAR_DAMPING: f32 = 0.8;
const BODY_LINEAR_DAMPING: f32 = 0.1;
/// A body's mass is its segment's share of the skeleton's height, floored so a hand is never so
/// light the chain whips it, and the hub (the pelvis) weighs this many times its share.
const MASS_FLOOR: f32 = 0.12;
const HUB_MASS: f32 = 2.5;
/// A ragdoll still moving after this long awake freezes where it is.
const MAX_SIM_SECS: f32 = 12.0;
/// Most ragdolls keeping bodies (moving or asleep) at once; past it the oldest freezes.
const MAX_LIVE: usize = 16;
/// The base push at death (yd/s), away from the killer, else from where the unit faced: full at
/// the head, a quarter at the feet, so the body topples backwards instead of sliding; plus a little lift.
const DEATH_PUSH: f32 = 2.5;
/// Added to the push per whole health bar the killing blow took (yd/s): a blow for half the
/// unit's health adds half of it. The push never exceeds [`MAX_PUSH`].
const BLOW_PUSH: f32 = 14.0;
const MAX_PUSH: f32 = 12.0;
/// A hit older than this when the unit reads dead was not the killing blow (s).
const BLOW_WINDOW: f32 = 2.0;
const DEATH_LIFT: f32 = 0.5;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<RagdollBodies>()
        .add_systems(Update, (track_unit_motion, start_ragdolls).chain())
        .add_systems(FixedUpdate, release_parted_pairs)
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
    /// When it fell, for the cap's oldest-first order.
    born: f32,
    /// When its bodies last all slept (or it fell): awake past [`MAX_SIM_SECS`] from here freezes.
    awake_since: f32,
    /// Each root bone (a bone with no parent) and its pose relative to the hub's at death. A root
    /// carries every bone outside the body tree (a quadruped's front half, a tail, a cloak), so it
    /// rides the hub instead of holding the death animation where the unit stood.
    anchors: Vec<(usize, Affine3A)>,
    /// Every bone local once frozen; `None` while the bodies simulate.
    frozen: Option<Vec<Transform>>,
}

/// A ragdoll body's capsule in its own frame: from its origin to `end`, this thick (yd).
#[derive(Component, Clone, Copy)]
struct Capsule {
    end: Vec3,
    radius: f32,
}

impl Capsule {
    /// The capsule's segment in the world, from the body's pose.
    fn segment(&self, at: Vec3, rot: Quat) -> (Vec3, Vec3) {
        (at, at + rot * self.end)
    }
}

/// Two bodies' capsules overlap, with a little slack so a pair that only grazes at spawn is
/// still let through until it parts.
fn capsules_overlap(a: (Capsule, Vec3, Quat), b: (Capsule, Vec3, Quat)) -> bool {
    let (a0, a1) = a.0.segment(a.1, a.2);
    let (b0, b1) = b.0.segment(b.1, b.2);
    segment_distance(a0, a1, b0, b1) < (a.0.radius + b.0.radius) * 1.1
}

/// The closest distance between segments `p0..p1` and `q0..q1`.
fn segment_distance(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> f32 {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    let (s, t) = if a <= f32::EPSILON && e <= f32::EPSILON {
        (0.0, 0.0)
    } else if a <= f32::EPSILON {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e <= f32::EPSILON {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s = if denom > f32::EPSILON {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    ((p0 + d1 * s) - (q0 + d2 * t)).length()
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
            Option<&LastHit>,
        ),
        (
            With<SeenAlive>,
            With<AnimDriver>,
            Without<Ragdoll>,
            Without<MountBody>,
        ),
    >,
    frames: Query<&GlobalTransform>,
    index: Res<GuidIndex>,
    mut active: Query<(Entity, &mut Ragdoll)>,
    mut bodies: ResMut<RagdollBodies>,
) {
    if ragdolls_off() {
        return;
    }
    let dt = time.delta_secs().max(1e-3);
    for (unit, store, net, rig, unit_tf, spot, blow) in &dying {
        // Creatures and players alike, our own character included; a ghost reads alive again and
        // `cleanup` hands it back to its animation.
        let living_kind = matches!(net.kind, EntityKind::Unit | EntityKind::Player);
        if !living_kind || !store.0.unit_is_dead() {
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
        // Away from the killer, flat; backwards from the facing when there is none to hand.
        let away = blow
            .filter(|b| time.elapsed_secs() - b.at <= BLOW_WINDOW)
            .and_then(|b| index.0.get(&b.attacker))
            .and_then(|&a| frames.get(a).ok())
            .and_then(|a| {
                (unit_tf.translation() - a.translation())
                    .with_y(0.0)
                    .try_normalize()
            })
            .unwrap_or(-facing);
        let max_health = store.0.unit_max_health().unwrap_or(0).max(1) as f32;
        let blow_share = blow
            .filter(|b| time.elapsed_secs() - b.at <= BLOW_WINDOW)
            .map_or(0.0, |b| (b.damage as f32 / max_health).min(1.0));
        let strength = (DEATH_PUSH + BLOW_PUSH * blow_share).min(MAX_PUSH);
        let (lo, hi) = profile
            .bodies
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), b| {
                (lo.min(b.pivot.y), hi.max(b.pivot.y))
            });
        let span = (hi - lo).max(f32::EPSILON);

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
            let up = ((body.pivot.y - lo) / span).clamp(0.0, 1.0);
            let push = away * strength * (0.25 + 0.75 * up) + Vec3::Y * DEATH_LIFT;
            let share = (body.segment.length() / span).max(MASS_FLOOR);
            let mass = if body.parent.is_none() {
                share * HUB_MASS
            } else {
                share
            };
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
                    ActiveCollisionHooks::FILTER_PAIRS,
                    Capsule {
                        end: body.segment * scale,
                        radius,
                    },
                    Mass(mass),
                    LinearDamping(BODY_LINEAR_DAMPING),
                    AngularDamping(BODY_ANGULAR_DAMPING),
                    LinearVelocity(unit_velocity + push),
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
                JointDamping {
                    linear: 0.0,
                    angular: JOINT_DAMPING,
                },
            ));
        }
        // A rig's own bodies collide, but never across a joint, and a pair overlapping now only
        // once it has parted (`release_parted_pairs`).
        let capsules: Vec<(Capsule, Vec3, Quat)> = profile
            .bodies
            .iter()
            .zip(&isos)
            .map(|(b, &(at, rot))| {
                let capsule = Capsule {
                    end: b.segment * scale,
                    radius: b.radius * scale,
                };
                (capsule, at, rot)
            })
            .collect();
        for k in 0..spawned.len() {
            let mut ignore = RagdollIgnore::default();
            for j in 0..spawned.len() {
                if j == k {
                    continue;
                }
                // Across a joint, or two limbs off the same body (a spider's legs, the thighs).
                let jointed = profile.bodies[k].parent == Some(j)
                    || profile.bodies[j].parent == Some(k)
                    || (profile.bodies[k].parent.is_some()
                        && profile.bodies[k].parent == profile.bodies[j].parent);
                if jointed {
                    ignore.joined.push(spawned[j]);
                } else if capsules_overlap(capsules[k], capsules[j]) {
                    ignore.overlapping.push(spawned[j]);
                }
            }
            commands.entity(spawned[k]).insert(ignore);
        }
        let hub = profile.bodies[0].bone as usize;
        let anchors = match rig.model.get(hub) {
            Some(hub_model) => {
                let hub_inv = hub_model.inverse();
                rig.parents
                    .iter()
                    .enumerate()
                    .filter(|&(i, &p)| p < 0 && i != hub)
                    .filter_map(|(i, _)| rig.model.get(i).map(|m| (i, hub_inv * *m)))
                    .collect()
            }
            None => Vec::new(),
        };
        // Each body as `bone<parent bone`, so a model that falls badly can be read off the log.
        let layout: Vec<String> = profile
            .bodies
            .iter()
            .map(|b| match b.parent {
                Some(p) => format!("{}<{}", b.bone, profile.bodies[p].bone),
                None => format!("{}(hub)", b.bone),
            })
            .collect();
        info!(
            "ragdoll: unit {unit} falls with {} bodies (scale {scale:.2}): {}",
            spawned.len(),
            layout.join(" ")
        );
        bodies.0.insert(unit, spawned);
        commands.entity(unit).insert(Ragdoll {
            profile,
            born: time.elapsed_secs(),
            awake_since: time.elapsed_secs(),
            anchors,
            frozen: None,
        });
    }

    // Past the cap, the oldest ragdoll with bodies freezes where it is.
    let mut live: Vec<(Entity, f32)> = active
        .iter()
        .filter(|(_, r)| r.frozen.is_none())
        .map(|(e, r)| (e, r.born))
        .collect();
    if live.len() > MAX_LIVE {
        live.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (e, _) in live.iter().take(live.len() - MAX_LIVE) {
            if let Ok((_, mut r)) = active.get_mut(*e) {
                r.awake_since = f32::NEG_INFINITY; // `freeze_settled` takes it this frame
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
            for (slot, local) in rig.locals.iter_mut().zip(frozen) {
                *slot = *local;
            }
            rig.pose_dirty = true;
            continue;
        }
        let (Ok(frame), Some(ids)) = (frames.get(rig.joints_root), bodies.0.get(&unit)) else {
            continue;
        };
        write_pose(rig, rag, frame, ids, &body_tfs);
    }
}

/// The body-to-bone write for one rig; see [`drive_bones_from_bodies`].
fn write_pose(
    rig: &mut RigPose,
    rag: &Ragdoll,
    frame: &GlobalTransform,
    ids: &[Entity],
    body_tfs: &Query<&Transform, With<RigidBody>>,
) {
    let profile = &rag.profile;
    let world_inv = frame.affine().inverse();
    let (_, frame_rot, _) = frame.affine().to_scale_rotation_translation();
    let frame_rot_inv = frame_rot.inverse();
    let body_model = |tf: &Transform| {
        Affine3A::from_rotation_translation(
            (frame_rot_inv * tf.rotation).normalize(),
            world_inv.transform_point3(tf.translation),
        )
    };
    // The roots follow the hub body, so the bones outside the body tree come along.
    if let Some(hub) = ids.first().and_then(|id| body_tfs.get(*id).ok()) {
        let hub = body_model(hub);
        for &(i, rel) in &rag.anchors {
            let (_, r, t) = (hub * rel).to_scale_rotation_translation();
            if let Some(slot) = rig.locals.get_mut(i) {
                slot.translation = t;
                slot.rotation = r;
            }
        }
    }
    let n = rig.locals.len().min(rig.parents.len());
    let mut model = vec![Affine3A::IDENTITY; n];
    for i in 0..n {
        let parent = usize::try_from(rig.parents[i])
            .ok()
            .filter(|&p| p < i)
            .map(|p| model[p]);
        let body = profile.body_of.get(i).copied().flatten();
        if let Some(k) = body {
            let tf = ids.get(k).and_then(|id| body_tfs.get(*id).ok());
            if let Some(tf) = tf {
                let m = body_model(tf);
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

/// Freezes a ragdoll awake for [`MAX_SIM_SECS`] without settling (or pushed past the cap): its
/// body bones keep their last locals and the bodies and joints go. One whose bodies all sleep keeps
/// them, its clock reset, so a player's push can wake it.
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
        if asleep && rag.awake_since.is_finite() {
            rag.awake_since = now;
            continue;
        }
        if now - rag.awake_since < MAX_SIM_SECS {
            continue;
        }
        rag.frozen = Some(rig.locals.clone());
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

/// Lets a rig's bodies that overlapped at spawn collide once their capsules have parted, so an arm
/// folded against the chest at death is let go but cannot pass back through it.
fn release_parted_pairs(
    mut bodies: Query<(&mut RagdollIgnore, &Capsule, &Position, &Rotation)>,
    poses: Query<(&Capsule, &Position, &Rotation)>,
) {
    for (mut ignore, capsule, at, rot) in &mut bodies {
        if ignore.overlapping.is_empty() {
            continue;
        }
        let me = (*capsule, at.0, rot.0);
        ignore.overlapping.retain(|other| {
            poses
                .get(*other)
                .is_ok_and(|(c, p, r)| capsules_overlap(me, (*c, p.0, r.0)))
        });
    }
}
