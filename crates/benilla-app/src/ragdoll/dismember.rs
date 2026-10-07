//! Fork: dismemberment, behind the options window's Dismemberment box (`dismemberment`).
//!
//! A killing blow that takes a big enough share of the unit's health severs one limb, two on a
//! huge one: the limb's body keeps no joint to its parent and flies off ([`choose_limbs`] picks
//! it, `life` skips the joint). The unit has one skinned mesh and one palette, so the limb is
//! drawn by a copy of the unit's skinned parts on a second palette slot, whose rows pose the
//! limb's bones where their bodies are and collapse every other bone to a point. On the unit's own
//! palette the limb's root bone is scaled to nothing, which folds the whole limb into the stump.
//! Gear drawn as its own model (a held weapon, a helm) rides the collapsed bones and vanishes with
//! the limb.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::math::Affine3A;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_world::mesh_tag::{rig_of, with_rig};
use benilla_world::model_render::ModelPart;
use benilla_world::rig_anim::RigPose;
use benilla_world::rig_palette::{rebase_global, rebase_origin, RigPalettes, RigPart, RigSkin};

use super::life::Ragdoll;
use super::rig::Profile;

/// A killing blow must take this share of the unit's health to sever a limb, and this much to
/// sever two.
const SEVER_SHARE: f32 = 0.2;
const SECOND_LIMB_SHARE: f32 = 0.5;
/// A limb is a body this few joints from the hub (a shoulder, a hip, the neck, or one further
/// down), holding at most this share of the bodies and at least this share of the skeleton's
/// length, so a severed piece is a real limb and never half the body or a stub.
const MAX_DEPTH: usize = 2;
const MAX_LIMB_BODIES: f32 = 0.4;
const MIN_LIMB_LENGTH: f32 = 0.12;
/// The scale a collapsed bone is drawn at: zero would leave the normals undefined.
const TINY: f32 = 1e-4;

pub(super) fn plugin(app: &mut App) {
    app.add_message::<Severed>()
        .add_systems(Update, (build_limb_copies, heal_limbs));
}

/// A limb came off: where its joint was, and the way the body was thrown.
#[derive(Message, Clone, Copy)]
pub(super) struct Severed {
    pub(super) unit: Entity,
    pub(super) at: Vec3,
    pub(super) away: Vec3,
}

/// The unit's severed limbs.
#[derive(Component)]
pub(super) struct Dismembered {
    limbs: Vec<Limb>,
}

/// One severed limb: its root bone, every bone under it, and the entity holding its copy's palette
/// slot (with the mesh copies under it), once built.
struct Limb {
    root: usize,
    mask: Vec<bool>,
    holder: Option<Entity>,
    tried: bool,
}

impl Dismembered {
    /// How many limbs the unit lost.
    pub(super) fn count(&self) -> usize {
        self.limbs.len()
    }

    /// The limbs rooted at `roots`, each taking every bone below it.
    pub(super) fn new(parents: &[i16], roots: &[usize]) -> Self {
        let limbs = roots
            .iter()
            .map(|&root| {
                let mut mask = vec![false; parents.len()];
                for i in 0..parents.len() {
                    let under = usize::try_from(parents[i])
                        .ok()
                        .filter(|&p| p < i)
                        .is_some_and(|p| mask[p]);
                    mask[i] = i == root || under;
                }
                Limb {
                    root,
                    mask,
                    holder: None,
                    tried: false,
                }
            })
            .collect();
        Self { limbs }
    }
}

/// On a limb copy's palette holder: the unit it was cut from.
#[derive(Component)]
struct LimbHolder(Entity);

/// The bodies a killing blow taking `share` of the unit's health severs: none below
/// [`SEVER_SHARE`], else one limb, or two apart from each other past [`SECOND_LIMB_SHARE`].
pub(super) fn choose_limbs(profile: &Profile, share: f32, seed: u64) -> Vec<usize> {
    if share < SEVER_SHARE {
        return Vec::new();
    }
    let n = profile.bodies.len();
    let ancestors =
        |k: usize| std::iter::successors(profile.bodies[k].parent, |&p| profile.bodies[p].parent);
    let mut count = vec![1usize; n];
    let mut length: Vec<f32> = profile.bodies.iter().map(|b| b.segment.length()).collect();
    let total: f32 = length.iter().sum();
    for k in 0..n {
        let own = profile.bodies[k].segment.length();
        for a in ancestors(k).collect::<Vec<_>>() {
            count[a] += 1;
            length[a] += own;
        }
    }
    let candidates: Vec<usize> = (0..n)
        .filter(|&k| {
            profile.bodies[k].parent.is_some()
                && ancestors(k).count() <= MAX_DEPTH
                && count[k] as f32 <= MAX_LIMB_BODIES * n as f32
                && length[k] >= MIN_LIMB_LENGTH * total
        })
        .collect();
    let mut rng = seed;
    let mut next = || {
        // splitmix64
        rng = rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let wanted = if share >= SECOND_LIMB_SHARE { 2 } else { 1 };
    let mut chosen: Vec<usize> = Vec::new();
    let mut pool = candidates;
    while chosen.len() < wanted && !pool.is_empty() {
        let k = pool.swap_remove((next() % pool.len() as u64) as usize);
        // Never a limb inside one already taken, or around it.
        let related = chosen
            .iter()
            .any(|&c| ancestors(k).any(|a| a == c) || ancestors(c).any(|a| a == k));
        if !related {
            chosen.push(k);
        }
    }
    chosen
}

/// The unit is whole again (a resurrection): its limbs' bones come back to full size, and their
/// copies go ([`heal_limbs`]).
pub(super) fn heal(commands: &mut Commands, unit: Entity) {
    commands.entity(unit).queue(|mut e: EntityWorldMut| {
        let Some(cut) = e.take::<Dismembered>() else {
            return;
        };
        if let Some(mut rig) = e.get_mut::<RigPose>() {
            for limb in &cut.limbs {
                if let Some(local) = rig.locals.get_mut(limb.root) {
                    local.scale = Vec3::ONE;
                }
            }
            rig.pose_dirty = true;
        }
    });
}

/// What a limb copy takes from each of its unit's skinned parts.
type SkinnedPart = (
    &'static Mesh3d,
    &'static MeshMaterial3d<WowModelMaterial>,
    &'static MeshTag,
    Option<&'static ModelPart>,
);

/// Gives each severed limb a second palette slot and a copy of the unit's skinned parts on it.
fn build_limb_copies(
    mut commands: Commands,
    mut units: Query<(Entity, &mut Dismembered, &RigSkin, &Children)>,
    parts: Query<SkinnedPart, With<RigPart>>,
    mut palettes: ResMut<RigPalettes>,
) {
    for (unit, mut cut, skin, children) in &mut units {
        for limb in cut.limbs.iter_mut().filter(|l| !l.tried) {
            limb.tried = true;
            let Some(copy_skin) =
                RigSkin::allocate_bones(&mut palettes, skin.bones(), skin.ibp().clone())
            else {
                continue;
            };
            let slot = copy_skin.slot;
            let holder = commands
                .spawn((
                    Name::new("severed limb"),
                    Transform::default(),
                    Visibility::default(),
                    copy_skin,
                    LimbHolder(unit),
                    ChildOf(unit),
                ))
                .id();
            for child in children.iter() {
                let Ok((mesh, material, tag, part)) = parts.get(child) else {
                    continue;
                };
                if rig_of(tag.0) != skin.slot {
                    continue;
                }
                let mut copy = commands.spawn((
                    Mesh3d(mesh.0.clone()),
                    MeshMaterial3d(material.0.clone()),
                    MeshTag(with_rig(tag.0, slot)),
                    Transform::default(),
                    Visibility::default(),
                    NoFrustumCulling,
                    ChildOf(holder),
                ));
                if let Some(part) = part {
                    copy.insert(*part);
                }
            }
            limb.holder = Some(holder);
        }
    }
}

/// Drops the limb copies of a unit made whole.
fn heal_limbs(
    mut commands: Commands,
    holders: Query<(Entity, &LimbHolder)>,
    cut: Query<(), With<Dismembered>>,
) {
    for (holder, of) in &holders {
        if !cut.contains(of.0) {
            commands.entity(holder).despawn();
        }
    }
}

/// After the bodies drive the bones: pose each limb copy's palette from the full pose (its own
/// bones where they are, the rest collapsed onto its joint), then fold the limb into the stump on
/// the unit's own pose. A frozen ragdoll keeps its copies' last rows.
pub(super) fn pose_limbs(
    mut rigs: Query<(&mut RigPose, &Ragdoll, &Dismembered)>,
    frames: Query<&GlobalTransform>,
    skins: Query<&RigSkin>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<RigPalettes>,
) {
    for (rig, rag, cut) in &mut rigs {
        let rig = rig.into_inner();
        if rag.frozen.is_none() {
            if let Ok(root_g) = frames.get(rig.joints_root) {
                let n = rig.locals.len().min(rig.parents.len());
                let mut model = vec![Affine3A::IDENTITY; n];
                for i in 0..n {
                    let local = rig.locals[i].compute_affine();
                    model[i] = match usize::try_from(rig.parents[i]).ok().filter(|&p| p < i) {
                        Some(p) => model[p] * local,
                        None => local,
                    };
                }
                let origin = rebase_origin(root_g.translation());
                let root_rel = rebase_global(*root_g, origin).affine();
                for limb in &cut.limbs {
                    let Some(skin) = limb.holder.and_then(|h| skins.get(h).ok()) else {
                        continue;
                    };
                    let Some(ibp) = ibps.get(skin.ibp()) else {
                        continue;
                    };
                    let joint = model
                        .get(limb.root)
                        .map_or(Vec3::ZERO, |m| Vec3::from(m.translation));
                    let collapsed = root_rel
                        * Affine3A::from_scale_rotation_translation(
                            Vec3::splat(TINY),
                            Quat::IDENTITY,
                            joint,
                        );
                    let worlds: Vec<GlobalTransform> = (0..n)
                        .map(|i| {
                            let m = if limb.mask.get(i).copied().unwrap_or(false) {
                                root_rel * model[i]
                            } else {
                                collapsed
                            };
                            GlobalTransform::from(m)
                        })
                        .collect();
                    palettes.write_rig_worlds(skin, &worlds, ibp, origin);
                }
            }
        }
        // The stump: the limb's root folded to nothing where it joins its parent, not out where
        // its flying body is, which would stretch the skin between the two.
        for limb in &cut.limbs {
            let bind = rig.binds.get(limb.root).copied();
            if let (Some(local), Some(bind)) = (rig.locals.get_mut(limb.root), bind) {
                local.translation = bind;
                local.scale = Vec3::splat(TINY);
            }
        }
        rig.pose_dirty = true;
    }
}
