//! The ragdoll profile, built automatically from a rig's bind skeleton: which bones get a body,
//! the capsule each carries, and which body each one hangs from. No per-model tables: 1.12 bones
//! carry no names, so a bone earns a body by the length of the limb segment it starts.

use bevy::math::Vec3;

/// A bone's segment must be at least this fraction of the skeleton's height to get a body; the
/// fingers, face and toes fall below it and ride their parent body rigidly.
const MIN_SEGMENT: f32 = 0.08;
/// Most bodies a rig gets, keeping the longest segments.
const MAX_BODIES: usize = 20;
/// Capsule radius as a fraction of its segment, clamped to a band of the skeleton's height.
const RADIUS_OF_SEGMENT: f32 = 0.22;
const RADIUS_MIN: f32 = 0.025;
const RADIUS_MAX: f32 = 0.07;

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
    /// Per bone, whether it has a body.
    pub(super) physical: Vec<bool>,
}

/// Build the profile from bind-pose locals (`RigPose::binds`) and parents. `None` when fewer than
/// three bones qualify: a rock or a floating eye has nothing to flop.
pub(super) fn build_profile(binds: &[Vec3], parents: &[i16]) -> Option<Profile> {
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
    // Each bone's main child, the one its segment runs to: the child that best continues the bone
    // (the elbow's hand, the pelvis's spine), else the largest subtree, else the farthest pivot.
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
        (straight, subtree[c], (pivots[c] - pivots[p]).length())
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
                a.0.total_cmp(&b.0)
                    .then(a.1.cmp(&b.1))
                    .then(a.2.total_cmp(&b.2))
                    .is_gt()
            }
        };
        if better {
            main_child[p] = Some(i);
        }
    }

    let mut candidates: Vec<(usize, Vec3)> = (0..n)
        .filter_map(|i| {
            let seg = pivots[main_child[i]?] - pivots[i];
            // A root bone at the model origin spans origin to pelvis: no limb.
            let root_like = parent_of(i).is_none() && pivots[i].length() < 0.05 * height;
            (seg.length() >= MIN_SEGMENT * height && !root_like).then_some((i, seg))
        })
        .collect();
    if candidates.len() > MAX_BODIES {
        candidates.sort_by(|a, b| b.1.length().total_cmp(&a.1.length()));
        candidates.truncate(MAX_BODIES);
        candidates.sort_by_key(|c| c.0);
    }
    if candidates.len() < 3 {
        return None;
    }

    let mut physical = vec![false; n];
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
        physical[i] = true;
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
    Some(Profile { bodies, physical })
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
        assert!(!p.physical[0], "the origin root spans no limb");
        assert!(!p.physical[8], "a finger is under the threshold");
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
    fn a_tiny_model_gets_no_ragdoll() {
        assert!(build_profile(&[Vec3::ZERO, Vec3::Y], &[-1, 0]).is_none());
    }
}
