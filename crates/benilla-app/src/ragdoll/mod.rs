//! Fork: physics ragdolls. 1.12.1 has none; this module and the `ragdoll` feature that gates it
//! are this fork's, kept apart so upstream merges stay small.
//!
//! A unit seen alive that dies falls as a ragdoll (`life`), its bodies picked automatically from
//! its skeleton (`rig`); `WOW_NO_RAGDOLL=1` turns that off. Living players carry a capsule that
//! shoves the bodies aside (`pusher`). The dev chord + `B` drops a test box (`testbox`).

use bevy::prelude::*;

mod life;
mod pusher;
mod rig;
mod testbox;

pub(crate) struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        testbox::plugin(app);
        life::plugin(app);
        pusher::plugin(app);
    }
}
