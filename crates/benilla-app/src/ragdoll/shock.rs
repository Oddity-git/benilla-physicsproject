//! Fork: a body a lightning or thunder spell killed (by the spell's name, so Thunder Clap counts),
//! or that died carrying such a debuff, shakes in a seizure: for a few seconds its bodies take
//! random jolts, fading out, and its model flickers a pale electric blue on the per-instance tint
//! channel, as the frost tint does. It never stiffens; once the shaking ends it lies as any
//! ragdoll.

use avian3d::prelude::*;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use benilla_world::instance_tint::{pack, InstanceTints};
use benilla_world::rig_palette::RigSkin;

use super::life::{Ragdoll, RagdollBodies};
use crate::net::ObjectStore;

/// How long the seizure lasts (s), and how often a jolt lands.
const SEIZURE: f32 = 2.5;
const JOLT_EVERY: f32 = 0.06;
/// A jolt's strongest kick, fading with the seizure: linear (yd/s) and spin (rad/s).
const JOLT_SPEED: f32 = 2.2;
const JOLT_SPIN: f32 = 9.0;
/// The flicker's tint, shown about this share of the frames while it lasts.
const SPARK: [u8; 3] = [190, 220, 255];
const FLICKER: f32 = 0.5;

/// A shock debuff seen this recently (s) counts at death: the server clears the auras as it kills.
pub(super) const DEBUFF_MEMORY: f32 = 1.0;

/// Spell names that read as lightning or thunder.
const WORDS: [&str; 6] = ["lightning", "thunder", "shock", "storm", "static", "spark"];

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<ShockSpells>()
        .add_systems(Update, (note_shock_debuffs, jolt_shocked))
        .add_systems(
            Update,
            tint_shocked.after(crate::aura_visual::apply_aura_tint),
        );
}

/// Whether a spell named `name` is lightning or thunder.
pub(super) fn is_shock(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    WORDS.iter().any(|w| name.contains(w))
}

/// Which spells are lightning or thunder, by id, as read off the catalog.
#[derive(Resource, Default)]
struct ShockSpells(HashMap<u32, bool>);

/// On a living unit: when it last carried a lightning or thunder aura.
#[derive(Component)]
pub(super) struct ShockDebuff(pub(super) f32);

/// Notes the living units carrying a lightning or thunder aura.
fn note_shock_debuffs(
    mut commands: Commands,
    time: Res<Time>,
    units: Query<(Entity, &ObjectStore), Without<Ragdoll>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut known: ResMut<ShockSpells>,
) {
    let Some(spells) = spells else {
        return;
    };
    let now = time.elapsed_secs();
    for (unit, store) in &units {
        if store.0.unit_is_dead() {
            continue;
        }
        let carries = store.0.unit_aura_ids().filter(|&id| id != 0).any(|id| {
            *known
                .0
                .entry(id)
                .or_insert_with(|| spells.catalog.get(id).is_some_and(|s| is_shock(&s.name)))
        });
        if carries {
            commands.entity(unit).insert(ShockDebuff(now));
        }
    }
}

/// A ragdoll in a seizure: when it started, and when its next jolt lands.
#[derive(Component)]
pub(super) struct Shocked {
    start: f32,
    next: f32,
    rng: u32,
}

impl Shocked {
    pub(super) fn new(now: f32) -> Self {
        Self {
            start: now,
            next: now,
            rng: now.to_bits() | 1,
        }
    }

    /// A uniform value in -1..1 (xorshift).
    fn signed(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn kick(&mut self) -> Vec3 {
        Vec3::new(self.signed(), self.signed(), self.signed())
    }

    /// The seizure's strength now, 1 fading to 0, or `None` once it is over.
    fn strength(&self, now: f32) -> Option<f32> {
        let t = (now - self.start) / SEIZURE;
        (t < 1.0).then_some(1.0 - t * t)
    }
}

/// Jolts each shaking ragdoll's bodies, waking them, until its seizure ends.
fn jolt_shocked(
    mut commands: Commands,
    time: Res<Time>,
    mut shocked: Query<(Entity, &mut Shocked)>,
    bodies: Res<RagdollBodies>,
    mut velocities: Query<(&mut LinearVelocity, &mut AngularVelocity)>,
) {
    let now = time.elapsed_secs();
    for (unit, mut shock) in &mut shocked {
        let Some(strength) = shock.strength(now) else {
            commands.entity(unit).remove::<Shocked>();
            continue;
        };
        if now < shock.next {
            continue;
        }
        shock.next = now + JOLT_EVERY;
        let Some(ids) = bodies.0.get(&unit) else {
            continue;
        };
        for &body in ids {
            let (Ok((mut linear, mut angular)), Ok(mut e)) =
                (velocities.get_mut(body), commands.get_entity(body))
            else {
                continue;
            };
            e.remove::<Sleeping>();
            linear.0 += shock.kick() * (JOLT_SPEED * strength);
            angular.0 += shock.kick() * (JOLT_SPIN * strength);
        }
    }
}

/// Flickers each shaking body's tint, after the auras have published this frame's.
fn tint_shocked(
    time: Res<Time>,
    mut shocked: Query<(&RigSkin, &mut Shocked)>,
    mut tints: ResMut<InstanceTints>,
) {
    let now = time.elapsed_secs();
    for (skin, mut shock) in &mut shocked {
        if shock.strength(now).is_some() && shock.signed() * 0.5 + 0.5 < FLICKER {
            tints.set(skin.slot, pack(SPARK));
        }
    }
}
