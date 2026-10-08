//! The last hit each unit took, from the swing and spell-damage logs, so its death push can scale
//! with the share of its health the killing blow took. The log reaches us before the health field
//! that reads dead, so the blow is on the unit when its ragdoll starts.

use benilla_protocol::messages::PeriodicTick;
use benilla_protocol::{SessionEvent, SessionEventKind as K};
use bevy::prelude::*;

use crate::net::{GuidIndex, NetHandlerApp};

pub(super) fn plugin(app: &mut App) {
    app.add_message::<Hit>()
        .net_handler(K::AttackerState, on_attacker_state)
        .net_handler(K::SpellDamageLog, on_spell_damage_log)
        .net_handler(K::PeriodicAuraLog, on_periodic_aura_log);
}

/// A physical damage-over-time tick (a bleed: Rend, Rupture, Rip, Garrote, Deep Wounds) keeps the
/// victim squirting blood until the next tick is due ([`super::gore::bleed`]).
fn on_periodic_aura_log(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    time: Res<Time>,
) {
    if let SessionEvent::PeriodicAuraLog(s) = ev {
        let bleeds = s
            .ticks
            .iter()
            .any(|t| matches!(t, PeriodicTick::Damage { amount, school: 0, .. } if *amount > 0));
        if bleeds {
            if let Some(&victim) = index.0.get(&s.target) {
                super::gore::bleed(&mut commands, victim, time.elapsed_secs());
            }
        }
    }
}

/// One damaging hit, for the gore's spray: `physical` is a swing or a physical spell's direct hit.
#[derive(Message, Clone, Copy)]
pub(super) struct Hit {
    pub(super) victim: Entity,
    pub(super) attacker: u64,
    pub(super) damage: u32,
    pub(super) physical: bool,
}

/// The latest damage a unit took, who dealt it, and when.
#[derive(Component, Clone, Copy)]
pub(super) struct LastHit {
    pub(super) attacker: u64,
    pub(super) damage: u32,
    pub(super) at: f32,
    /// A spell's school (0 physical, 4 frost), `None` for a swing.
    pub(super) school: Option<u32>,
    /// The spell, `None` for a swing.
    pub(super) spell: Option<u32>,
}

fn on_attacker_state(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    time: Res<Time>,
    mut hits: MessageWriter<Hit>,
) {
    if let SessionEvent::AttackerState(s) = ev {
        let blow = Blow {
            attacker: s.attacker,
            victim: s.victim,
            damage: s.damage,
            physical: true,
            school: None,
            spell: None,
        };
        record(&mut commands, &index, &time, &mut hits, blow);
    }
}

fn on_spell_damage_log(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    time: Res<Time>,
    mut hits: MessageWriter<Hit>,
) {
    if let SessionEvent::SpellDamageLog(s) = ev {
        if s.periodic && s.school == 0 && s.damage > 0 {
            if let Some(&victim) = index.0.get(&s.target) {
                super::gore::bleed(&mut commands, victim, time.elapsed_secs());
            }
        }
        let blow = Blow {
            attacker: s.attacker,
            victim: s.target,
            damage: s.damage,
            // School 0 is physical; a bleed's tick is no fresh wound.
            physical: s.school == 0 && !s.periodic,
            school: Some(u32::from(s.school)),
            spell: Some(s.spell_id),
        };
        record(&mut commands, &index, &time, &mut hits, blow);
    }
}

struct Blow {
    attacker: u64,
    victim: u64,
    damage: u32,
    physical: bool,
    school: Option<u32>,
    spell: Option<u32>,
}

fn record(
    commands: &mut Commands,
    index: &GuidIndex,
    time: &Time,
    hits: &mut MessageWriter<Hit>,
    blow: Blow,
) {
    if blow.damage == 0 {
        return;
    }
    if let Some(&victim) = index.0.get(&blow.victim) {
        if let Ok(mut e) = commands.get_entity(victim) {
            e.insert(LastHit {
                attacker: blow.attacker,
                damage: blow.damage,
                at: time.elapsed_secs(),
                school: blow.school,
                spell: blow.spell,
            });
            hits.write(Hit {
                victim,
                attacker: blow.attacker,
                damage: blow.damage,
                physical: blow.physical,
            });
        }
    }
}
