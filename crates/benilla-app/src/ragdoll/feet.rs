//! Fork: foot planting, behind the Physics page's Foot Planting slider (`footPlanting`, 0 off).
//!
//! 1.12 stands every unit on the one ground height under its middle, so on a slope or a step one
//! foot sinks in and the other hangs. Players only: creatures are left as they are. Here each leg probes the ground under its own ankle: the hips
//! sink to the lowest foot's ground, and every other leg bends at hip and knee (two-bone IK) to lift
//! its ankle onto its own ground, the foot turned to the ground's slope. Only the offset between a
//! foot's ground and the middle's counts, never the height of the step itself, so a swinging foot
//! keeps its stride and a rider's feet stay in the stirrups.
//!
//! The legs come from the ragdoll profile ([`build_profile`]): a body that runs down from one that
//! does not is a hip, its body child the knee, and the knee's child at the segment's end the ankle.
//! The edit goes on the animated `locals` in the pose post-pass, remembered so a frame the
//! animation does not rewrite starts from the animated pose again, not from this frame's edit.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_world::rig_anim::{AnimParked, PosePost, RigPose};
use benilla_world::view::WorldCamera;

use super::life::Ragdoll;
use super::rig::build_profile;
use benilla_protocol::messages::ObjectType;

use crate::net::ObjectStore;

/// The slider's top: 1 plants fully.
const MAX_AMOUNT: f32 = 1.0;
/// Units past this far from the camera (yd) are left as they are.
const RANGE: f32 = 60.0;
/// The ankle's ground probe: from this far above the unit's feet to this far below.
const PROBE_UP: f32 = 1.0;
const PROBE_DOWN: f32 = 1.5;
/// The middle's ground more than this (yd) from the unit's feet: in the air or the water.
const AIRBORNE: f32 = 0.3;
/// A foot reaches at most this share of its leg's length up or down from the middle's ground.
const MAX_REACH: f32 = 0.35;
/// How fast (1/s) the hips and feet follow the ground.
const RATE: f32 = 14.0;
/// How much of the ground's slope the foot takes, and the steepest ground (normal y) it follows.
const FOOT_TILT: f32 = 0.6;
const STEEPEST: f32 = 0.6;
/// An offset this small is no edit.
const SETTLED: f32 = 1e-3;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(PostUpdate, plant_feet.in_set(PosePost));
}

/// One leg's bones.
#[derive(Clone, Copy)]
struct Leg {
    hip: usize,
    knee: usize,
    ankle: usize,
}

/// One rig's legs and its planting state.
struct Planted {
    bones: usize,
    hub: usize,
    legs: Vec<Leg>,
    /// Per leg, the smoothed lift (yd) of its ankle; the smoothed sink of the hips.
    lift: Vec<f32>,
    sink: f32,
    /// The locals this wrote last frame and the animated ones they replaced.
    written: Vec<(usize, Transform, Transform)>,
}

/// The rig's legs, or none when it has fewer than two.
fn find_legs(rig: &RigPose) -> Option<(usize, Vec<Leg>)> {
    let profile = build_profile(&rig.binds, &rig.parents)?;
    let n = rig.binds.len().min(rig.parents.len());
    let parent_of = |i: usize| usize::try_from(rig.parents[i]).ok().filter(|&p| p < i);
    let mut pivots = vec![Vec3::ZERO; n];
    for i in 0..n {
        pivots[i] = parent_of(i).map_or(Vec3::ZERO, |p| pivots[p]) + rig.binds[i];
    }
    let down = |s: Vec3| s.y < -0.5 * s.length();
    let bodies = &profile.bodies;
    let mut legs = Vec::new();
    for (b, body) in bodies.iter().enumerate() {
        let Some(parent) = body.parent else {
            continue;
        };
        // A leg hangs straight off the hub (the pelvis); an arm hangs off the shoulder.
        if parent != 0 || !down(body.segment) {
            continue;
        }
        let Some(knee) = bodies
            .iter()
            .find(|k| k.parent == Some(b) && down(k.segment))
        else {
            continue;
        };
        let knee_bone = knee.bone as usize;
        let end = knee.pivot + knee.segment;
        let Some(ankle) = (0..n)
            .filter(|&c| parent_of(c) == Some(knee_bone))
            .min_by(|&a, &c| {
                pivots[a]
                    .distance_squared(end)
                    .total_cmp(&pivots[c].distance_squared(end))
            })
        else {
            continue;
        };
        legs.push(Leg {
            hip: body.bone as usize,
            knee: knee_bone,
            ankle,
        });
    }
    (legs.len() >= 2).then_some((bodies[0].bone as usize, legs))
}

/// Run `change` on `bone`'s local, remembering what it wrote over.
fn edit(
    rig: &mut RigPose,
    bone: usize,
    written: &mut Vec<(usize, Transform, Transform)>,
    change: impl FnOnce(&mut RigPose),
) {
    let base = rig.locals[bone];
    change(rig);
    written.push((bone, rig.locals[bone], base));
}

/// Turn `bone` by the model-space rotation `turn` about its own pivot, in its local.
fn turn_bone(rig: &mut RigPose, bone: usize, turn: Quat) {
    let model = rig.model[bone].to_scale_rotation_translation().1;
    let local = &mut rig.locals[bone];
    local.rotation = (local.rotation * (model.inverse() * turn * model)).normalize();
}

#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn plant_feet(
    mut rigs: Query<
        (Entity, &mut RigPose, &GlobalTransform, &ObjectStore),
        (Without<AnimParked>, Without<Ragdoll>),
    >,
    roots: Query<&GlobalTransform>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    collide: benilla_world::collision::WorldCollision,
    time: Res<Time>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    mut planted: Local<EntityHashMap<Planted>>,
    mut failed: Local<EntityHashMap<usize>>,
) {
    let amount = cvars
        .as_deref()
        .and_then(|c| c.num("footPlanting"))
        .unwrap_or(1.0)
        .clamp(0.0, MAX_AMOUNT);
    let eye = camera.single().ok().map(|g| g.translation());
    let follow = 1.0 - (-RATE * time.delta_secs()).exp();
    // What the body walks on: terrain, buildings and doodads such as rocks.
    let ground = |at: Vec3| {
        collide
            .ray_body(at + Vec3::Y * PROBE_UP, Dir3::NEG_Y, PROBE_UP + PROBE_DOWN)
            .map(|hit| (at.y + PROBE_UP - hit.distance, hit.normal))
    };
    planted.retain(|e, _| rigs.contains(*e));
    failed.retain(|e, _| rigs.contains(*e));

    for (unit, rig, at, store) in &mut rigs {
        let feet = at.translation();
        let n = rig.locals.len().min(rig.model.len());
        // A player standing on the ground, near, alive and on foot plants; any other lets go.
        let wanted = amount > 0.0
            && store.0.object_type() == Some(ObjectType::Player)
            && eye.is_some_and(|eye| eye.distance(feet) < RANGE)
            && store.0.unit_mount_display_id() == 0
            && store.0.unit_health() != Some(0);
        if !planted.contains_key(&unit) {
            if !wanted || failed.get(&unit) == Some(&n) {
                continue;
            }
            let Some((hub, legs)) = find_legs(&rig) else {
                failed.insert(unit, n);
                continue;
            };
            planted.insert(
                unit,
                Planted {
                    bones: n,
                    hub,
                    lift: vec![0.0; legs.len()],
                    legs,
                    sink: 0.0,
                    written: Vec::new(),
                },
            );
        }
        let Some(state) = planted.get_mut(&unit) else {
            continue;
        };
        if state.bones != n {
            planted.remove(&unit);
            continue;
        }
        // Back to the animated pose where the animation left last frame's edit standing.
        let rig = rig.into_inner();
        // Field by field: a clip may rewrite a bone's rotation and leave its translation, and a
        // whole-transform compare would then keep the old edit and add this frame's on top.
        for &(bone, wrote, base) in state.written.iter().rev() {
            let local = &mut rig.locals[bone];
            if local.translation == wrote.translation {
                local.translation = base.translation;
            }
            if local.rotation == wrote.rotation {
                local.rotation = base.rotation;
            }
        }
        let undone = !state.written.is_empty();
        state.written.clear();
        rig.compose();
        let Ok(root) = roots.get(rig.joints_root).map(GlobalTransform::affine) else {
            continue;
        };
        let to_model = root.inverse();

        // The ground under the middle and each ankle, as lifts from the middle's.
        let middle = ground(feet).filter(|&(y, _)| (y - feet.y).abs() < AIRBORNE);
        let mut targets = vec![(0.0, None); state.legs.len()];
        if let (true, Some((centre, _))) = (wanted, middle) {
            for (leg, target) in state.legs.iter().zip(&mut targets) {
                let hip = root.transform_point3(rig.model[leg.hip].translation.into());
                let knee = root.transform_point3(rig.model[leg.knee].translation.into());
                let ankle = root.transform_point3(rig.model[leg.ankle].translation.into());
                let reach = MAX_REACH * (hip.distance(knee) + knee.distance(ankle));
                if let Some((y, normal)) = ground(ankle.with_y(feet.y)) {
                    *target = (
                        ((y - centre) * amount).clamp(-reach, reach),
                        (normal.y >= STEEPEST).then_some(normal),
                    );
                }
            }
        }
        for (lift, &(target, _)) in state.lift.iter_mut().zip(&targets) {
            *lift += (target - *lift) * follow;
        }
        let sink_target = state.lift.iter().copied().fold(0.0f32, f32::min);
        state.sink += (sink_target - state.sink) * follow;
        let settled = state.sink.abs() < SETTLED && state.lift.iter().all(|l| l.abs() < SETTLED);
        if settled {
            if !wanted {
                planted.remove(&unit);
            }
            if undone {
                rig.pose_dirty = true;
            }
            continue;
        }

        // The hips sink to the lowest foot's ground.
        let hub = state.hub;
        let sink = to_model.transform_vector3(Vec3::Y * state.sink);
        let hub_parent = usize::try_from(rig.parents[hub]).ok().filter(|&p| p < hub);
        let sink_local =
            hub_parent.map_or(sink, |p| rig.model[p].inverse().transform_vector3(sink));
        edit(rig, hub, &mut state.written, |rig| {
            rig.locals[hub].translation += sink_local;
        });
        rig.compose();

        // Each leg lifts its ankle the rest of the way to its own ground.
        for (i, leg) in state.legs.clone().into_iter().enumerate() {
            let rise = state.lift[i] - state.sink;
            let normal = targets[i].1;
            if rise < SETTLED && normal.is_none() {
                continue;
            }
            let hip = Vec3::from(rig.model[leg.hip].translation);
            let knee = Vec3::from(rig.model[leg.knee].translation);
            let ankle = Vec3::from(rig.model[leg.ankle].translation);
            let goal = ankle + to_model.transform_vector3(Vec3::Y * rise.max(0.0));
            let (upper, lower) = (hip.distance(knee), knee.distance(ankle));
            let reach = goal
                .distance(hip)
                .clamp((upper - lower).abs() + 1e-3, upper + lower - 1e-3);
            // Bend the knee to the angle that spans the reach (law of cosines).
            let (to_hip, to_ankle) = (hip - knee, ankle - knee);
            let axis = to_hip
                .cross(to_ankle)
                .try_normalize()
                .unwrap_or_else(|| Vec3::from(rig.model[leg.hip].matrix3.x_axis).normalize());
            let now = to_hip.angle_between(to_ankle);
            let want = ((upper * upper + lower * lower - reach * reach) / (2.0 * upper * lower))
                .clamp(-1.0, 1.0)
                .acos();
            let bend = Quat::from_axis_angle(axis, want - now);
            let bent = knee + bend * to_ankle;
            // Swing the whole leg from the hip so the ankle meets the goal.
            let swing = Quat::from_rotation_arc(
                (bent - hip).normalize_or_zero(),
                (goal - hip).normalize_or_zero(),
            );
            let ankle_model = rig.model[leg.ankle].to_scale_rotation_translation().1;
            edit(rig, leg.knee, &mut state.written, |rig| {
                turn_bone(rig, leg.knee, bend)
            });
            edit(rig, leg.hip, &mut state.written, |rig| {
                turn_bone(rig, leg.hip, swing)
            });
            // The foot takes the ground's slope.
            if let Some(normal) = normal {
                let up = to_model.transform_vector3(Vec3::Y).normalize_or_zero();
                let slope = to_model.transform_vector3(normal).normalize_or_zero();
                let tilt = Quat::IDENTITY.slerp(Quat::from_rotation_arc(up, slope), FOOT_TILT);
                // The ankle's model rotation once the hip and knee turned.
                let placed = swing * bend * ankle_model;
                edit(rig, leg.ankle, &mut state.written, |rig| {
                    let local = &mut rig.locals[leg.ankle];
                    local.rotation =
                        (local.rotation * (placed.inverse() * tilt * placed)).normalize();
                });
            }
            rig.compose();
        }
        rig.pose_dirty = true;
    }
}
