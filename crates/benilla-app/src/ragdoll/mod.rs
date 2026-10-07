//! Fork: physics ragdolls. 1.12.1 has none; this module and the `ragdoll` feature that gates it
//! are this fork's, kept apart so upstream merges stay small.
//!
//! A unit seen alive that dies falls as a ragdoll (`life`), its bodies picked automatically from
//! its skeleton (`rig`); `WOW_NO_RAGDOLL=1` turns that off. Living players carry a capsule that
//! shoves the bodies aside (`pusher`). A lootable ragdoll drops a sack where its unit died
//! (`lootbag`), and the killing blow (`blow`) scales its push. Hits spray blood and ragdolls
//! bleed onto the ground (`gore`), as the Blood option allows. A heavy killing blow can sever a
//! limb (`dismember`), as the Dismemberment option allows. A knockdown, the game's own or our kick,
//! ragdolls a living unit and stands it back up (`knock`). The dev chord + `B` drops a test box (`testbox`).

use bevy::prelude::*;

mod blow;
mod dismember;
mod gore;
mod knock;
mod life;
mod lootbag;
mod pusher;
mod rig;
mod testbox;

pub(crate) struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        testbox::plugin(app);
        blow::plugin(app);
        dismember::plugin(app);
        gore::plugin(app);
        knock::plugin(app);
        life::plugin(app);
        lootbag::plugin(app);
        pusher::plugin(app);
    }
}
