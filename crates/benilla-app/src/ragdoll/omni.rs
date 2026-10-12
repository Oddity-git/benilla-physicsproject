//! Fork: turn around instead of backpedalling, behind the Physics page's Turn Around on Back box
//! (`omniMovement`, off by default).
//!
//! 1.12 walks the character backward at a slower pace while S is held. With the box on, and the
//! right mouse button up, pressing S (or pulling a gamepad stick down) turns the character about
//! and runs it forward at full pace while S stays held; once it has turned, S drives like W and
//! A/D, Q/E and the camera work as they always do. Holding the right mouse button is the stock
//! backpedal, untouched.
//!
//! The player controller calls [`steer`] after it reads the keys (its one `Fork:` hook); the facing
//! it sets streams to the server as any facing change does (`SET_FACING`).

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use bevy::prelude::*;

/// How fast the character turns about (rad/s): a half turn takes about a quarter second.
const TURN_RATE: f32 = 12.0;
/// A turn this close (rad) to its direction is done.
const DONE: f32 = 0.01;

/// The box, read once a frame.
static ON: AtomicBool = AtomicBool::new(false);
/// S held last frame, so pressing it is an edge.
static BACKING: AtomicBool = AtomicBool::new(false);
/// Turning about, toward [`TARGET`] (f32 bits).
static TURNING: AtomicBool = AtomicBool::new(false);
static TARGET: AtomicU32 = AtomicU32::new(0);

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

/// What the controller does this frame instead of backpedalling.
pub(crate) struct Steer {
    /// The character's new facing while it turns about; `None` once turned, when the turn keys
    /// work as usual.
    pub(crate) face_yaw: Option<f32>,
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

/// `fwd` is the W/S axis (−1 backward). `None` leaves the stock controls alone: the box off, the
/// right button held or movement locked (`stock`), or S up. Otherwise the controller runs forward,
/// turning the facing about first.
pub(crate) fn steer(fwd: i32, stock: bool, face_yaw: f32, dt: f32) -> Option<Steer> {
    if !ON.load(Ordering::Relaxed) || stock || fwd >= 0 {
        BACKING.store(false, Ordering::Relaxed);
        TURNING.store(false, Ordering::Relaxed);
        return None;
    }
    if !BACKING.swap(true, Ordering::Relaxed) {
        TARGET.store(wrap(face_yaw + PI).to_bits(), Ordering::Relaxed);
        TURNING.store(true, Ordering::Relaxed);
    }
    if !TURNING.load(Ordering::Relaxed) {
        return Some(Steer { face_yaw: None });
    }
    let left = wrap(f32::from_bits(TARGET.load(Ordering::Relaxed)) - face_yaw);
    if left.abs() < DONE {
        TURNING.store(false, Ordering::Relaxed);
    }
    let budget = TURN_RATE * dt;
    Some(Steer {
        face_yaw: Some(face_yaw + left.clamp(-budget, budget)),
    })
}
