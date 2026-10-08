//! Fork: cloth cloaks, behind the Physics page's Cloak Physics box (`cloakPhysics`).
//!
//! A 1.12 cloak is the body's own 15xx geosets, skinned to the spine, hips and thighs it shares
//! with the body, so no bone of its own can swing it. Instead the stock cloak part is hidden and a
//! copy of its mesh drawn in its place, every vertex moved on the CPU: the cloak as the animation
//! skins it is the target, its top rows ride the shoulders exactly, and the rest hang from them as
//! verlet cloth (gravity, edge lengths, a pull back toward the target that weakens down the
//! cloak), kept out of the legs and back by capsules on the bones the cloak rides. The copy is
//! drawn through its own one-bone palette slot, as a severed limb is, so it takes the stock
//! material and every fade and fog bit of the part's tag. Only the nearest few cloaks simulate.
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
/// The fixed simulation step (s), at most this many a frame.
const STEP: f32 = 1.0 / 60.0;
const MAX_STEPS: u32 = 4;
/// Constraint passes per step.
const ITERATIONS: usize = 4;
/// Gravity (yd/s²) and the velocity a particle keeps per step.
const GRAVITY: f32 = 9.0;
const KEEP: f32 = 0.97;
/// The top share of the cloak's height rides the shoulders exactly.
const PINNED: f32 = 0.15;
/// The pull back toward the animated cloak per step: this strong just under the pinned rows,
/// this weak at the hem.
const FOLLOW_TOP: f32 = 0.35;
const FOLLOW_HEM: f32 = 0.02;
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

/// Every simulating cloak, by its stock part.
#[derive(Resource, Default)]
struct Cloths {
    live: EntityHashMap<Cloth>,
    /// Parts whose mesh could not be read; never retried.
    failed: HashSet<Entity>,
    /// Which materials are capes, by id.
    capes: HashMap<AssetId<WowModelMaterial>, bool>,
    /// The one-bone slot's identity bindpose, made once.
    ibp: Option<Handle<SkinnedMeshInverseBindposes>>,
}

/// One cloak's cloth: its rig, the copy drawn in its place, and the particles.
struct Cloth {
    rig: Entity,
    holder: Entity,
    copy: Entity,
    mesh: Handle<Mesh>,
    /// Each mesh vertex's particle (vertices at one spot are one particle) and the triangles.
    vertex_particle: Vec<u32>,
    triangles: Vec<[u32; 3]>,
    /// Per particle: its bind position and skin, where it is and was, how hard it follows the
    /// animated cloak, and whether it rides it exactly.
    bind: Vec<Vec3>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    follow: Vec<f32>,
    pinned: Vec<bool>,
    edges: Vec<(u32, u32)>,
    /// Bone segments the cloak may not enter, with their radius; fitted on the first step.
    capsules: Vec<(usize, usize, f32)>,
    length: f32,
    acc: f32,
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
    rigs: Query<(&RigSkin, &GlobalTransform)>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    materials: Res<Assets<WowModelMaterial>>,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut ibps: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<RigPalettes>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    mut visibility: Query<&mut Visibility>,
) {
    let on = cvars
        .as_deref()
        .and_then(|c| c.flag("cloakPhysics"))
        .unwrap_or(true);
    let eye = camera.single().ok().map(|g| g.translation());
    let cloths = &mut *cloths;
    // The nearest cape parts on a live rig, when on.
    let mut wanted: Vec<(f32, Entity)> = Vec::new();
    if let (true, Some(eye)) = (on, eye) {
        for (part, rig_part, material, _, tag, _) in &parts {
            let Ok((skin, at)) = rigs.get(rig_part.0) else {
                continue;
            };
            if rig_of(tag.0) != skin.slot || cloths.failed.contains(&part) {
                continue;
            }
            let cape = *cloths.capes.entry(material.id()).or_insert_with(|| {
                materials
                    .get(material.id())
                    .and_then(|m| m.base.base_color_texture.as_ref())
                    .and_then(|t| server.get_path(t.id()))
                    .is_some_and(|p| {
                        p.to_string()
                            .to_ascii_lowercase()
                            .contains("objectcomponents/cape")
                    })
            });
            let d = at.translation().distance(eye);
            if cape && d <= MAX_RANGE {
                wanted.push((d, part));
            }
        }
    }
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    wanted.truncate(MAX_CLOTHS);
    let keep: HashSet<Entity> = wanted.iter().map(|&(_, p)| p).collect();

    // Drop the cloths no longer wanted, or whose part or rig went, and show their stock cloak.
    let gone: Vec<Entity> = cloths
        .live
        .iter()
        .filter(|(part, c)| !keep.contains(*part) || !rigs.contains(c.rig))
        .map(|(part, _)| *part)
        .collect();
    for part in gone {
        let Some(cloth) = cloths.live.remove(&part) else {
            continue;
        };
        if let Ok(mut e) = commands.get_entity(cloth.holder) {
            e.despawn();
        }
        meshes.remove(&cloth.mesh);
        if let Ok(mut vis) = visibility.get_mut(part) {
            *vis = Visibility::Inherited;
        }
    }

    // Build the new ones.
    let ibp = cloths
        .ibp
        .get_or_insert_with(|| ibps.add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY])))
        .clone();
    for (_, part) in wanted {
        if cloths.live.contains_key(&part) {
            continue;
        }
        let Ok((_, rig_part, material, Some(mesh), tag, model_part)) = parts.get(part) else {
            continue;
        };
        let Some(stock) = meshes.get(&mesh.0) else {
            continue;
        };
        let Some((mut cloth, copy_mesh)) = build_cloth(stock) else {
            cloths.failed.insert(part);
            continue;
        };
        let Some(skin) = RigSkin::allocate_bones(&mut palettes, 1, ibp.clone()) else {
            continue;
        };
        let slot = skin.slot;
        cloth.mesh = meshes.add(copy_mesh);
        cloth.rig = rig_part.0;
        cloth.holder = commands
            .spawn((
                Name::new("cloth cloak"),
                Transform::default(),
                Visibility::default(),
                skin,
                ChildOf(rig_part.0),
            ))
            .id();
        let mut copy = commands.spawn((
            Mesh3d(cloth.mesh.clone()),
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
        cloth.copy = copy.id();
        cloths.live.insert(part, cloth);
    }
}

/// The cloth for a stock cloak mesh, and the copy's mesh: the stock one with every vertex on the
/// copy's one bone. `None` when the mesh carries no skin.
fn build_cloth(stock: &Mesh) -> Option<(Cloth, Mesh)> {
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
    let indices: Vec<u32> = match stock.indices()? {
        Indices::U16(i) => i.iter().map(|&v| u32::from(v)).collect(),
        Indices::U32(i) => i.clone(),
    };
    let n = positions.len();
    if n == 0 || joints.len() != n || weights.len() != n || indices.len() < 3 {
        return None;
    }
    // Weld the vertices at one spot (a UV seam, the two sides) into one particle.
    let mut by_spot: HashMap<[i32; 3], u32> = HashMap::new();
    let mut vertex_particle = Vec::with_capacity(n);
    let (mut bind, mut skin_joints, mut skin_weights) = (Vec::new(), Vec::new(), Vec::new());
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
    }
    let triangles: Vec<[u32; 3]> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| t.iter().all(|&i| (i as usize) < n))
        .copied()
        .collect();
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    for t in &triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (a, b) = (vertex_particle[a as usize], vertex_particle[b as usize]);
            if a != b {
                edges.insert((a.min(b), a.max(b)));
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
    let mut copy = stock.clone();
    copy.insert_attribute(
        ATTRIBUTE_WOW_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(vec![[0; 4]; n]),
    );
    copy.insert_attribute(
        ATTRIBUTE_WOW_JOINT_WEIGHT,
        VertexAttributeValues::Float32x4(vec![[1.0, 0.0, 0.0, 0.0]; n]),
    );
    let count = bind.len();
    Some((
        Cloth {
            rig: Entity::PLACEHOLDER,
            holder: Entity::PLACEHOLDER,
            copy: Entity::PLACEHOLDER,
            mesh: Handle::default(),
            vertex_particle,
            triangles,
            bind,
            joints: skin_joints,
            weights: skin_weights,
            pos: vec![Vec3::ZERO; count],
            prev: vec![Vec3::ZERO; count],
            follow,
            pinned,
            edges: edges.into_iter().collect(),
            capsules: Vec::new(),
            length: 0.0,
            acc: 0.0,
            started: false,
        },
        copy,
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

/// Steps every cloak's cloth and writes its copy: the vertices, the normals, its slot's row, and
/// the stock part's material and tag.
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
    let dt = time.delta_secs();
    let Some(ibp) = cloths
        .ibp
        .as_ref()
        .and_then(|h| ibps.get(h))
        .map(|i| i.to_vec())
    else {
        return;
    };
    for (part, cloth) in cloths.live.iter_mut() {
        let Ok((skin, rig)) = rigs.get(cloth.rig) else {
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
        let jumped = cloth.started
            && target
                .first()
                .zip(cloth.pos.first())
                .is_some_and(|(t, p)| t.distance(*p) > TELEPORT);
        if !cloth.started || jumped {
            let top = target.iter().map(|p| p.y).fold(f32::MIN, f32::max);
            let bottom = target.iter().map(|p| p.y).fold(f32::MAX, f32::min);
            cloth.length = (top - bottom).max(0.05);
            let joint_at: Vec<Vec3> = rig
                .model
                .iter()
                .map(|m| root.transform_point(Vec3::from(m.translation)))
                .collect();
            fit_capsules(cloth, rig, &joint_at, &target);
            cloth.pos.clone_from(&target);
            cloth.prev.clone_from(&target);
            cloth.acc = 0.0;
            cloth.started = true;
            commands.entity(cloth.copy).insert(ClothCopy);
        }
        let joint_at: Vec<Vec3> = rig
            .model
            .iter()
            .map(|m| root.transform_point(Vec3::from(m.translation)))
            .collect();
        cloth.acc = (cloth.acc + dt).min(STEP * MAX_STEPS as f32);
        let drift = MAX_DRIFT * cloth.length;
        while cloth.acc >= STEP {
            cloth.acc -= STEP;
            let fall = Vec3::NEG_Y * GRAVITY * STEP * STEP;
            for i in 0..cloth.pos.len() {
                if cloth.pinned[i] {
                    cloth.prev[i] = target[i];
                    cloth.pos[i] = target[i];
                } else {
                    let v = (cloth.pos[i] - cloth.prev[i]) * KEEP;
                    cloth.prev[i] = cloth.pos[i];
                    cloth.pos[i] += v + fall;
                }
            }
            for _ in 0..ITERATIONS {
                for &(a, b) in &cloth.edges {
                    let (a, b) = (a as usize, b as usize);
                    let rest = target[a].distance(target[b]);
                    let d = cloth.pos[b] - cloth.pos[a];
                    let len = d.length();
                    if len < 1e-6 {
                        continue;
                    }
                    let fix = d * ((len - rest) / len);
                    match (cloth.pinned[a], cloth.pinned[b]) {
                        (true, true) => {}
                        (true, false) => cloth.pos[b] -= fix,
                        (false, true) => cloth.pos[a] += fix,
                        (false, false) => {
                            cloth.pos[a] += fix * 0.5;
                            cloth.pos[b] -= fix * 0.5;
                        }
                    }
                }
            }
            for i in 0..cloth.pos.len() {
                if cloth.pinned[i] {
                    continue;
                }
                let mut p = cloth.pos[i];
                p += (target[i] - p) * cloth.follow[i];
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
                let off = p - target[i];
                if off.length() > drift {
                    p = target[i] + off.normalize() * drift;
                }
                cloth.pos[i] = p;
            }
        }
        // The copy's vertices, relative to the rig's frame, and their normals by winding.
        if let Some(mesh) = meshes.get_mut(&cloth.mesh) {
            let local: Vec<[f32; 3]> = cloth
                .vertex_particle
                .iter()
                .map(|&p| (cloth.pos[p as usize] - anchor).to_array())
                .collect();
            let mut normals = vec![Vec3::ZERO; local.len()];
            for t in &cloth.triangles {
                let [a, b, c] = t.map(|i| Vec3::from(local[i as usize]));
                let n = (b - a).cross(c - a);
                for &i in t {
                    normals[i as usize] += n;
                }
            }
            let normals: Vec<[f32; 3]> = normals
                .into_iter()
                .map(|n| n.try_normalize().unwrap_or(Vec3::Y).to_array())
                .collect();
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, local);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        }
        if let Ok(own) = skins.get(cloth.holder) {
            let origin = rebase_origin(anchor);
            let frame = rebase_global(GlobalTransform::from_translation(anchor), origin);
            palettes.write_rig_worlds(own, &[frame], &ibp, origin);
            // The stock part's material and tag (its fade and fog), on the copy's slot.
            if let (Ok((mat, tag)), Ok((mut copy_mat, mut copy_tag))) =
                (stock.get(*part), copies.get_mut(cloth.copy))
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

/// On a cloak's copy, so the stock and copy queries stay apart.
#[derive(Component)]
struct ClothCopy;

/// Keeps each simulated cloak's stock part hidden, after the model-visibility pass shows it.
fn hide_stock_cloaks(cloths: Res<Cloths>, mut visibility: Query<&mut Visibility>) {
    for part in cloths.live.keys() {
        if let Ok(mut vis) = visibility.get_mut(*part) {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        }
    }
}
