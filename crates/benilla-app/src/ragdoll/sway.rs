//! Fork: soft body sway on female units, behind the Physics page's Body Physics slider
//! (`bodyPhysics`, 0 off).
//!
//! The chest has no bone of its own, so each near female unit's skinned parts that carry it are
//! drawn as copies on a second palette slot with two bones more than the unit's: every row the
//! unit's own, plus one per side that is the bone carrying that side, moved by a damped spring
//! that lags the body's motion. The copies' chest vertices are re-weighted onto those two bones,
//! fading out toward the edge. The chest is found from the bind pose: the highest arm root (a
//! bone off the centre with a long chain under it) sets the shoulder height, and the most forward
//! vertex each side in the band under it is a tip. A model where that finds no matching pair is
//! left alone, and the log says why.

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
/// A jump of the anchor past this (yd) in one frame is a teleport: the spring starts over.
const TELEPORT: f32 = 2.0;

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
struct Sways {
    live: EntityHashMap<Sway>,
    /// Rigs whose chest could not be found; never retried.
    failed: HashSet<Entity>,
    /// Which materials are capes, by id: the cloth cloaks draw those.
    capes: HashMap<AssetId<WowModelMaterial>, bool>,
    /// Identity bindposes, by bone count.
    ibps: HashMap<u32, Handle<SkinnedMeshInverseBindposes>>,
}

/// One swaying unit: its candidate parts (to notice a re-dress), the copies, and the springs.
struct Sway {
    holder: Entity,
    parts: Vec<Entity>,
    copies: Vec<(Entity, Entity, Handle<Mesh>)>,
    bones: usize,
    sides: [Side; 2],
    radius: f32,
}

/// One side's spring: the bone carrying it, its centre at bind, and the lag.
struct Side {
    carrier: usize,
    centre: Vec3,
    last: Option<Vec3>,
    offset: Vec3,
    velocity: Vec3,
}

/// On a sway copy, so the stock and copy queries stay apart.
#[derive(Component)]
struct SwayCopy;

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

/// The bind-pose chest a rig's parts show: per side the tip, and the radius.
struct Chest {
    tips: [Vec3; 2],
    radius: f32,
}

/// Picks the nearest female units to sway, builds their copies, and drops the rest.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn manage_sways(
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
    let on = cvars
        .as_deref()
        .and_then(|c| c.num("bodyPhysics"))
        .unwrap_or(1.0)
        > 0.0;
    let eye = camera.single().ok().map(|g| g.translation());
    // Each near female rig's body parts (not its cloak), when on.
    let mut by_rig: EntityHashMap<(f32, Vec<Entity>)> = EntityHashMap::default();
    if let (true, Some(eye)) = (on, eye) {
        for (part, rig_part, material, _, _, tag, _) in &parts {
            let Ok((skin, _, at, store, _)) = rigs.get(rig_part.0) else {
                continue;
            };
            if rig_of(tag.0) != skin.slot
                || sways.failed.contains(&rig_part.0)
                || store.0.unit_gender() != Some(FEMALE)
            {
                continue;
            }
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
                            p.to_string()
                                .to_ascii_lowercase()
                                .contains("objectcomponents/cape")
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
                    .push(part);
            }
        }
    }
    let mut wanted: Vec<(f32, Entity, Vec<Entity>)> = by_rig
        .into_iter()
        .map(|(rig, (d, mut parts))| {
            parts.sort();
            (d, rig, parts)
        })
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
        for &part in &part_ids {
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
        let chest = {
            let positions: Vec<&[[f32; 3]]> = stocks
                .iter()
                .filter_map(
                    |(_, h)| match meshes.get(h)?.attribute(Mesh::ATTRIBUTE_POSITION) {
                        Some(VertexAttributeValues::Float32x3(p)) => Some(p.as_slice()),
                        _ => None,
                    },
                )
                .collect();
            find_chest(&positions, &pivots, &pose.parents)
        };
        let chest = match chest {
            Ok(chest) => chest,
            Err(why) => {
                info!("body physics: {name} left as is: {why}");
                sways.failed.insert(rig);
                continue;
            }
        };
        let bones = skin.bones() as usize;
        let radius = chest.radius;
        let centres = chest.tips.map(|t| t + Vec3::Z * (CENTRE_DEPTH * radius));
        // The copies: each part with chest vertices, re-weighted onto the two new bones.
        let mut built: Vec<(Entity, Mesh)> = Vec::new();
        let mut carried = [HashMap::<u16, f32>::new(), HashMap::<u16, f32>::new()];
        for (part, handle) in &stocks {
            let Some(stock) = meshes.get(handle) else {
                continue;
            };
            if let Some(copy) = reweight(stock, &centres, radius, bones, &mut carried) {
                built.push((*part, copy));
            }
        }
        if built.is_empty() {
            info!("body physics: {name} left as is: no vertices near the chest");
            sways.failed.insert(rig);
            continue;
        }
        let carrier = |side: &HashMap<u16, f32>| {
            side.iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map_or(0, |(&bone, _)| bone as usize)
        };
        let sides = [0, 1].map(|s| Side {
            carrier: carrier(&carried[s]),
            centre: centres[s],
            last: None,
            offset: Vec3::ZERO,
            velocity: Vec3::ZERO,
        });
        let total = bones as u32 + 2;
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
            "body physics: {name}: {} parts, radius {:.3}, tips {:?}, carriers {} {}",
            built.len(),
            radius,
            chest.tips,
            sides[0].carrier,
            sides[1].carrier
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
                radius,
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

/// The chest at bind: the shoulder height from the highest arm root, then each side's most
/// forward vertex (Bevy's front is −Z) in the band under it, between the centre and the arm.
fn find_chest(
    positions: &[&[[f32; 3]]],
    pivots: &[Vec3],
    parents: &[i16],
) -> Result<Chest, &'static str> {
    let all = || {
        positions
            .iter()
            .flat_map(|p| p.iter())
            .map(|&v| Vec3::from(v))
    };
    let top = all().map(|v| v.y).fold(f32::MIN, f32::max);
    let bottom = all().map(|v| v.y).fold(f32::MAX, f32::min);
    let height = top - bottom;
    if !height.is_finite() || height <= 0.0 {
        return Err("no vertices");
    }
    // Bones under each bone.
    let mut under = vec![0usize; pivots.len()];
    for i in (0..pivots.len()).rev() {
        if let Some(p) = parents
            .get(i)
            .and_then(|&p| usize::try_from(p).ok())
            .filter(|&p| p < i)
        {
            under[p] += under[i] + 1;
        }
    }
    let shoulder = (0..pivots.len())
        .filter(|&i| pivots[i].x.abs() > ARM_OFF_CENTRE * height && under[i] >= ARM_CHAIN)
        .max_by(|&a, &b| pivots[a].y.total_cmp(&pivots[b].y))
        .ok_or("no arm found")?;
    let (shoulder_y, shoulder_x) = (pivots[shoulder].y, pivots[shoulder].x.abs());
    let band = (shoulder_y - BAND * height)..=shoulder_y;
    let tip = |sign: f32| {
        all()
            .filter(|v| band.contains(&v.y))
            .filter(|v| {
                let x = v.x * sign;
                x >= INNER * shoulder_x && x <= OUTER * shoulder_x
            })
            .min_by(|a, b| a.z.total_cmp(&b.z))
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
    // The tips must stand out from the band's middle depth.
    let mut depths: Vec<f32> = all().filter(|v| band.contains(&v.y)).map(|v| v.z).collect();
    depths.sort_by(f32::total_cmp);
    let middle = depths[depths.len() / 2];
    if left.z.max(right.z) > middle - 0.3 * radius {
        return Err("the tips do not stand out");
    }
    Ok(Chest {
        tips: [left, right],
        radius,
    })
}

/// The stock mesh with its chest vertices re-weighted onto bone `bones` (the +X side) and
/// `bones + 1`, the weight fading from the centre; each side's old bones are summed into
/// `carried` by that weight. `None` when no vertex is near.
fn reweight(
    stock: &Mesh,
    centres: &[Vec3; 2],
    radius: f32,
    bones: usize,
    carried: &mut [HashMap<u16, f32>; 2],
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
    let reach = REACH * radius;
    let mut any = false;
    for (v, p) in positions.iter().enumerate() {
        let p = Vec3::from(*p);
        let side = if p.x >= 0.0 { 0 } else { 1 };
        let centre = centres[side];
        // Only the front: nothing behind the centre's depth past a fifth of the radius.
        if p.z > centre.z + 0.2 * radius {
            continue;
        }
        let t = 1.0 - p.distance(centre) / reach;
        if t <= 0.0 {
            continue;
        }
        let w = t * t * (3.0 - 2.0 * t);
        if w < 0.01 {
            continue;
        }
        any = true;
        let (js, ws) = (&mut joints[v], &mut weights[v]);
        for k in 0..4 {
            if ws[k] > 0.0 {
                *carried[side].entry(js[k]).or_default() += ws[k] * w;
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
        js[slot] = (bones + side) as u16;
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

/// Writes each sway's slot: the unit's rows, then the two spring bones, and keeps the copies on
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
    let dt = time.delta_secs();
    let stiffness = (std::f32::consts::TAU * FREQUENCY).powi(2);
    let damping = 2.0 * DAMPING * stiffness.sqrt();
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
        let limit = MAX_OFFSET * sway.radius * amount;
        let mut worlds: Vec<GlobalTransform> = rows
            .iter()
            .map(|m| GlobalTransform::from(Affine3A::from_mat4(*m)))
            .collect();
        for side in &mut sway.sides {
            let carrier = rows.get(side.carrier).copied().unwrap_or(Mat4::IDENTITY);
            // The anchor in the world, kept apart as rig-relative + origin for precision.
            let rel = carrier.transform_point3(side.centre);
            let moved = side.last.map(|last| rel + origin - last);
            side.last = Some(rel + origin);
            match moved {
                Some(moved) if moved.length() < TELEPORT && dt > 0.0 => {
                    // The soft part stays where it was while the body moves under it.
                    let moved = Vec3::new(moved.x * HORIZONTAL, moved.y, moved.z * HORIZONTAL);
                    side.offset -= moved * amount;
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
            // Never into the body: the part of the lag pressing back against the chest is dropped,
            // so moving forward does not flatten it.
            let forward = carrier.transform_vector3(Vec3::NEG_Z).normalize_or_zero();
            let into = side.offset.dot(forward).min(0.0);
            let shown = side.offset - forward * into;
            let bone = Affine3A::from_translation(shown) * Affine3A::from_mat4(carrier);
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
