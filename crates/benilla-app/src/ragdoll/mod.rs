//! Fork: physics ragdolls. 1.12.1 has none; this module and the `ragdoll` feature that gates it
//! are this fork's, kept apart so upstream merges stay small.
//!
//! So far only the test box: the dev chord + `B` drops a dynamic crate in front of the camera,
//! which proves the solver, the contacts and the world colliders before any rig depends on them.

use avian3d::prelude::*;
use bevy::prelude::*;

use benilla_world::collision::ragdoll_layers;
use benilla_world::view::WorldCamera;

/// The test box's edge (yd), about a crate's.
const BOX_SIZE: f32 = 1.0;
/// How far in front of the camera (yd) and above its eye line the box appears.
const DROP_AHEAD: f32 = 6.0;
const DROP_ABOVE: f32 = 4.0;
/// A box despawns after this many seconds, and the oldest goes once this many are live.
const BOX_LIFETIME: f32 = 60.0;
const MAX_BOXES: usize = 32;

pub(crate) struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TestBoxes>()
            .add_systems(Update, (drop_test_box, expire_test_boxes));
    }
}

/// A test box and when it was dropped.
#[derive(Component)]
struct TestBox {
    born: f32,
}

/// The shared mesh and material, built on the first drop.
#[derive(Resource, Default)]
struct TestBoxes {
    art: Option<(Handle<Mesh>, Handle<StandardMaterial>)>,
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
    commands.spawn((
        TestBox {
            born: time.elapsed_secs(),
        },
        Name::new("ragdoll test box"),
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::from_translation(at),
        RigidBody::Dynamic,
        Collider::cuboid(BOX_SIZE, BOX_SIZE, BOX_SIZE),
        ragdoll_layers(),
        AngularVelocity(Vec3::new(1.5, 0.7, -1.1)),
        TransformInterpolation,
    ));
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
