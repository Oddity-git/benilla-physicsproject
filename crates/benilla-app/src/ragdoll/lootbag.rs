//! The loot bag: a ragdoll can be shoved or thrown far from where its unit died, but the server
//! keeps the corpse there, so a lootable ragdoll drops a sack on that spot, and pointing at the
//! sack points at the corpse. The sack is a model from the install, found once by name; a plain
//! brown blob stands in when the install has none.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_assets::{m2_url, M2Model, WorldAssets};
use benilla_world::mesh_tag::spawn_tag;
use benilla_world::view::WorldCamera;
use bevy::mesh::MeshTag;

use super::life::Ragdoll;
use crate::entities::ModelBound;
use crate::net::{Guid, NetEntity, ObjectStore};
use crate::player::CameraControl;
use crate::target::{Hovered, PickOcclusion};
use crate::ui_script::PointerOverUi;

/// Sack models tried first, by path; then any `.m2` under `World\Generic` named like a sack.
const SACK_MODELS: &[&str] = &[
    "World\\Generic\\PassiveDoodads\\LootBags\\LootBag01.m2",
    "World\\Generic\\Human\\Passive Doodads\\Sacks\\Sack01.m2",
    "World\\Generic\\Human\\Passive Doodads\\Sacks\\SackOfGrain01.m2",
];
/// A unit this tall (yd, its model's height times its scale) or shorter gets the default sack;
/// a taller one scales the sack up with it, never down.
const DEFAULT_HEIGHT: f32 = 2.2;
/// The click sphere, in sack units: its radius and how high its centre sits.
const PICK_RADIUS: f32 = 0.5;
const PICK_CENTRE: f32 = 0.35;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<SackArt>()
        .init_resource::<Bags>()
        .add_systems(Update, (find_sack_model, sync_bags, build_bags).chain())
        .add_systems(Update, pick_bags.in_set(crate::target::UnitPickOverride));
}

/// The sack model, looked up once: `None` until searched, then the handle if the install has one.
#[derive(Resource, Default)]
struct SackArt(Option<Option<Handle<M2Model>>>);

/// Each lootable ragdoll's sack root.
#[derive(Resource, Default)]
struct Bags(EntityHashMap<Entity>);

/// A sack on the spot a unit died, its size in sack units, and whether its meshes are built.
#[derive(Component)]
struct LootBag {
    scale: f32,
    built: bool,
}

/// Searches the patch chain for a sack model, once, on the first lootable ragdoll.
fn find_sack_model(
    mut art: ResMut<SackArt>,
    bags: Res<Bags>,
    assets: Option<Res<WorldAssets>>,
    server: Res<AssetServer>,
) {
    if art.0.is_some() || bags.0.is_empty() {
        return;
    }
    let Some(assets) = assets else {
        return;
    };
    let path = {
        let Ok(chain) = assets.chain.lock() else {
            return;
        };
        SACK_MODELS
            .iter()
            .map(|p| p.to_string())
            .find(|p| chain.contains(p))
            .or_else(|| {
                let names = chain.list().ok()?;
                names
                    .into_iter()
                    .map(|e| e.name)
                    .filter(|n| {
                        let l = n.to_ascii_lowercase();
                        l.starts_with("world\\generic")
                            && l.ends_with(".m2")
                            && (l.contains("lootbag") || l.contains("\\sack"))
                    })
                    .min_by_key(|n| n.len())
            })
    };
    match &path {
        Some(p) => info!("ragdoll loot bag: using {p}"),
        None => info!("ragdoll loot bag: no sack model in the install, using a plain bag"),
    }
    art.0 = Some(path.map(|p| server.load::<M2Model>(m2_url(&p))));
}

/// What [`sync_bags`] reads of a ragdolled unit.
type BagUnit = (
    Entity,
    &'static ObjectStore,
    &'static GlobalTransform,
    &'static NetEntity,
    Option<&'static ModelBound>,
);

/// Gives each lootable ragdoll a sack on its unit's spot, sized by the unit, and takes the sack
/// away once the corpse is looted, stands up or is gone.
fn sync_bags(mut commands: Commands, units: Query<BagUnit, With<Ragdoll>>, mut bags: ResMut<Bags>) {
    for (unit, store, tf, net, bound) in &units {
        if !store.0.unit_lootable() || bags.0.contains_key(&unit) {
            continue;
        }
        let height = bound.map_or(DEFAULT_HEIGHT, |b| 2.0 * b.0.half_extents.y) * net.scale;
        let scale = (height / DEFAULT_HEIGHT).max(1.0);
        let (_, rot, at) = tf.to_scale_rotation_translation();
        let root = commands
            .spawn((
                Name::new("ragdoll loot bag"),
                LootBag {
                    scale,
                    built: false,
                },
                Transform::from_translation(at)
                    .with_rotation(rot)
                    .with_scale(Vec3::splat(scale)),
                Visibility::default(),
            ))
            .id();
        bags.0.insert(unit, root);
    }
    bags.0.retain(|unit, root| {
        let keep = units
            .get(*unit)
            .is_ok_and(|(_, store, ..)| store.0.unit_lootable());
        if !keep {
            if let Ok(mut e) = commands.get_entity(*root) {
                e.despawn();
            }
        }
        keep
    });
}

/// Fills each new sack with the model's meshes once it has loaded, or the plain bag without one.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn build_bags(
    mut commands: Commands,
    art: Res<SackArt>,
    mut roots: Query<(Entity, &mut LootBag)>,
    m2s: Res<Assets<M2Model>>,
    mut forms: ResMut<benilla_world::model_forms::ModelForms>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    mut plain: ResMut<Assets<StandardMaterial>>,
    mut plain_art: Local<Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
) {
    let Some(found) = &art.0 else {
        return;
    };
    for (root, mut bag) in &mut roots {
        if bag.built {
            continue;
        }
        match found {
            Some(handle) => {
                let Some(model) = m2s.get(handle) else {
                    continue; // still loading
                };
                if !mats.ready() {
                    continue;
                }
                forms.ensure_now_rigged(handle, &model.submeshes, &mut mesh_assets);
                let stat = forms.slices(handle).stat;
                for (pi, sub) in model.submeshes.iter().enumerate() {
                    let Some(material) = mats.steady(sub, sub.texture.clone(), 0) else {
                        continue;
                    };
                    let mesh = stat.get(pi).map(|(h, _)| h.clone()).unwrap_or_default();
                    let child = commands
                        .spawn((
                            Mesh3d(mesh),
                            MeshMaterial3d(material),
                            MeshTag(spawn_tag(0, 1.0)),
                            Transform::IDENTITY,
                        ))
                        .id();
                    commands.entity(root).add_child(child);
                }
            }
            None => {
                let (mesh, material) = plain_art
                    .get_or_insert_with(|| {
                        (
                            mesh_assets.add(Sphere::new(PICK_RADIUS * 0.8)),
                            plain.add(StandardMaterial {
                                base_color: Color::srgb(0.45, 0.32, 0.18),
                                unlit: true,
                                ..default()
                            }),
                        )
                    })
                    .clone();
                let child = commands
                    .spawn((
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        Transform::from_translation(Vec3::Y * PICK_CENTRE)
                            .with_scale(Vec3::new(1.0, 0.8, 1.0)),
                    ))
                    .id();
                commands.entity(root).add_child(child);
            }
        }
        bag.built = true;
    }
}

/// Pointing at a sack points at its corpse: after the unit pick, a sack hit nearer than whatever
/// that pick found (and than the world) takes the mouseover, so a right-click loots.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn pick_bags(
    camera: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rig: Res<CameraControl>,
    pointer_over_ui: Res<PointerOverUi>,
    occlusion: Res<PickOcclusion>,
    bags: Res<Bags>,
    sacks: Query<(&LootBag, &GlobalTransform)>,
    guids: Query<&Guid>,
    mut hovered: ResMut<Hovered>,
) {
    if bags.0.is_empty() || rig.is_looking() || pointer_over_ui.0 {
        return;
    }
    let (Ok((camera, cam_tf)), Ok(window)) = (camera.single(), window.single()) else {
        return;
    };
    let Some(ray) = window
        .cursor_position()
        .and_then(|c| camera.viewport_to_world(cam_tf, c).ok())
    else {
        return;
    };
    let (origin, dir) = (ray.origin, *ray.direction);
    let mut best: Option<(Entity, f32)> = None;
    for (&unit, &root) in &bags.0 {
        let Ok((bag, tf)) = sacks.get(root) else {
            continue;
        };
        let centre = tf.translation() + Vec3::Y * PICK_CENTRE * bag.scale;
        let Some(t) = ray_sphere(origin, dir, centre, PICK_RADIUS * bag.scale) else {
            continue;
        };
        if t < occlusion.distance && best.is_none_or(|(_, b)| t < b) {
            best = Some((unit, t));
        }
    }
    let Some((unit, t)) = best else {
        return;
    };
    if t >= hovered.distance && hovered.any().is_some() {
        return;
    }
    let Ok(guid) = guids.get(unit) else {
        return;
    };
    hovered.target = Some(unit);
    hovered.guid = Some(guid.0);
    hovered.corpse = None;
    hovered.corpse_guid = None;
    hovered.distance = t;
    hovered.refused = false;
}

/// The nearest `t >= 0` where the ray meets the sphere.
fn ray_sphere(origin: Vec3, dir: Vec3, centre: Vec3, radius: f32) -> Option<f32> {
    let oc = origin - centre;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    [-b - s, -b + s].into_iter().find(|&t| t >= 0.0)
}
