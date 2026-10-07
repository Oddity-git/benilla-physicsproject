//! Fork: hitstop. When a melee swing lands, at its impact key (the frame the stock spurt and its
//! flash fire), the attacker's and the victim's animations hold for a moment, which sells the
//! weight of the hit. Longer on a crit and on a hit that takes a big share of the victim's health.
//! Only the drawn pose holds: the clips, the animation driver and the swing sounds keyed to them
//! run on untouched (pausing the clips themselves left the driver out of step: units walked in
//! place and swing sounds drifted), so the model catches up when the hold ends.

use bevy::prelude::*;

use benilla_world::rig_anim::{PosePost, RigPose};

use super::life::Ragdoll;
use crate::creature_anim::SwingImpact;
use crate::net::ObjectStore;

/// The hold on a plain hit and on a crit (s), plus up to this much more for a hit that takes the
/// victim's whole health bar.
const STOP: f32 = 0.06;
const CRIT_STOP: f32 = 0.12;
const SHARE_STOP: f32 = 0.08;
/// The longest hold, whatever the strength.
const MAX_STOP: f32 = 0.3;
/// `hitInfo`'s crit bit.
const HITINFO_CRITICAL: u32 = 0x80;
/// The hit landed (`0x2`).
const HITINFO_NORMALSWING: u32 = 0x2;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, start_stops)
        .add_systems(PostUpdate, hold_stops.in_set(PosePost));
}

/// This unit's drawn pose holds until `until`: the pose it had on the first held frame.
#[derive(Component)]
struct HitStop {
    until: f32,
    pose: Option<Vec<Transform>>,
}

/// Each landed swing stops both sides, if the option is on.
fn start_stops(
    mut commands: Commands,
    mut impacts: MessageReader<SwingImpact>,
    stores: Query<&ObjectStore>,
    stops: Query<&HitStop>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    time: Res<Time>,
) {
    let cvars = cvars.as_deref();
    let on = cvars.and_then(|c| c.flag("hitstop")).unwrap_or(true);
    let strength = cvars
        .and_then(|c| c.num("hitstopStrength"))
        .map_or(1.0, |v| v.clamp(0.0, 3.0));
    if !on || strength <= 0.0 {
        impacts.clear();
        return;
    }
    let now = time.elapsed_secs();
    for ev in impacts.read() {
        let swing = &ev.swing;
        // A hit or a block that dealt damage; a miss, dodge or parry never touched.
        if ev.text_only
            || swing.hit_info & HITINFO_NORMALSWING == 0
            || swing.damage == 0
            || !matches!(swing.victim_state, 1 | 5)
        {
            continue;
        }
        let Some(victim) = swing.victim else {
            continue;
        };
        let share = stores
            .get(victim)
            .ok()
            .and_then(|s| s.0.unit_max_health())
            .map_or(0.0, |max| {
                (swing.damage as f32 / max.max(1) as f32).min(1.0)
            });
        let base = if swing.hit_info & HITINFO_CRITICAL != 0 {
            CRIT_STOP
        } else {
            STOP
        };
        let hold = ((base + SHARE_STOP * share) * strength).min(MAX_STOP);
        for unit in [swing.attacker, victim] {
            // A stop already running keeps its pose and runs to the later end.
            let until = stops.get(unit).map_or(0.0, |s| s.until).max(now + hold);
            match stops.get(unit) {
                Ok(_) => {
                    commands.entity(unit).queue(move |mut e: EntityWorldMut| {
                        if let Some(mut stop) = e.get_mut::<HitStop>() {
                            stop.until = until;
                        }
                    });
                }
                Err(_) => {
                    commands
                        .entity(unit)
                        .try_insert(HitStop { until, pose: None });
                }
            }
        }
    }
}

/// Holds every stopped unit's drawn pose: the first held frame keeps the pose the clips gave it,
/// and every frame after shows that pose again until the stop ends. A ragdoll is left alone.
fn hold_stops(
    mut commands: Commands,
    mut units: Query<(Entity, &mut HitStop, &mut RigPose), Without<Ragdoll>>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    for (unit, mut stop, mut rig) in &mut units {
        if now >= stop.until {
            commands.entity(unit).remove::<HitStop>();
            continue;
        }
        match &stop.pose {
            None => stop.pose = Some(rig.locals.clone()),
            Some(pose) => {
                let n = rig.locals.len().min(pose.len());
                rig.locals[..n].copy_from_slice(&pose[..n]);
                rig.pose_dirty = true;
            }
        }
    }
}
