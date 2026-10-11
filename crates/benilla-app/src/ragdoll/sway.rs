//! Fork: soft body sway on female units, behind the Physics page's Body Physics slider
//! (`bodyPhysics`, 0 off), sized by the Chest Size and Rear Size sliders (`bodySize`,
//! `bodyRearSize`).
//!
//! The chest and the rear have no bones of their own, so each near female unit's skinned parts
//! that carry them are drawn as copies on a second palette slot with a bone more than the unit's
//! per soft part: every row the unit's own, plus one per part that is the bone carrying it, moved
//! by a damped spring that lags the body's motion. The copies' vertices near each part are
//! re-weighted onto its bone, fading out toward the edge. The parts are found from the bind pose:
//! the highest arm root (a bone off the centre with a long chain under it) sets the shoulder
//! height, and the most forward vertex each side in the band under it is a chest tip; the highest
//! leg root (a bone off the centre whose chain reaches the ground) sets the hip height, and the
//! most backward vertex each side in the band about it is a rear tip. The rear's spring is
//! stiffer and strays less, so it bounces less. A pair that is not found is left alone, and the
//! log says why.

use bevy::asset::AssetId;
use bevy::camera::visibility::{NoFrustumCulling, VisibilitySystems};
use bevy::ecs::entity::EntityHashMap;
use bevy::math::Affine3A;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::mesh::{MeshTag, VertexAttributeValues};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_assets::{ATTRIBUTE_WOW_JOINT_INDEX, ATTRIBUTE_WOW_JOINT_WEIGHT};
use benilla_world::billboard::BillboardPlace;
use benilla_world::mesh_tag::{rig_of, with_rig};
use benilla_world::model_render::park::ParkedMesh;
use benilla_world::model_render::ModelPart;
use benilla_world::rig_anim::{finalize_rig_worlds, RigPose};
use benilla_world::rig_palette::{RigPalettes, RigPart, RigSkin};
use benilla_world::view::WorldCamera;

use crate::net::ObjectStore;

/// At most this many units sway, the nearest to the camera within this range (yd).
const MAX_UNITS: usize = 10;
const MAX_RANGE: f32 = 40.0;
/// `UNIT_FIELD_BYTES_0`'s female gender.
const FEMALE: u8 = 1;
/// An arm root is off the centre by this share of the height, with this many bones under it.
const ARM_OFF_CENTRE: f32 = 0.05;
const ARM_CHAIN: usize = 4;
/// The band searched for the tips, under the shoulder, as shares of the height.
const BAND: f32 = 0.25;
/// The tips lie between these shares of the shoulder's distance from the centre.
const INNER: f32 = 0.2;
const OUTER: f32 = 0.85;
/// The sway's radius as a share of the tips' spread, how far behind the tip its centre sits and
/// how far out from there the weights fade to nothing, as shares of that radius.
const RADIUS: f32 = 0.55;
const CENTRE_DEPTH: f32 = 0.4;
const REACH: f32 = 1.4;
/// The spring: its frequency (Hz) and damping ratio, its step (s), and the furthest it strays as
/// a share of the radius at Body Physics 1.
const FREQUENCY: f32 = 5.0;
const DAMPING: f32 = 0.15;
/// The share of the body's sideways and forward motion the spring lags behind: a little, so the
/// sway is mostly an up-and-down bounce.
const HORIZONTAL: f32 = 0.2;
const STEP: f32 = 1.0 / 120.0;
const MAX_STEPS: u32 = 8;
const MAX_OFFSET: f32 = 0.35;
const MAX_AMOUNT: f32 = 3.0;
/// The Chest Size slider's range (`bodySize`), a scale about each side's centre.
const MIN_SIZE: f32 = 0.5;
const MAX_SIZE: f32 = 2.5;
/// A jump of the anchor past this (yd) in one frame is a teleport: the spring starts over.
const TELEPORT: f32 = 2.0;
/// A leg root is off the centre by this share of the height, with this many bones under it, sits
/// this share of the height under the shoulder, and its chain reaches this share of the height
/// from the ground. The hip found must sit at least `HIP_LOWEST` of the height off the ground, so
/// a knee is never taken for it.
const LEG_OFF_CENTRE: f32 = 0.02;
const LEG_CHAIN: usize = 2;
const LEG_UNDER_SHOULDER: f32 = 0.15;
const LEG_REACH: f32 = 0.15;
const HIP_LOWEST: f32 = 0.35;
/// The band searched for the rear's tips, under and over the hip, as shares of the height.
const REAR_BELOW: f32 = 0.15;
const REAR_ABOVE: f32 = 0.12;
/// The rear's tips lie between these shares of the hip's distance from the centre, and its
/// radius is this share of the height: the tips' spread and the hip's width vary too much by
/// race (a dwarf's wide hips, a night elf's narrow ones) to size it.
const REAR_INNER: f32 = 0.5;
const REAR_OUTER: f32 = 1.4;
const REAR_RADIUS: f32 = 0.055;
/// How far each part's weight reaches up and down against its reach outward: the chest's
/// reaches further up so it slopes from the collar instead of standing as a ball, the rear's
/// less so it stays off the waist and thighs.
const CHEST_REACH_UP: f32 = 2.0;
const REAR_REACH_VERTICAL: f32 = 0.9;
/// The share of each size step taken across and up and down: a part grows mostly outward, so
/// it neither balloons upward to the neck nor widens the waist.
const CHEST_GROW_ACROSS: f32 = 0.7;
const CHEST_GROW_VERTICAL: f32 = 0.5;
const REAR_GROW_ACROSS: f32 = 0.75;
const REAR_GROW_VERTICAL: f32 = 0.75;
/// How far behind its tip the rear's centre sits, as a share of its radius: deeper than the
/// chest's, so it swells round instead of drawing its tip out to a point.
const REAR_CENTRE_DEPTH: f32 = 0.8;
/// The rear's spring, against the chest's: its frequency (Hz) and damping ratio, the share of the
/// body's motion it lags, the share of the chest's furthest stray, and the most of a vertex its
/// bone takes, so the hips and thighs still bend it.
const REAR_FREQUENCY: f32 = 7.0;
const REAR_DAMPING: f32 = 0.3;
const REAR_LAG: f32 = 0.5;
const REAR_STRAY: f32 = 0.5;
const REAR_WEIGHT: f32 = 0.75;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Sways>()
        .add_systems(Update, manage_sways)
        .add_systems(
            PostUpdate,
            pose_sways.after(finalize_rig_worlds).in_set(BillboardPlace),
        )
        .add_systems(
            PostUpdate,
            hide_stock_parts.before(VisibilitySystems::VisibilityPropagate),
        );
}

/// Every swaying unit, by its rig.
#[derive(Resource, Default)]
pub(super) struct Sways {
    live: EntityHashMap<Sway>,
    /// Rigs whose chest could not be found, with the parts it was looked for on; retried once
    /// those change (a re-dress).
    failed: EntityHashMap<Vec<(Entity, AssetId<Mesh>)>>,
    /// Which materials are capes or hair, by id: the cloth cloaks draw the capes, and hair hanging
    /// over the chest would otherwise be taken for it.
    capes: HashMap<AssetId<WowModelMaterial>, bool>,
    /// Identity bindposes, by bone count.
    ibps: HashMap<u32, Handle<SkinnedMeshInverseBindposes>>,
}

impl Sways {
    /// The mesh of the copy drawn in a stock part's place, if one is: the stains paint it.
    pub(super) fn copy_mesh(&self, part: Entity) -> Option<&Handle<Mesh>> {
        self.live
            .values()
            .flat_map(|s| &s.copies)
            .find(|(p, _, _)| *p == part)
            .map(|(_, _, mesh)| mesh)
    }
}

/// One swaying unit: its candidate parts (to notice a re-dress), the copies, and the springs.
struct Sway {
    holder: Entity,
    /// The candidate parts and their meshes: a re-dress swaps either, and the copies are rebuilt.
    parts: Vec<(Entity, AssetId<Mesh>)>,
    copies: Vec<(Entity, Entity, Handle<Mesh>)>,
    bones: usize,
    sides: Vec<Side>,
}

/// One soft part's spring: the bone carrying it, the part at bind, and the lag.
struct Side {
    carrier: usize,
    lobe: Lobe,
    last: Option<Vec3>,
    offset: Vec3,
    velocity: Vec3,
}

/// On a sway copy, so the stock and copy queries stay apart.
#[derive(Component)]
pub(super) struct SwayCopy;

/// A rig's candidate parts with their meshes, by part.
type PartMeshes = Vec<(Entity, AssetId<Mesh>)>;

/// What [`manage_sways`] reads of a candidate part.
type BodyPart = (
    Entity,
    &'static RigPart,
    &'static MeshMaterial3d<WowModelMaterial>,
    Option<&'static Mesh3d>,
    Option<&'static ParkedMesh>,
    &'static MeshTag,
    Option<&'static ModelPart>,
);

/// A pair of soft parts at bind: per side the tip, and the radius.
struct Pair {
    tips: [Vec3; 2],
    radius: f32,
}

/// What a rig's parts show at bind: the chest and the rear, each found or why not.
struct Body {
    chest: Result<Pair, &'static str>,
    rear: Result<Pair, &'static str>,
}

/// One soft part at bind: its centre, the way out of the body, its radius, and whether it is
/// the rear.
#[derive(Clone, Copy)]
struct Lobe {
    centre: Vec3,
    out: Vec3,
    radius: f32,
    rear: bool,
}

/// Picks the nearest female units to sway, builds their copies, and drops the rest.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
pub(super) fn manage_sways(
    mut commands: Commands,
    mut sways: ResMut<Sways>,
    parts: Query<BodyPart, Without<SwayCopy>>,
    rigs: Query<(
        &RigSkin,
        &RigPose,
        &GlobalTransform,
        &ObjectStore,
        Option<&Name>,
    )>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    materials: Res<Assets<WowModelMaterial>>,
    mut material_events: MessageReader<AssetEvent<WowModelMaterial>>,
    mut mesh_events: MessageReader<AssetEvent<Mesh>>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut ibps: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<RigPalettes>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    mut visibility: Query<&mut Visibility>,
) {
    let sways = &mut *sways;
    for ev in material_events.read() {
        if let AssetEvent::Modified { id } | AssetEvent::Removed { id } = ev {
            sways.capes.remove(id);
        }
    }
    sways.failed.retain(|rig, _| rigs.contains(*rig));
    // A stock mesh rewritten in place (a re-dress) rebuilds the sways drawing it.
    let rewritten: HashSet<AssetId<Mesh>> = mesh_events
        .read()
        .filter_map(|ev| match ev {
            AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    let on = cvars
        .as_deref()
        .and_then(|c| c.num("bodyPhysics"))
        .unwrap_or(1.0)
        > 0.0;
    let eye = camera.single().ok().map(|g| g.translation());
    // Each near female rig's body parts (not its cloak), when on.
    let mut by_rig: EntityHashMap<(f32, PartMeshes)> = EntityHashMap::default();
    if let (true, Some(eye)) = (on, eye) {
        for (part, rig_part, material, mesh, parked, tag, _) in &parts {
            let Ok((skin, _, at, store, _)) = rigs.get(rig_part.0) else {
                continue;
            };
            if rig_of(tag.0) != skin.slot || store.0.unit_gender() != Some(FEMALE) {
                continue;
            }
            let Some(mesh_id) = mesh.map(|m| m.0.id()).or(parked.map(|p| p.0.id())) else {
                continue;
            };
            let cape = match sways.capes.get(&material.id()) {
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
                            let p = p.to_string().to_ascii_lowercase();
                            p.contains("objectcomponents/cape") || p.contains("hair")
                        });
                    sways.capes.insert(material.id(), cape);
                    cape
                }
            };
            let d = at.translation().distance(eye);
            if !cape && d <= MAX_RANGE {
                by_rig
                    .entry(rig_part.0)
                    .or_insert((d, Vec::new()))
                    .1
                    .push((part, mesh_id));
            }
        }
    }
    let mut wanted: Vec<(f32, Entity, PartMeshes)> = by_rig
        .into_iter()
        .map(|(rig, (d, mut parts))| {
            parts.sort_by_key(|p| p.0);
            (d, rig, parts)
        })
        .filter(|(_, rig, parts)| sways.failed.get(rig) != Some(parts))
        .collect();
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    wanted.truncate(MAX_UNITS);

    // Drop the sways no longer wanted, or whose rig or parts changed, and show their stock parts.
    let gone: Vec<Entity> = sways
        .live
        .iter()
        .filter(|(rig, s)| {
            !wanted
                .iter()
                .any(|(_, r, parts)| r == *rig && *parts == s.parts)
                || s.parts.iter().any(|(_, m)| rewritten.contains(m))
        })
        .map(|(rig, _)| *rig)
        .collect();
    for rig in gone {
        let Some(sway) = sways.live.remove(&rig) else {
            continue;
        };
        if let Ok(mut e) = commands.get_entity(sway.holder) {
            e.despawn();
        }
        for (part, _, mesh) in &sway.copies {
            meshes.remove(mesh);
            if let Ok(mut vis) = visibility.get_mut(*part) {
                *vis = Visibility::Inherited;
            }
        }
    }

    for (_, rig, part_ids) in wanted {
        if sways.live.contains_key(&rig) {
            continue;
        }
        let Ok((skin, pose, _, _, name)) = rigs.get(rig) else {
            continue;
        };
        // Every part's mesh, put down or not, must be in.
        let mut stocks: Vec<(Entity, Handle<Mesh>)> = Vec::new();
        for &(part, _) in &part_ids {
            let Ok((_, _, _, mesh, parked, _, _)) = parts.get(part) else {
                continue;
            };
            if let Some(handle) = mesh.map(|m| m.0.clone()).or(parked.map(|p| p.0.clone())) {
                if meshes.contains(&handle) {
                    stocks.push((part, handle));
                }
            }
        }
        if stocks.len() != part_ids.len() {
            continue;
        }
        let name = name.map_or_else(|| format!("{rig}"), |n| n.as_str().to_owned());
        let pivots = bind_pivots(pose);
        let body = {
            let positions: Vec<&[[f32; 3]]> = stocks
                .iter()
                .filter_map(
                    |(_, h)| match meshes.get(h)?.attribute(Mesh::ATTRIBUTE_POSITION) {
                        Some(VertexAttributeValues::Float32x3(p)) => Some(p.as_slice()),
                        _ => None,
                    },
                )
                .collect();
            find_body(&positions, &pivots, &pose.parents)
        };
        let body = match body {
            Ok(body) => body,
            Err(why) => {
                warn!("body physics: {name} left as is: {why}");
                sways.failed.insert(rig, part_ids.clone());
                continue;
            }
        };
        // Each pair found gives a part per side, the chest first.
        let mut lobes: Vec<Lobe> = Vec::new();
        for (pair, rear, what) in [(&body.chest, false, "chest"), (&body.rear, true, "rear")] {
            let pair = match pair {
                Ok(pair) => pair,
                Err(why) => {
                    warn!("body physics: {name}: no {what}: {why}");
                    continue;
                }
            };
            let out = if rear { Vec3::Z } else { Vec3::NEG_Z };
            for tip in pair.tips {
                lobes.push(Lobe {
                    centre: tip
                        - out
                            * (if rear {
                                REAR_CENTRE_DEPTH
                            } else {
                                CENTRE_DEPTH
                            } * pair.radius),
                    out,
                    radius: pair.radius,
                    rear,
                });
            }
        }
        if lobes.is_empty() {
            sways.failed.insert(rig, part_ids.clone());
            continue;
        }
        let bones = skin.bones() as usize;
        // The copies: each part with vertices near a soft part, re-weighted onto the new bones.
        let mut built: Vec<(Entity, Mesh)> = Vec::new();
        let mut carried = vec![HashMap::<u16, f32>::new(); lobes.len()];
        for (part, handle) in &stocks {
            let Some(stock) = meshes.get(handle) else {
                continue;
            };
            if let Some(copy) = reweight(stock, &lobes, bones, &mut carried) {
                built.push((*part, copy));
            }
        }
        if built.is_empty() {
            warn!("body physics: {name} left as is: no vertices near the chest or rear");
            sways.failed.insert(rig, part_ids.clone());
            continue;
        }
        // The chest rides its heaviest bone; the rear the root-most of its heavy ones (the pelvis
        // over a thigh), so a stride does not swing it.
        let under = bones_under(&pose.parents, pivots.len());
        let carrier = |side: &HashMap<u16, f32>, rear: bool| {
            let heaviest = side.values().copied().fold(0.0, f32::max);
            let rank = |bone: &u16| under.get(*bone as usize).copied().unwrap_or(0);
            side.iter()
                .filter(|(_, &w)| !rear || w >= 0.25 * heaviest)
                .max_by(|a, b| {
                    if rear {
                        rank(a.0).cmp(&rank(b.0)).then(a.1.total_cmp(b.1))
                    } else {
                        a.1.total_cmp(b.1)
                    }
                })
                .map_or(0, |(&bone, _)| bone as usize)
        };
        let sides: Vec<Side> = lobes
            .iter()
            .zip(&carried)
            .map(|(lobe, side)| Side {
                carrier: carrier(side, lobe.rear),
                lobe: *lobe,
                last: None,
                offset: Vec3::ZERO,
                velocity: Vec3::ZERO,
            })
            .collect();
        let total = (bones + sides.len()) as u32;
        let ibp = sways
            .ibps
            .entry(total)
            .or_insert_with(|| {
                ibps.add(SkinnedMeshInverseBindposes::from(vec![
                    Mat4::IDENTITY;
                    total as usize
                ]))
            })
            .clone();
        let Some(own) = RigSkin::allocate_bones(&mut palettes, total, ibp) else {
            continue;
        };
        info!(
            "body physics: {name}: {} parts, chest {}, rear {}, carriers {:?}, radii {:?}, \
             centres {:?}",
            built.len(),
            body.chest.is_ok(),
            body.rear.is_ok(),
            sides.iter().map(|s| s.carrier).collect::<Vec<_>>(),
            sides.iter().map(|s| s.lobe.radius).collect::<Vec<_>>(),
            sides.iter().map(|s| s.lobe.centre).collect::<Vec<_>>()
        );
        let slot = own.slot;
        let holder = commands
            .spawn((
                Name::new("body sway"),
                Transform::default(),
                Visibility::default(),
                own,
                ChildOf(rig),
            ))
            .id();
        let mut copies = Vec::new();
        for (part, mesh) in built {
            let Ok((_, _, material, _, _, tag, model_part)) = parts.get(part) else {
                continue;
            };
            let mesh = meshes.add(mesh);
            let mut copy = commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.0.clone()),
                MeshTag(with_rig(tag.0, slot)),
                Transform::default(),
                Visibility::default(),
                NoFrustumCulling,
                SwayCopy,
                ChildOf(holder),
            ));
            if let Some(model_part) = model_part {
                copy.insert(*model_part);
            }
            copies.push((part, copy.id(), mesh));
        }
        sways.live.insert(
            rig,
            Sway {
                holder,
                parts: part_ids,
                copies,
                bones,
                sides,
            },
        );
    }
}

/// Each bone's pivot at bind, in the model's space.
fn bind_pivots(pose: &RigPose) -> Vec<Vec3> {
    let n = pose.binds.len().min(pose.parents.len());
    let mut pivots = vec![Vec3::ZERO; n];
    for i in 0..n {
        let parent = usize::try_from(pose.parents[i]).ok().filter(|&p| p < i);
        pivots[i] = parent.map_or(Vec3::ZERO, |p| pivots[p]) + pose.binds[i];
    }
    pivots
}

/// How many bones are under each bone.
fn bones_under(parents: &[i16], n: usize) -> Vec<usize> {
    let mut under = vec![0usize; n];
    for i in (0..n).rev() {
        if let Some(p) = parent_of(parents, i) {
            under[p] += under[i] + 1;
        }
    }
    under
}

/// Bone `i`'s parent, when it comes before it.
fn parent_of(parents: &[i16], i: usize) -> Option<usize> {
    parents
        .get(i)
        .and_then(|&p| usize::try_from(p).ok())
        .filter(|&p| p < i)
}

/// The chest and the rear at bind. The chest: the shoulder height from the highest arm root, then
/// each side's most forward vertex (Bevy's front is −Z) in the band under it, between the centre
/// and the arm. The rear: the hip height from the highest leg root, then each side's most backward
/// vertex in the band about it.
fn find_body(
    positions: &[&[[f32; 3]]],
    pivots: &[Vec3],
    parents: &[i16],
) -> Result<Body, &'static str> {
    let all: Vec<Vec3> = positions
        .iter()
        .flat_map(|p| p.iter())
        .map(|&v| Vec3::from(v))
        .collect();
    let top = all.iter().map(|v| v.y).fold(f32::MIN, f32::max);
    let bottom = all.iter().map(|v| v.y).fold(f32::MAX, f32::min);
    let height = top - bottom;
    if !height.is_finite() || height <= 0.0 {
        return Err("no vertices");
    }
    let under = bones_under(parents, pivots.len());
    // The lowest pivot in each bone's chain.
    let mut lowest: Vec<f32> = pivots.iter().map(|p| p.y).collect();
    for i in (0..pivots.len()).rev() {
        if let Some(p) = parent_of(parents, i) {
            lowest[p] = lowest[p].min(lowest[i]);
        }
    }
    let shoulder = (0..pivots.len())
        .filter(|&i| pivots[i].x.abs() > ARM_OFF_CENTRE * height && under[i] >= ARM_CHAIN)
        .max_by(|&a, &b| pivots[a].y.total_cmp(&pivots[b].y))
        .ok_or("no arm found")?;
    let (shoulder_y, shoulder_x) = (pivots[shoulder].y, pivots[shoulder].x.abs());
    let chest = find_pair(
        &all,
        (shoulder_y - BAND * height)..=shoulder_y,
        shoulder_x * INNER..=shoulder_x * OUTER,
        false,
    );
    let rear = (0..pivots.len())
        .filter(|&i| {
            pivots[i].x.abs() > LEG_OFF_CENTRE * height
                && under[i] >= LEG_CHAIN
                && pivots[i].y < shoulder_y - LEG_UNDER_SHOULDER * height
                && lowest[i] < bottom + LEG_REACH * height
        })
        .max_by(|&a, &b| pivots[a].y.total_cmp(&pivots[b].y))
        .ok_or("no leg found")
        .and_then(|hip| {
            let (hip_y, hip_x) = (pivots[hip].y, pivots[hip].x.abs());
            if hip_y < bottom + HIP_LOWEST * height {
                return Err("the hip found is too low");
            }
            find_pair(
                &all,
                (hip_y - REAR_BELOW * height)..=(hip_y + REAR_ABOVE * height),
                hip_x * REAR_INNER..=hip_x * REAR_OUTER,
                true,
            )
            .map(|pair| Pair {
                radius: REAR_RADIUS * height,
                ..pair
            })
        });
    Ok(Body { chest, rear })
}

/// A pair in `band` (heights): each side's vertex standing furthest out of the body (forward, or
/// back for the rear) at a distance from the centre in `span`, the two matching and standing out
/// from the band's middle depth.
fn find_pair(
    all: &[Vec3],
    band: std::ops::RangeInclusive<f32>,
    span: std::ops::RangeInclusive<f32>,
    rear: bool,
) -> Result<Pair, &'static str> {
    let out = |v: &Vec3| if rear { v.z } else { -v.z };
    let tip = |sign: f32| {
        all.iter()
            .filter(|v| band.contains(&v.y) && span.contains(&(v.x * sign)))
            .max_by(|a, b| out(a).total_cmp(&out(b)))
            .copied()
    };
    let (Some(left), Some(right)) = (tip(1.0), tip(-1.0)) else {
        return Err("no tips in the band");
    };
    let spread = left.distance(right);
    let radius = RADIUS * spread;
    if radius <= 1e-3 {
        return Err("the tips coincide");
    }
    if (left.y - right.y).abs() > 0.3 * radius || (left.z - right.z).abs() > 0.3 * radius {
        return Err("the tips do not match");
    }
    let mut depths: Vec<f32> = all
        .iter()
        .filter(|v| band.contains(&v.y))
        .map(out)
        .collect();
    depths.sort_by(f32::total_cmp);
    let middle = depths[depths.len() / 2];
    if out(&left).min(out(&right)) < middle + 0.3 * radius {
        return Err("the tips do not stand out");
    }
    Ok(Pair {
        tips: [left, right],
        radius,
    })
}

/// The stock mesh with each vertex near a soft part re-weighted onto that part's bone (`bones`
/// plus its index in `lobes`), the weight fading from the centre; each part's old bones are summed
/// into `carried` by that weight. `None` when no vertex is near.
fn reweight(
    stock: &Mesh,
    lobes: &[Lobe],
    bones: usize,
    carried: &mut [HashMap<u16, f32>],
) -> Option<Mesh> {
    let Some(VertexAttributeValues::Float32x3(positions)) =
        stock.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return None;
    };
    let Some(VertexAttributeValues::Uint16x4(joints)) = stock.attribute(ATTRIBUTE_WOW_JOINT_INDEX)
    else {
        return None;
    };
    let Some(VertexAttributeValues::Float32x4(weights)) =
        stock.attribute(ATTRIBUTE_WOW_JOINT_WEIGHT)
    else {
        return None;
    };
    if joints.len() != positions.len() || weights.len() != positions.len() {
        return None;
    }
    let (mut joints, mut weights) = (joints.clone(), weights.clone());
    let mut any = false;
    for (v, p) in positions.iter().enumerate() {
        let p = Vec3::from(*p);
        // The part weighing this vertex most.
        let near = lobes.iter().enumerate().filter_map(|(i, lobe)| {
            // Only its outer side: nothing deeper into the body than a fifth of the radius past
            // the centre.
            let deepest = if lobe.rear { 0.0 } else { -0.2 };
            if (p - lobe.centre).dot(lobe.out) < deepest * lobe.radius {
                return None;
            }
            let mut d = p - lobe.centre;
            if lobe.rear {
                d.y /= REAR_REACH_VERTICAL;
            } else if d.y > 0.0 {
                d.y /= CHEST_REACH_UP;
            }
            let t = 1.0 - d.length() / (REACH * lobe.radius);
            let most = if lobe.rear { REAR_WEIGHT } else { 1.0 };
            (t > 0.0).then_some((i, t * t * (3.0 - 2.0 * t) * most))
        });
        let Some((lobe, w)) = near.max_by(|a, b| a.1.total_cmp(&b.1)) else {
            continue;
        };
        if w < 0.01 {
            continue;
        }
        any = true;
        let (js, ws) = (&mut joints[v], &mut weights[v]);
        for k in 0..4 {
            if ws[k] > 0.0 {
                *carried[lobe].entry(js[k]).or_default() += ws[k] * w;
            }
        }
        // The lightest slot takes the new bone; the rest share what is left.
        let slot = (0..4).min_by(|&a, &b| ws[a].total_cmp(&ws[b])).unwrap_or(3);
        let rest: f32 = (0..4).filter(|&k| k != slot).map(|k| ws[k]).sum();
        for (k, wk) in ws.iter_mut().enumerate() {
            if k != slot {
                *wk = if rest > 0.0 {
                    *wk / rest * (1.0 - w)
                } else {
                    0.0
                };
            }
        }
        js[slot] = (bones + lobe) as u16;
        ws[slot] = if rest > 0.0 { w } else { 1.0 };
    }
    if !any {
        return None;
    }
    let mut copy = stock.clone();
    copy.insert_attribute(
        ATTRIBUTE_WOW_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(joints),
    );
    copy.insert_attribute(
        ATTRIBUTE_WOW_JOINT_WEIGHT,
        VertexAttributeValues::Float32x4(weights),
    );
    Some(copy)
}

/// The scale for a size: the full step outward (along Z, front or back), less across and up and
/// down.
fn grow(size: f32, rear: bool) -> Vec3 {
    let (across, vertical) = if rear {
        (REAR_GROW_ACROSS, REAR_GROW_VERTICAL)
    } else {
        (CHEST_GROW_ACROSS, CHEST_GROW_VERTICAL)
    };
    let step = size - 1.0;
    Vec3::new(1.0 + step * across, 1.0 + step * vertical, size)
}

/// Writes each sway's slot: the unit's rows, then the spring bones, and keeps the copies on
/// the stock parts' material and tag.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn pose_sways(
    mut sways: ResMut<Sways>,
    rigs: Query<&RigSkin>,
    stock: Query<(&MeshMaterial3d<WowModelMaterial>, &MeshTag), Without<SwayCopy>>,
    mut copies: Query<(&mut MeshMaterial3d<WowModelMaterial>, &mut MeshTag), With<SwayCopy>>,
    mut palettes: ResMut<RigPalettes>,
    time: Res<Time>,
    cvars: Option<Res<crate::cvars::Cvars>>,
) {
    let amount = cvars
        .as_deref()
        .and_then(|c| c.num("bodyPhysics"))
        .unwrap_or(1.0)
        .clamp(0.0, MAX_AMOUNT);
    let size = cvars
        .as_deref()
        .and_then(|c| c.num("bodySize"))
        .unwrap_or(1.0)
        .clamp(MIN_SIZE, MAX_SIZE);
    let rear_size = cvars
        .as_deref()
        .and_then(|c| c.num("bodyRearSize"))
        .unwrap_or(1.0)
        .clamp(MIN_SIZE, MAX_SIZE);
    let dt = time.delta_secs();
    let sways = &mut *sways;
    for (rig, sway) in sways.live.iter_mut() {
        let (Ok(unit), Ok(own)) = (rigs.get(*rig), rigs.get(sway.holder)) else {
            continue;
        };
        let (Some(rows), Some(origin)) = (
            palettes.rig_rows(unit.slot, sway.bones),
            palettes.slot_origin(unit.slot),
        ) else {
            continue;
        };
        if rows.len() < sway.bones {
            continue;
        }
        let mut worlds: Vec<GlobalTransform> = rows
            .iter()
            .map(|m| GlobalTransform::from(Affine3A::from_mat4(*m)))
            .collect();
        for side in &mut sway.sides {
            let lobe = side.lobe;
            let (frequency, ratio, lag, stray, size) = if lobe.rear {
                (
                    REAR_FREQUENCY,
                    REAR_DAMPING,
                    REAR_LAG,
                    REAR_STRAY,
                    rear_size,
                )
            } else {
                (FREQUENCY, DAMPING, 1.0, 1.0, size)
            };
            let stiffness = (std::f32::consts::TAU * frequency).powi(2);
            let damping = 2.0 * ratio * stiffness.sqrt();
            let limit = MAX_OFFSET * lobe.radius * amount * stray;
            let carrier = rows.get(side.carrier).copied().unwrap_or(Mat4::IDENTITY);
            // The anchor in the world, kept apart as rig-relative + origin for precision.
            let rel = carrier.transform_point3(lobe.centre);
            let moved = side.last.map(|last| rel + origin - last);
            side.last = Some(rel + origin);
            match moved {
                Some(moved) if moved.length() < TELEPORT && dt > 0.0 => {
                    // The soft part stays where it was while the body moves under it.
                    let moved = Vec3::new(moved.x * HORIZONTAL, moved.y, moved.z * HORIZONTAL);
                    side.offset -= moved * amount * lag;
                    let steps = ((dt / STEP).ceil() as u32).clamp(1, MAX_STEPS);
                    let h = dt / steps as f32;
                    for _ in 0..steps {
                        let accel = -stiffness * side.offset - damping * side.velocity;
                        side.velocity += accel * h;
                        side.offset += side.velocity * h;
                    }
                    if side.offset.length() > limit {
                        side.offset = side.offset.normalize_or_zero() * limit;
                        side.velocity *= 0.5;
                    }
                }
                _ => {
                    side.offset = Vec3::ZERO;
                    side.velocity = Vec3::ZERO;
                }
            }
            // Never into the body: the part of the lag pressing back into it is dropped, so moving
            // forward does not flatten the chest.
            let out = carrier.transform_vector3(lobe.out).normalize_or_zero();
            let into = side.offset.dot(out).min(0.0);
            let shown = side.offset - out * into;
            // Sized about the centre at bind, then carried and swayed.
            let sized = Affine3A::from_translation(lobe.centre)
                * Affine3A::from_scale(grow(size, lobe.rear))
                * Affine3A::from_translation(-lobe.centre);
            let bone = Affine3A::from_translation(shown) * Affine3A::from_mat4(carrier) * sized;
            worlds.push(GlobalTransform::from(bone));
        }
        let ibp = vec![Mat4::IDENTITY; worlds.len()];
        palettes.write_rig_worlds(own, &worlds, &ibp, origin);
        for (part, copy, _) in &sway.copies {
            if let (Ok((mat, tag)), Ok((mut copy_mat, mut copy_tag))) =
                (stock.get(*part), copies.get_mut(*copy))
            {
                if copy_mat.0 != mat.0 {
                    copy_mat.0 = mat.0.clone();
                }
                let want = with_rig(tag.0, own.slot);
                if copy_tag.0 != want {
                    copy_tag.0 = want;
                }
            }
        }
    }
}

/// Keeps each swaying unit's copied stock parts hidden, after the model-visibility pass.
fn hide_stock_parts(sways: Res<Sways>, mut visibility: Query<&mut Visibility>) {
    for (part, _, _) in sways.live.values().flat_map(|s| &s.copies) {
        if let Ok(mut vis) = visibility.get_mut(*part) {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        }
    }
}
