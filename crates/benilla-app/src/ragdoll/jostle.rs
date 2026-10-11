//! Fork: stowed weapons jostle as their wearer moves, behind the Physics page's Weapon Motion
//! slider (`weaponMotion`, 0 off).
//!
//! A weapon or shield on the back or the hip hangs rigid from its attachment point in 1.12. Here
//! each one keeps a damped spring on that point's world position, as the body's sway does: the
//! point moves, the spring lags behind it, and the lag tips the item about the point, the part
//! above it trailing the motion and a bounce tilting it about the body's side axis. The tilt goes
//! on the item root's `Transform`, which the collision, picking and glow read, and on its
//! [`RiderTilt`], which the rendered rider frame takes. A drawn weapon is left alone.

use bevy::ecs::entity::EntityHashMap;
use bevy::math::Affine3A;
use bevy::prelude::*;

use benilla_world::rig_rider::{RiderTilt, RigRider};

use crate::entities::{attach_id, BoneAttach, HeldAttached};

/// The spring: its frequency (Hz) and damping ratio, its step (s) and most steps per frame.
const FREQUENCY: f32 = 4.0;
const DAMPING: f32 = 0.25;
const STEP: f32 = 1.0 / 120.0;
const MAX_STEPS: u32 = 8;
/// The share of the attachment point's motion the spring lags, and the furthest it lags (yd) at
/// Weapon Motion 1.
const LAG: f32 = 0.6;
const MAX_LAG: f32 = 0.08;
/// The lever the lag tips the item by (yd): a lag this long is a radian.
const ARM: f32 = 0.35;
/// The most the item tips (rad) at Weapon Motion 1.
const MAX_TILT: f32 = 0.3;
/// The Weapon Motion slider's top.
const MAX_AMOUNT: f32 = 3.0;
/// A jump of the point past this (yd) in one frame is a teleport: the spring starts over.
const TELEPORT: f32 = 2.0;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        jostle_stowed.before(bevy::transform::TransformSystems::Propagate),
    );
}

/// One stowed item's spring.
struct Spring {
    attach: u16,
    last: Option<Vec3>,
    offset: Vec3,
    velocity: Vec3,
}

/// The stowed attach points: the back, the lower back and the hips, never a hand.
fn stowed(attach: u16) -> bool {
    use attach_id::*;
    matches!(
        attach,
        BACK_RIGHT
            | BACK_LEFT
            | SHIELD_BACK
            | BACK_LOWER_MAIN
            | BACK_LOWER_OFF
            | HIP_MAIN
            | HIP_OFF
    )
}

fn jostle_stowed(
    units: Query<(&HeldAttached, &BoneAttach, &GlobalTransform)>,
    mut roots: Query<(
        &mut Transform,
        &ChildOf,
        Has<RigRider>,
        Option<&mut RiderTilt>,
    )>,
    anchors: Query<&GlobalTransform, Without<HeldAttached>>,
    time: Res<Time>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    mut springs: Local<EntityHashMap<Spring>>,
    mut seen: Local<Vec<Entity>>,
    mut commands: Commands,
) {
    let amount = cvars
        .as_deref()
        .and_then(|c| c.num("weaponMotion"))
        .unwrap_or(1.0)
        .clamp(0.0, MAX_AMOUNT);
    let dt = time.delta_secs();
    let stiffness = (std::f32::consts::TAU * FREQUENCY).powi(2);
    let damping = 2.0 * DAMPING * stiffness.sqrt();
    seen.clear();
    for (held, bones, unit) in &units {
        let side = unit.right().as_vec3();
        // Only the weapon slots: mainhand and offhand.
        for (_, root, attach) in held.spawned_attach().filter(|&(slot, _, _)| slot < 2) {
            if amount <= 0.0 || !stowed(attach) {
                continue;
            }
            let Some(&(_, offset)) = bones.points.get(&attach) else {
                continue;
            };
            let Ok((mut transform, parent, rider, mut rider_tilt)) = roots.get_mut(root) else {
                continue;
            };
            let Ok(anchor) = anchors.get(parent.parent()) else {
                continue;
            };
            seen.push(root);
            let spring = springs.entry(root).or_insert(Spring {
                attach,
                last: None,
                offset: Vec3::ZERO,
                velocity: Vec3::ZERO,
            });
            if spring.attach != attach {
                *spring = Spring {
                    attach,
                    last: None,
                    offset: Vec3::ZERO,
                    velocity: Vec3::ZERO,
                };
            }
            let point = anchor.transform_point(offset);
            let moved = spring.last.map(|last| point - last);
            spring.last = Some(point);
            match moved {
                Some(moved) if moved.length() < TELEPORT && dt > 0.0 => {
                    spring.offset -= moved * LAG;
                    let steps = ((dt / STEP).ceil() as u32).clamp(1, MAX_STEPS);
                    let h = dt / steps as f32;
                    for _ in 0..steps {
                        let accel = -stiffness * spring.offset - damping * spring.velocity;
                        spring.velocity += accel * h;
                        spring.offset += spring.velocity * h;
                    }
                    if spring.offset.length() > MAX_LAG {
                        spring.offset = spring.offset.normalize_or_zero() * MAX_LAG;
                        spring.velocity *= 0.5;
                    }
                }
                _ => {
                    spring.offset = Vec3::ZERO;
                    spring.velocity = Vec3::ZERO;
                }
            }
            // A sideways or forward lag tips the part above the point back along it; a bounce
            // tips the item about the body's side axis.
            let lag = spring.offset * amount;
            let level = Vec3::new(lag.x, 0.0, lag.z);
            let axis = (Vec3::Y.cross(level) - side * lag.y) / ARM;
            let axis = axis.clamp_length_max(MAX_TILT * amount);
            let (_, parent_rot, _) = anchor.to_scale_rotation_translation();
            let tilt = parent_rot.inverse() * Quat::from_scaled_axis(axis) * parent_rot;
            let want = Transform::from_translation(offset).with_rotation(tilt);
            if *transform != want {
                *transform = want;
            }
            let placed = Affine3A::from_quat(tilt);
            match rider_tilt.as_deref_mut() {
                Some(RiderTilt(now)) => *now = placed,
                None if rider => {
                    commands.entity(root).insert(RiderTilt(placed));
                }
                None => {}
            }
        }
    }
    // Items no longer stowed, swapped out, or with the slider at 0: back to the stock placement.
    let gone: Vec<Entity> = springs
        .keys()
        .copied()
        .filter(|e| !seen.contains(e))
        .collect();
    for root in gone {
        springs.remove(&root);
        if let Ok((mut transform, _, _, _)) = roots.get_mut(root) {
            transform.rotation = Quat::IDENTITY;
        }
        if let Ok(mut entity) = commands.get_entity(root) {
            entity.remove::<RiderTilt>();
        }
    }
}
