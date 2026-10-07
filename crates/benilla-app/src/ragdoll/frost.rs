//! Fork: a body a frost spell killed lies frozen: `life` locks its joints at the pose it died in,
//! and here its whole model takes an icy tint, on the per-instance tint channel the auras use.

use bevy::prelude::*;

use benilla_world::instance_tint::{pack, InstanceTints};
use benilla_world::rig_palette::RigSkin;

/// The ice tint, multiplied over the model's lighting.
const ICE: [u8; 3] = [150, 195, 255];

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        tint_frozen.after(crate::aura_visual::apply_aura_tint),
    );
}

/// A ragdoll a frost spell killed.
#[derive(Component)]
pub(super) struct Frozen;

/// Paints every frozen body icy, after the auras have published this frame's tints (which they
/// rewrite for every rig, so the ice is laid again each frame and lifts with the marker).
fn tint_frozen(frozen: Query<&RigSkin, With<Frozen>>, mut tints: ResMut<InstanceTints>) {
    for skin in &frozen {
        tints.set(skin.slot, pack(ICE));
    }
}
