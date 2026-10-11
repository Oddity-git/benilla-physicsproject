//! Fork: omni-directional movement, behind the Physics page's Omni-Directional Movement box
//! (`omniMovement`, off by default).
//!
//! 1.12 walks the character along its own facing: S backpedals and A/D turn. With the box on, and
//! the right mouse button up, the movement keys name a direction from the camera instead: W runs
//! away from it, S toward it, A/D and Q/E to its sides, and diagonals between. The character turns
//! to that direction and runs forward, never backpedalling. Letting go in combat turns it back to
//! where the camera looks, so it faces what it fights. Holding the right mouse button is the stock
//! steering, untouched.
//!
//! The player controller calls [`steer`] after it reads the keys (its one `Fork:` hook); the facing
//! it sets streams to the server as any facing change does (`SET_FACING`).

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;

/// How fast the character turns to its direction (rad/s): a turn about takes a quarter second.
const TURN_RATE: f32 = 12.0;
/// A turn this close (rad) to its direction is done.
const DONE: f32 = 0.01;

/// The box, read once a frame.
static ON: AtomicBool = AtomicBool::new(false);
/// Steering last frame, so letting go is an edge.
static MOVING: AtomicBool = AtomicBool::new(false);
/// Turning back to the camera after letting go in combat.
static FACING_CAMERA: AtomicBool = AtomicBool::new(false);

pub(super) fn plugin(app: &mut App) {
    app.add_systems(PreUpdate, read_box);
}

fn read_box(cvars: Option<Res<crate::cvars::Cvars>>) {
    let on = cvars
        .as_deref()
        .and_then(|c| c.flag("omniMovement"))
        .unwrap_or(false);
    ON.store(on, Ordering::Relaxed);
}

/// What the controller does this frame instead of the stock axes.
pub(crate) struct Steer {
    /// The character's new facing.
    pub(crate) face_yaw: f32,
    /// Run forward along it.
    pub(crate) run: bool,
}

/// `angle` wrapped into (−π, π].
fn wrap(angle: f32) -> f32 {
    let a = (angle + PI).rem_euclid(2.0 * PI) - PI;
    if a <= -PI {
        a + 2.0 * PI
    } else {
        a
    }
}

/// The facing turned toward `target` by at most `TURN_RATE · dt`.
fn turn_toward(face: f32, target: f32, dt: f32) -> f32 {
    let left = wrap(target - face);
    let budget = TURN_RATE * dt;
    face + left.clamp(-budget, budget)
}

/// Steer from the camera: `fwd` is W/S (+1 away from the camera), `right` the A/D and Q/E keys
/// (+1 right). `None` leaves the stock controls alone: the box off, the right button held
/// (`stock`), or nothing to do. The yaws are the camera's and the character's, one convention.
pub(crate) fn steer(
    fwd: i32,
    right: i32,
    stock: bool,
    cam_yaw: f32,
    face_yaw: f32,
    in_combat: bool,
    dt: f32,
) -> Option<Steer> {
    if !ON.load(Ordering::Relaxed) || stock {
        MOVING.store(false, Ordering::Relaxed);
        FACING_CAMERA.store(false, Ordering::Relaxed);
        return None;
    }
    let (fwd, right) = (fwd.signum(), right.signum());
    if fwd != 0 || right != 0 {
        MOVING.store(true, Ordering::Relaxed);
        FACING_CAMERA.store(false, Ordering::Relaxed);
        // Facing yaw grows leftward; forward is −Z. Away from the camera is its yaw, right of it
        // is a quarter turn less.
        let target = cam_yaw + (-(right as f32)).atan2(fwd as f32);
        return Some(Steer {
            face_yaw: turn_toward(face_yaw, target, dt),
            run: true,
        });
    }
    if MOVING.swap(false, Ordering::Relaxed) && in_combat {
        FACING_CAMERA.store(true, Ordering::Relaxed);
    }
    if !FACING_CAMERA.load(Ordering::Relaxed) {
        return None;
    }
    if wrap(cam_yaw - face_yaw).abs() < DONE {
        FACING_CAMERA.store(false, Ordering::Relaxed);
    }
    Some(Steer {
        face_yaw: turn_toward(face_yaw, cam_yaw, dt),
        run: false,
    })
}
