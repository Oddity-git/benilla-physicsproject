//! Fork: knockdowns that ragdoll a living unit and stand it back up (`life::KnockDown`).
//!
//! - The game's own: a unit made to play Knockdown (121), as Charge's stun and Lash do, falls
//!   instead and gets up when the animation would have.
//! - The kick: the dev chord + `K` plays our character's kick and knocks the targeted creature
//!   flying if it is close. Only on this screen: the server still has it standing, so one we are
//!   fighting keeps fighting.

use bevy::prelude::*;

use benilla_protocol::EntityKind;

use super::blow::LastHit;
use super::life::{KnockDown, Ragdoll};
use crate::creature_anim::{AnimData, EmoteAnim, PlaySeq};
use crate::net::{GuidIndex, NetEntity, ObjectStore, SelfPlayer};
use crate::target::Selection;

/// AnimationData's Knockdown, which kits play on a knocked-down unit.
const KNOCKDOWN_ANIM: u16 = 121;
/// A game knockdown: a light push backwards, down about as long as the animation.
const KNOCKDOWN_PUSH: f32 = 3.0;
const KNOCKDOWN_LIFT: f32 = 1.0;
const KNOCKDOWN_HOLD: f32 = 1.8;
/// A hit older than this did not cause the knockdown, so it does not aim it (s).
const KNOCKDOWN_BLOW: f32 = 1.0;
/// The kick reaches this far (yd), lands this long after the key (s), and throws the creature
/// this hard (yd/s) with this much lift, to lie this long before it gets up.
const KICK_RANGE: f32 = 5.0;
const KICK_DELAY: f32 = 0.25;
const KICK_PUSH: f32 = 7.0;
const KICK_LIFT: f32 = 3.0;
const KICK_HOLD: f32 = 2.5;
/// The kick animations to try, by `AnimationData` name, in order.
const KICK_ANIMS: &[&str] = &["Kick", "SpecialUnarmed", "AttackUnarmed"];

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, (game_knockdowns, kick, land_kicks));
}

/// A kick on its way to its target.
#[derive(Component)]
struct KickLanding {
    from: Entity,
    at: f32,
}

/// Knock down each unit the game plays Knockdown on.
fn game_knockdowns(
    mut commands: Commands,
    mut anims: MessageReader<EmoteAnim>,
    units: Query<(&GlobalTransform, &NetEntity, &ObjectStore, Option<&LastHit>), Without<Ragdoll>>,
    frames: Query<&GlobalTransform>,
    index: Res<GuidIndex>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    for ev in anims.read() {
        if ev.anim_id != KNOCKDOWN_ANIM {
            continue;
        }
        let Ok((tf, net, store, blow)) = units.get(ev.entity) else {
            continue;
        };
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) || store.0.unit_is_dead() {
            continue;
        }
        // Away from whoever just hit it, else backwards.
        let back = tf.rotation() * Vec3::Z;
        let away = blow
            .filter(|b| now - b.at <= KNOCKDOWN_BLOW)
            .and_then(|b| index.0.get(&b.attacker))
            .and_then(|&a| frames.get(a).ok())
            .and_then(|a| {
                (tf.translation() - a.translation())
                    .with_y(0.0)
                    .try_normalize()
            })
            .unwrap_or(back);
        commands.entity(ev.entity).insert(KnockDown {
            away: away * KNOCKDOWN_PUSH,
            lift: KNOCKDOWN_LIFT,
            hold: KNOCKDOWN_HOLD,
        });
    }
}

/// The dev chord + `K`: play our kick, and send it at the target if one is in reach.
#[allow(clippy::too_many_arguments)] // one Bevy system's resources
fn kick(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    me: Query<(Entity, &GlobalTransform), With<SelfPlayer>>,
    targets: Query<(&GlobalTransform, &NetEntity, &ObjectStore), Without<Ragdoll>>,
    selection: Res<Selection>,
    catalog: Option<Res<AnimData>>,
    mut seq: ResMut<PlaySeq>,
    mut anims: MessageWriter<EmoteAnim>,
    time: Res<Time>,
    mut kick_anim: Local<Option<Option<u16>>>,
) {
    if !crate::run_mode::dev_chord(&keys, KeyCode::KeyK) {
        return;
    }
    let Ok((me, my_tf)) = me.single() else {
        return;
    };
    let anim = *kick_anim.get_or_insert_with(|| {
        let catalog = catalog.as_deref()?;
        let found = KICK_ANIMS.iter().find_map(|want| {
            (0..1024u16).find(|&id| {
                catalog
                    .0
                    .name(id)
                    .is_some_and(|n| n.eq_ignore_ascii_case(want))
            })
        });
        info!(
            "ragdoll kick: animation {:?} ({:?})",
            found,
            found.and_then(|id| catalog.0.name(id))
        );
        found
    });
    if let Some(anim_id) = anim {
        anims.write(EmoteAnim {
            entity: me,
            anim_id,
            seq: seq.next(),
            via_player: false,
        });
    }
    let Some(target) = selection.target.filter(|&t| t != me) else {
        return;
    };
    let Ok((tf, net, store)) = targets.get(target) else {
        return;
    };
    if net.kind != EntityKind::Unit
        || store.0.unit_is_dead()
        || tf.translation().distance(my_tf.translation()) > KICK_RANGE
    {
        return;
    }
    commands.entity(target).insert(KickLanding {
        from: me,
        at: time.elapsed_secs() + KICK_DELAY,
    });
}

/// A kick that has landed knocks its target away from the kicker.
fn land_kicks(
    mut commands: Commands,
    landing: Query<(Entity, &KickLanding, &GlobalTransform, Has<Ragdoll>)>,
    frames: Query<&GlobalTransform>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    for (target, kick, tf, down) in &landing {
        if now < kick.at {
            continue;
        }
        // Already down (or dead and fallen): the kick finds nothing standing.
        if down {
            commands.entity(target).remove::<KickLanding>();
            continue;
        }
        let away = frames
            .get(kick.from)
            .ok()
            .and_then(|k| {
                (tf.translation() - k.translation())
                    .with_y(0.0)
                    .try_normalize()
            })
            .unwrap_or(tf.rotation() * Vec3::Z);
        commands
            .entity(target)
            .remove::<KickLanding>()
            .insert(KnockDown {
                away: away * KICK_PUSH,
                lift: KICK_LIFT,
                hold: KICK_HOLD,
            });
    }
}
