//! The ragdoll profile, built automatically from a rig's bind skeleton: which bones get a body,
//! the capsule each carries, and which body each one hangs from. No per-model tables: 1.12 bones
//! carry no names, so a bone earns a body by the length of the limb segment it starts.

use bevy::math::Vec3;

/// A bone's segment must be at least this fraction of the skeleton's height to get a body; the
/// fingers, face and toes fall below it and ride their parent body rigidly.
const MIN_SEGMENT: f32 = 0.08;
/// When no profile comes out, the threshold is tried again at these shares of [`MIN_SEGMENT`].
const SHORT_SEGMENT_RETRIES: [f32; 3] = [1.0, 0.5, 0.25];
/// Most bodies a rig gets, keeping the hub and the longest segments: room for a spider's legs.
const MAX_BODIES: usize = 32;
/// A debris rock's body runs this share of the height up from its bone.
const DEBRIS_SEGMENT: f32 = 0.1;
/// Walking down from the root, the hub is the first bone no single child of which holds this share
/// of the limb length below it: the pelvis, where the spine and the legs part.
const HUB_SHARE: f32 = 0.75;
/// A profile whose bodies span less than this share of the skeleton's limb length rides mostly
/// rigid, so the hub moves to where the chain starts ([`chain_start`]).
const CHAIN_COVER: f32 = 0.4;
/// A child of the hub this close to it (a share of the height), with a segment this short, is part
/// of the hub, not a body of its own: a waist bone between the hub and the legs would wobble.
const HUB_MERGE_NEAR: f32 = 0.05;
const HUB_MERGE_SHORT: f32 = 0.1;
/// An added head body runs this share of the height up from the head bone, with a radius this
/// share of its segment: a head is rounder than a limb.
const HEAD_SEGMENT: f32 = 0.1;
const HEAD_RADIUS: f32 = 0.5;
/// Capsule radius as a fraction of its segment, clamped to a band of the skeleton's height.
const RADIUS_OF_SEGMENT: f32 = 0.3;
const RADIUS_MIN: f32 = 0.03;
const RADIUS_MAX: f32 = 0.1;

/// One body of the profile, in bind model space (bind rotations are identity in a 1.12 M2).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BodySpec {
    pub(super) bone: u16,
    /// The bone's pivot, where the body's origin sits.
    pub(super) pivot: Vec3,
    /// From the pivot to the end of the segment (the main child's pivot).
    pub(super) segment: Vec3,
    pub(super) radius: f32,
    /// The body it is jointed to, as an index into the profile's bodies; `None` for the hub.
    pub(super) parent: Option<usize>,
}

/// The bodies of one rig, the hub first.
#[derive(Debug, Clone, Default)]
pub(super) struct Profile {
    pub(super) bodies: Vec<BodySpec>,
    /// Per bone, its body's index in `bodies`, if it has one.
    pub(super) body_of: Vec<Option<usize>>,
}

/// Build the profile from bind-pose locals (`RigPose::binds`) and parents. `None` when fewer than
/// three bones qualify: a rock or a floating eye has nothing to flop.
pub(super) fn build_profile(binds: &[Vec3], parents: &[i16]) -> Option<Profile> {
    // A skeleton of many short bones (a thunderhawk's neck, tail and wing fingers) has no segment
    // long enough at the usual threshold, so it tries again with shorter ones.
    SHORT_SEGMENT_RETRIES
        .iter()
        .find_map(|&share| build_profile_at(binds, parents, MIN_SEGMENT * share))
}

/// [`build_profile`] with segments of at least `min_segment` of the height earning a body.
fn build_profile_at(binds: &[Vec3], parents: &[i16], min_segment: f32) -> Option<Profile> {
    let n = binds.len().min(parents.len());
    if n == 0 {
        return None;
    }
    let parent_of = |i: usize| usize::try_from(parents[i]).ok().filter(|&p| p < i);

    let mut pivots = vec![Vec3::ZERO; n];
    for i in 0..n {
        pivots[i] = parent_of(i).map_or(Vec3::ZERO, |p| pivots[p]) + binds[i];
    }
    let (lo, hi) = pivots.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p.y), hi.max(p.y))
    });
    let height = (hi - lo).max(f32::EPSILON);

    // Subtree sizes, children after parents, so one reverse pass sums them.
    let mut subtree = vec![1usize; n];
    for i in (0..n).rev() {
        if let Some(p) = parent_of(i) {
            subtree[p] += subtree[i];
        }
    }
    // Each bone's main child, the one its segment runs to: a child with children of its own over a
    // leaf (a leaf is a helper: an attachment point, a pad, an unused bone at the origin), then the
    // child that best continues the bone (the elbow's hand, the pelvis's spine), then the largest
    // subtree, then the farthest pivot.
    let incoming = |p: usize| {
        parent_of(p)
            .map(|g| pivots[p] - pivots[g])
            .and_then(|d| d.try_normalize())
    };
    let score = |p: usize, c: usize| {
        let dir = (pivots[c] - pivots[p]).try_normalize();
        let straight = match (incoming(p), dir) {
            (Some(a), Some(b)) => a.dot(b),
            _ => 0.0,
        };
        (
            subtree[c] > 1,
            straight,
            subtree[c],
            (pivots[c] - pivots[p]).length(),
        )
    };
    let mut main_child: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        let Some(p) = parent_of(i) else {
            continue;
        };
        let better = match main_child[p] {
            None => true,
            Some(c) => {
                let (a, b) = (score(p, i), score(p, c));
                a.0.cmp(&b.0)
                    .then(a.1.total_cmp(&b.1))
                    .then(a.2.cmp(&b.2))
                    .then(a.3.total_cmp(&b.3))
                    .is_gt()
            }
        };
        if better {
            main_child[p] = Some(i);
        }
    }

    // Each bone's limb length, its segment's when that is long enough for a body, and the sum over
    // its subtree: what the hub search weighs, so a hand's dozen finger bones count for nothing.
    let limb: Vec<f32> = (0..n)
        .map(|i| {
            main_child[i]
                .map(|c| (pivots[c] - pivots[i]).length())
                .filter(|&l| l >= min_segment * height)
                .unwrap_or(0.0)
        })
        .collect();
    let mut limb_below = limb.clone();
    for i in (0..n).rev() {
        if let Some(p) = parent_of(i) {
            limb_below[p] += limb_below[i];
        }
    }
    let root_like = |i: usize| parent_of(i).is_none() && pivots[i].length() < 0.05 * height;
    let build = |hub: usize| -> Option<Profile> {
        // The hub's ancestors ride it rigidly (the origin root, a bone above the pelvis): a body there
        // would hang off the hub upside down.
        let mut above_hub = vec![false; n];
        let mut up = parent_of(hub);
        while let Some(p) = up {
            above_hub[p] = true;
            up = parent_of(p);
        }
        let mut candidates: Vec<(usize, Vec3)> = (0..n)
            .filter(|&i| i != hub && !above_hub[i])
            .filter_map(|i| {
                let seg = pivots[main_child[i]?] - pivots[i];
                let merged = parent_of(i) == Some(hub)
                    && (pivots[i] - pivots[hub]).length() < HUB_MERGE_NEAR * height
                    && seg.length() < HUB_MERGE_SHORT * height;
                if merged {
                    return None;
                }
                // A root bone at the model origin spans origin to pelvis: no limb.
                (seg.length() >= min_segment * height && !root_like(i)).then_some((i, seg))
            })
            .collect();
        if candidates.len() + 1 > MAX_BODIES {
            candidates.sort_by(|a, b| b.1.length().total_cmp(&a.1.length()));
            candidates.truncate(MAX_BODIES - 1);
        }
        if candidates.len() < 2 {
            return None;
        }
        candidates.sort_by_key(|c| c.0);
        // The hub always has a body, however short its own segment: everything hangs off it. A hub
        // with no child to run to gets a short stub up.
        let hub_seg = main_child[hub]
            .map(|c| pivots[c] - pivots[hub])
            .filter(|s| s.length() >= 0.5 * min_segment * height)
            .unwrap_or(Vec3::Y * min_segment * height);
        candidates.insert(0, (hub, hub_seg));

        let mut body_of = vec![None; n];
        let mut bodies = Vec::with_capacity(candidates.len());
        for (i, seg) in candidates {
            // The nearest ancestor with a body, else the hub (body 0): every body hangs off one tree.
            let mut up = parent_of(i);
            let mut parent = None;
            while let Some(p) = up {
                if let Some(b) = body_of[p] {
                    parent = Some(b);
                    break;
                }
                up = parent_of(p);
            }
            if parent.is_none() && !bodies.is_empty() {
                parent = Some(0);
            }
            body_of[i] = Some(bodies.len());
            bodies.push(BodySpec {
                bone: i as u16,
                pivot: pivots[i],
                segment: seg,
                radius: (seg.length() * RADIUS_OF_SEGMENT)
                    .clamp(RADIUS_MIN * height, RADIUS_MAX * height),
                parent,
            });
        }
        Some(Profile { bodies, body_of })
    };
    let walked = build(find_hub(&limb, &limb_below, &parent_of, n));
    // A chain (a snake, a lasher's vine) has no branch to stop at, so the walk runs to its tip and
    // the whole chain rides that one body. When the walked profile covers too little of the limb
    // length, the hub goes to where the chain starts instead and the chain hangs off it.
    let total = (0..n)
        .filter(|&i| parent_of(i).is_none())
        .map(|i| limb_below[i])
        .fold(0.0, f32::max);
    let covered = |p: &Profile| p.bodies.iter().map(|b| b.segment.length()).sum::<f32>();
    if walked
        .as_ref()
        .is_some_and(|p| covered(p) >= CHAIN_COVER * total)
    {
        return walked;
    }
    let chained = build(chain_start(&limb, &limb_below, &parent_of, &root_like, n));
    match (walked, chained) {
        (Some(w), Some(c)) if covered(&c) <= covered(&w) => Some(w),
        (w, c) => c.or(w),
    }
}

/// Fork: an earth elemental's rocks are leaf bones that float around its middle, which no limb
/// segment reaches, so the whole pile rode one body. Each leaf bone without a body gets a stubby
/// body of its own, jointed to its nearest bodied ancestor, so the rocks fall apart. An empty
/// profile first gets a hub: the bone with the biggest subtree below the root.
pub(super) fn add_debris(profile: &mut Profile, binds: &[Vec3], parents: &[i16]) {
    let n = binds.len().min(parents.len());
    if n == 0 {
        return;
    }
    let parent_of = |i: usize| usize::try_from(parents[i]).ok().filter(|&p| p < i);
    let mut pivots = vec![Vec3::ZERO; n];
    for i in 0..n {
        pivots[i] = parent_of(i).map_or(Vec3::ZERO, |p| pivots[p]) + binds[i];
    }
    let (lo, hi) = pivots.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p.y), hi.max(p.y))
    });
    let height = (hi - lo).max(f32::EPSILON);
    let mut subtree = vec![1usize; n];
    let mut has_child = vec![false; n];
    for i in (0..n).rev() {
        if let Some(p) = parent_of(i) {
            subtree[p] += subtree[i];
            has_child[p] = true;
        }
    }
    if profile.body_of.len() != n {
        profile.body_of = vec![None; n];
    }
    let segment = Vec3::Y * DEBRIS_SEGMENT * height;
    let radius = (segment.length() * HEAD_RADIUS).clamp(RADIUS_MIN * height, RADIUS_MAX * height);
    if profile.bodies.is_empty() {
        let hub = (0..n)
            .filter(|&i| parent_of(i).is_some())
            .max_by_key(|&i| (subtree[i], std::cmp::Reverse(i)))
            .unwrap_or(0);
        profile.body_of[hub] = Some(0);
        profile.bodies.push(BodySpec {
            bone: hub as u16,
            pivot: pivots[hub],
            segment,
            radius,
            parent: None,
        });
    }
    for i in 0..n {
        if profile.bodies.len() >= MAX_BODIES {
            break;
        }
        if has_child[i] || profile.body_of[i].is_some() {
            continue;
        }
        let mut up = parent_of(i);
        let mut parent = 0;
        while let Some(p) = up {
            if let Some(b) = profile.body_of[p] {
                parent = b;
                break;
            }
            up = parent_of(p);
        }
        profile.body_of[i] = Some(profile.bodies.len());
        profile.bodies.push(BodySpec {
            bone: i as u16,
            pivot: pivots[i],
            segment,
            radius,
            parent: Some(parent),
        });
    }
}

/// The head's own body, from the model's head key bone, when it has none: the neck and head are
/// shorter than [`MIN_SEGMENT`], so they rode the chest and a severed head was never possible. The
/// body hangs from the head's nearest bodied ancestor and runs [`HEAD_SEGMENT`] of the height up.
/// Nothing changes when the head already has a body, a body hangs below it, or the profile is full.
pub(super) fn add_head(profile: &mut Profile, binds: &[Vec3], parents: &[i16], head: usize) {
    let n = binds.len().min(parents.len());
    if head >= n || profile.body_of.get(head).copied().flatten().is_some() {
        return;
    }
    if profile.bodies.len() >= MAX_BODIES {
        return;
    }
    let parent_of = |i: usize| usize::try_from(parents[i]).ok().filter(|&p| p < i);
    let under_head = |mut i: usize| loop {
        match parent_of(i) {
            Some(p) if p == head => return true,
            Some(p) => i = p,
            None => return false,
        }
    };
    if profile.bodies.iter().any(|b| under_head(b.bone as usize)) {
        return;
    }
    let mut pivots = vec![Vec3::ZERO; n];
    for i in 0..n {
        pivots[i] = parent_of(i).map_or(Vec3::ZERO, |p| pivots[p]) + binds[i];
    }
    let (lo, hi) = pivots.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p.y), hi.max(p.y))
    });
    let height = (hi - lo).max(f32::EPSILON);
    let mut up = parent_of(head);
    let mut parent = None;
    while let Some(p) = up {
        if let Some(b) = profile.body_of.get(p).copied().flatten() {
            parent = Some(b);
            break;
        }
        up = parent_of(p);
    }
    let Some(parent) = parent else {
        return;
    };
    let segment = Vec3::Y * HEAD_SEGMENT * height;
    if let Some(slot) = profile.body_of.get_mut(head) {
        *slot = Some(profile.bodies.len());
    }
    profile.bodies.push(BodySpec {
        bone: head as u16,
        pivot: pivots[head],
        segment,
        radius: (segment.length() * HEAD_RADIUS).clamp(RADIUS_MIN * height, RADIUS_MAX * height),
        parent: Some(parent),
    });
}

/// Where a chain starts: from the root with the most limb length, down the child with the most,
/// to the first bone that is a limb itself (not the origin root).
fn chain_start(
    limb: &[f32],
    limb_below: &[f32],
    parent_of: &impl Fn(usize) -> Option<usize>,
    root_like: &impl Fn(usize) -> bool,
    n: usize,
) -> usize {
    let mut at = (0..n)
        .filter(|&i| parent_of(i).is_none())
        .max_by(|&a, &b| limb_below[a].total_cmp(&limb_below[b]))
        .unwrap_or(0);
    while limb[at] <= 0.0 || root_like(at) {
        let next = (at + 1..n)
            .filter(|&c| parent_of(c) == Some(at))
            .max_by(|&a, &b| limb_below[a].total_cmp(&limb_below[b]));
        match next {
            Some(c) => at = c,
            None => break,
        }
    }
    at
}

/// The hub: from the root with the most limb length, step into the child that holds at least
/// [`HUB_SHARE`] of the limb length below the bone, until no child does. That is where the body
/// branches (a pelvis, a spider's thorax), whatever its index or its own segment's length.
fn find_hub(
    limb: &[f32],
    limb_below: &[f32],
    parent_of: &impl Fn(usize) -> Option<usize>,
    n: usize,
) -> usize {
    let mut at = (0..n)
        .filter(|&i| parent_of(i).is_none())
        .max_by(|&a, &b| limb_below[a].total_cmp(&limb_below[b]))
        .unwrap_or(0);
    loop {
        let below = (limb_below[at] - limb[at]).max(f32::EPSILON);
        let next = (at + 1..n)
            .filter(|&c| parent_of(c) == Some(at))
            .max_by(|&a, &b| limb_below[a].total_cmp(&limb_below[b]))
            .filter(|&c| limb_below[c] >= HUB_SHARE * below);
        match next {
            Some(c) => at = c,
            None => return at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stick figure 2 yd tall: root at the origin, pelvis, spine, head, two arms and two legs,
    /// with a short finger the threshold drops.
    fn stick() -> (Vec<Vec3>, Vec<i16>) {
        let binds = vec![
            Vec3::ZERO,                // 0 root
            Vec3::new(0.0, 1.0, 0.0),  // 1 pelvis
            Vec3::new(0.0, 0.4, 0.0),  // 2 spine
            Vec3::new(0.0, 0.5, 0.0),  // 3 neck/head
            Vec3::new(0.0, 0.1, 0.0),  // 4 head top
            Vec3::new(0.3, 0.4, 0.0),  // 5 shoulder L (from spine)
            Vec3::new(0.3, 0.0, 0.0),  // 6 elbow L
            Vec3::new(0.3, 0.0, 0.0),  // 7 hand L
            Vec3::new(0.05, 0.0, 0.0), // 8 finger L
            Vec3::new(0.15, 0.0, 0.0), // 9 hip L (from pelvis)
            Vec3::new(0.0, -0.5, 0.0), // 10 knee L
            Vec3::new(0.0, -0.5, 0.0), // 11 foot L
        ];
        let parents = vec![-1, 0, 1, 2, 3, 2, 5, 6, 7, 1, 9, 10];
        (binds, parents)
    }

    #[test]
    fn limbs_get_bodies_and_the_root_and_finger_do_not() {
        let (binds, parents) = stick();
        let p = build_profile(&binds, &parents).expect("a profile");
        let bones: Vec<u16> = p.bodies.iter().map(|b| b.bone).collect();
        assert!(p.body_of[0].is_none(), "the origin root spans no limb");
        assert!(p.body_of[8].is_none(), "a finger is under the threshold");
        assert_eq!(p.bodies[0].bone, 1, "the pelvis is the hub");
        for limb in [1, 2, 5, 6, 9, 10] {
            assert!(
                bones.contains(&limb),
                "bone {limb} should have a body: {bones:?}"
            );
        }
    }

    #[test]
    fn every_body_but_the_hub_hangs_off_an_earlier_one() {
        let (binds, parents) = stick();
        let p = build_profile(&binds, &parents).unwrap();
        assert_eq!(p.bodies[0].parent, None);
        for (k, b) in p.bodies.iter().enumerate().skip(1) {
            let parent = b.parent.expect("jointed");
            assert!(parent < k, "body {k} hangs off a later body");
        }
        // The forearm (bone 6) hangs off the upper arm (bone 5), the arm off the spine.
        let idx = |bone| p.bodies.iter().position(|b| b.bone == bone).unwrap();
        assert_eq!(p.bodies[idx(6)].parent, Some(idx(5)));
        assert_eq!(p.bodies[idx(5)].parent, Some(idx(2)));
        // The spine's segment runs up the neck, not out to the shoulder.
        assert!(p.bodies[idx(2)].segment.x.abs() < 1e-6);
    }

    #[test]
    fn a_helper_leaf_does_not_steal_the_limb() {
        // An upper arm whose helper leaf points straight on, while the elbow bends down.
        let (mut binds, mut parents) = stick();
        binds.push(Vec3::new(0.1, 0.0, 0.0)); // 12 shoulder pad, child of the upper arm
        parents.push(5);
        binds[6] = Vec3::new(0.1, -0.3, 0.0); // the elbow drops
        let p = build_profile(&binds, &parents).unwrap();
        let arm = p.bodies.iter().find(|b| b.bone == 5).unwrap();
        assert!(
            arm.segment.y < 0.0,
            "the arm runs to the elbow: {:?}",
            arm.segment
        );
    }

    #[test]
    fn a_tiny_model_gets_no_ragdoll() {
        assert!(build_profile(&[Vec3::ZERO, Vec3::Y], &[-1, 0]).is_none());
    }
}
