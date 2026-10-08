//! Fork: cloth cloaks, behind the Physics page's Cloak Physics box (`cloakPhysics`).
//!
//! A 1.12 cloak is the body's own 15xx geosets, skinned to the spine, hips and thighs it shares
//! with the body, so no bone of its own can swing it. Instead the stock cloak part is hidden and a
//! copy of its mesh drawn in its place, every vertex moved on the CPU: the cloak as the animation
//! skins it is the target, its top rows ride the shoulders exactly, and the rest hang from them as
//! verlet cloth (gravity, edge lengths, a pull back toward the target that weakens down the
//! cloak), kept out of the legs and back by capsules on the bones the cloak rides. The copy is
//! drawn through its own one-bone palette slot, as a severed limb is, so it takes the stock
//! material and every fade and fog bit of the part's tag. A cloak drawn in several parts is one
//! cloth, so its parts never pull apart. Only the nearest few cloaks simulate.
//!
//! The cloak part is told apart by its texture, the cape BLP under `Item\ObjectComponents\Cape\`.

use bevy::asset::AssetId;
use bevy::camera::visibility::{NoFrustumCulling, VisibilitySystems};
use bevy::ecs::entity::EntityHashMap;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::mesh::{Indices, MeshTag, VertexAttributeValues};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_assets::{ATTRIBUTE_WOW_JOINT_INDEX, ATTRIBUTE_WOW_JOINT_WEIGHT};
use benilla_world::billboard::BillboardPlace;
use benilla_world::mesh_tag::{rig_of, with_rig};
use benilla_world::model_render::ModelPart;
use benilla_world::rig_anim::{finalize_rig_worlds, RigPose};
use benilla_world::rig_palette::{rebase_global, rebase_origin, RigPalettes, RigPart, RigSkin};
use benilla_world::view::WorldCamera;

/// At most this many cloaks simulate, the nearest to the camera within this range (yd).
const MAX_CLOTHS: usize = 10;
const MAX_RANGE: f32 = 60.0;
/// The simulation's step (s): a frame takes as many equal steps as fit this, at most this many.
const STEP: f32 = 1.0 / 60.0;
const MAX_STEPS: u32 = 4;
/// Constraint passes per step.
const ITERATIONS: usize = 4;
/// Gravity (yd/s²) and the velocity a particle keeps per step of [`STEP`]. The animated cloak
/// already hangs, so gravity only gives the cloth some weight.
const GRAVITY: f32 = 4.0;
const KEEP: f32 = 0.96;
/// The share of the body's own motion the cloth does not take at once: what trails behind.
const INERTIA: f32 = 0.35;
/// How hard the cloth keeps its curve across each pair of triangles (0..1).
const BEND: f32 = 0.5;
/// The top share of the cloak's height rides the shoulders exactly.
const PINNED: f32 = 0.15;
/// The pull back toward the animated cloak per step: this strong just under the pinned rows,
/// this weak at the hem.
const FOLLOW_TOP: f32 = 0.35;
const FOLLOW_HEM: f32 = 0.04;
/// No particle strays further from its animated spot than this share of the cloak's length.
const MAX_DRIFT: f32 = 0.6;
/// A target jump past this (yd) is a teleport: the cloth starts over on the target.
const TELEPORT: f32 = 5.0;
/// A capsule's radius is this share of the cloak's closest approach to its bone at rest, and
/// bones further than this share of the cloak's length from it get none.
const CAPSULE_FIT: f32 = 0.85;
const CAPSULE_REACH: f32 = 0.5;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Cloths>()
        .add_systems(Update, manage_cloths)
        .add_systems(
            PostUpdate,
            simulate_cloths
                .after(finalize_rig_worlds)
                .in_set(BillboardPlace),
        )
        .add_systems(
            PostUpdate,
            hide_stock_cloaks.before(VisibilitySystems::VisibilityPropagate),
        );
}

/// Every simulating cloak, by its rig.
#[derive(Resource, Default)]
struct Cloths {
    live: EntityHashMap<Cloth>,
    /// Rigs whose cloak could not be read; never retried.
    failed: HashSet<Entity>,
    /// Which materials are capes, by id.
    capes: HashMap<AssetId<WowModelMaterial>, bool>,
    /// The one-bone slot's identity bindpose, made once.
    ibp: Option<Handle<SkinnedMeshInverseBindposes>>,
}

/// One stock cloak part (a cloak can be drawn in several) and the copy drawn in its place.
struct Piece {
    part: Entity,
    copy: Entity,
    mesh: Handle<Mesh>,
    /// Each mesh vertex's particle and its smooth-shading group, and the triangles.
    vertex_particle: Vec<u32>,
    vertex_group: Vec<u32>,
    triangles: Vec<[u32; 3]>,
}

/// One cloak's cloth: its pieces and the particles they share.
struct Cloth {
    holder: Entity,
    pieces: Vec<Piece>,
    /// How many smooth-shading groups: the vertices of one particle facing one way.
    groups: usize,
    /// Per particle: its bind position and skin, where it is and was, where the animation had it
    /// last frame, how hard it follows the animated cloak, and whether it rides it exactly.
    bind: Vec<Vec3>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    last_target: Vec<Vec3>,
    follow: Vec<f32>,
    pinned: Vec<bool>,
    /// The triangles' edges, and the bends: the far corners of each two triangles sharing one.
    edges: Vec<(u32, u32)>,
    bends: Vec<(u32, u32)>,
    /// Bone segments the cloak may not enter, with their radius; fitted on the first step.
    capsules: Vec<(usize, usize, f32)>,
    length: f32,
    /// The last step's length, which the particles' velocity is measured in.
    last_step: f32,
    started: bool,
}

/// What [`manage_cloths`] reads of a candidate part.
type CapePart = (
    Entity,
    &'static RigPart,
    &'static MeshMaterial3d<WowModelMaterial>,
    Option<&'static Mesh3d>,
    &'static MeshTag,
    Option<&'static ModelPart>,
);

/// Picks the nearest cloaks to simulate, builds their cloth, and drops the rest.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn manage_cloths(
    mut commands: Commands,
    mut cloths: ResMut<Cloths>,
    parts: Query<CapePart>,
    rigs: Query<(&RigSkin, &GlobalTransform, Option<&Name>)>,
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
    let cloths = &mut *cloths;
    // A material whose texture changed is asked again.
    for ev in material_events.read() {
        if let AssetEvent::Modified { id } | AssetEvent::Removed { id } = ev {
            cloths.capes.remove(id);
        }
    }
    let on = cvars
        .as_deref()
        .and_then(|c| c.flag("cloakPhysics"))
        .unwrap_or(true);
    let eye = camera.single().ok().map(|g| g.translation());
    // The cape parts of each live rig near enough, when on.
    let mut by_rig: EntityHashMap<(f32, Vec<Entity>)> = EntityHashMap::default();
    if let (true, Some(eye)) = (on, eye) {
        for (part, rig_part, material, _, tag, _) in &parts {
            let Ok((skin, at, _)) = rigs.get(rig_part.0) else {
                continue;
            };
            if rig_of(tag.0) != skin.slot || cloths.failed.contains(&rig_part.0) {
                continue;
            }
            let cape = match cloths.capes.get(&material.id()) {
                Some(&cape) => cape,
                None => {
                    // Not loaded yet: asked again next frame.
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
                    cloths.capes.insert(material.id(), cape);
                    cape
                }
            };
            let d = at.translation().distance(eye);
            if cape && d <= MAX_RANGE {
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
    wanted.truncate(MAX_CLOTHS);

    // Drop the cloths no longer wanted, or whose rig or parts changed, and show their stock cloak.
    let gone: Vec<Entity> = cloths
        .live
        .iter()
        .filter(|(rig, c)| {
            !wanted.iter().any(|(_, r, parts)| {
                r == *rig
                    && parts.len() == c.pieces.len()
                    && parts
                        .iter()
                        .zip(&c.pieces)
                        .all(|(p, piece)| *p == piece.part)
            })
        })
        .map(|(rig, _)| *rig)
        .collect();
    for rig in gone {
        let Some(cloth) = cloths.live.remove(&rig) else {
            continue;
        };
        if let Ok(mut e) = commands.get_entity(cloth.holder) {
            e.despawn();
        }
        for piece in &cloth.pieces {
            meshes.remove(&piece.mesh);
            if let Ok(mut vis) = visibility.get_mut(piece.part) {
                *vis = Visibility::Inherited;
            }
        }
    }

    // Build the new ones, once every piece's mesh is in.
    let ibp = cloths
        .ibp
        .get_or_insert_with(|| ibps.add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY])))
        .clone();
    for (_, rig, part_ids) in wanted {
        if cloths.live.contains_key(&rig) {
            continue;
        }
        let mut stocks: Vec<&Mesh> = Vec::new();
        for &part in &part_ids {
            if let Some(mesh) = parts
                .get(part)
                .ok()
                .and_then(|p| p.3)
                .and_then(|m| meshes.get(&m.0))
            {
                stocks.push(mesh);
            }
        }
        if stocks.len() != part_ids.len() {
            continue;
        }
        let name = rigs
            .get(rig)
            .ok()
            .and_then(|r| r.2)
            .map_or_else(|| format!("{rig}"), |n| n.as_str().to_owned());
        let (mut cloth, copy_meshes) = match build_cloth(&stocks) {
            Ok(built) => built,
            Err(why) => {
                warn!("cloth cloak: {name} keeps its stock cloak: {why}");
                cloths.failed.insert(rig);
                continue;
            }
        };
        let Some(skin) = RigSkin::allocate_bones(&mut palettes, 1, ibp.clone()) else {
            continue;
        };
        info!(
            "cloth cloak: {name}: {} pieces, {} particles, {} pinned",
            cloth.pieces.len(),
            cloth.bind.len(),
            cloth.pinned.iter().filter(|&&p| p).count()
        );
        let slot = skin.slot;
        cloth.holder = commands
            .spawn((
                Name::new("cloth cloak"),
                Transform::default(),
                Visibility::default(),
                skin,
                ChildOf(rig),
            ))
            .id();
        for (piece, (part, copy_mesh)) in cloth
            .pieces
            .iter_mut()
            .zip(part_ids.iter().zip(copy_meshes))
        {
            let Ok((_, _, material, _, tag, model_part)) = parts.get(*part) else {
                continue;
            };
            piece.part = *part;
            piece.mesh = meshes.add(copy_mesh);
            let mut copy = commands.spawn((
                Mesh3d(piece.mesh.clone()),
                MeshMaterial3d(material.0.clone()),
                MeshTag(with_rig(tag.0, slot)),
                Transform::default(),
                Visibility::default(),
                NoFrustumCulling,
                ChildOf(cloth.holder),
            ));
            if let Some(model_part) = model_part {
                copy.insert(*model_part);
            }
            piece.copy = copy.id();
        }
        cloths.live.insert(rig, cloth);
    }
}

/// The cloth for a cloak's stock meshes, and each one's copy: the stock mesh with every vertex on
/// the copy's one bone. Vertices at one spot, in one mesh or across them, are one particle.
fn build_cloth(stocks: &[&Mesh]) -> Result<(Cloth, Vec<Mesh>), &'static str> {
    let mut by_spot: HashMap<[i32; 3], u32> = HashMap::new();
    let (mut bind, mut skin_joints, mut skin_weights) = (Vec::new(), Vec::new(), Vec::new());
    // Each particle's shading groups, by the way their vertices face.
    let mut particle_groups: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut group_facing: Vec<Vec3> = Vec::new();
    let mut pieces = Vec::new();
    let mut copies = Vec::new();
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut opposite: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for stock in stocks {
        let Some(VertexAttributeValues::Float32x3(positions)) =
            stock.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return Err("no positions");
        };
        let Some(VertexAttributeValues::Uint16x4(joints)) =
            stock.attribute(ATTRIBUTE_WOW_JOINT_INDEX)
        else {
            return Err("no bone indices");
        };
        let Some(VertexAttributeValues::Float32x4(weights)) =
            stock.attribute(ATTRIBUTE_WOW_JOINT_WEIGHT)
        else {
            return Err("no bone weights");
        };
        let normals = match stock.attribute(Mesh::ATTRIBUTE_NORMAL) {
            Some(VertexAttributeValues::Float32x3(n)) if n.len() == positions.len() => Some(n),
            _ => None,
        };
        let indices: Vec<u32> = match stock.indices() {
            Some(Indices::U16(i)) => i.iter().map(|&v| u32::from(v)).collect(),
            Some(Indices::U32(i)) => i.clone(),
            None => return Err("no indices"),
        };
        let n = positions.len();
        if n == 0 || joints.len() != n || weights.len() != n || indices.len() < 3 {
            return Err("an empty or mismatched mesh");
        }
        let mut vertex_particle = Vec::with_capacity(n);
        let mut vertex_group = Vec::with_capacity(n);
        for v in 0..n {
            let p = Vec3::from(positions[v]);
            let key = (p * 1000.0).round().as_ivec3().to_array();
            let id = *by_spot.entry(key).or_insert_with(|| {
                bind.push(p);
                skin_joints.push(joints[v]);
                skin_weights.push(weights[v]);
                (bind.len() - 1) as u32
            });
            vertex_particle.push(id);
            // A vertex shades with the particle's others facing its way (a UV seam), never with
            // the other side's; without normals, alone.
            let facing = normals.map_or(Vec3::ZERO, |n| Vec3::from(n[v]));
            let groups = particle_groups.entry(id).or_default();
            let group = groups
                .iter()
                .copied()
                .find(|&g| group_facing[g as usize].dot(facing) > 0.5)
                .unwrap_or_else(|| {
                    group_facing.push(facing);
                    groups.push((group_facing.len() - 1) as u32);
                    (group_facing.len() - 1) as u32
                });
            vertex_group.push(group);
        }
        let triangles: Vec<[u32; 3]> = indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| t.iter().all(|&i| (i as usize) < n))
            .copied()
            .collect();
        for t in &triangles {
            let [a, b, c] = t.map(|i| vertex_particle[i as usize]);
            if a == b || b == c || c == a {
                continue;
            }
            for (x, y, far) in [(a, b, c), (b, c, a), (c, a, b)] {
                let key = (x.min(y), x.max(y));
                edges.insert(key);
                let list = opposite.entry(key).or_default();
                if !list.contains(&far) {
                    list.push(far);
                }
            }
        }
        let mut copy = (*stock).clone();
        copy.insert_attribute(
            ATTRIBUTE_WOW_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(vec![[0; 4]; n]),
        );
        copy.insert_attribute(
            ATTRIBUTE_WOW_JOINT_WEIGHT,
            VertexAttributeValues::Float32x4(vec![[1.0, 0.0, 0.0, 0.0]; n]),
        );
        copies.push(copy);
        pieces.push(Piece {
            part: Entity::PLACEHOLDER,
            copy: Entity::PLACEHOLDER,
            mesh: Handle::default(),
            vertex_particle,
            vertex_group,
            triangles,
        });
    }
    if bind.is_empty() || edges.is_empty() {
        return Err("no triangles");
    }
    let mut bends: HashSet<(u32, u32)> = HashSet::new();
    for far in opposite.values() {
        for (i, &x) in far.iter().enumerate() {
            for &y in &far[i + 1..] {
                let key = (x.min(y), x.max(y));
                if x != y && !edges.contains(&key) {
                    bends.insert(key);
                }
            }
        }
    }
    // Down the cloak from its top: the pinned rows, then a follow that weakens to the hem.
    let top = bind.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let bottom = bind.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let span = (top - bottom).max(1e-3);
    let depth: Vec<f32> = bind.iter().map(|p| (top - p.y) / span).collect();
    let pinned: Vec<bool> = depth.iter().map(|&d| d <= PINNED).collect();
    let follow: Vec<f32> = depth
        .iter()
        .map(|&d| {
            let t = ((d - PINNED) / (1.0 - PINNED)).clamp(0.0, 1.0);
            FOLLOW_TOP + (FOLLOW_HEM - FOLLOW_TOP) * t
        })
        .collect();
    let count = bind.len();
    Ok((
        Cloth {
            holder: Entity::PLACEHOLDER,
            pieces,
            groups: group_facing.len(),
            bind,
            joints: skin_joints,
            weights: skin_weights,
            pos: vec![Vec3::ZERO; count],
            prev: vec![Vec3::ZERO; count],
            last_target: vec![Vec3::ZERO; count],
            follow,
            pinned,
            edges: edges.into_iter().collect(),
            bends: bends.into_iter().collect(),
            capsules: Vec::new(),
            length: 0.0,
            last_step: STEP,
            started: false,
        },
        copies,
    ))
}

/// Where the animation skins each particle this frame, in the world.
fn targets(cloth: &Cloth, palette: &[Mat4]) -> Vec<Vec3> {
    (0..cloth.bind.len())
        .map(|i| {
            let (js, ws) = (cloth.joints[i], cloth.weights[i]);
            let mut p = Vec3::ZERO;
            let mut total = 0.0;
            for k in 0..4 {
                if ws[k] > 0.0 {
                    if let Some(m) = palette.get(js[k] as usize) {
                        p += m.transform_point3(cloth.bind[i]) * ws[k];
                        total += ws[k];
                    }
                }
            }
            if total > 0.0 {
                p / total
            } else {
                cloth.bind[i]
            }
        })
        .collect()
}

/// The closest point to `p` on the segment `a`..`b`.
fn closest_on(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-8)).clamp(0.0, 1.0);
    a + ab * t
}

/// Fits the capsules: each bone the cloak rides, and each child of one, to its children, sized
/// to just under the cloak's closest approach at rest.
fn fit_capsules(cloth: &mut Cloth, rig: &RigPose, joint_at: &[Vec3], target: &[Vec3]) {
    let mut rides: HashSet<usize> = HashSet::new();
    for (js, ws) in cloth.joints.iter().zip(&cloth.weights) {
        for k in 0..4 {
            if ws[k] > 0.0 {
                rides.insert(js[k] as usize);
            }
        }
    }
    let child_of = |b: usize| {
        (0..rig.parents.len()).filter(move |&c| usize::try_from(rig.parents[c]).ok() == Some(b))
    };
    let mut bones: HashSet<usize> = rides.clone();
    for &b in &rides {
        bones.extend(child_of(b));
    }
    cloth.capsules.clear();
    for &b in &bones {
        for c in child_of(b) {
            let (Some(&a), Some(&e)) = (joint_at.get(b), joint_at.get(c)) else {
                continue;
            };
            let near = target
                .iter()
                .map(|&p| p.distance(closest_on(a, e, p)))
                .fold(f32::MAX, f32::min);
            if near < CAPSULE_REACH * cloth.length {
                cloth.capsules.push((b, c, near * CAPSULE_FIT));
            }
        }
    }
}

/// Moves `a` and `b` toward `rest` apart, by `stiffness` of the error; a pinned end stays.
fn constrain(pos: &mut [Vec3], pinned: &[bool], a: usize, b: usize, rest: f32, stiffness: f32) {
    let d = pos[b] - pos[a];
    let len = d.length();
    if len < 1e-6 {
        return;
    }
    let fix = d * ((len - rest) / len * stiffness);
    match (pinned[a], pinned[b]) {
        (true, true) => {}
        (true, false) => pos[b] -= fix,
        (false, true) => pos[a] += fix,
        (false, false) => {
            pos[a] += fix * 0.5;
            pos[b] -= fix * 0.5;
        }
    }
}

/// Steps every cloak's cloth and writes its copies: the vertices, the normals, its slot's row, and
/// the stock parts' material and tag.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
#[allow(clippy::needless_range_loop)]
fn simulate_cloths(
    mut cloths: ResMut<Cloths>,
    rigs: Query<(&RigSkin, &RigPose)>,
    skins: Query<&RigSkin>,
    globals: Query<&GlobalTransform>,
    stock: Query<(&MeshMaterial3d<WowModelMaterial>, &MeshTag), Without<ClothCopy>>,
    mut copies: Query<(&mut MeshMaterial3d<WowModelMaterial>, &mut MeshTag), With<ClothCopy>>,
    mut meshes: ResMut<Assets<Mesh>>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<RigPalettes>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let dt = time.delta_secs().min(STEP * MAX_STEPS as f32);
    let Some(ibp) = cloths
        .ibp
        .as_ref()
        .and_then(|h| ibps.get(h))
        .map(|i| i.to_vec())
    else {
        return;
    };
    for (rig_entity, cloth) in cloths.live.iter_mut() {
        let Ok((skin, rig)) = rigs.get(*rig_entity) else {
            continue;
        };
        let Some(palette) = palettes.world_palette(skin.slot, skin.bones() as usize) else {
            continue;
        };
        let Ok(root) = globals.get(rig.joints_root) else {
            continue;
        };
        let target = targets(cloth, &palette);
        let anchor = root.translation();
        let joint_at: Vec<Vec3> = rig
            .model
            .iter()
            .map(|m| root.transform_point(Vec3::from(m.translation)))
            .collect();
        let jumped = cloth.started
            && target
                .first()
                .zip(cloth.last_target.first())
                .is_some_and(|(t, l)| t.distance(*l) > TELEPORT);
        if !cloth.started || jumped {
            let top = target.iter().map(|p| p.y).fold(f32::MIN, f32::max);
            let bottom = target.iter().map(|p| p.y).fold(f32::MAX, f32::min);
            cloth.length = (top - bottom).max(0.05);
            fit_capsules(cloth, rig, &joint_at, &target);
            cloth.pos.clone_from(&target);
            cloth.prev.clone_from(&target);
            cloth.last_target.clone_from(&target);
            cloth.last_step = STEP;
            if !cloth.started {
                for piece in &cloth.pieces {
                    commands.entity(piece.copy).insert(ClothCopy);
                }
            }
            cloth.started = true;
        } else {
            // The cloth takes most of the body's own motion at once, so only the rest trails.
            for i in 0..cloth.pos.len() {
                let carry = (target[i] - cloth.last_target[i]) * (1.0 - INERTIA);
                cloth.pos[i] += carry;
                cloth.prev[i] += carry;
            }
        }
        if dt > 0.0 {
            // Equal steps every frame, so the cloth never stutters between one step and two.
            let steps = ((dt / STEP).ceil() as u32).clamp(1, MAX_STEPS);
            let h = dt / steps as f32;
            let keep = KEEP.powf(h / STEP);
            let fall = Vec3::NEG_Y * GRAVITY * h * h;
            let drift = MAX_DRIFT * cloth.length;
            for _ in 0..steps {
                let scale = h / cloth.last_step;
                cloth.last_step = h;
                for i in 0..cloth.pos.len() {
                    if cloth.pinned[i] {
                        cloth.prev[i] = target[i];
                        cloth.pos[i] = target[i];
                    } else {
                        let v = (cloth.pos[i] - cloth.prev[i]) * (keep * scale);
                        cloth.prev[i] = cloth.pos[i];
                        cloth.pos[i] += v + fall;
                    }
                }
                for _ in 0..ITERATIONS {
                    for &(a, b) in &cloth.edges {
                        let (a, b) = (a as usize, b as usize);
                        let rest = target[a].distance(target[b]);
                        constrain(&mut cloth.pos, &cloth.pinned, a, b, rest, 1.0);
                    }
                    for &(a, b) in &cloth.bends {
                        let (a, b) = (a as usize, b as usize);
                        let rest = target[a].distance(target[b]);
                        constrain(&mut cloth.pos, &cloth.pinned, a, b, rest, BEND);
                    }
                }
                for i in 0..cloth.pos.len() {
                    if cloth.pinned[i] {
                        continue;
                    }
                    let mut p = cloth.pos[i];
                    p += (target[i] - p) * (cloth.follow[i] * h / STEP).min(1.0);
                    for &(b, c, r) in &cloth.capsules {
                        let (Some(&a), Some(&e)) = (joint_at.get(b), joint_at.get(c)) else {
                            continue;
                        };
                        let q = closest_on(a, e, p);
                        let away = p - q;
                        let d = away.length();
                        if d < r && d > 1e-6 {
                            p = q + away * (r / d);
                        }
                    }
                    // Held at the leash without a bounce back: the clamp moves `prev` with it.
                    let off = p - target[i];
                    if off.length() > drift {
                        let held = target[i] + off.normalize() * drift;
                        cloth.prev[i] += held - p;
                        p = held;
                    }
                    cloth.pos[i] = p;
                }
            }
        }
        cloth.last_target = target;
        // Smooth normals: each shading group sums the triangles around its vertices.
        let mut group_normal = vec![Vec3::ZERO; cloth.groups];
        for piece in &cloth.pieces {
            for t in &piece.triangles {
                let [a, b, c] = t.map(|i| cloth.pos[piece.vertex_particle[i as usize] as usize]);
                let n = (b - a).cross(c - a);
                for &i in t {
                    group_normal[piece.vertex_group[i as usize] as usize] += n;
                }
            }
        }
        for piece in &cloth.pieces {
            let Some(mesh) = meshes.get_mut(&piece.mesh) else {
                continue;
            };
            let local: Vec<[f32; 3]> = piece
                .vertex_particle
                .iter()
                .map(|&p| (cloth.pos[p as usize] - anchor).to_array())
                .collect();
            let normals: Vec<[f32; 3]> = piece
                .vertex_group
                .iter()
                .map(|&g| {
                    group_normal[g as usize]
                        .try_normalize()
                        .unwrap_or(Vec3::Y)
                        .to_array()
                })
                .collect();
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, local);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        }
        if let Ok(own) = skins.get(cloth.holder) {
            let origin = rebase_origin(anchor);
            let frame = rebase_global(GlobalTransform::from_translation(anchor), origin);
            palettes.write_rig_worlds(own, &[frame], &ibp, origin);
            // The stock parts' material and tag (their fade and fog), on the copy's slot.
            for piece in &cloth.pieces {
                if let (Ok((mat, tag)), Ok((mut copy_mat, mut copy_tag))) =
                    (stock.get(piece.part), copies.get_mut(piece.copy))
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
}

/// On a cloak's copy, so the stock and copy queries stay apart.
#[derive(Component)]
struct ClothCopy;

/// Keeps each simulated cloak's stock part hidden, after the model-visibility pass shows it.
fn hide_stock_cloaks(cloths: Res<Cloths>, mut visibility: Query<&mut Visibility>) {
    for piece in cloths.live.values().flat_map(|c| &c.pieces) {
        if let Ok(mut vis) = visibility.get_mut(piece.part) {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        }
    }
}
