//! Fork: physics ragdolls. 1.12.1 has none; this module and the `ragdoll` feature that gates it
//! are this fork's, kept apart so upstream merges stay small.
//!
//! A unit seen alive that dies falls as a ragdoll (`life`), its bodies picked automatically from
//! its skeleton (`rig`); `WOW_NO_RAGDOLL=1` turns that off. Living players carry a capsule that
//! shoves the bodies aside (`pusher`). A lootable ragdoll drops a sack where its unit died
//! (`lootbag`), and the killing blow (`blow`) scales its push. Hits spray blood and ragdolls
//! bleed onto the ground (`gore`), as the Blood option allows, and stain the body they land on
//! (`stain`). A heavy killing blow can sever a
//! limb (`dismember`), as the Dismemberment option allows. A knockdown, the game's own or our kick,
//! ragdolls a living unit and stands it back up (`knock`). A lightning kill shakes the body in a
//! seizure (`shock`), and bleed ticks squirt blood (`gore`). Stowed weapons jostle as their wearer
//! moves (`jostle`), feet stand on the ground under each of them (`feet`), and the movement keys can steer from the
//! camera (`omni`). The dev chord + `B` drops a test box (`testbox`).

use bevy::prelude::*;

mod blow;
mod bounds;
mod cloak;
mod dismember;
mod feet;
mod frost;
mod gore;
mod hitstop;
mod jostle;
mod knock;
mod life;
mod lootbag;
pub(crate) mod omni;
mod pusher;
mod rig;
mod shock;
mod stain;
mod sway;
mod testbox;

pub(crate) struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        testbox::plugin(app);
        blow::plugin(app);
        bounds::plugin(app);
        cloak::plugin(app);
        dismember::plugin(app);
        frost::plugin(app);
        gore::plugin(app);
        hitstop::plugin(app);
        feet::plugin(app);
        jostle::plugin(app);
        omni::plugin(app);
        knock::plugin(app);
        life::plugin(app);
        lootbag::plugin(app);
        pusher::plugin(app);
        shock::plugin(app);
        stain::plugin(app);
        sway::plugin(app);
        crate::blob_shadow::ragdoll_shadows(app);
    }
}
