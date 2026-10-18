//! Fork: stowed weapons jostle as their wearer moves, behind the Physics page's Weapon Motion
//! slider (`weaponMotion`, 0 off).
//!
//! A weapon or shield on the back or the hip hangs rigid from its attachment point in 1.12. Here
//! each one keeps a damped spring on that point's height, as the body's sway does: each footstep
//! moves the point up and down, the spring lags behind it, and the item is shifted by the lag,
//! straight up and down and never tipped. The shift goes on the item root's `Transform`, which
//! the collision, picking and glow read, and on its [`RiderTilt`], which the rendered rider frame
//! takes. A drawn weapon is left alone.

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
/// The share of the attachment point's rise and fall the spring lags, and the furthest the item
/// strays from its spot (yd) at Weapon Motion 1.
const LAG: f32 = 0.6;
const MAX_BOUNCE: f32 = 0.025;
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
    last: Option<f32>,
    offset: f32,
    velocity: f32,
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
    units: Query<(&HeldAttached, &BoneAttach)>,
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
    for (held, bones) in &units {
        // Only the weapon slots: mainhand and offhand.
        for (_, root, attach) in held.spawned_attach().filter(|&(slot, _, _)| slot < 2) {
            if !stowed(attach) {
                continue;
            }
            let Some(&(_, offset)) = bones.points.get(&attach) else {
                continue;
            };
            let Ok((mut transform, parent, rider, mut rider_tilt)) = roots.get_mut(root) else {
                continue;
            };
            // Off: back to the stock spot once, then left alone.
            if amount <= 0.0 {
                if springs.remove(&root).is_some() {
                    *transform = Transform::from_translation(offset);
                    commands.entity(root).try_remove::<RiderTilt>();
                }
                continue;
            }
            let Ok(anchor) = anchors.get(parent.parent()) else {
                continue;
            };
            seen.push(root);
            let spring = springs.entry(root).or_insert(Spring {
                attach,
                last: None,
                offset: 0.0,
                velocity: 0.0,
            });
            if spring.attach != attach {
                *spring = Spring {
                    attach,
                    last: None,
                    offset: 0.0,
                    velocity: 0.0,
                };
            }
            let height = anchor.transform_point(offset).y;
            let moved = spring.last.map(|last| height - last);
            spring.last = Some(height);
            match moved {
                Some(moved) if moved.abs() < TELEPORT && dt > 0.0 => {
                    spring.offset -= moved * LAG;
                    let steps = ((dt / STEP).ceil() as u32).clamp(1, MAX_STEPS);
                    let h = dt / steps as f32;
                    for _ in 0..steps {
                        let accel = -stiffness * spring.offset - damping * spring.velocity;
                        spring.velocity += accel * h;
                        spring.offset += spring.velocity * h;
                    }
                    if spring.offset.abs() > MAX_BOUNCE {
                        spring.offset = spring.offset.clamp(-MAX_BOUNCE, MAX_BOUNCE);
                        spring.velocity *= 0.5;
                    }
                }
                _ => {
                    spring.offset = 0.0;
                    spring.velocity = 0.0;
                }
            }
            // The lag, straight up or down in the world, in the attach bone's frame.
            let bounce = anchor
                .affine()
                .inverse()
                .transform_vector3(Vec3::Y * (spring.offset * amount));
            let want = Transform::from_translation(offset + bounce);
            if *transform != want {
                *transform = want;
            }
            let placed = Affine3A::from_translation(bounce);
            match rider_tilt.as_deref_mut() {
                Some(RiderTilt(now)) => *now = placed,
                None if rider => {
                    commands.entity(root).try_insert(RiderTilt(placed));
                }
                None => {}
            }
        }
    }
    // Items drawn or swapped out: the move or the rebuild placed them anew, so only the extra
    // placement goes.
    let gone: Vec<Entity> = springs
        .keys()
        .copied()
        .filter(|e| !seen.contains(e))
        .collect();
    for root in gone {
        springs.remove(&root);
        if let Ok(mut entity) = commands.get_entity(root) {
            entity.remove::<RiderTilt>();
        }
    }
}
