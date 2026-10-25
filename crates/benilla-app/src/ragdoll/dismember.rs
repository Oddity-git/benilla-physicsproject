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
use benilla_world::model_render::park::ParkedMesh;
use benilla_world::model_render::ModelPart;
use benilla_world::rig_anim::RigPose;
use benilla_world::rig_palette::{rebase_global, rebase_origin, RigPalettes, RigPart, RigSkin};

use super::life::Ragdoll;
use super::rig::Profile;

/// A killing blow must take this share of the unit's health to sever a limb, and this much to
/// sever two.
const SEVER_SHARE: f32 = 0.2;
const SECOND_LIMB_SHARE: f32 = 0.5;
/// A limb holds at most this share of the bodies and at least this share of the skeleton's length,
/// so a severed piece is never half the body or a stub. Above the waist the floor is lower, so a
/// hand or the head can come off on its own.
const MAX_LIMB_BODIES: f32 = 0.4;
const MIN_LIMB_LENGTH: f32 = 0.12;
const MIN_UPPER_LENGTH: f32 = 0.04;
/// How likely each kind of limb is to be the one cut: arms and hands, the head, the upper body off
/// the hips, and a leg (rare). A body is central when it sits within this share of the height of
/// the hub's vertical line.
const ARM_WEIGHT: f32 = 1.0;
const HEAD_WEIGHT: f32 = 1.5;
const TORSO_WEIGHT: f32 = 0.5;
const LEG_WEIGHT: f32 = 0.1;
const CENTRAL: f32 = 0.08;
/// A central limb of at most this many bodies is the head (with the neck); more is the torso.
const HEAD_BODIES: usize = 3;
/// The scale a collapsed bone is drawn at: zero would leave the normals undefined.
const TINY: f32 = 1e-4;

pub(super) fn plugin(app: &mut App) {
    app.add_message::<Severed>()
        .add_systems(Update, (build_limb_copies, heal_limbs))
        .add_systems(
            PostUpdate,
            sync_limb_copies
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
        );
}

/// A limb came off: where its joint was, and the way the body was thrown.
#[derive(Message, Clone, Copy)]
pub(super) struct Severed {
    pub(super) unit: Entity,
    /// The limb's root bone, where the cut is.
    pub(super) bone: usize,
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
    /// Where the cut is this frame, on the stump and on the severed piece ([`pose_limbs`]).
    wounds: Option<Wounds>,
}

/// The two faces of a cut in the world: each a point and the way it faces out of the flesh.
#[derive(Clone, Copy)]
pub(super) struct Wounds {
    pub(super) stump: (Vec3, Vec3),
    pub(super) piece: (Vec3, Vec3),
}

impl Dismembered {
    /// How many limbs the unit lost.
    pub(super) fn count(&self) -> usize {
        self.limbs.len()
    }

    /// The cut of the limb rooted at `bone`, once posed.
    pub(super) fn wounds(&self, bone: usize) -> Option<Wounds> {
        self.limbs.iter().find(|l| l.root == bone)?.wounds
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
                    wounds: None,
                }
            })
            .collect();
        Self { limbs }
    }
}

/// On each part of a limb copy: the stock part it copies and that part's mesh, so the stains can
/// draw it with the unit's stained mesh (the cut's own stain included) and put it back after.
#[derive(Component)]
pub(super) struct LimbPart {
    pub(super) stock: Entity,
    pub(super) mesh: Handle<Mesh>,
}

/// On a limb copy's palette holder: the unit it was cut from.
#[derive(Component)]
struct LimbHolder(Entity);

/// The bodies a killing blow taking `share` of the unit's health severs: none below
/// [`SEVER_SHARE`], else one limb, two apart from each other past [`SECOND_LIMB_SHARE`], and one
/// more for each further [`SECOND_LIMB_SHARE`] (a high Dismemberment Amount gets there). Each cut
/// is drawn by weight: mostly arms, hands and the head, sometimes the upper body off the hips, a
/// leg seldom.
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
    let (lo, hi) = profile
        .bodies
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), b| {
            let end = b.pivot + b.segment;
            (lo.min(b.pivot.y).min(end.y), hi.max(b.pivot.y).max(end.y))
        });
    let height = (hi - lo).max(f32::EPSILON);
    // The hub is body 0, the hips on a biped: the waistline.
    let hub = profile.bodies[0].pivot;
    let weight = |k: usize| -> Option<f32> {
        let body = &profile.bodies[k];
        body.parent?;
        // Above the waist: on a branch off the hub that runs up (the spine), not down (a leg),
        // wherever the bind pose holds the hands.
        let branch = std::iter::once(k)
            .chain(ancestors(k))
            .find(|&b| profile.bodies[b].parent == Some(0))
            .unwrap_or(k);
        let upper = profile.bodies[branch].segment.y > 0.0;
        let central = (body.pivot - hub).with_y(0.0).length() < CENTRAL * height;
        // The upper body straight off the hub may hold most of the bodies: the cut in half.
        let torso = upper && central && body.parent == Some(0) && count[k] < n;
        let floor = if upper {
            MIN_UPPER_LENGTH
        } else {
            MIN_LIMB_LENGTH
        };
        // The head is short (its body is a stub, `rig::add_head`), so no length floor for it.
        let head = upper && central && count[k] <= HEAD_BODIES;
        if !torso && count[k] as f32 > MAX_LIMB_BODIES * n as f32
            || !head && length[k] < floor * total
        {
            return None;
        }
        Some(match (upper, central) {
            (false, _) => LEG_WEIGHT,
            (true, false) => ARM_WEIGHT,
            (true, true) if count[k] <= HEAD_BODIES => HEAD_WEIGHT,
            (true, true) => TORSO_WEIGHT,
        })
    };
    let mut pool: Vec<(usize, f32)> = (0..n).filter_map(|k| Some((k, weight(k)?))).collect();
    let mut rng = seed;
    let mut next = || {
        // splitmix64
        rng = rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) as f32 / u64::MAX as f32
    };
    let wanted = 1 + (share / SECOND_LIMB_SHARE).floor() as usize;
    let mut chosen: Vec<usize> = Vec::new();
    while chosen.len() < wanted && !pool.is_empty() {
        let sum: f32 = pool.iter().map(|&(_, w)| w).sum();
        let mut pick = next() * sum;
        let at = pool
            .iter()
            .position(|&(_, w)| {
                pick -= w;
                pick <= 0.0
            })
            .unwrap_or(pool.len() - 1);
        let (k, _) = pool.swap_remove(at);
        // Never a limb inside one already taken, or around it.
        let related = chosen
            .iter()
            .any(|&c| ancestors(k).any(|a| a == c) || ancestors(c).any(|a| a == k));
        if !related {
            chosen.push(k);
        }
    }
    let kinds: Vec<&str> = chosen
        .iter()
        .map(|&k| match weight(k) {
            Some(w) if w == LEG_WEIGHT => "leg",
            Some(w) if w == HEAD_WEIGHT => "head",
            Some(w) if w == TORSO_WEIGHT => "torso",
            _ => "arm",
        })
        .collect();
    info!("dismember: severed {kinds:?} (bodies {chosen:?})");
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

/// What a limb copy takes from each of its unit's skinned parts: the mesh, or the parked one of a
/// part a blood stain or the body sway hides and draws a copy of instead.
type SkinnedPart = (
    Option<&'static Mesh3d>,
    Option<&'static ParkedMesh>,
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
    materials: Res<Assets<WowModelMaterial>>,
    server: Res<AssetServer>,
) {
    // A cape part: it would hang stiff off a severed limb.
    let cape = |material: &MeshMaterial3d<WowModelMaterial>| {
        materials
            .get(material.id())
            .and_then(|m| m.base.base_color_texture.as_ref())
            .and_then(|t| server.get_path(t.id()))
            .is_some_and(|p| {
                p.to_string()
                    .to_ascii_lowercase()
                    .contains("objectcomponents/cape")
            })
    };
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
            let mut copied = 0;
            for child in children.iter() {
                let Ok((mesh, parked, material, tag, part)) = parts.get(child) else {
                    continue;
                };
                // A part hidden long enough is parked: its mesh moved off it, so a bloodied or
                // swaying unit's limb would otherwise copy nothing and vanish.
                let Some(mesh) = mesh.map(|m| m.0.clone()).or(parked.map(|p| p.0.clone())) else {
                    continue;
                };
                if rig_of(tag.0) != skin.slot {
                    continue;
                }
                if parked.is_some() && cape(material) {
                    continue;
                }
                let mut copy = commands.spawn((
                    LimbPart {
                        stock: child,
                        mesh: mesh.clone(),
                    },
                    Mesh3d(mesh),
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
                copied += 1;
            }
            if copied == 0 {
                warn!("dismember: unit {unit}'s severed limb found no parts to draw");
            }
            limb.holder = Some(holder);
        }
    }
}

/// Keeps each limb copy on its stock part's material and tag (fades, fog, the batch's animated
/// alpha), on its own slot, and culled while that alpha is nothing, as the stock part is.
#[allow(clippy::type_complexity)]
fn sync_limb_copies(
    stock: Query<(&MeshMaterial3d<WowModelMaterial>, &MeshTag), Without<LimbPart>>,
    mut copies: Query<(
        &LimbPart,
        &mut MeshMaterial3d<WowModelMaterial>,
        &mut MeshTag,
        &mut Visibility,
    )>,
) {
    for (limb, mut copy_mat, mut copy_tag, mut vis) in &mut copies {
        let Ok((mat, tag)) = stock.get(limb.stock) else {
            continue;
        };
        if copy_mat.0 != mat.0 {
            copy_mat.0 = mat.0.clone();
        }
        let want_tag = with_rig(tag.0, rig_of(copy_tag.0));
        if copy_tag.0 != want_tag {
            copy_tag.0 = want_tag;
        }
        let want = super::stain::shown(tag.0);
        if *vis != want {
            *vis = want;
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
    mut rigs: Query<(&mut RigPose, &Ragdoll, &mut Dismembered)>,
    frames: Query<&GlobalTransform>,
    skins: Query<&RigSkin>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<RigPalettes>,
) {
    for (rig, rag, cut) in &mut rigs {
        let rig = rig.into_inner();
        let cut = cut.into_inner();
        if rag.frozen.is_none() {
            if let Ok(root_g) = frames.get(rig.joints_root) {
                let n = rig.locals.len().min(rig.parents.len());
                let mut model = vec![Affine3A::IDENTITY; n];
                for i in 0..n {
                    let mut local = rig.locals[i];
                    // The fold below leaves a limb root at `TINY`, and the pose keeps scale from
                    // frame to frame, so the piece itself is drawn at full size.
                    if local.scale == Vec3::splat(TINY) && cut.limbs.iter().any(|l| l.root == i) {
                        local.scale = Vec3::ONE;
                    }
                    let local = local.compute_affine();
                    model[i] = match usize::try_from(rig.parents[i]).ok().filter(|&p| p < i) {
                        Some(p) => model[p] * local,
                        None => local,
                    };
                }
                let origin = rebase_origin(root_g.translation());
                let root_rel = rebase_global(*root_g, origin).affine();
                let world = root_g.affine();
                for limb in cut.limbs.iter_mut() {
                    limb.wounds = wounds(rig, &model, limb, &world);
                }
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
                // A bone the model scales to nothing keeps that: it hides spare geometry.
                if local.scale.max_element() > TINY {
                    local.scale = Vec3::splat(TINY);
                }
            }
        }
        rig.pose_dirty = true;
    }
}

/// Where `limb`'s cut is in the world, from the pose before the fold: the stump's face at the
/// root's bind point on its parent, facing along the bone, and the piece's at the root's joint,
/// facing back up it.
fn wounds(rig: &RigPose, model: &[Affine3A], limb: &Limb, world: &Affine3A) -> Option<Wounds> {
    let at = |i: usize| {
        model
            .get(i)
            .map(|m| world.transform_point3(m.translation.into()))
    };
    let parent = usize::try_from(*rig.parents.get(limb.root)?)
        .ok()
        .filter(|&p| p < limb.root)?;
    let bind = *rig.binds.get(limb.root)?;
    let stump = world.transform_point3(model.get(parent)?.transform_point3(bind));
    let piece = at(limb.root)?;
    let out = (stump - at(parent)?).normalize_or(Vec3::Y);
    // The piece faces away from its first child, else straight back at the stump.
    let child = (limb.root + 1..rig.parents.len())
        .find(|&i| usize::try_from(rig.parents[i]).ok() == Some(limb.root));
    let back = child
        .and_then(at)
        .and_then(|c| (piece - c).try_normalize())
        .unwrap_or(-out);
    Some(Wounds {
        stump: (stump, out),
        piece: (piece, back),
    })
}
