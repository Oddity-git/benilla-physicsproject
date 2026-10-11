# The physics fork: how it is built

This is the guide to the physics fork of benilla (`Oddity-git/benilla-physicsproject`, branch
`ragdoll`). Read it before changing anything under `crates/benilla-app/src/ragdoll/` or any line
marked `Fork:`. It explains what the fork adds, how each piece works, where it hooks into
upstream benilla, and the traps that have already cost time.

Upstream's own docs (`AGENTS.md`, `docs/METHOD.md`, `docs/MAP.md`, `docs/CONTRIBUTING.md`) still
describe the engine. Where they conflict with this file about the fork's own features, this file
wins. The "Where upstream rules do not apply" section below lists each conflict.

---

## 1. What the fork is

benilla is a from-scratch World of Warcraft 1.12.1 client in Rust and Bevy. Upstream's rule is
fidelity to the 1.12.1 reference client. This fork deliberately adds features 1.12.1 never had.

| Feature | Module | Setting |
|---|---|---|
| Physics ragdolls on death | `life`, `rig` | always on (`WOW_NO_RAGDOLL=1` disables) |
| Death push and lift scaled by the killing blow | `blow`, `life` | `ragdollPush`, `ragdollLift`, `ragdollSpellPush` |
| Melee kills wait for the swing to land | `life` | `ragdollMeleeDelay` |
| Players shove corpses | `pusher` | always |
| Loot sack where the server's corpse is | `lootbag` | always |
| Flung ragdolls stay drawn | `bounds` | always |
| Blood spray, splats, pools, bleed squirts | `gore` | `violenceLevel`, `goreAmount` |
| Dismemberment | `dismember` | `dismemberment`, `dismemberAmount` |
| Frost kills freeze stiff and icy | `frost`, `life` | always |
| Lightning kills shake in a seizure | `shock`, `life` | always |
| Knockdowns ragdoll and stand back up; a debug kick | `knock` | always |
| Hitstop on landed melee hits | `hitstop` | `hitstop`, `hitstopStrength` |
| Cloth cloaks | `cloak` | `cloakPhysics`, `cloakMotion`, `cloakLooseDrawn` |
| Soft-body sway on female units | `sway` | `bodyPhysics`, `bodySize`, `bodyRearSize` |
| Stowed weapons jostle while moving | `jostle` | `weaponMotion` |
| Test crate drop | `testbox` | dev chord + `B` |

All of it lives behind the `ragdoll` Cargo feature, which is on by default.

---

## 2. Working rules for this repo

These come from the maintainer (Mathieu). They override generic habits.

- **Environment.** CachyOS with the fish shell. The local clone is
  `~/Games/benilla-physicsproject`. He tests against a local cMaNGOS server with
  `WOW_HOST=localhost:3726`.
- **Commands for him are fish blocks that start with the `cd`**, for example:
  ```fish
  cd ~/Games/benilla-physicsproject
  git pull origin ragdoll
  cargo build --release -p benilla
  ```
- **Verification is compile and clippy only.** Run `cargo build --release -p benilla` and
  `cargo clippy -p benilla-app --features ragdoll --all-targets -- -D warnings` (without `--features ragdoll` the whole `ragdoll/` module is skipped and clippy passes trivially) (add `-p benilla-world` when touching
  it). Do not run the test suite; he tests in game. "Done" means it compiles clean and is
  pushed. The in-game result comes from him.
- **Do not write files into `~`.** Scratch work goes elsewhere.
- **Keep it fork-only and upstream-light.** New behaviour goes in `ragdoll/`. Touch an upstream
  file only when nothing else works, keep the change tiny, and mark it `Fork:` (section 5).
- **README.md:**
  - Run `git pull origin ragdoll` before editing it. He edits it on GitHub.
  - Keep his `<img …>` GIF tags exactly as they are.
  - **Never mention the body/sway physics (`sway.rs`, Body Physics, Chest Size, Rear Size) in the README.**
    This is a firm, repeated instruction.
- **Commits.** Push to `ragdoll` (no PRs). Subjects read "Ragdoll: <what a player sees>", in
  the style of `git log`. In practice he asks for a build, and the change is pushed after it
  compiles.
- **Logs go to the terminal**, not to a file. There is no `benilla-config/Logs/*.log` for these
  lines. To capture them:
  ```fish
  cd ~/Games/benilla-physicsproject
  env WOW_HOST=localhost:3726 cargo run --release -p benilla 2>&1 | tee /tmp/benilla.log
  grep -E "cloth cloak|body physics" /tmp/benilla.log
  ```

---

## 3. Where upstream rules do not apply

An LLM that reads `docs/METHOD.md` will find rules this fork breaks on purpose. These are the
deliberate exceptions. Do not "fix" them.

| Upstream rule | What the fork does instead |
|---|---|
| A change must make benilla more like 1.12.1. | The fork adds features 1.12.1 never had. That is its purpose. |
| Features beyond 1.12.1 go in their own crate via `run_with` and keep their own settings, never CVar rows. | The fork lives inside `benilla-app` and registers its settings as CVar rows with `ours(...)` in `cvars/table.rs`. This keeps the options-window plumbing simple. |
| Settings boot at the reference default. | The fork's rows are `ours` rows, which have no reference value. Their defaults are the fork's choice. |
| The UI layer does not grow, and a test names its files. | The fork adds one file, `assets/ui/PhysicsOptions.xml`. It is listed in `layer.toc` and in `shipped_xml_tests.rs`. |
| Gates include `cargo test --workspace` and smoke runs. | Mathieu's rule is compile and clippy only (section 2). |
| land by squash-merged PR to `main` | The fork pushes straight to `ragdoll`. |

The upstream hard rules still apply: never commit anything from the WoW install, never write into
the install, and keep local state in `benilla-config/`.

---

## 4. Repository layout

- **Upstream base:** commit `2e82d34c`. The first fork commit is `6ead3cdf` ("Ragdoll step 1").
  `git diff 2e82d34 HEAD` shows everything the fork changed.
- **Fork code:** `crates/benilla-app/src/ragdoll/`, about 5,500 lines in 16 files.
- **Fork options page:** `crates/benilla-app/assets/ui/PhysicsOptions.xml`.
- **Outside those two:** 23 files, +766/−118 lines, nearly all marked `Fork:`.

### Cargo features

```
benilla      default = ["dev", "ragdoll"];  ragdoll = ["benilla-app/ragdoll"]
benilla-app  ragdoll = ["benilla-world/dynamics"]
benilla-world dynamics = []   # keeps avian's solver, contacts and joints on
```

Upstream runs avian3d for collision queries only, with the solver disabled. `dynamics` turns
the solver back on and sets `SubstepCount(8)` (`world_plugins.rs`), because ragdoll joints fall
apart with upstream's 1 substep.

### Engine versions

Bevy 0.18.1 and avian3d 0.6. API names in this doc and the code are for these versions.

---

## 5. Upstream hook points (`Fork:` markers)

Every change outside `ragdoll/` carries a `// Fork:` or `/// Fork:` comment so a merge from
upstream can find it. `grep -rn "Fork:" crates` lists them. These are the hooks:

**benilla-app**
- `lib.rs`: adds `ragdoll::RagdollPlugin` behind `#[cfg(feature = "ragdoll")]`.
- `blob_shadow.rs`: an appended block at the end of the file, `ragdoll_shadows(app)` and
  `drop_ragdoll_shadows`. A ragdoll leaves no blob shadow behind. It is appended rather than
  woven in so other forks' merges stay clean.
- `creature_anim.rs`: re-exports `LootSparkleOn`, `FxEffectScale` and blood helpers for `gore`.
- `creature_anim/blood.rs`: exposes `violence_level` and `unit_blood_id`, and sets
  `SPURT_SCALE 0.5`, which plays the stock melee spurt at half size.
- `creature_anim/spell_visual.rs`: lets the loot sparkle attach to the loot sack.
- `entities/spell_fx/mod.rs`: an effect can be scaled about its point (the gore spurt).
- `target/mod.rs`: `UnitPickOverride` set, so pointing at a loot sack targets the corpse.
- `cvars/table.rs`: the fork's CVar rows (section 7).
- `entities/equipment/mod.rs`: `HeldAttached::spawned_attach`, each held item's root and attach
  point, for `jostle`.
- `ui_script/shipped_xml_tests.rs`: lists `PhysicsOptions.xml`.
- `assets/ui/layer.toc`: loads `PhysicsOptions.xml`.
- `tests/world_api_wall.rs`: allows the ragdoll collision layer through the API wall.

**benilla-world**
- `collision.rs`: the `Ragdoll` and `Pusher` layers, `ragdoll_layers`, `pusher_layers`,
  `RagdollIgnore`, and collision hooks that drop the pairs it names.
- `world_plugins.rs`: the `dynamics` feature path and `SubstepCount 8`.
- `mesh_tag.rs`: `with_rig` made public.
- `rig_palette.rs`: `rebase_origin`, `rebase_global` and `write_rig_worlds` made public.
- `rig_anim/pose.rs`: `RigPose::binds` made public.
- `rig_rider.rs`: the `RiderTilt` component. `write_rig_riders` appends it to a rider's frame
  (`F × tilt`), so `jostle` can tip a stowed weapon's rendered model.
- `straddle.rs` and `zfill.rs`: water twin meshes are spawned before Bevy reads which meshes need
  pipelines. This fixed a crash near water that ragdolls exposed. It is a real bug fix, not a
  feature.

**When merging upstream:** keep each hook a one- or two-line change. If upstream moved the code,
re-apply the hook in the new place and keep the `Fork:` comment.

---

## 6. Rendering concepts every feature depends on

The cloak, sway and dismemberment code all depend on how benilla draws skinned models.
Misunderstanding this causes most bugs.

### 6.1 Palette rig slots

benilla does not use Bevy's `SkinnedMesh`. Each rigged model owns a **slot** in a shared bone
palette (a storage buffer of `Mat4` rows), and its meshes' shader reads rows from that slot.

- `RigSkin { slot, … }` is on the unit. `RigSkin::allocate_bones(&mut palettes, n, ibp)` allocates
  a new slot of `n` bones with the given inverse bind poses.
- `palettes.write_rig_worlds(skin, &worlds, &ibp, origin)` writes rows as `world × ibp`.
  `world_palette` and `rig_rows` read back rows. `rig_rows` is rig-relative.
- Rows are relative to `slot_origin`. `rebase_origin` and `rebase_global` convert between world
  space and slot space. Getting this wrong puts copies at the world origin or doubles the offset.
- Each mesh entity carries a `MeshTag`. `with_rig(tag, slot)` writes the slot into the tag, and
  `rig_of(tag)` reads it back. Other bits in the tag are fade, fog and per-instance flags. **A
  copy must take the stock part's whole tag every frame and only swap the slot**, or it loses
  fades and fog.
- `RigPart(rig)` links a part entity to its unit.
- `RigPose { joints_root, locals, model, parents, binds, pose_dirty }` is the CPU pose. `model`
  holds model-space bone matrices, and `binds` holds local bind translations (made public by the
  fork).

### 6.2 The mesh-copy technique

1.12 models have no cloth bones, no breast bones and one skinned mesh per unit. So `cloak`,
`sway` and `dismember` all use the same trick:

1. Hide the stock part by setting `Visibility::Hidden` in a system that runs
   `.before(VisibilitySystems::VisibilityPropagate)`.
2. Spawn a copy entity with the part's mesh (or a CPU-built mesh), `ModelPart`,
   `NoFrustumCulling`, and the stock material.
3. Give the copy its **own palette slot**, and every frame write that slot's rows and copy the
   stock part's material and `MeshTag` (with the copy's slot).

Gotchas, each learned the hard way:

- **Parking.** `benilla_world::model_render::park` takes the `Mesh3d` off a part that has been
  hidden for 30 frames (`PARK_AFTER_FRAMES`) and stores it in `ParkedMesh`. After that the
  stock entity has no `Mesh3d`. Code that reads the stock mesh must also check `ParkedMesh`
  (`sway.rs` does). `WOW_NO_MESH_PARK=1` turns parking off when debugging.
- **`ModelVisSet` only writes on a flip.** The upstream visibility authority writes `Visibility`
  only when its own decision changes. Your hide system must therefore re-assert `Hidden` every
  frame, ordered before propagation, and must restore `Inherited` itself when it releases the
  part.
- **Re-dressing swaps meshes.** Equipping or removing gear changes which part entities exist and
  which mesh handle each one has. A copy built from the old mesh then draws the wrong geometry.
  `sway` tracks `(Entity, AssetId<Mesh>)` per part and rebuilds when the set changes or when an
  `AssetEvent<Mesh>::Modified` names one of them. A failed build is remembered in
  `Sways.failed` and retried only when the parts change.
- **Ordering.** Upstream writes palette rows in `finalize_rig_worlds` (PostUpdate). Systems that
  write copy rows run `.after(finalize_rig_worlds).in_set(BillboardPlace)` so they read this
  frame's pose and are not overwritten.

### 6.3 Coordinates

- Bevy: +Y up, model front is −Z.
- WoW to Bevy: `wow_to_bevy(x, y, z) = (−y, z, −x)`.
- 1.12 bones have **no names**. Everything (ragdoll bodies, cloak capsules, chest bones) is
  found from the bind skeleton's geometry, never by bone index tables.

### 6.4 Tints

`benilla_world::instance_tint::InstanceTints` is a per-slot colour channel the aura visuals use.
`frost` and `shock` set it with `tints.set(skin.slot, pack(rgb))` in a system ordered
`.after(crate::aura_visual::apply_aura_tint)`, so the aura pass does not overwrite it.

---

## 7. Settings: CVar plus options row

Every slider or checkbox needs changes in three places:

1. **`crates/benilla-app/src/cvars/table.rs`**: add a row near the others,
   ```rust
   // benilla's own: the Physics page's Cloak Motion slider, how much the cloth cloaks swing.
   ours("cloakMotion", "1", "benilla's own — 1.12 draws a cloak stiff on the body's bones"),
   ```
2. **`crates/benilla-app/assets/ui/PhysicsOptions.xml`**:
   - a `BENILLA_TOOLTIP_*` string in the `<Script>`;
   - a `<Frame name="$parentRowX" inherits="BenillaOptions{Slider,Checkbox,Dropdown}RowTemplate">`
     anchored under the previous row, with `BenillaOptionsRow_OnLoad(this, "cvar", "Label",
     "TOOLTIP")` and, for a slider, `BenillaOptionsSliderRow_Setup(this, min, max, step)`;
   - the row name added to `BENILLA_OPTIONS_PAGE_ROWS.Physics` in
     `BenillaPhysicsOptions_Register()`. **If the next row's anchor pointed at the row you
     inserted after, re-point it to the new row.**
3. **Read it in Rust**: take `cvars: Option<Res<crate::cvars::Cvars>>`, read
   `c.num("name")` or `c.flag(...)`, and clamp to the slider's range with a sensible fallback.
   See `gore.rs::gore_amount`.

The page's rows, in order:

| Row | CVar | Range | Default |
|---|---|---|---|
| Blood | `violenceLevel` | Off / Green / Red | 2 |
| Gore Amount | `goreAmount` | 0–10 | 1 |
| Dismemberment | `dismemberment` | box | 0 |
| Dismember Amount | `dismemberAmount` | 0.25–10 | 1 |
| Hitstop | `hitstop` | box | 1 |
| Hitstop Strength | `hitstopStrength` | 0.25–3 | 1 |
| Death Push | `ragdollPush` | 0–10 | 1 |
| Death Lift | `ragdollLift` | 0–10 | 1 |
| Spell Push | `ragdollSpellPush` | 0.5–10 | 2 |
| Melee Death Delay | `ragdollMeleeDelay` | 0–1 s, step 0.05 | 0.3 |
| Cloak Physics | `cloakPhysics` | box | 1 |
| Cloak Motion | `cloakMotion` | 0–3 | 1 |
| Loose Cloak When Armed | `cloakLooseDrawn` | box | 1 |
| Body Physics | `bodyPhysics` | 0–3, 0 off | 1 |
| Chest Size | `bodySize` | 0.5–2.5 | 1 |
| Rear Size | `bodyRearSize` | 0.5–2.5 | 1 |
| Weapon Motion | `weaponMotion` | 0–3, 0 off | 1 |

Wide slider ranges (up to 10×) are intentional. Mathieu asked for "wacky high settings".

---

## 8. Combat-event plumbing

The fork reacts to server messages, not to animations. This matters because the server sends
damage and death **before** the attacker's swing animation reaches its impact frame.

- **`blow.rs`** records `LastHit { attacker, damage, at, school, spell }` on the victim from
  `AttackerState` (melee) and `SpellDamageLog` (spells). The log arrives before the health field
  reads dead, so the hit is already there when the ragdoll starts. `school: None` means melee,
  and `Some(4)` is frost.
- **Periodic damage.** `on_periodic_aura_log` (`PeriodicAuraLog`, with `PeriodicTick` from
  `benilla_protocol::messages::PeriodicTick`; it is not re-exported at the crate root) calls
  `gore::bleed` on a physical damage tick with `amount > 0`. A periodic physical
  `SpellDamageLog` bleeds too.
- **Swings.** `SwingMessage` and `SwingImpact` (the impact key of the swing clip) drive hitstop.
- **Spell names.** `crate::ui_action::Spells` holds a `catalog`. Use
  `spells.catalog.get(id).map(|s| &s.name)`. `shock` classifies spells by name.
- **Unit fields.** `ObjectStore` gives `unit_is_dead()`, `unit_gender()` (1 is female),
  `unit_aura_ids()` and `unit_sheath_state()`.

---

## 9. The modules

All paths are under `crates/benilla-app/src/ragdoll/`. Each file opens with a `//!` header that
explains it. Read the header first.

### `mod.rs`
Registers every module's plugin plus `crate::blob_shadow::ragdoll_shadows`. New modules are
added here.

### `rig.rs` (ragdoll profile)
`build_profile(binds, parents) -> Option<Profile>` builds the bodies from the bind skeleton.

- A bone earns a body if the limb segment it starts is at least `MIN_SEGMENT` (0.08) long. There
  are at most `MAX_BODIES` (32).
- The **hub** is the pelvis-like bone where the limbs split (`find_hub`, `HUB_SHARE 0.75`).
  Nearby or short bones merge into it.
- The capsule radius is `RADIUS_OF_SEGMENT` × the segment length, clamped to 0.03–0.1.
- Bones without a body ride their nearest bodied ancestor. Bones outside the tree ride the hub.

### `life.rs` (ragdoll lifecycle)
- `SeenAlive` marks units seen alive. Only those ragdoll. A unit that streams in already dead
  keeps its stock Death pose.
- `start_ragdolls` (Update) creates bodies at the current pose, joints with `SWING_LIMIT 55°`
  and `TWIST_LIMIT 20°`, and the death push and lift:
  - push = `DEATH_PUSH` plus `BLOW_PUSH` × the killing blow's share of health, capped by
    `MAX_PUSH`, × `ragdollPush`;
  - lift works the same way with `ragdollLift`;
  - spells push `ragdollSpellPush` times harder.
  It also decides frozen, shocked and severed limbs.
- **Melee delay.** A melee kill (`school None`) gets `AwaitingBlow` and is skipped until
  `now − hit.at ≥ ragdollMeleeDelay` (default 0.3 s, max 1 s). `track_unit_motion` keeps
  `LastSpot` updated meanwhile. Spells are never delayed. Mathieu was explicit about that.
- **Frost** (school 4): joints are locked near the death pose (`FROZEN_LIMIT 2°`,
  `FROZEN_DAMPING 60`), and the body inserts `frost::Frozen`.
- **Shock**: `shocked = !frozen && no knockdown && (killing spell name is_shock ||
  ShockDebuff seen within DEBUFF_MEMORY)`. It inserts `shock::Shocked::new(now)`.
- PostUpdate chain in `rig_anim::PosePost`: `drive_bones_from_bodies → dismember::pose_limbs →
  knocked_to_death → get_up → blend_getting_up → freeze_settled → cleanup`.
- **Sleep and freeze.** Settled bodies sleep but stay, so players can still shove them. A body
  that never settles within `MAX_SIM_SECS` (12), or the oldest past `MAX_LIVE` (16), freezes its
  pose and drops its bodies.
- `RagdollBodies` is a resource mapping unit to body entities. `shock`, `gore` and others use it.
- Speed caps: `MAX_BODY_SPEED 25` and `MAX_BODY_SPIN 20`. Without them a big blow tunnels
  through terrain.

### `blow.rs`
Records `LastHit` and calls `bleed` (section 8).

### `pusher.rs`
A kinematic capsule (radius 0.45) follows each living player in FixedUpdate. Its collision layer
hits only ragdoll bodies, so corpses get shoved but never block the player, the camera or the
server. A jump of more than `TELEPORT` (8) snaps it instead of sweeping.

### `bounds.rs`
The exterior cull selects units by `WorldUnit::bound` around their standing spot. While a unit is
a ragdoll, the bound is stretched over its bodies (padded by 0.8), so a flung body does not vanish
when its old spot leaves view.

### `lootbag.rs`
The server keeps the corpse where the unit died, which the ragdoll may have left. A lootable
ragdoll drops a sack model (found by name in the install, with a brown blob as fallback) on that
spot. `pick_bags` runs in `crate::target::UnitPickOverride`, so pointing at the sack targets the
corpse. The stock loot sparkle is moved to the sack.

### `gore.rs`
All blood colours and textures come from the stock `UnitBlood` tables. Violence 1 is green,
0 is nothing. A bloodless creature row gives nothing.

- **Spray.** A hit throws droplets away from the attacker, more for a bigger share of health and
  most on a kill. Each droplet splats where it lands. `FAST_SHARE 0.3` of them fly faster, scaled
  by `FAST_PER_AMOUNT 0.5` × `goreAmount`.
- **Splats** are decals on the shared ground projector and fade out. The cap is
  `MAX_SPLATS 400 × goreAmount`, up to `MAX_SPLATS_EVER 4000`.
- **Pools** start under a ragdoll, centred on its bodies' middle at the lowest body's height. A
  pool follows the body when it drifts more than `POOL_FOLLOW 0.15`. Once the bodies are gone (on
  freeze), it stays where it was.
- **Bleeding.** `bleed()` inserts or extends `Bleeding` (`BLEED_HOLD 3.5` s). `squirt_bleeds`
  shoots heartbeat pulses (`PULSE 0.7`, `SPURT 0.35`) from one wound at
  `WOUND_HEIGHT 0.35–0.75` of the body.
- **Cuts.** `bleed_cuts` makes each severed limb burst and splash once.
- Droplets are drawn as effect quads through `WorldEffectDraw` (`push_decals` after
  `begin_effect_frame`). Do not spawn an entity per droplet; that was too slow.
- Update chain: `spray_hits → bleed_cuts → squirt_bleeds → fly_droplets → start_pools →
  grow_pools → age_splats`.

### `dismember.rs`
- `choose_limbs(profile, share, seed)` severs `1 + floor(share / SECOND_LIMB_SHARE)` limbs when
  the killing blow's share of health, scaled by `dismemberAmount`, is at least `SEVER_SHARE`
  (0.2). It only cuts limbs within `MAX_DEPTH 2` of the hub, of at most `MAX_LIMB_BODIES 0.4` of
  the body, and at least `MIN_LIMB_LENGTH` long.
- `life` skips the severed limb's joint and flings it (`SEVER_FLING`, `SEVER_LIFT`).
- **Drawing.** There is one mesh per unit, so the limb is drawn by copies of the unit's skinned
  parts on a **second palette slot**. That slot poses the limb's bones at their bodies and
  collapses every other bone to a point (`TINY`). On the unit's own palette, the limb root is
  scaled to zero, which folds the limb into the stump **at its joint**.
- `heal` undoes this when a unit revives.

### `hitstop.rs`
On a landed melee hit (`SwingMessage`), at the clip's impact key, it holds the **drawn pose** of
the attacker and victim for `STOP 0.06` s (`CRIT_STOP 0.12` on a crit, more for a big share, at
most 0.3, × `hitstopStrength`). It holds only the drawn pose, not the animation clips: pausing
the clips desynced the animation driver, so units walked in place and swing sounds drifted. Clips
with no impact key (some blunt weapons) stop after `FALLBACK 0.3` s instead.

### `knock.rs`
- The game's own knockdown animation (121), as from Charge or Lash, ragdolls the unit and stands
  it up when the animation would have ended.
- The dev chord (Ctrl+Shift) + `K` plays a kick and knocks the targeted creature flying. This is
  client-only; the server still has it standing.
- `get_up` blends from the ragdoll pose back to animation (`GET_UP_SECS 0.6`).

### `frost.rs` and `shock.rs`
- **Frost.** `Frozen` units get an icy tint (`ICE`). The stiffness itself is in `life`.
- **Shock.** `is_shock(name)` matches lightning, thunder, shock, storm, static or spark, so
  Thunder Clap counts. `note_shock_debuffs` stamps `ShockDebuff(now)` on living units that carry
  such an aura. The server clears auras on death, so `DEBUFF_MEMORY 1.0` s of memory is needed.
  `jolt_shocked` removes `Sleeping` and adds random velocity to every body every 0.06 s for 2.5 s,
  fading out. `tint_shocked` flickers a pale blue. The body never stiffens; it shakes and then
  lies as normal.

### `cloak.rs` (cloth cloaks)
A 1.12 cloak is body geosets (15xx) skinned to the spine, hips and thighs, with no bone of its
own.

- Cape parts are found by material texture path containing `objectcomponents/cape`. **All cape
  parts of one rig merge into one `Cloth`** (`pieces`). Separate cloths per part tore apart at
  the seams.
- The cloak as the animation skins it is the **target**. The top rows (pinned) ride the
  shoulders exactly. The rest is verlet cloth with gravity (`GRAVITY 4`), edge constraints,
  bending (`BEND 0.5`), damping (`KEEP 0.96`), and a pull toward the target that weakens down the
  cloak (`FOLLOW_HEM 0.04` at the hem).
- Capsules fitted to the bones the cloak rides keep it out of the legs and back.
- **Vibration fix.** Each frame takes an equal number of fixed substeps (at most `MAX_STEPS`), and
  inertia is `INERTIA 0.35 × cloakMotion`. When the drift clamp (`MAX_DRIFT`) moves a vertex it
  moves `prev` by the same amount, or the clamp injects velocity.
- **Seams.** Normals are averaged over smooth groups across pieces.
- **Weapon drawn** (`cloakLooseDrawn`). Read from the AnimDriver sheath state, falling back to
  `ObjectStore::unit_sheath_state() != 0`. The lower cloak eases (`LOOSE_RAMP`, `LOOSE_EASE`) to a
  rigid carry by the pinned rows' main bone, so it hangs from the shoulders and not the hips.
- Only the nearest `MAX_CLOTHS` (10) within `MAX_RANGE` (60) simulate.
- Copies use a one-bone palette slot (identity rows) because the CPU writes final positions.
- Logs `cloth cloak: …` once per model.

### `sway.rs` (soft-body sway)
Do not describe this one in the README (section 2).

- Applies to female units (`unit_gender() == 1`): the nearest `MAX_UNITS` (10) within 40 yd.
  Parts whose texture path contains `cape` or `hair` are excluded. Hair was being picked up on
  Night Elf females.
- **Chest finding** (`find_chest`, from the bind pose):
  1. The shoulder is the highest bone with |x| > 0.05 × height and at least 4 descendants (an
     arm root).
  2. Search the band from shoulder − 0.25 H up to the shoulder.
  3. A tip is the most forward vertex (min z) on each side with |x| between 0.2 and 0.85 of the
     shoulder's x.
  4. Validate that the two sides are symmetric and the tips stand out from the body. If not, the
     log says why with `warn!` and the model is left alone.
- **Rear finding** (same function, `find_body`):
  1. The hip is the highest leg root: a bone with |x| > 0.02 × height, at least 2 descendants,
     at least 0.15 H under the shoulder, whose chain reaches within 0.15 H of the ground. A hip
     under 0.35 H off the ground is refused (`HIP_LOWEST`). At 0.35 H under the shoulder the real
     hip was skipped and the knee taken for it.
  2. Search the band from hip − 0.15 H to hip + 0.12 H.
  3. A tip is the most backward vertex (max z) on each side with |x| between 0.5 and 1.4 of the
     hip's x. The same checks as the chest apply (`find_pair`).
  4. The radius is `REAR_RADIUS 0.055 × H`, not the tips' spread. Sizing it from the spread or
     the hip's width left human and night elf rears too small and swelled a dwarf's waist.
  The chest and the rear are found independently, so a model can get one without the other. Each
  failure is logged as `body physics: <name>: no chest|rear: <why>`.
- Each found pair gives two `Lobe`s (centre, outward direction, radius, rear flag). `reweight`
  moves each vertex near a lobe onto that lobe's extra bone (`bones + lobe index`), replacing the
  lightest joint slot and fading out toward `REACH × radius`. Rear lobes take at most
  `REAR_WEIGHT 0.75` of a vertex, so the hips and thighs still bend it. The distance is
  stretched per axis: the chest's reaches `CHEST_REACH_UP 2` further upward so it slopes from
  the collar (Mathieu found the orc chest a harsh ball), and the rear's reaches only
  `REAR_REACH_VERTICAL 0.9` up and down and takes nothing deeper than its centre, to stay off the
  waist and thighs. The rear's centre sits `REAR_CENTRE_DEPTH 0.8 × radius` behind its tip
  (the chest's 0.4), so it swells round.
- Carriers: a chest lobe rides its heaviest original bone. A rear lobe rides the root-most of
  its heavy bones (the pelvis rather than a thigh), so a stride does not swing it.
- The copy slot has `bones + lobes` rows (2 or 4) with identity ibp. Its rows are the unit's own
  `rig_rows` plus the spring bones.
- **Spring.** `FREQUENCY 5`, `DAMPING 0.15`, fixed `STEP 1/120`, horizontal response × 0.2, offset
  capped at `MAX_OFFSET 0.35 × radius × bodyPhysics`. The rear's is calmer on purpose (Mathieu
  asked for less bounce than the chest): `REAR_FREQUENCY 7`, `REAR_DAMPING 0.3`, `REAR_LAG 0.5`
  of the motion, `REAR_STRAY 0.5` of the cap. A jump over `TELEPORT 2` resets it. **The
  component pointing into the body (against the lobe's outward direction, −Z for the chest and +Z
  for the rear, carried by the bone) is dropped.** Without that the chest flattened when running
  forward.
- Bone matrix: `T(shown) · carrier · T(c) · S(grow) · T(−c)`, where size is `bodySize` for the
  chest and `bodyRearSize` for the rear, and `grow` takes the full size step along Z (outward)
  but only part of it across and vertically: chest 0.7 and 0.5, rear 0.75 and 0.75 (at 0.35 and
  0.5 the orc rear grew to a point). A uniform scale
  ballooned the chest toward the neck and widened the rear around the waist.
- Rebuilds on re-dress (6.2). Removing a chest item on orcs and dwarves used to break it.
- Logs `body physics: …`.

### `jostle.rs` (stowed weapons)
- Applies to the mainhand and offhand roots (`HeldAttached::spawned_attach`, slots 0 and 1) on a
  stowed attach point: back (26, 27), shield on the back (28), lower back (30, 31) or hip (32,
  33). A drawn weapon is left alone. Every unit with held items gets it; there is no range cap.
- Each item keeps a damped spring on its attach point's world position (the anchor's
  `GlobalTransform`, one frame old, × the attach offset): `FREQUENCY 4`, `DAMPING 0.25`, `LAG 0.6`
  of the motion, capped at `MAX_LAG 0.08` yd. A jump over `TELEPORT 2` resets it.
- The lag becomes a tilt about the attach point: a horizontal lag tips the part above the point
  back along it (`Y × lag`), a vertical one tips it about the unit's side axis. `ARM 0.35` yd of
  lag is a radian, capped at `MAX_TILT 0.3` rad, both × `weaponMotion`.
- The tilt goes on two places: the root's `Transform` (rotation, translation kept at the attach
  offset), which collision, picking and the glow read, and `RiderTilt`, which the rendered rider
  frame takes. Runs in PostUpdate before `TransformSystems::Propagate`.
- When an item stops jostling (drawn, swapped, or `weaponMotion` 0), its rotation goes back to
  identity and `RiderTilt` is removed.

### `testbox.rs`
The dev chord + `B` drops a crate model from the install in front of the camera, cycling through
every `World\Generic` crate and logging which. It proves the solver and colliders independently
of rigs.

---

## 10. Schedules at a glance

| Schedule | Systems |
|---|---|
| Update | `life`: `track_unit_motion → start_ragdolls`; `gore` chain; `dismember`: `build_limb_copies`, `heal_limbs`; `cloak::manage_cloths`; `sway::manage_sways`; `knock`; `hitstop::start_stops`; `shock`: `note_shock_debuffs`, `jolt_shocked`; `frost`/`shock` tints after `apply_aura_tint`; `lootbag` chain; `bounds::fit_bounds` |
| FixedUpdate | `pusher::follow_players`, `life::release_parted_pairs` |
| PostUpdate, before animation | `life::reset_getting_up` |
| PostUpdate, `PosePost` | `life` chain (section 9), `hitstop::hold_stops` |
| PostUpdate, after `finalize_rig_worlds`, in `BillboardPlace` | `cloak::simulate_cloths`, `sway::pose_sways` |
| PostUpdate, before `VisibilityPropagate` | `cloak::hide_stock_cloaks`, `sway::hide_stock_parts` |
| PostUpdate, after `begin_effect_frame` | `gore::push_decals` |
| PostUpdate, before `TransformSystems::Propagate` | `jostle::jostle_stowed` |

---

## 11. How a feature gets added (the usual recipe)

1. Make a new `ragdoll/<name>.rs` with a `//! Fork: …` header saying what a player sees and how
   it works, then a `pub(super) fn plugin(app: &mut App)`. Register it in `mod.rs` and add a
   word about it to the `mod.rs` header.
2. React to messages (section 8) or to `Ragdoll`, `RagdollBodies` and `LastHit`. Do not patch
   upstream systems.
3. To change what a model looks like, use the mesh-copy technique (section 6.2) and respect all
   of its gotchas.
4. Add the setting (section 7).
5. Name constants at the top with a one-line `///` comment and units. Comments state what the code
   does, not its history (upstream rule).
6. Run `cargo build --release -p benilla` and clippy with `-D warnings`. Clippy fixes used before:
   `as_chunks::<3>()` instead of `chunks_exact(3)`, type aliases for big query tuples
   (`type_complexity`), `then_some`, and `iter_mut().enumerate()` for `needless_range_loop`.
7. Commit as "Ragdoll: …", push to `ragdoll`, and give Mathieu a fish block to pull and build.
8. Add a log line (`info!` once per model, `warn!` on failure) for anything that can silently
   fail on some races. He sends the terminal output back.

---

## 12. Known issues and open questions

- **Troll male cloak pinch.** The cloak narrows at the waist into an hourglass shape. Not yet
  diagnosed. The likely cause is the capsule fit or the target-follow on the hip bones. A
  comparison with Cloak Physics off was requested to rule out the stock mesh.
- **Gnome female sway** does not activate. A `body physics:` log line was requested. The likely
  cause is `find_chest` validation failing on the gnome's proportions.
- Not yet confirmed in game: pool follow, lightning seizure, bleed squirts, the hair exclusion
  on Night Elf females, rebuild on re-dress, and the Chest Size slider.

---

## 13. History

| Commit | What it did |
|---|---|
| `6ead3cdf` | Step 1: avian dynamics back on, test box |
| `1b1d00a6` | Step 2: units seen alive fall as ragdolls |
| `28f4737c`…`00b535b7` | Rig fixes, corpse shoving, collisions between parts, hub at the pelvis, death throw, loot bag |
| `91816922` | Blood option, ground blood, hit spray, speed cap |
| `f48cc2e1` | Knockdowns, kick, dismemberment |
| `289b3f16`, `29488e6b`, `69e6f303` | Stump folds at its joint, gore fixes |
| `dd2cfc15` | Droplets as effect quads |
| `a3815a31` | Water twin crash fix |
| `eecd06ed` | Draw box follows the body |
| `4de5c228` | Spell kills throw harder, frost freezes |
| `2f1520b7`, `20b95aa8` | Hitstop (holding the pose, not the clips) and gore sliders |
| `e54af430`, `7c397fed` | The Physics options page in its own file, push and lift sliders |
| `f79b2261`…`b4bdd2df` | Cloth cloaks, one cloth per cloak, Cloak Motion, loose cloak when armed |
| `57b6ac60`, `0a963fad` | Melee death timing, first by impact key, then as a delay slider; 10× sliders; Body Physics |
| `0e004276` | Sway bounces vertically and never presses inward |
| `12399c9c` | Gore Amount raises the splat cap and spray speed |
| `936f4bdc` | Pools follow the body, lightning seizures, bleed squirts, chest fixes, Chest Size |

The melee timing went through two designs. Waiting for the swing's impact key felt unclear, so it
was replaced with a plain delay slider. Do not bring back the impact-key wait without asking.
