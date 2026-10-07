//! Fork: groundwork for cloak physics. A 1.12 cloak is the character body's own 15xx geosets,
//! skinned to the skeleton. Before choosing how to simulate it, this logs, once per character
//! model, which bones the cloak's vertices ride and whether any of them move the cloak alone (a
//! chain a spring simulation could drive) or are shared with the body (cloth would need its own
//! mesh).

use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use benilla_assets::M2Model;

/// The cloak's geoset group: ids 1500..=1599.
const CLOAK_GROUP: u16 = 15;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, log_cloak_bones);
}

/// Logs each newly loaded character model's cloak bones.
fn log_cloak_bones(
    mut events: MessageReader<AssetEvent<M2Model>>,
    models: Res<Assets<M2Model>>,
    server: Res<AssetServer>,
    mut seen: Local<HashSet<AssetId<M2Model>>>,
) {
    for ev in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::LoadedWithDependencies { id }) = ev else {
            continue;
        };
        if !seen.insert(*id) {
            continue;
        }
        let Some(model) = models.get(*id) else {
            continue;
        };
        let path = server
            .get_path(*id)
            .map_or_else(|| format!("{id:?}"), |p| p.to_string());
        if !path.to_ascii_lowercase().contains("character") {
            continue;
        }
        let n = model.skeleton.joints.len();
        // Weighted vertex counts per bone, in the cloak and in the rest of the body.
        let mut cloak = vec![0usize; n];
        let mut body = vec![0usize; n];
        let mut cloak_geosets = Vec::new();
        for sub in &model.submeshes {
            let is_cloak = sub.geoset_id / 100 == CLOAK_GROUP;
            if is_cloak {
                cloak_geosets.push(sub.geoset_id);
            }
            let tally = if is_cloak { &mut cloak } else { &mut body };
            for (joints, weights) in sub.geometry.joints.iter().zip(&sub.geometry.weights) {
                for (&j, &w) in joints.iter().zip(weights) {
                    if w > 0.0 {
                        if let Some(c) = tally.get_mut(j as usize) {
                            *c += 1;
                        }
                    }
                }
            }
        }
        if cloak_geosets.is_empty() {
            info!("ragdoll cloak: {path}: no cloak geosets");
            continue;
        }
        cloak_geosets.sort_unstable();
        cloak_geosets.dedup();
        let rows: Vec<String> = (0..n)
            .filter(|&b| cloak[b] > 0)
            .map(|b| {
                let parent = model.skeleton.joints[b].parent;
                let only = if body[b] == 0 { " ONLY" } else { "" };
                format!(
                    "{b}(parent {parent}, cloak {} body {}{only})",
                    cloak[b], body[b]
                )
            })
            .collect();
        let exclusive = (0..n).filter(|&b| cloak[b] > 0 && body[b] == 0).count();
        info!(
            "ragdoll cloak: {path}: geosets {cloak_geosets:?}, {} bones, {exclusive} cloak-only: {}",
            rows.len(),
            rows.join(" ")
        );
    }
}
