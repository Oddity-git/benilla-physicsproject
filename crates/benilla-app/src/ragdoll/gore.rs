//! Fork: blood on the ground and in the air, all from the stock blood tables and gated by the
//! Blood option (`violenceLevel`).
//!
//! - A damaging hit throws a burst of droplets away from the attacker, more the bigger the hit's
//!   share of the victim's health and the most on a killing blow; each leaves a splat where it lands.
//! - A ragdoll bleeds a pool that spreads under its body.
//!
//! The splat art is `UnitBlood`'s ground-splat column, which 1.12.1 ships but never draws
//! (`benilla_formats::blood`): through the same censored row as the spurt, so green blood at
//! violence 1 and nothing at 0, and a bloodless row leaves nothing. Splats and pools are decals on
//! the shared ground projector, faded out after a while.

use std::collections::{HashMap, VecDeque};

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use benilla_assets::texture_url;
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::{begin_effect_frame, EffectVertex, WorldEffectDraw};
use benilla_world::sky_order::Rung;
use benilla_world::view::WorldCamera;

use super::blow::Hit;
use super::life::{Ragdoll, RagdollBodies};
use crate::creature_anim::{unit_blood_id, violence_level, BloodTables};
use crate::entities::{Creatures, ModelBound};
use crate::net::{GuidIndex, NetEntity, ObjectStore};

/// Droplets per hit: the base, plus this many per whole health bar the hit took; a killing blow
/// throws [`MAX_DROPS`].
const BASE_DROPS: f32 = 6.0;
const DROPS_PER_BAR: f32 = 60.0;
const MAX_DROPS: usize = 48;
/// Launch speed (yd/s): the base, plus this per whole health bar, capped as a killing blow.
const BASE_SPEED: f32 = 2.5;
const SPEED_PER_BAR: f32 = 7.0;
/// How far a droplet strays off the away direction (rad), and its climb range.
const SPREAD: f32 = 0.7;
const MIN_CLIMB: f32 = 0.15;
const MAX_CLIMB: f32 = 0.9;
/// The droplets fall at world gravity (yd/s², `world_plugins`), drag a little, and give up after
/// this long.
const GRAVITY: f32 = 19.29;
const DROP_DRAG: f32 = 0.6;
const DROP_LIFE: f32 = 2.5;
/// A droplet's ball radius (yd), and its stretch along its flight per yd/s.
const DROP_RADIUS: f32 = 0.03;
const DROP_STRETCH: f32 = 0.12;
/// The spray starts this far up the victim's height (its chest).
const CHEST: f32 = 0.6;
/// A landed droplet's splat half-size range (yd), and the one under a hit victim's feet.
const DROP_SPLAT: (f32, f32) = (0.08, 0.22);
const HIT_SPLAT: (f32, f32) = (0.25, 0.4);
/// A splat stays this long, the last [`FADE`] of it fading.
const SPLAT_LIFE: f32 = 45.0;
const FADE: f32 = 6.0;
/// Most splats on the ground at once; past it the oldest goes.
const MAX_SPLATS: usize = 400;
/// A ragdoll's pool starts after its body has fallen, then spreads to its full size (a share of the
/// unit's height, clamped) over [`POOL_SPREAD`]; it is re-projected at most this often while it
/// grows.
const POOL_DELAY: f32 = 1.0;
const POOL_SPREAD: f32 = 10.0;
const POOL_SHARE: f32 = 0.4;
const POOL_SIZE: (f32, f32) = (0.45, 2.2);
const POOL_REPROJECT: f32 = 0.1;
/// A pool outlasts the corpse by nothing: it goes with the unit, else after this long.
const POOL_LIFE: f32 = 180.0;
/// The decal box's reach below and above the ground estimate (yd).
const BELOW: f32 = 1.5;
const ABOVE: f32 = 0.8;
/// Unlit decals are dimmed this much so a night-time splat does not glow.
const DIM: f32 = 0.7;
/// A cut bursts this many droplets, then its stump spurts this many each pulse for a while, at this
/// speed range (yd/s).
const CUT_DROPS: usize = 30;
const PULSE_DROPS: usize = 5;
const GUSH_PULSE: f32 = 0.15;
const GUSH_SECS: f32 = 2.0;
const GUSH_SPEED: (f32, f32) = (1.5, 4.5);
/// The height a unit with no model bound counts as (yd).
const DEFAULT_HEIGHT: f32 = 1.8;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Gore>()
        .add_systems(Startup, setup_gore)
        .add_systems(
            Update,
            (
                spray_hits,
                gush_stumps,
                fly_droplets,
                start_pools,
                grow_pools,
                age_splats,
            )
                .chain(),
        )
        .add_systems(PostUpdate, push_decals.after(begin_effect_frame));
}

/// The gore's shared state.
#[derive(Resource, Default)]
struct Gore {
    /// The procedural splat drawn while a stock one loads, or when it fails to.
    fallback: Handle<Image>,
    droplet_mesh: Handle<Mesh>,
    /// Droplet materials by quantized colour.
    materials: HashMap<[u8; 3], Handle<StandardMaterial>>,
    /// The stock splat textures by `UnitBlood` path.
    textures: HashMap<String, Handle<Image>>,
    /// What each loaded splat texture holds ([`TexLook`]).
    looks: HashMap<AssetId<Image>, TexLook>,
    /// The ground splats, oldest first.
    splats: VecDeque<Entity>,
    rng: u32,
}

/// A loaded splat texture: whether it carries its own colour (drawn white) or is a grey mask
/// (tinted), and its average blood colour when that could be read.
#[derive(Clone, Copy)]
struct TexLook {
    coloured: bool,
    colour: Option<[f32; 3]>,
}

impl Gore {
    /// A uniform draw in `[0, 1)`, xorshift.
    fn rand(&mut self) -> f32 {
        let mut x = self.rng.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + (hi - lo) * self.rand()
    }

    /// One of the blood's stock splats, loading it on first use.
    fn pick_texture(&mut self, server: &AssetServer, splats: &[String]) -> Option<Handle<Image>> {
        if splats.is_empty() {
            return None;
        }
        let i = ((self.rand() * splats.len() as f32) as usize).min(splats.len() - 1);
        let path = &splats[i];
        Some(
            self.textures
                .entry(path.clone())
                .or_insert_with(|| {
                    info!("gore: blood splat texture {path}");
                    server.load(texture_url(&format!("{path}.blp"), (false, false)))
                })
                .clone(),
        )
    }
}

/// One blood's look: its splat art and the violence level it was drawn at (green at 1).
#[derive(Clone)]
struct Blood {
    texture: Handle<Image>,
    violence: usize,
}

/// A droplet in flight.
#[derive(Component)]
struct Droplet {
    velocity: Vec3,
    ground: f32,
    born: f32,
    blood: Blood,
}

/// A splat on the ground, projected once.
#[derive(Component)]
struct Splat {
    verts: Vec<EffectVertex>,
    blood: Blood,
    anchor: Vec3,
    born: f32,
}

/// A pool spreading under a ragdoll.
#[derive(Component)]
struct Pool {
    unit: Entity,
    centre: Vec3,
    yaw: f32,
    size: f32,
    born: f32,
    projected_at: f32,
    verts: Vec<EffectVertex>,
    blood: Blood,
}

/// A ragdoll waiting for its body to fall before it bleeds.
#[derive(Component)]
struct PoolDue {
    at: f32,
}

fn setup_gore(
    mut gore: ResMut<Gore>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    gore.fallback = images.add(fallback_splat());
    gore.droplet_mesh = meshes.add(Sphere::new(1.0));
    gore.rng = 0x9e37_79b9;
}

/// A white splat under a ragged alpha edge, for the tint to colour.
fn fallback_splat() -> Image {
    const N: u32 = 64;
    let mut data = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (
                (x as f32 + 0.5) / N as f32 * 2.0 - 1.0,
                (y as f32 + 0.5) / N as f32 * 2.0 - 1.0,
            );
            let (r, a) = (u.hypot(v), v.atan2(u));
            let edge = 0.62
                + 0.12 * (5.0 * a).sin()
                + 0.07 * (9.0 * a + 1.0).sin()
                + 0.04 * (17.0 * a).sin();
            let alpha = ((edge - r) / 0.06).clamp(0.0, 1.0);
            data.extend_from_slice(&[255, 255, 255, (alpha * 230.0) as u8]);
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

/// The colour blood is when its art cannot say: red, or green at violence 1.
fn default_colour(violence: usize) -> [f32; 3] {
    if violence == 1 {
        [0.2, 0.42, 0.05]
    } else {
        [0.42, 0.02, 0.02]
    }
}

/// What a loaded splat texture holds: read off its pixels when it is plain RGBA, else taken as
/// coloured art.
fn tex_look(img: &Image) -> TexLook {
    let plain = matches!(
        img.texture_descriptor.format,
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
    );
    let Some(data) = img.data.as_ref().filter(|_| plain) else {
        return TexLook {
            coloured: true,
            colour: None,
        };
    };
    let (mut sum, mut weight) = ([0.0f32; 3], 0.0f32);
    for px in data.as_chunks::<4>().0 {
        let a = px[3] as f32 / 255.0;
        for c in 0..3 {
            sum[c] += px[c] as f32 / 255.0 * a;
        }
        weight += a;
    }
    if weight <= 0.0 {
        return TexLook {
            coloured: false,
            colour: None,
        };
    }
    let avg = sum.map(|c| c / weight);
    let spread =
        avg.iter().fold(f32::MIN, |m, &c| m.max(c)) - avg.iter().fold(f32::MAX, |m, &c| m.min(c));
    TexLook {
        coloured: spread > 0.1,
        colour: (spread > 0.1).then_some(avg),
    }
}

/// The texture and vertex tint a blood draws with this frame: its stock art once loaded, the
/// procedural splat until then (or if it never loads).
fn resolve_look(
    gore: &mut Gore,
    images: &Assets<Image>,
    blood: &Blood,
) -> (AssetId<Image>, [f32; 3]) {
    let tint = default_colour(blood.violence);
    if let Some(img) = images.get(&blood.texture) {
        let look = *gore
            .looks
            .entry(blood.texture.id())
            .or_insert_with(|| tex_look(img));
        return if look.coloured {
            (blood.texture.id(), [1.0; 3])
        } else {
            (blood.texture.id(), tint)
        };
    }
    (gore.fallback.id(), tint)
}

/// The colour a droplet of this blood flies as.
fn droplet_colour(gore: &Gore, blood: &Blood) -> [f32; 3] {
    gore.looks
        .get(&blood.texture.id())
        .and_then(|l| l.colour)
        .unwrap_or_else(|| default_colour(blood.violence))
}

/// The unit's height in yards, off its model bound and scale.
fn height_of(net: &NetEntity, bound: Option<&ModelBound>) -> f32 {
    let h = bound.map_or(0.0, |b| 2.0 * b.0.half_extents.y) * net.scale;
    if h > 0.1 {
        h
    } else {
        DEFAULT_HEIGHT
    }
}

/// What the gore reads of a hit unit.
type Victim = (
    &'static Transform,
    &'static NetEntity,
    &'static ObjectStore,
    Option<&'static ModelBound>,
);

/// Throw each hit's droplets and drop a splat under the victim.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn spray_hits(
    mut commands: Commands,
    mut hits: MessageReader<Hit>,
    victims: Query<Victim>,
    transforms: Query<&Transform>,
    index: Res<GuidIndex>,
    creatures: Option<Res<Creatures>>,
    blood: Option<Res<BloodTables>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    server: Res<AssetServer>,
    decals: WorldDecal,
    time: Res<Time>,
    mut gore: ResMut<Gore>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let violence = violence_level(cvars.as_deref());
    let Some(blood) = blood.filter(|_| violence > 0) else {
        hits.clear();
        return;
    };
    let now = time.elapsed_secs();
    for hit in hits.read() {
        let Ok((tf, net, store, bound)) = victims.get(hit.victim) else {
            continue;
        };
        let max = store.0.unit_max_health().unwrap_or(0).max(1) as f32;
        let health = store.0.unit_health().unwrap_or(0);
        // The log lands before the health field: a hit at least as big as what is left kills.
        let killing = health == 0 || hit.damage >= health;
        if !hit.physical && !killing {
            continue;
        }
        let Some(id) = unit_blood_id(creatures.as_deref(), &blood, net.display_id) else {
            continue;
        };
        let splats = blood.0.splats(id, violence).to_vec();
        if splats.is_empty() {
            continue; // bloodless
        }
        let share = if killing {
            1.0
        } else {
            (hit.damage as f32 / max).min(1.0)
        };
        let count = if killing {
            MAX_DROPS
        } else {
            ((BASE_DROPS + DROPS_PER_BAR * share) as usize).min(MAX_DROPS)
        };
        let speed = BASE_SPEED + SPEED_PER_BAR * share;
        let feet = tf.translation;
        let chest = feet + Vec3::Y * height_of(net, bound) * CHEST;
        // Away from the attacker, else back from where the victim faces.
        let away = index
            .0
            .get(&hit.attacker)
            .and_then(|&a| transforms.get(a).ok())
            .map(|a| (feet - a.translation).with_y(0.0))
            .filter(|d| d.length_squared() > 1e-4)
            .map_or(tf.rotation * Vec3::Z, |d| d.normalize());
        let away = if away.is_finite() { away } else { Vec3::X };
        for _ in 0..count {
            let Some(texture) = gore.pick_texture(&server, &splats) else {
                break;
            };
            let b = Blood { texture, violence };
            let yaw = gore.range((-SPREAD, SPREAD));
            let climb = gore.range((MIN_CLIMB, MAX_CLIMB));
            let dir = Quat::from_rotation_y(yaw) * away;
            let v = (dir * climb.cos() + Vec3::Y * climb.sin()) * speed * gore.range((0.45, 1.2));
            let start = chest + Vec3::new(gore.range((-0.1, 0.1)), 0.0, gore.range((-0.1, 0.1)));
            throw_droplet(
                &mut commands,
                &mut gore,
                &mut materials,
                b,
                start,
                v,
                feet.y,
                now,
            );
        }
        // The splat under the victim.
        if let Some(texture) = gore.pick_texture(&server, &splats) {
            let half = gore.range(HIT_SPLAT);
            let at = feet + Vec3::new(gore.range((-0.3, 0.3)), 0.0, gore.range((-0.3, 0.3)));
            let b = Blood { texture, violence };
            lay_splat(&mut commands, &mut gore, &decals, at, half, b, now);
        }
    }
}

/// Launch one droplet of `blood` from `start` at `velocity`, to splat at height `ground`.
#[allow(clippy::too_many_arguments)] // the droplet's whole state
fn throw_droplet(
    commands: &mut Commands,
    gore: &mut Gore,
    materials: &mut Assets<StandardMaterial>,
    blood: Blood,
    start: Vec3,
    velocity: Vec3,
    ground: f32,
    now: f32,
) {
    let colour = droplet_colour(gore, &blood);
    let key = colour.map(|c| (c * 255.0) as u8);
    let material = gore
        .materials
        .entry(key)
        .or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(colour[0], colour[1], colour[2]),
                unlit: true,
                ..default()
            })
        })
        .clone();
    commands.spawn((
        Name::new("blood droplet"),
        Mesh3d(gore.droplet_mesh.clone()),
        MeshMaterial3d(material),
        Transform::from_translation(start).with_scale(Vec3::splat(DROP_RADIUS)),
        Droplet {
            velocity,
            ground,
            born: now,
            blood,
        },
    ));
}

/// A stump spurting: pulses of droplets from where a limb came off, for a while.
#[derive(Component)]
struct Gusher {
    at: Vec3,
    away: Vec3,
    ground: f32,
    until: f32,
    next: f32,
    splats: Vec<String>,
    violence: usize,
}

/// A severed limb bursts blood from the cut, and its stump keeps spurting.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn gush_stumps(
    mut commands: Commands,
    mut severs: MessageReader<super::dismember::Severed>,
    units: Query<(&Transform, &NetEntity)>,
    mut gushers: Query<(Entity, &mut Gusher)>,
    creatures: Option<Res<Creatures>>,
    blood: Option<Res<BloodTables>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    server: Res<AssetServer>,
    time: Res<Time>,
    mut gore: ResMut<Gore>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let now = time.elapsed_secs();
    let violence = violence_level(cvars.as_deref());
    let blood = blood.filter(|_| violence > 0);
    for cut in severs.read() {
        let Some(blood) = blood.as_deref() else {
            continue;
        };
        let Ok((tf, net)) = units.get(cut.unit) else {
            continue;
        };
        let Some(id) = unit_blood_id(creatures.as_deref(), blood, net.display_id) else {
            continue;
        };
        let splats = blood.0.splats(id, violence).to_vec();
        if splats.is_empty() {
            continue;
        }
        let gusher = Gusher {
            at: cut.at,
            away: cut.away,
            ground: tf.translation.y,
            until: now + GUSH_SECS,
            next: now + GUSH_PULSE,
            splats,
            violence,
        };
        spurt(
            &mut commands,
            &mut gore,
            &mut materials,
            &server,
            &gusher,
            CUT_DROPS,
            now,
        );
        commands.spawn((Name::new("blood gusher"), gusher));
    }
    for (e, mut gusher) in &mut gushers {
        if now >= gusher.until {
            commands.entity(e).despawn();
            continue;
        }
        if now < gusher.next {
            continue;
        }
        gusher.next = now + GUSH_PULSE;
        spurt(
            &mut commands,
            &mut gore,
            &mut materials,
            &server,
            &gusher,
            PULSE_DROPS,
            now,
        );
    }
}

/// One spurt of `count` droplets from a gusher, up and out along the throw.
fn spurt(
    commands: &mut Commands,
    gore: &mut Gore,
    materials: &mut Assets<StandardMaterial>,
    server: &AssetServer,
    gusher: &Gusher,
    count: usize,
    now: f32,
) {
    for _ in 0..count {
        let Some(texture) = gore.pick_texture(server, &gusher.splats) else {
            return;
        };
        let blood = Blood {
            texture,
            violence: gusher.violence,
        };
        let jitter = Vec3::new(
            gore.range((-1.0, 1.0)),
            gore.range((0.0, 1.0)),
            gore.range((-1.0, 1.0)),
        );
        let v = (gusher.away * 0.6 + Vec3::Y * 1.2 + jitter * 0.6).normalize_or_zero()
            * gore.range(GUSH_SPEED);
        throw_droplet(
            commands,
            gore,
            materials,
            blood,
            gusher.at,
            v,
            gusher.ground,
            now,
        );
    }
}

/// Project one splat and keep it, retiring the oldest past [`MAX_SPLATS`].
fn lay_splat(
    commands: &mut Commands,
    gore: &mut Gore,
    decals: &WorldDecal,
    at: Vec3,
    half: f32,
    blood: Blood,
    now: f32,
) {
    let yaw = gore.range((0.0, std::f32::consts::TAU));
    let mut verts = Vec::new();
    if !project(decals, &mut verts, at, yaw, half) {
        return;
    }
    let id = commands
        .spawn((
            Name::new("blood splat"),
            Splat {
                verts,
                blood,
                anchor: at,
                born: now,
            },
        ))
        .id();
    gore.splats.push_back(id);
    while gore.splats.len() > MAX_SPLATS {
        if let Some(old) = gore.splats.pop_front() {
            if let Ok(mut e) = commands.get_entity(old) {
                e.despawn();
            }
        }
    }
}

/// A square decal of half-size `half` turned `yaw` about `at`.
fn project(
    decals: &WorldDecal,
    out: &mut Vec<EffectVertex>,
    at: Vec3,
    yaw: f32,
    half: f32,
) -> bool {
    let frame = DecalFrame {
        center: at,
        sin: yaw.sin(),
        cos: yaw.cos(),
        min_x: -half,
        max_x: half,
        min_z: -half,
        max_z: half,
        min_y: -BELOW,
        max_y: ABOVE,
    };
    decals.project(out, &frame, |_| 1.0, |x, z| frame.rect_uv(x, z))
}

/// Fly the droplets under gravity; one that reaches its victim's ground level splats there.
fn fly_droplets(
    mut commands: Commands,
    mut drops: Query<(Entity, &mut Droplet, &mut Transform)>,
    decals: WorldDecal,
    time: Res<Time>,
    mut gore: ResMut<Gore>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    for (e, mut drop, mut tf) in &mut drops {
        drop.velocity.y -= GRAVITY * dt;
        drop.velocity *= 1.0 - (DROP_DRAG * dt).min(1.0);
        tf.translation += drop.velocity * dt;
        let speed = drop.velocity.length();
        if speed > 1e-3 {
            tf.rotation = Quat::from_rotation_arc(Vec3::Y, drop.velocity / speed);
        }
        tf.scale = Vec3::new(1.0, 1.0 + speed * DROP_STRETCH, 1.0) * DROP_RADIUS;
        let landed = tf.translation.y <= drop.ground;
        if landed || now - drop.born > DROP_LIFE {
            if landed {
                let half = gore.range(DROP_SPLAT);
                let at = tf.translation.with_y(drop.ground);
                lay_splat(
                    &mut commands,
                    &mut gore,
                    &decals,
                    at,
                    half,
                    drop.blood.clone(),
                    now,
                );
            }
            commands.entity(e).despawn();
        }
    }
}

/// A dead ragdoll bleeds once its body has fallen; a knocked-down one does not. A unit risen again
/// may bleed again on its next death.
#[allow(clippy::type_complexity)] // the filtered queries
fn start_pools(
    mut commands: Commands,
    fresh: Query<(Entity, &ObjectStore), (With<Ragdoll>, Without<Bled>)>,
    risen: Query<Entity, (With<Bled>, Without<Ragdoll>)>,
    time: Res<Time>,
) {
    for (unit, store) in &fresh {
        if store.0.unit_is_dead() {
            commands.entity(unit).insert((
                Bled,
                PoolDue {
                    at: time.elapsed_secs() + POOL_DELAY,
                },
            ));
        }
    }
    for unit in &risen {
        commands.entity(unit).remove::<Bled>();
    }
}

/// This ragdoll's pool has been started.
#[derive(Component)]
struct Bled;

/// What a pool reads of its ragdolled unit.
type Bleeder = (
    Entity,
    &'static PoolDue,
    &'static Transform,
    &'static NetEntity,
    Option<&'static ModelBound>,
);

/// Open the due pools under their bodies, spread them, and drop them with their unit.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn grow_pools(
    mut commands: Commands,
    due: Query<Bleeder, With<Ragdoll>>,
    rags: Query<(), With<Ragdoll>>,
    bodies: Res<RagdollBodies>,
    body_tf: Query<&Transform, Without<Ragdoll>>,
    mut pools: Query<(Entity, &mut Pool)>,
    creatures: Option<Res<Creatures>>,
    blood: Option<Res<BloodTables>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    server: Res<AssetServer>,
    decals: WorldDecal,
    time: Res<Time>,
    mut gore: ResMut<Gore>,
) {
    let now = time.elapsed_secs();
    let violence = violence_level(cvars.as_deref());
    for (unit, pool_due, tf, net, bound) in &due {
        if now < pool_due.at {
            continue;
        }
        commands.entity(unit).remove::<PoolDue>();
        let Some(blood) = blood.as_deref().filter(|_| violence > 0) else {
            continue;
        };
        let Some(id) = unit_blood_id(creatures.as_deref(), blood, net.display_id) else {
            continue;
        };
        let Some(texture) = gore.pick_texture(&server, blood.0.splats(id, violence)) else {
            continue; // bloodless
        };
        // Under the middle of the fallen body, on the unit's ground.
        let parts: Vec<Vec3> = bodies
            .0
            .get(&unit)
            .into_iter()
            .flatten()
            .filter_map(|b| body_tf.get(*b).ok().map(|t| t.translation))
            .collect();
        let middle = if parts.is_empty() {
            tf.translation
        } else {
            parts.iter().sum::<Vec3>() / parts.len() as f32
        };
        let (lo, hi) = POOL_SIZE;
        let size = (height_of(net, bound) * POOL_SHARE).clamp(lo, hi);
        let yaw = gore.range((0.0, std::f32::consts::TAU));
        commands.spawn((
            Name::new("blood pool"),
            Pool {
                unit,
                centre: middle.with_y(tf.translation.y),
                yaw,
                size,
                born: now,
                projected_at: f32::MIN,
                verts: Vec::new(),
                blood: Blood { texture, violence },
            },
        ));
    }
    for (e, mut pool) in &mut pools {
        if !rags.contains(pool.unit) || now - pool.born > POOL_LIFE {
            commands.entity(e).despawn();
            continue;
        }
        let grown = ((now - pool.born) / POOL_SPREAD).min(1.0);
        if grown >= 1.0 && !pool.verts.is_empty() && pool.projected_at > pool.born + POOL_SPREAD {
            continue;
        }
        if now - pool.projected_at < POOL_REPROJECT {
            continue;
        }
        // Fast at first, easing out as it reaches its edge.
        let half = pool.size * (0.15 + 0.85 * (1.0 - (1.0 - grown).powi(2)));
        let (centre, yaw) = (pool.centre, pool.yaw);
        let mut verts = std::mem::take(&mut pool.verts);
        verts.clear();
        project(&decals, &mut verts, centre, yaw, half);
        pool.verts = verts;
        pool.projected_at = now;
    }
}

/// Retire the splats past their life.
fn age_splats(
    mut commands: Commands,
    splats: Query<(Entity, &Splat)>,
    time: Res<Time>,
    mut gore: ResMut<Gore>,
) {
    let now = time.elapsed_secs();
    for (e, splat) in &splats {
        if now - splat.born > SPLAT_LIFE {
            commands.entity(e).despawn();
        }
    }
    gore.splats.retain(|e| splats.contains(*e));
}

/// Draw every splat and pool: alpha-blended on the footprint rung, so they paint over the blob
/// shadows and under everything standing.
fn push_decals(
    cam: Query<Entity, With<WorldCamera>>,
    mut draw: WorldEffectDraw,
    splats: Query<(Entity, &Splat)>,
    pools: Query<(Entity, &Pool)>,
    images: Res<Assets<Image>>,
    time: Res<Time>,
    mut gore: ResMut<Gore>,
) {
    let Ok(cam) = cam.single() else { return };
    let now = time.elapsed_secs();
    let draws = splats
        .iter()
        .map(|(e, s)| (e, &s.verts, &s.blood, s.anchor, SPLAT_LIFE - (now - s.born)))
        .chain(
            pools
                .iter()
                .map(|(e, p)| (e, &p.verts, &p.blood, p.centre, POOL_LIFE - (now - p.born))),
        );
    for (owner, verts, blood, anchor, left) in draws {
        if verts.is_empty() {
            continue;
        }
        let fade = (left / FADE).clamp(0.0, 1.0);
        let (texture, tint) = resolve_look(&mut gore, &images, blood);
        let colour = [tint[0] * DIM, tint[1] * DIM, tint[2] * DIM];
        let mut batch = draw
            .batch(cam, texture)
            .alpha()
            .anchored(anchor)
            .rung(Rung::FOOTPRINT, Rung::DECAL_RASTER)
            .owner(owner);
        batch.extend(verts.iter().map(|v| EffectVertex {
            color: [colour[0], colour[1], colour[2], v.color[3] * fade],
            ..*v
        }));
        batch.tris();
    }
}
