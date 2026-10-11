//! Fork: blood stains on the body, gated by the Blood option (`violenceLevel`) and sized by the
//! Physics page's Blood Stains slider (`bloodStains`, 0 off).
//!
//! A hit that sprays blood (`gore`) also leaves a stain on its victim, on the side facing the
//! attacker, bigger the bigger the hit's share of the victim's health and biggest on a killing
//! blow. Stains last a while, then fade. A body's texture and materials are shared by every unit
//! of the same look, so a stain cannot be painted there: each stained unit's skinned parts are
//! drawn as copies of their meshes carrying a vertex colour, which the model shader multiplies
//! into the texture, on the unit's own palette slot. A part the body sway already copies is
//! stained on that copy instead. Stains sit in bind space, so they ride the body through every
//! pose and a ragdoll.

use bevy::asset::AssetId;
use bevy::camera::visibility::{NoFrustumCulling, VisibilitySystems};
use bevy::ecs::entity::EntityHashMap;
use bevy::mesh::{MeshTag, VertexAttributeValues};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_world::mesh_tag::rig_of;
use benilla_world::model_render::park::ParkedMesh;
use benilla_world::model_render::ModelPart;
use benilla_world::rig_palette::{RigPart, RigSkin};

use super::blow::Hit;
use super::gore::default_colour;
use super::sway::{manage_sways, Sways};
use crate::creature_anim::{unit_blood_id, violence_level, BloodTables};
use crate::entities::{Creatures, HeldAttached};
use crate::net::{GuidIndex, NetEntity, ObjectStore};

/// At most this many units carry stains; a newly stained unit drops the one hit longest ago.
const MAX_UNITS: usize = 12;
/// At most this many stains on one unit; a new one drops the oldest.
const MAX_STAINS: usize = 16;
/// A stain's radius as a share of the body's height: this much for any hit, plus this much more
/// for a hit worth the victim's whole health, × Blood Stains.
const BASE_RADIUS: f32 = 0.09;
const RADIUS_PER_BAR: f32 = 0.2;
/// The Blood Stains slider's top.
const MAX_AMOUNT: f32 = 3.0;
/// Stains land at a height drawn evenly between these shares of the body's height off the feet
/// (the thighs to the chest, under the face), on the surface the way the attacker stands, turned
/// up to this much (rad) either side; the vertices searched lie within this share of the height
/// of that height. A spot drawn evenly, not a vertex, so the dense face and hands are not
/// favoured.
const BAND: (f32, f32) = (0.35, 0.72);
const SPREAD: f32 = 0.7;
const SLICE: f32 = 0.04;
/// The attacker's weapons are bloodied too when it stands within this (yd) of its victim: the
/// mainhand always, the offhand on this share of hits. A weapon stain's radius is this share of
/// the item's size, plus this much more for a hit worth the victim's whole health.
const WEAPON_REACH: f32 = 8.0;
const OFFHAND_SHARE: f32 = 0.5;
const WEAPON_RADIUS: f32 = 0.15;
const WEAPON_RADIUS_PER_BAR: f32 = 0.25;
/// At most this many bloodied weapon parts.
const MAX_WEAPONS: usize = 24;
/// How long a stain lasts (s), the last stretch of it fading out.
const LIFE: f32 = 120.0;
const FADE: f32 = 40.0;
/// The most a stain covers the texture, and how ragged its edge is (a share of the radius).
const MAX_COVER: f32 = 0.9;
const RAGGED: f32 = 0.6;
/// The red a stain takes on the races whose skin or fur is green or dark (orc 2, tauren 6,
/// troll 8): over 1 in red, so multiplied into green or grey-blue it still reads as red, not as
/// brown or maroon.
const STRONG_RED: [f32; 3] = [1.8, 0.1, 0.07];
const STRONG_RACES: [u8; 3] = [2, 6, 8];
/// Fading stains are repainted this often (s).
const REPAINT: f32 = 0.5;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Stains>()
        .add_systems(
            Update,
            (stain_hits, manage_stains, manage_weapon_stains)
                .chain()
                .after(manage_sways),
        )
        .add_systems(
            PostUpdate,
            (sync_stain_copies, hide_stained_parts)
                .chain()
                .before(VisibilitySystems::VisibilityPropagate),
        );
}

/// Every stained unit, by its rig.
#[derive(Resource, Default)]
struct Stains {
    units: EntityHashMap<Stained>,
    /// Bloodied weapon mesh parts, by entity: an item's model is the unit's own entity, so its
    /// `Mesh3d` is swapped for a stained clone and put back when the stains fade.
    weapons: EntityHashMap<Weapon>,
    /// Which materials are capes, by id: the cloth cloaks draw those.
    capes: HashMap<AssetId<WowModelMaterial>, bool>,
    rng: u32,
}

impl Stains {
    /// A uniform draw in `[0, 1)`, xorshift.
    fn rand(&mut self) -> f32 {
        let mut x = self.rng.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// One stained unit: its stains, their colour, and the parts drawn with them.
struct Stained {
    stains: Vec<Stain>,
    colour: [f32; 3],
    parts: Vec<StainedPart>,
    last_hit: f32,
    last_paint: f32,
    dirty: bool,
}

/// One bloodied weapon part: the mesh it had, our stained clone, and its stains.
struct Weapon {
    original: Handle<Mesh>,
    mine: Option<Handle<Mesh>>,
    stains: Vec<Stain>,
    colour: [f32; 3],
    dirty: bool,
    last_paint: f32,
}

/// One stain, in bind space.
struct Stain {
    centre: Vec3,
    radius: f32,
    born: f32,
    seed: u32,
}

/// A stock part and where its stains are drawn.
struct StainedPart {
    part: Entity,
    stock: AssetId<Mesh>,
    target: Target,
}

/// Our own copy of the part, or the body sway's.
enum Target {
    Own { copy: Entity, mesh: Handle<Mesh> },
    Sway { mesh: AssetId<Mesh> },
}

/// Marks our copies.
#[derive(Component)]
struct StainCopy;

/// A unit's skinned part, as [`stain_hits`] and [`manage_stains`] read it.
type StockPart = (
    Entity,
    &'static RigPart,
    &'static MeshMaterial3d<WowModelMaterial>,
    Option<&'static Mesh3d>,
    Option<&'static ParkedMesh>,
    &'static MeshTag,
    Option<&'static ModelPart>,
);

/// The rig's own parts, not capes, each with its stock mesh.
fn rig_parts(
    rig: Entity,
    slot: u16,
    parts: &Query<StockPart, Without<StainCopy>>,
    stains: &mut Stains,
    materials: &Assets<WowModelMaterial>,
    server: &AssetServer,
) -> Vec<(Entity, AssetId<Mesh>)> {
    let mut out = Vec::new();
    for (part, rig_part, material, mesh, parked, tag, _) in parts {
        if rig_part.0 != rig || rig_of(tag.0) != slot {
            continue;
        }
        let Some(mesh) = mesh.map(|m| m.0.id()).or(parked.map(|p| p.0.id())) else {
            continue;
        };
        let cape = match stains.capes.get(&material.id()) {
            Some(&cape) => cape,
            None => {
                let Some(m) = materials.get(material.id()) else {
                    continue;
                };
                let cape = m
                    .base
                    .base_color_texture
                    .as_ref()
                    .and_then(|t| server.get_path(t.id()))
                    .is_some_and(|p| {
                        p.to_string()
                            .to_ascii_lowercase()
                            .contains("objectcomponents/cape")
                    });
                stains.capes.insert(material.id(), cape);
                cape
            }
        };
        if !cape {
            out.push((part, mesh));
        }
    }
    out.sort_by_key(|p| p.0);
    out
}

fn positions(mesh: &Mesh) -> Option<&[[f32; 3]]> {
    match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(p)) => Some(p),
        _ => None,
    }
}

/// Leaves a stain on each bleeding hit's victim, facing the attacker.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn stain_hits(
    mut hits: MessageReader<Hit>,
    victims: Query<(&RigSkin, &GlobalTransform, &NetEntity, &ObjectStore)>,
    places: Query<&GlobalTransform>,
    parts: Query<StockPart, Without<StainCopy>>,
    index: Res<GuidIndex>,
    creatures: Option<Res<Creatures>>,
    blood: Option<Res<BloodTables>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    materials: Res<Assets<WowModelMaterial>>,
    meshes: Res<Assets<Mesh>>,
    server: Res<AssetServer>,
    time: Res<Time>,
    mut stains: ResMut<Stains>,
    held: Query<&HeldAttached>,
    children: Query<&Children>,
    items: Query<&Mesh3d, With<MeshMaterial3d<WowModelMaterial>>>,
) {
    let violence = violence_level(cvars.as_deref());
    let scale = cvars
        .as_deref()
        .and_then(|c| c.num("bloodStains"))
        .unwrap_or(1.0)
        .clamp(0.0, MAX_AMOUNT);
    let Some(blood) = blood.filter(|_| violence > 0 && scale > 0.0) else {
        hits.clear();
        return;
    };

    let now = time.elapsed_secs();
    for hit in hits.read() {
        let Ok((skin, at, net, store)) = victims.get(hit.victim) else {
            continue;
        };
        let max = store.0.unit_max_health().unwrap_or(0).max(1) as f32;
        let health = store.0.unit_health().unwrap_or(0);
        let killing = health == 0 || hit.damage >= health;
        if !hit.physical && !killing {
            continue;
        }
        let Some(id) = unit_blood_id(creatures.as_deref(), &blood, net.display_id) else {
            continue;
        };
        if blood.0.splats(id, violence).is_empty() {
            continue; // bloodless
        }
        let share = if killing {
            1.0
        } else {
            (hit.damage as f32 / max).min(1.0)
        };
        let stock = rig_parts(
            hit.victim,
            skin.slot,
            &parts,
            &mut stains,
            &materials,
            &server,
        );
        let verts: Vec<Vec3> = stock
            .iter()
            .filter_map(|(_, m)| meshes.get(*m).and_then(positions))
            .flat_map(|p| p.iter().map(|&v| Vec3::from(v)))
            .collect();
        let (bottom, top) = verts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
            (lo.min(v.y), hi.max(v.y))
        });
        let height = top - bottom;
        if verts.is_empty() || !height.is_finite() || height <= 0.0 {
            continue;
        }
        // The attacker's way in the body's own frame, else a random side.
        let toward = index
            .0
            .get(&hit.attacker)
            .and_then(|&a| places.get(a).ok())
            .map(|a| {
                at.affine()
                    .inverse()
                    .transform_vector3(a.translation() - at.translation())
                    .with_y(0.0)
            })
            .filter(|d| d.length_squared() > 1e-6)
            .unwrap_or_else(|| {
                let angle = stains.rand() * std::f32::consts::TAU;
                Vec3::new(angle.cos(), 0.0, angle.sin())
            })
            .normalize();
        // A spot drawn evenly: a height in the band, a way round from the attacker's, and the
        // outermost vertex that way in a thin slice at that height.
        let y = bottom + height * (BAND.0 + (BAND.1 - BAND.0) * stains.rand());
        let turn = (stains.rand() * 2.0 - 1.0) * SPREAD;
        let way = Quat::from_rotation_y(turn) * toward;
        let slice = SLICE * height;
        let Some(centre) = verts
            .iter()
            .copied()
            .filter(|v| (v.y - y).abs() <= slice)
            .max_by(|a, b| a.with_y(0.0).dot(way).total_cmp(&b.with_y(0.0).dot(way)))
        else {
            continue;
        };
        let radius = height * (BASE_RADIUS + RADIUS_PER_BAR * share) * scale;
        let seed = (stains.rand() * u32::MAX as f32) as u32;
        let colour = if violence == 2
            && store
                .0
                .unit_race()
                .is_some_and(|r| STRONG_RACES.contains(&r))
        {
            STRONG_RED
        } else {
            default_colour(violence)
        };
        if !stains.units.contains_key(&hit.victim) {
            info!(
                "blood stain: unit {} stained over {} parts, {} vertices",
                hit.victim,
                stock.len(),
                verts.len()
            );
        }
        let unit = stains.units.entry(hit.victim).or_insert_with(|| Stained {
            stains: Vec::new(),
            colour,
            parts: Vec::new(),
            last_hit: now,
            last_paint: now,
            dirty: true,
        });
        unit.colour = colour;
        unit.last_hit = now;
        unit.dirty = true;
        unit.stains.push(Stain {
            centre,
            radius,
            born: now,
            seed,
        });
        if unit.stains.len() > MAX_STAINS {
            unit.stains.remove(0);
        }
        // The attacker's weapon, when it stood close enough to have struck with it.
        let Some(&attacker) = index.0.get(&hit.attacker) else {
            continue;
        };
        let close = places
            .get(attacker)
            .is_ok_and(|a| a.translation().distance(at.translation()) <= WEAPON_REACH);
        let Some(attached) = held.get(attacker).ok().filter(|_| close) else {
            continue;
        };
        let offhand = stains.rand() < OFFHAND_SHARE;
        let roots: Vec<Entity> = attached
            .spawned_attach()
            .filter(|&(slot, _, _)| slot == 0 || slot == 1 && offhand)
            .map(|(_, root, _)| root)
            .collect();
        for root in roots {
            // The item's mesh parts: its root's children and theirs.
            let mut found = Vec::new();
            for child in children.get(root).into_iter().flatten() {
                found.push(*child);
                found.extend(children.get(*child).into_iter().flatten().copied());
            }
            for part in found {
                let Ok(mesh) = items.get(part) else {
                    continue;
                };
                let original = stains
                    .weapons
                    .get(&part)
                    .map_or_else(|| mesh.0.clone(), |w| w.original.clone());
                let Some(verts) = meshes.get(&original).and_then(positions) else {
                    continue;
                };
                if verts.is_empty() {
                    continue;
                }
                let (lo, hi) = verts.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &v| {
                    (lo.min(Vec3::from(v)), hi.max(Vec3::from(v)))
                });
                let size = lo.distance(hi);
                let pick = ((stains.rand() * verts.len() as f32) as usize).min(verts.len() - 1);
                let centre = Vec3::from(verts[pick]);
                let seed = (stains.rand() * u32::MAX as f32) as u32;
                let entry = stains.weapons.entry(part).or_insert_with(|| Weapon {
                    original,
                    mine: None,
                    stains: Vec::new(),
                    colour,
                    dirty: true,
                    last_paint: now,
                });
                entry.colour = colour;
                entry.dirty = true;
                entry.stains.push(Stain {
                    centre,
                    radius: size * (WEAPON_RADIUS + WEAPON_RADIUS_PER_BAR * share) * scale,
                    born: now,
                    seed,
                });
                if entry.stains.len() > MAX_STAINS {
                    entry.stains.remove(0);
                }
            }
        }
    }
}

/// How much of a stain's colour is left: whole, then fading over its last stretch.
fn left(stain: &Stain, now: f32) -> f32 {
    ((LIFE - (now - stain.born)) / FADE).clamp(0.0, 1.0)
}

/// A hash in `[0, 1)` of a vertex and a stain, for a ragged edge.
fn ragged(index: usize, seed: u32) -> f32 {
    let mut x = (index as u32).wrapping_mul(0x9e37_79b9) ^ seed;
    x ^= x >> 16;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// The stock mesh with its stains as a vertex colour over its own.
fn paint(stock: &Mesh, stains: &[Stain], colour: [f32; 3], now: f32) -> Option<Vec<[f32; 4]>> {
    let positions = positions(stock)?;
    let base: Option<Vec<[f32; 4]>> = match stock.attribute(Mesh::ATTRIBUTE_COLOR) {
        Some(VertexAttributeValues::Float32x4(c)) if c.len() == positions.len() => Some(c.clone()),
        _ => None,
    };
    let tint = Vec3::from(colour);
    Some(
        positions
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let p = Vec3::from(*p);
                let cover = stains
                    .iter()
                    .map(|s| {
                        let edge = s.radius * (1.0 - RAGGED * 0.5 + RAGGED * ragged(i, s.seed));
                        let t = (1.0 - p.distance(s.centre) / edge).clamp(0.0, 1.0);
                        t * t * (3.0 - 2.0 * t) * left(s, now)
                    })
                    .fold(0.0, f32::max)
                    * MAX_COVER;
                let c = Vec3::ONE.lerp(tint, cover);
                let b = base.as_ref().map_or([1.0; 4], |b| b[i]);
                [b[0] * c.x, b[1] * c.y, b[2] * c.z, b[3]]
            })
            .collect(),
    )
}

/// Keeps each stained unit's parts drawn with their stains: our copies, or the sway's, rebuilt on
/// a re-dress; drops units whose stains have faded or that are gone.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn manage_stains(
    mut commands: Commands,
    mut stains: ResMut<Stains>,
    sways: Res<Sways>,
    rigs: Query<&RigSkin>,
    parts: Query<StockPart, Without<StainCopy>>,
    materials: Res<Assets<WowModelMaterial>>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut visibility: Query<&mut Visibility>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    let stains = &mut *stains;
    // Faded, gone, or past the cap: dropped, the stock parts shown again.
    for unit in stains.units.values_mut() {
        unit.stains.retain(|s| left(s, now) > 0.0);
    }
    let mut by_hit: Vec<(f32, Entity)> = stains
        .units
        .iter()
        .map(|(rig, u)| (u.last_hit, *rig))
        .collect();
    by_hit.sort_by(|a, b| b.0.total_cmp(&a.0));
    let keep: Vec<Entity> = by_hit
        .iter()
        .take(MAX_UNITS)
        .map(|(_, rig)| *rig)
        .filter(|rig| rigs.contains(*rig))
        .collect();
    let gone: Vec<Entity> = stains
        .units
        .iter()
        .filter(|(rig, u)| u.stains.is_empty() || !keep.contains(rig))
        .map(|(rig, _)| *rig)
        .collect();
    for rig in gone {
        if let Some(unit) = stains.units.remove(&rig) {
            for part in unit.parts {
                release(&mut commands, &sways, &mut meshes, &mut visibility, part);
            }
        }
    }

    let rigs_now: Vec<Entity> = stains.units.keys().copied().collect();
    for rig in rigs_now {
        let Ok(skin) = rigs.get(rig) else {
            continue;
        };
        let stock = rig_parts(rig, skin.slot, &parts, stains, &materials, &server);
        let Some(unit) = stains.units.get_mut(&rig) else {
            continue;
        };
        // Parts gone or re-dressed lose their copies; the rest are checked against the sway.
        let old = std::mem::take(&mut unit.parts);
        for part in old {
            let current = stock
                .iter()
                .any(|&(p, m)| p == part.part && m == part.stock);
            let sway = sways.copy_mesh(part.part).map(|h| h.id());
            let keep = current
                && match &part.target {
                    Target::Own { .. } => sway.is_none(),
                    Target::Sway { mesh } => sway == Some(*mesh),
                };
            if keep {
                unit.parts.push(part);
            } else {
                release(&mut commands, &sways, &mut meshes, &mut visibility, part);
                unit.dirty = true;
            }
        }
        for &(part, mesh) in &stock {
            if unit.parts.iter().any(|p| p.part == part) {
                continue;
            }
            let target = match sways.copy_mesh(part) {
                Some(sway) => Target::Sway { mesh: sway.id() },
                None => {
                    let (Some(stock_mesh), Ok((_, _, material, _, _, tag, model_part))) =
                        (meshes.get(mesh).cloned(), parts.get(part))
                    else {
                        continue;
                    };
                    let handle = meshes.add(stock_mesh);
                    let mut copy = commands.spawn((
                        Name::new("blood stain"),
                        Mesh3d(handle.clone()),
                        MeshMaterial3d(material.0.clone()),
                        MeshTag(tag.0),
                        Transform::default(),
                        Visibility::default(),
                        NoFrustumCulling,
                        StainCopy,
                        ChildOf(rig),
                    ));
                    if let Some(model_part) = model_part {
                        copy.insert(*model_part);
                    }
                    Target::Own {
                        copy: copy.id(),
                        mesh: handle,
                    }
                }
            };
            unit.parts.push(StainedPart {
                part,
                stock: mesh,
                target,
            });
            unit.dirty = true;
        }
        // Painted on a change, and again now and then while a stain fades.
        let fading = unit.stains.iter().any(|s| now - s.born > LIFE - FADE);
        if !(unit.dirty || fading && now - unit.last_paint >= REPAINT) {
            continue;
        }
        unit.dirty = false;
        unit.last_paint = now;
        for part in &unit.parts {
            let Some(colours) = meshes
                .get(part.stock)
                .and_then(|m| paint(m, &unit.stains, unit.colour, now))
            else {
                continue;
            };
            let target = match &part.target {
                Target::Own { mesh, .. } => mesh.id(),
                Target::Sway { mesh } => *mesh,
            };
            if let Some(mesh) = meshes.get_mut(target) {
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours);
            }
        }
    }
}

/// Stops drawing a part's stains: our copy goes and the stock part shows again, unless the sway
/// draws it now.
fn release(
    commands: &mut Commands,
    sways: &Sways,
    meshes: &mut Assets<Mesh>,
    visibility: &mut Query<&mut Visibility>,
    part: StainedPart,
) {
    match part.target {
        Target::Own { copy, mesh } => {
            if let Ok(mut e) = commands.get_entity(copy) {
                e.despawn();
            }
            meshes.remove(&mesh);
            if sways.copy_mesh(part.part).is_none() {
                if let Ok(mut vis) = visibility.get_mut(part.part) {
                    *vis = Visibility::Inherited;
                }
            }
        }
        // The sway's copy keeps its stains until it is rebuilt.
        Target::Sway { .. } => {}
    }
}

/// Keeps our copies on the stock parts' material and tag: fades, fog and the slot.
fn sync_stain_copies(
    stains: Res<Stains>,
    stock: Query<(&MeshMaterial3d<WowModelMaterial>, &MeshTag), Without<StainCopy>>,
    mut copies: Query<(&mut MeshMaterial3d<WowModelMaterial>, &mut MeshTag), With<StainCopy>>,
) {
    for part in stains.units.values().flat_map(|u| &u.parts) {
        let Target::Own { copy, .. } = part.target else {
            continue;
        };
        if let (Ok((mat, tag)), Ok((mut copy_mat, mut copy_tag))) =
            (stock.get(part.part), copies.get_mut(copy))
        {
            if copy_mat.0 != mat.0 {
                copy_mat.0 = mat.0.clone();
            }
            if copy_tag.0 != tag.0 {
                copy_tag.0 = tag.0;
            }
        }
    }
}

/// Keeps the stock parts our copies stand in for hidden, after the model-visibility pass.
fn hide_stained_parts(stains: Res<Stains>, mut visibility: Query<&mut Visibility>) {
    for part in stains.units.values().flat_map(|u| &u.parts) {
        if let Target::Own { .. } = part.target {
            if let Ok(mut vis) = visibility.get_mut(part.part) {
                if *vis != Visibility::Hidden {
                    *vis = Visibility::Hidden;
                }
            }
        }
    }
}

/// Draws each bloodied weapon part with its stained clone, repainted as the stains fade, and
/// puts its own mesh back once they have.
fn manage_weapon_stains(
    mut stains: ResMut<Stains>,
    mut items: Query<&mut Mesh3d>,
    mut meshes: ResMut<Assets<Mesh>>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    let stains = &mut *stains;
    // The newest kept: an over-cap part goes like a faded one.
    if stains.weapons.len() > MAX_WEAPONS {
        let mut newest: Vec<(f32, Entity)> = stains
            .weapons
            .iter()
            .map(|(e, w)| (w.stains.last().map_or(0.0, |s| s.born), *e))
            .collect();
        newest.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, e) in newest.into_iter().skip(MAX_WEAPONS) {
            if let Some(w) = stains.weapons.get_mut(&e) {
                w.stains.clear();
            }
        }
    }
    let mut done = Vec::new();
    for (part, weapon) in stains.weapons.iter_mut() {
        weapon.stains.retain(|s| left(s, now) > 0.0);
        let Ok(mut mesh) = items.get_mut(*part) else {
            // The item is gone.
            done.push(*part);
            continue;
        };
        let ours = weapon.mine.as_ref().is_some_and(|m| m.id() == mesh.0.id());
        if !ours && mesh.0.id() != weapon.original.id() {
            // The model was rebuilt under us: let it be.
            done.push(*part);
            continue;
        }
        if weapon.stains.is_empty() {
            if ours {
                mesh.0 = weapon.original.clone();
            }
            done.push(*part);
            continue;
        }
        let fading = weapon.stains.iter().any(|s| now - s.born > LIFE - FADE);
        if !(weapon.dirty || fading && now - weapon.last_paint >= REPAINT) {
            continue;
        }
        weapon.dirty = false;
        weapon.last_paint = now;
        let Some(colours) = meshes
            .get(&weapon.original)
            .and_then(|m| paint(m, &weapon.stains, weapon.colour, now))
        else {
            continue;
        };
        match &weapon.mine {
            Some(mine) => {
                if let Some(m) = meshes.get_mut(mine) {
                    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours);
                }
            }
            None => {
                let Some(mut copy) = meshes.get(&weapon.original).cloned() else {
                    continue;
                };
                copy.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours);
                weapon.mine = Some(meshes.add(copy));
            }
        }
        if let Some(mine) = &weapon.mine {
            if mesh.0.id() != mine.id() {
                mesh.0 = mine.clone();
            }
        }
    }
    for part in done {
        if let Some(weapon) = stains.weapons.remove(&part) {
            if let Some(mine) = weapon.mine {
                meshes.remove(&mine);
            }
        }
    }
}
