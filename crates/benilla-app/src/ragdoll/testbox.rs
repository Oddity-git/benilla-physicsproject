//! The test box: the dev chord + `B` drops a dynamic crate in front of the camera, which proves
//! the solver, the contacts and the world colliders before any rig depends on them. The crate is a
//! crate model from the install, its collider fitted to the model's box: each drop takes the next
//! of every model named crate under `World\Generic`, logging which, so the one wanted can be found.
//! A plain orange cube stands in when the install has none.

use avian3d::prelude::*;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_assets::{m2_url, M2Model, WorldAssets};
use benilla_world::collision::ragdoll_layers;
use benilla_world::mesh_tag::spawn_tag;
use benilla_world::view::WorldCamera;

/// The test box's edge (yd), about a crate's.
const BOX_SIZE: f32 = 1.0;
/// How far in front of the camera (yd) and above its eye line the box appears.
const DROP_AHEAD: f32 = 6.0;
const DROP_ABOVE: f32 = 4.0;
/// A box despawns after this many seconds, and the oldest goes once this many are live.
const BOX_LIFETIME: f32 = 60.0;
const MAX_BOXES: usize = 32;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TestBoxes>().add_systems(
        Update,
        (drop_test_box, build_crates, expire_test_boxes).chain(),
    );
}

/// A test box, when it was dropped, its crate model, and whether that model is in it yet.
#[derive(Component)]
struct TestBox {
    born: f32,
    model: Option<Handle<M2Model>>,
    built: bool,
}

/// The stand-in cube under a box, dropped for the crate model.
#[derive(Component)]
struct PlainCube;

/// The plain cube's mesh and material, the crate models (`None` until searched), and which one the
/// next drop takes.
#[derive(Resource, Default)]
struct TestBoxes {
    art: Option<(Handle<Mesh>, Handle<StandardMaterial>)>,
    models: Option<Vec<String>>,
    next: usize,
}

/// Every `.m2` under `World\Generic` named crate, by path.
fn find_crate_models(assets: &WorldAssets) -> Vec<String> {
    let Ok(chain) = assets.chain.lock() else {
        return Vec::new();
    };
    let mut names: Vec<String> = chain
        .list()
        .map(|names| {
            names
                .into_iter()
                .map(|e| e.name)
                .filter(|n| {
                    let l = n.to_ascii_lowercase();
                    l.starts_with("world\\generic") && l.ends_with(".m2") && l.contains("crate")
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    info!(
        "ragdoll test box: {} crate models in the install",
        names.len()
    );
    names
}

/// The dev chord + `B`: a box falls from in front of the camera, tumbling, and logs where it
/// started; [`expire_test_boxes`] logs where it came to rest.
fn drop_test_box(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut boxes: ResMut<TestBoxes>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    live: Query<(Entity, &TestBox)>,
    assets: Option<Res<WorldAssets>>,
    server: Res<AssetServer>,
) {
    if !crate::run_mode::dev_chord(&keys, KeyCode::KeyB) {
        return;
    }
    let Ok(cam) = camera.single() else {
        return;
    };
    if live.iter().count() >= MAX_BOXES {
        if let Some((oldest, _)) = live.iter().min_by(|a, b| a.1.born.total_cmp(&b.1.born)) {
            commands.entity(oldest).despawn();
        }
    }
    if boxes.models.is_none() {
        if let Some(assets) = assets.as_deref() {
            boxes.models = Some(find_crate_models(assets));
        }
    }
    let model = match boxes.models.as_deref() {
        Some(models) if !models.is_empty() => {
            let i = boxes.next % models.len();
            info!(
                "ragdoll test box: crate {}/{}: {}",
                i + 1,
                models.len(),
                models[i]
            );
            let handle = server.load::<M2Model>(m2_url(&models[i]));
            boxes.next = i + 1;
            Some(handle)
        }
        _ => {
            info!("ragdoll test box: no crate model in the install, using a plain cube");
            None
        }
    };
    let (mesh, material) = boxes
        .art
        .get_or_insert_with(|| {
            (
                meshes.add(Cuboid::from_length(BOX_SIZE)),
                materials.add(StandardMaterial {
                    base_color: Color::srgb(0.85, 0.35, 0.1),
                    ..default()
                }),
            )
        })
        .clone();
    let mut ahead = cam.forward().as_vec3();
    ahead.y = 0.0;
    let ahead = ahead.try_normalize().unwrap_or(Vec3::X);
    let at = cam.translation() + ahead * DROP_AHEAD + Vec3::Y * DROP_ABOVE;
    info!("ragdoll test box: dropped at {at:.2}");
    // The plain cube shows until the crate model is in, or for good without one.
    commands.spawn((
        TestBox {
            born: time.elapsed_secs(),
            model,
            built: false,
        },
        Name::new("ragdoll test box"),
        Visibility::default(),
        Transform::from_translation(at),
        RigidBody::Dynamic,
        Collider::cuboid(BOX_SIZE, BOX_SIZE, BOX_SIZE),
        ragdoll_layers(),
        AngularVelocity(Vec3::new(1.5, 0.7, -1.1)),
        TransformInterpolation,
        children![(PlainCube, Mesh3d(mesh), MeshMaterial3d(material))],
    ));
}

/// Swaps each new box's plain cube for the crate model once it has loaded, its collider fitted to
/// the model's box.
fn build_crates(
    mut commands: Commands,
    mut crates: Query<(Entity, &mut TestBox, &Children)>,
    plain: Query<(), With<PlainCube>>,
    m2s: Res<Assets<M2Model>>,
    mut forms: ResMut<benilla_world::model_forms::ModelForms>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
) {
    if !mats.ready() {
        return;
    }
    for (root, mut test_box, children) in &mut crates {
        if test_box.built {
            continue;
        }
        let Some(handle) = test_box.model.clone() else {
            continue; // the plain cube for good
        };
        let Some(model) = m2s.get(&handle) else {
            continue; // still loading
        };
        test_box.built = true;
        forms.ensure_now_rigged(&handle, &model.submeshes, &mut mesh_assets);
        let stat = forms.slices(&handle).stat;
        for child in children.iter().filter(|&c| plain.contains(c)) {
            commands.entity(child).despawn();
        }
        let mut e = commands.entity(root);
        for (pi, sub) in model.submeshes.iter().enumerate() {
            let Some(material) = mats.steady(sub, sub.texture.clone(), 0) else {
                continue;
            };
            let mesh = stat.get(pi).map(|(h, _)| h.clone()).unwrap_or_default();
            e.with_child((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                MeshTag(spawn_tag(0, 1.0)),
                Transform::IDENTITY,
            ));
        }
        if let Some(b) = &model.bounds {
            let (p, q) = (wow_to_bevy(b.bbox_min), wow_to_bevy(b.bbox_max));
            let (lo, hi) = (p.min(q), p.max(q));
            let size = (hi - lo).max(Vec3::splat(0.05));
            e.insert(Collider::compound(vec![(
                Position((lo + hi) * 0.5),
                Rotation::default(),
                Collider::cuboid(size.x, size.y, size.z),
            )]));
        }
    }
}

/// Logs each box's first rest (avian put it to sleep) and despawns it after [`BOX_LIFETIME`].
fn expire_test_boxes(
    mut commands: Commands,
    time: Res<Time>,
    boxes: Query<(Entity, &TestBox, &Transform, Has<Sleeping>)>,
    mut rested: Local<bevy::ecs::entity::EntityHashSet>,
) {
    let now = time.elapsed_secs();
    for (e, b, tf, sleeping) in &boxes {
        if sleeping && rested.insert(e) {
            info!(
                "ragdoll test box: came to rest at {:.2} after {:.1}s",
                tf.translation,
                now - b.born
            );
        }
        if now - b.born > BOX_LIFETIME {
            rested.remove(&e);
            commands.entity(e).despawn();
        }
    }
}
