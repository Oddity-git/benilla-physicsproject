//! Fork: what a killing blow leaves on the body as it falls, behind the Physics page's Death
//! Effects box (`deathEffects`, on by default).
//!
//! - A severed limb's cut bursts and jets blood (`gore`'s arteries).
//! - A melee kill gushes blood upward from the corpse for a moment (`gore::gush`, so the Blood
//!   option gates it like any blood).
//! - A fire kill burns: Immolate's own burning model rides the body for a few seconds, embers rise
//!   off it, then it smokes and the smoke thins out.
//! - A frost kill bursts: Frostbolt's own impact flash, a spray of ice shards falling off the body
//!   and a cold mist hanging round it, on top of the frozen pose and icy tint (`life`, `frost`).
//!
//! The stock models go through the spell-effect lane (`SpellKitFx`) as any kit does; the embers,
//! smoke, shards and mist are soft quads on the effect lane, drawn from the bodies where they are.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use benilla_world::particles::buffer::{begin_effect_frame, EffectVertex, WorldEffectDraw};
use benilla_world::view::WorldCamera;

use super::blow::LastHit;
use super::life::{Ragdoll, RagdollBodies};
use crate::creature_anim::spell_visual::kit_effects;
use crate::creature_anim::{FxClass, FxStage, SpellKitFx, SpellVisuals};

/// A killing blow this recent (s) when the body falls counts; the melee delay is at most 1 s.
const BLOW_WINDOW: f32 = 2.0;
/// The schools (`LastHit::school`): physical, fire and frost.
const PHYSICAL: u32 = 0;
const FIRE: u32 = 2;
const FROST: u32 = 4;
/// The spells whose stock models dress the body: Immolate's burning state, Frostbolt's impact.
const BURN_SPELL: u32 = 348;
const FROST_SPELL: u32 = 116;
/// The key our burning model goes by on the effect lane, apart from any real aura's.
const BURN_KEY: u32 = u32::MAX - 0x0b17;

/// How long a body burns (s), then smokes (s); how often each throws an ember or a puff (s).
const BURN: f32 = 4.0;
const SMOKE: f32 = 5.0;
const EMBER_EVERY: f32 = 0.05;
const SMOKE_EVERY: f32 = 0.09;
/// How long the cold mist hangs (s), how often it puffs, and how many shards burst at once.
const MIST: f32 = 2.5;
const MIST_EVERY: f32 = 0.1;
const SHARDS: usize = 28;
/// The most soft quads alive at once, past which the oldest go.
const MAX_PUFFS: usize = 1200;
/// Gravity on a falling shard (yd/s²).
const GRAVITY: f32 = 19.29;

/// The colours: an ember (additive), smoke, a shard, the mist.
const EMBER: [f32; 4] = [1.0, 0.45, 0.12, 1.0];
const SMOKE_GREY: [f32; 4] = [0.22, 0.21, 0.2, 0.55];
const SHARD: [f32; 4] = [0.75, 0.9, 1.0, 0.95];
const MIST_WHITE: [f32; 4] = [0.8, 0.9, 1.0, 0.3];

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Puffs>()
        .add_systems(Startup, setup_puffs)
        .add_systems(Update, (start_effects, run_effects, age_puffs).chain())
        .add_systems(PostUpdate, draw_puffs.after(begin_effect_frame));
}

/// What a body is doing after its death, from when.
#[derive(Component)]
struct DeathFx {
    kind: Kind,
    start: f32,
    next: f32,
    burning: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Fire,
    Frost,
}

/// A soft quad: embers, smoke, shards and mist.
struct Puff {
    at: Vec3,
    velocity: Vec3,
    born: f32,
    life: f32,
    /// Its half-size at birth and at death.
    size: (f32, f32),
    colour: [f32; 4],
    /// Pulled down at this share of gravity; negative rises.
    fall: f32,
    glow: bool,
}

#[derive(Resource, Default)]
struct Puffs {
    texture: Handle<Image>,
    live: Vec<Puff>,
    rng: u32,
}

impl Puffs {
    /// A uniform draw in `[lo, hi)`, xorshift.
    fn range(&mut self, (lo, hi): (f32, f32)) -> f32 {
        let mut x = self.rng.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        lo + (hi - lo) * ((x >> 8) as f32 / (1u32 << 24) as f32)
    }

    fn spread(&mut self, r: f32) -> Vec3 {
        Vec3::new(
            self.range((-r, r)),
            self.range((-r, r)),
            self.range((-r, r)),
        )
    }
}

fn setup_puffs(mut puffs: ResMut<Puffs>, mut images: ResMut<Assets<Image>>) {
    puffs.texture = images.add(soft_puff());
    puffs.rng = 0x2545_f491;
}

/// A white disc fading out from its middle, for every soft quad to tint.
fn soft_puff() -> Image {
    const N: u32 = 32;
    let mut data = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (
                (x as f32 + 0.5) / N as f32 * 2.0 - 1.0,
                (y as f32 + 0.5) / N as f32 * 2.0 - 1.0,
            );
            let r = u.hypot(v);
            let a = (1.0 - r).clamp(0.0, 1.0);
            data.extend_from_slice(&[255, 255, 255, (a * a * 255.0) as u8]);
        }
    }
    Image::new(
        Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::all(),
    )
}

/// The box.
pub(super) fn enabled(cvars: Option<&crate::cvars::Cvars>) -> bool {
    cvars.and_then(|c| c.flag("deathEffects")).unwrap_or(true)
}

/// A body that just fell from a recent killing blow takes that blow's effect.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn start_effects(
    mut commands: Commands,
    fallen: Query<(Entity, &Ragdoll, Option<&LastHit>), Added<Ragdoll>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    visuals: Option<Res<SpellVisuals>>,
    mut puffs: ResMut<Puffs>,
    bodies: Res<RagdollBodies>,
    frames: Query<&GlobalTransform>,
    time: Res<Time>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    if !enabled(cvars.as_deref()) {
        return;
    }
    let now = time.elapsed_secs();
    let kit = |spell: u32, impact: bool| match (spells.as_deref(), visuals.as_deref()) {
        (Some(s), Some(v)) => kit_effects(s, &v.0, spell, impact),
        _ => Vec::new(),
    };
    for (unit, rag, hit) in &fallen {
        let Some(hit) = hit.filter(|h| now - h.at <= BLOW_WINDOW && !rag.knocked()) else {
            continue;
        };
        match hit.school {
            None | Some(PHYSICAL) => super::gore::gush(&mut commands, unit, now),
            Some(FIRE) => {
                let effects = kit(BURN_SPELL, false);
                let burning = !effects.is_empty();
                if burning {
                    fx.write(SpellKitFx::Begin {
                        entity: unit,
                        spell_id: BURN_KEY,
                        persistent: true,
                        class: FxClass::AuraState,
                        stage: FxStage::State,
                        effects,
                    });
                } else {
                    warn!("death effects: no burning model (Immolate's state kit) for unit {unit}");
                }
                commands.entity(unit).try_insert(DeathFx {
                    kind: Kind::Fire,
                    start: now,
                    next: now,
                    burning,
                });
            }
            Some(FROST) => {
                let effects = kit(FROST_SPELL, true);
                if !effects.is_empty() {
                    fx.write(SpellKitFx::Begin {
                        entity: unit,
                        spell_id: 0,
                        persistent: false,
                        class: FxClass::Hold,
                        stage: FxStage::OneShot,
                        effects,
                    });
                }
                if let Some(at) = body_point(&mut puffs, &bodies, &frames, unit) {
                    for _ in 0..SHARDS {
                        let out = puffs.spread(1.0) + Vec3::Y * 0.8;
                        let velocity = out.normalize_or(Vec3::Y) * puffs.range((1.5, 4.0));
                        let life = puffs.range((0.8, 1.5));
                        let half = puffs.range((0.025, 0.05));
                        puffs.live.push(Puff {
                            at,
                            velocity,
                            born: now,
                            life,
                            size: (half, half * 0.6),
                            colour: SHARD,
                            fall: 1.0,
                            glow: false,
                        });
                    }
                }
                commands.entity(unit).try_insert(DeathFx {
                    kind: Kind::Frost,
                    start: now,
                    next: now,
                    burning: false,
                });
            }
            _ => {}
        }
    }
}

/// A point on one of `unit`'s bodies, picked at random, else `None` once its bodies are gone.
fn body_point(
    puffs: &mut Puffs,
    bodies: &RagdollBodies,
    frames: &Query<&GlobalTransform>,
    unit: Entity,
) -> Option<Vec3> {
    let ids = bodies.0.get(&unit).filter(|ids| !ids.is_empty())?;
    let i = (puffs.range((0.0, ids.len() as f32)) as usize).min(ids.len() - 1);
    frames.get(ids[i]).ok().map(|g| g.translation())
}

/// Feeds each body's burning, smoke or mist, and takes the burning model off when its time is up.
fn run_effects(
    mut commands: Commands,
    mut units: Query<(Entity, &Transform, &mut DeathFx)>,
    bodies: Res<RagdollBodies>,
    frames: Query<&GlobalTransform>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    time: Res<Time>,
    mut puffs: ResMut<Puffs>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    let now = time.elapsed_secs();
    let on = enabled(cvars.as_deref());
    for (unit, tf, mut death) in &mut units {
        let t = now - death.start;
        let over = match death.kind {
            Kind::Fire => t > BURN + SMOKE,
            Kind::Frost => t > MIST,
        };
        if death.burning && (t > BURN || !on) {
            death.burning = false;
            fx.write(SpellKitFx::Reap {
                entity: unit,
                spell_id: BURN_KEY,
                class: FxClass::AuraState,
            });
        }
        if over || !on {
            commands.entity(unit).try_remove::<DeathFx>();
            continue;
        }
        if now < death.next {
            continue;
        }
        // A frozen body has dropped its bodies: it smokes from where it lies.
        let at = body_point(&mut puffs, &bodies, &frames, unit)
            .unwrap_or(tf.translation + Vec3::Y * 0.4);
        match death.kind {
            Kind::Fire if t < BURN => {
                death.next = now + EMBER_EVERY;
                let drift = puffs.spread(0.4);
                let velocity = Vec3::Y * puffs.range((1.0, 2.2)) + drift;
                let life = puffs.range((0.6, 1.2));
                let at = at + puffs.spread(0.15);
                puffs.live.push(Puff {
                    at,
                    velocity,
                    born: now,
                    life,
                    size: (0.03, 0.01),
                    colour: EMBER,
                    fall: -0.02,
                    glow: true,
                });
                // The smoke starts while it still burns.
                if t > BURN * 0.5 && puffs.range((0.0, 1.0)) < 0.3 {
                    smoke(&mut puffs, at, now, 1.0);
                }
            }
            Kind::Fire => {
                death.next = now + SMOKE_EVERY;
                smoke(&mut puffs, at, now, 1.0 - (t - BURN) / SMOKE);
            }
            Kind::Frost => {
                death.next = now + MIST_EVERY;
                let thin = 1.0 - t / MIST;
                let velocity = puffs.spread(0.25) + Vec3::Y * 0.15;
                let life = puffs.range((1.2, 2.0));
                let at = at + puffs.spread(0.2);
                let mut colour = MIST_WHITE;
                colour[3] *= thin;
                puffs.live.push(Puff {
                    at,
                    velocity,
                    born: now,
                    life,
                    size: (0.15, 0.4),
                    colour,
                    fall: 0.0,
                    glow: false,
                });
            }
        }
    }
}

/// One smoke puff rising from `at`, `thick` (0 to 1) of full strength.
fn smoke(puffs: &mut Puffs, at: Vec3, now: f32, thick: f32) {
    let velocity = Vec3::Y * puffs.range((0.5, 1.0)) + puffs.spread(0.15);
    let life = puffs.range((2.0, 3.0));
    let mut colour = SMOKE_GREY;
    colour[3] *= thick.clamp(0.0, 1.0);
    let at = at + puffs.spread(0.1);
    puffs.live.push(Puff {
        at,
        velocity,
        born: now,
        life,
        size: (0.12, 0.6),
        colour,
        fall: 0.0,
        glow: false,
    });
}

/// Moves every quad and drops the ones past their life, or the oldest past the cap.
fn age_puffs(time: Res<Time>, mut puffs: ResMut<Puffs>) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    puffs.live.retain(|p| now - p.born < p.life);
    let excess = puffs.live.len().saturating_sub(MAX_PUFFS);
    puffs.live.drain(..excess);
    for p in &mut puffs.live {
        p.velocity.y -= GRAVITY * p.fall * dt;
        // Smoke and mist slow as they spread; embers and shards fly on.
        if p.fall == 0.0 {
            p.velocity *= 1.0 - (0.8 * dt).min(1.0);
        }
        p.at += p.velocity * dt;
    }
}

/// Draws the quads facing the eye: embers glowing, the rest blended.
fn draw_puffs(
    cam: Query<(Entity, &GlobalTransform), With<WorldCamera>>,
    mut draw: WorldEffectDraw,
    puffs: Res<Puffs>,
    time: Res<Time>,
) {
    let Ok((cam, cam_tf)) = cam.single() else {
        return;
    };
    if puffs.live.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    let (right, up) = (*cam_tf.right(), *cam_tf.up());
    for glow in [false, true] {
        let mut verts = Vec::new();
        let mut anchor = Vec3::ZERO;
        for p in puffs.live.iter().filter(|p| p.glow == glow) {
            let age = ((now - p.born) / p.life).clamp(0.0, 1.0);
            let half = p.size.0 + (p.size.1 - p.size.0) * age;
            // In over the first tenth, out over the rest.
            let fade = (age * 10.0).min(1.0) * (1.0 - age);
            let color = [p.colour[0], p.colour[1], p.colour[2], p.colour[3] * fade];
            let (a, b) = (right * half, up * half);
            let corner = |q: Vec3, uv: [f32; 2]| EffectVertex {
                pos: q.to_array(),
                uv,
                color,
            };
            let (p00, p10, p11, p01) = (p.at - a - b, p.at + a - b, p.at + a + b, p.at - a + b);
            verts.extend([
                corner(p00, [0.0, 0.0]),
                corner(p10, [1.0, 0.0]),
                corner(p11, [1.0, 1.0]),
                corner(p00, [0.0, 0.0]),
                corner(p11, [1.0, 1.0]),
                corner(p01, [0.0, 1.0]),
            ]);
            anchor += p.at;
        }
        if verts.is_empty() {
            continue;
        }
        let anchor = anchor / (verts.len() / 6) as f32;
        let batch = draw.batch(cam, puffs.texture.id());
        let mut batch = if glow {
            batch.additive()
        } else {
            batch.alpha()
        }
        .anchored(anchor)
        .owner(cam);
        batch.extend(verts);
        batch.tris();
    }
}
