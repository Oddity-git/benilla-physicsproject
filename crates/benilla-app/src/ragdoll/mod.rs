//! Fork: physics ragdolls. 1.12.1 has none; this module and the `ragdoll` feature that gates it
//! are this fork's, kept apart so upstream merges stay small.
//!
//! A unit seen alive that dies falls as a ragdoll (`life`), its bodies picked automatically from
//! its skeleton (`rig`); `WOW_NO_RAGDOLL=1` turns that off. Living players carry a capsule that
//! shoves the bodies aside (`pusher`). A lootable ragdoll drops a sack where its unit died
//! (`lootbag`), and the killing blow (`blow`) scales its push. Hits spray blood and ragdolls
//! bleed onto the ground (`gore`), as the Blood option allows. A heavy killing blow can sever a
//! limb (`dismember`), as the Dismemberment option allows. A knockdown, the game's own or our kick,
//! ragdolls a living unit and stands it back up (`knock`). A lightning kill shakes the body in a
//! seizure (`shock`), and bleed ticks squirt blood (`gore`). The dev chord + `B` drops a test box (`testbox`).

use bevy::prelude::*;

mod blow;
mod bounds;
mod cloak;
mod dismember;
mod frost;
mod gore;
mod hitstop;
mod knock;
mod life;
mod lootbag;
mod pusher;
mod rig;
mod shock;
mod sway;
mod testbox;

/// The body geoset a cloth cloak keeps drawn under its cape: the character body's own no-cloak
/// back under-layer, `skinSectionId` 1501. The reference's cloak branch B8 (`0x47780a`) hides the
/// whole 15xx group and shows the cape in its place, so with a cape the under-layer is gone and only
/// the cape covers the gap; a cape that never leaves the back hides that.
pub(crate) const CLOAK_BACK_GEOSET: u16 = 1501;

/// Whether this part is the back under-layer a cloth cloak keeps under its cape (`cloak.rs`): the
/// skin piece (`CharSkinSlot::Body`) of [`CLOAK_BACK_GEOSET`], never its fur trim, which the cape
/// does not sit outside. The cape hides the skin while the cloak rests and it fills the gap where
/// the cloth has swung off the back.
pub(crate) fn keeps_back_part(
    geoset_id: u16,
    char_slot: Option<benilla_formats::CharSkinSlot>,
) -> bool {
    geoset_id == CLOAK_BACK_GEOSET && char_slot == Some(benilla_formats::CharSkinSlot::Body)
}

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
        knock::plugin(app);
        life::plugin(app);
        lootbag::plugin(app);
        pusher::plugin(app);
        shock::plugin(app);
        sway::plugin(app);
        crate::blob_shadow::ragdoll_shadows(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::CharSkinSlot;

    /// A cloth cloak keeps only the body-skin piece of the back under-layer (geoset 1501): never
    /// its fur trim, the cape's own geoset (1502+), or the body (0).
    #[test]
    fn a_cloth_cloak_keeps_only_the_body_skin_back_part() {
        assert!(keeps_back_part(CLOAK_BACK_GEOSET, Some(CharSkinSlot::Body)));
        assert!(!keeps_back_part(1501, Some(CharSkinSlot::SkinExtra)));
        assert!(!keeps_back_part(0, Some(CharSkinSlot::Body)));
        assert!(!keeps_back_part(1502, Some(CharSkinSlot::Object)));
        assert!(!keeps_back_part(1506, Some(CharSkinSlot::Body)));
    }
}
