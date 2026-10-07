//! Fork: hitstop. When a melee swing lands, at its impact key (the frame the stock spurt and its
//! flash fire), the attacker's and the victim's animations hold for a moment, which sells the
//! weight of the hit. Longer on a crit and on a hit that takes a big share of the victim's health.
//! Only the animation clocks stop: movement and the server's timing run on.

use bevy::prelude::*;

use crate::aura_visual::AnimRateFreeze;
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
    app.add_systems(
        Update,
        (start_stops, hold_stops)
            .chain()
            .after(crate::aura_visual::apply_aura_anim_rate),
    );
}

/// This unit's animations hold until `until`.
#[derive(Component)]
struct HitStop {
    until: f32,
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
            let until = stops.get(unit).map_or(0.0, |s| s.until).max(now + hold);
            commands.entity(unit).try_insert(HitStop { until });
        }
    }
}

/// Holds every stopped unit's clips, reasserted each frame so a clip armed mid-stop holds too, and
/// lets them go when the stop ends. A unit a freeze aura holds is left to the aura.
fn hold_stops(
    mut commands: Commands,
    mut units: Query<(Entity, &HitStop, &mut AnimationPlayer, Has<AnimRateFreeze>)>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    for (unit, stop, mut player, frozen) in &mut units {
        if now >= stop.until {
            if !frozen {
                for (_, anim) in player.playing_animations_mut() {
                    anim.resume();
                }
            }
            commands.entity(unit).remove::<HitStop>();
            continue;
        }
        for (_, anim) in player.playing_animations_mut() {
            anim.pause();
        }
    }
}
