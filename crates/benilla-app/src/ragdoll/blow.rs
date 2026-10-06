//! The last hit each unit took, from the swing and spell-damage logs, so its death push can scale
//! with the share of its health the killing blow took. The log reaches us before the health field
//! that reads dead, so the blow is on the unit when its ragdoll starts.

use benilla_protocol::{SessionEvent, SessionEventKind as K};
use bevy::prelude::*;

use crate::net::{GuidIndex, NetHandlerApp};

pub(super) fn plugin(app: &mut App) {
    app.net_handler(K::AttackerState, on_attacker_state)
        .net_handler(K::SpellDamageLog, on_spell_damage_log);
}

/// The latest damage a unit took, who dealt it, and when.
#[derive(Component, Clone, Copy)]
pub(super) struct LastHit {
    pub(super) attacker: u64,
    pub(super) damage: u32,
    pub(super) at: f32,
}

fn on_attacker_state(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    time: Res<Time>,
) {
    if let SessionEvent::AttackerState(s) = ev {
        record(&mut commands, &index, &time, s.attacker, s.victim, s.damage);
    }
}

fn on_spell_damage_log(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    time: Res<Time>,
) {
    if let SessionEvent::SpellDamageLog(s) = ev {
        record(&mut commands, &index, &time, s.attacker, s.target, s.damage);
    }
}

fn record(
    commands: &mut Commands,
    index: &GuidIndex,
    time: &Time,
    attacker: u64,
    victim: u64,
    damage: u32,
) {
    if damage == 0 {
        return;
    }
    if let Some(&e) = index.0.get(&victim) {
        if let Ok(mut e) = commands.get_entity(e) {
            e.insert(LastHit {
                attacker,
                damage,
                at: time.elapsed_secs(),
            });
        }
    }
}
