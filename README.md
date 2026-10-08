<div align="center">
  <h1>benilla physics project</h1>
  <p><b>A fork of <a href="https://github.com/samwhosung/benilla">benilla</a>, the from-scratch World of Warcraft 1.12.1 client, that adds ragdoll physics and gore</b></p>
  <p>
    <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue?style=for-the-badge" alt="License"></a>
  </p>
</div>

## What this fork is for

[benilla](https://github.com/samwhosung/benilla) is a complete WoW 1.12.1 client written in Rust
and [Bevy](https://bevy.org). Its goal is to be a faithful 1.12.1 client, so features vanilla never
had belong in forks. This is one of those forks: it uses the physics engine benilla already
ships ([avian](https://github.com/Jondolf/avian)) to make deaths feel physical.

Everything here is **client side only**. The server, other players and their clients see a normal
1.12.1 game; the ragdolls, blood and loot bags exist only on your screen.

### What it adds
<p align="center">
<img width="800" height="918" alt="ezgif-13c5c7c3d95d6217" src="https://github.com/user-attachments/assets/6634af5a-7ee9-49e5-996d-ef1e53fb1878" />
</p>

- **Ragdolls.** A unit or player that dies falls as a physics ragdoll instead of playing its death
  animation. The ragdoll is built automatically from each model's skeleton, so it works for every
  creature model, not only the ones it was tuned on. Limbs have joint limits and damping, body
  parts collide with each other and with other corpses, and parts are speed-capped so nothing
  launches into the sky.
- **Death push.** A body is thrown away from whoever killed it, and pops up a little before it
  falls, both more the more of its health the killing blow took.
- **Spell kills.**
<p align="center">
  <img width="800" height="918" alt="ezgif-1a2cb90c93966869" src="https://github.com/user-attachments/assets/a6ad0691-8040-4afe-afac-edf1140b07a4" />
</p>

- A spell's killing blow throws the body harder (the **Spell Push** slider,
  twice as hard by default). A frost spell's kill freezes it instead: the body stays stiff in the pose it died
  in, topples over like a statue, and turns icy blue.
- **Pushable corpses.** Living players carry an invisible capsule that shoves ragdolls aside as
  you walk through them.
- **Loot bags.** Because a ragdoll no longer lies where the server thinks the corpse is, a lootable
  ragdoll drops a sack on that spot. Click the sack to loot. Big creatures drop a bigger sack, and
  the loot sparkle sits on the sack instead of the body.
- **Blood.** Hits spray droplets away from the attacker, which leave splats where they land. More
  damage means more spray, and killing blows spray the most. Ragdolls bleed a pool that spreads
  under them. The stock melee blood spurt, and the yellow hit flash in it, play at half size. The colours and art come from
  the game's own blood tables, so each creature bleeds its own kind of blood, and bloodless ones
  (elementals, for example) leave nothing.
- **Knockdowns.** When the game knocks a unit down (Warrior Charge's stun, Lash and others that
  play the Knockdown animation), it falls as a ragdoll and then gets back up.
- **Kick.** In a dev build, Ctrl+Shift+K kicks the creature you have targeted (within 5 yards):
  it flies, lies there a moment and gets up. This happens only on your screen; the server still
  has it standing, so a creature you are fighting keeps fighting.

- **Dismemberment.**

<p align="center">
  <img width="800" height="918" alt="ezgif-15b6cb2616fe2b38" src="https://github.com/user-attachments/assets/e8fcf710-4343-4944-ad31-39cf5adf8712" />
</p>

  With the option on, a killing blow that takes a fifth of the target's health
  severs a limb (the head included), and one that takes half severs two. The limb flies off.
- **Hitstop.** When a melee hit lands, the attacker's and the target's drawn poses hold for a
  moment (longer on a crit or a heavy hit), so blows feel weightier. Only the animation stops.
  Swings whose animation has no impact moment (many blunt weapons) still get it, a moment after
  the hit is reported.
- **Cloth cloaks.** Cloaks hang and swing as cloth: they trail behind when you run, sway when you
  stop, and stay outside the legs. With a weapon drawn they hang from the shoulders only, so
  the weapon swings clear. The ten nearest cloaks are simulated.
- **Options.** The options window has its own **Physics** page, under Audio: a **Blood** choice
  (Red, Green or Off, the game's own `violenceLevel`), a **Gore Amount** slider, a
  **Dismemberment** checkbox (off by default) with a **Dismemberment Amount** slider for how easily
  limbs come off, a **Hitstop** checkbox (on by default) with a **Hitstop Strength** slider, a
  **Death Push** slider for how hard a dying body is thrown, a **Death Lift** slider for how much
  it pops up, the **Spell Push** slider, a **Melee Death Delay** slider for how long a body killed by
  a melee swing waits before it falls (0.3 s by default, so the swing lands first; spells never
  wait), a **Cloak Physics** checkbox (on by default), a **Cloak Motion** slider for how much
  cloaks swing, and a **Loose Cloak When Armed** checkbox (on by default). The gore,
  dismemberment, push and lift sliders go up to 10x, and a high Dismemberment Amount takes off more
  than two limbs. A higher Gore Amount also keeps more blood on the ground at once and throws some
  of the spray faster.

## Compatibility

| With | Works? | Notes |
| --- | --- | --- |
| 1.12.1 servers (cMaNGOS, vmangos, others) | Yes | Nothing changes on the wire. Developed against cMaNGOS Classic with playerbots. |
| Playing with people on the stock client | Yes | They see normal corpses; only you see the physics. |
| 1.12 addons | Yes | Same addon support as benilla. |
| MPQ patches | Mostly | benilla reads your install's patch chain as the stock client does. Blood patch mods are not needed and may draw oddly (a flat splat in the air); use the Blood option instead. |
| Upstream benilla | Yes | The fork tracks upstream. The fork's code sits in its own module behind a `ragdoll` feature, so upstream changes merge in cleanly. It is never sent back upstream as a pull request. |
| `benilla-config/` | Yes | Shares upstream's settings folder. This fork adds its own settings (`violenceLevel`, `goreAmount`, `dismemberment`, `dismemberAmount`, `hitstop`, `hitstopStrength`, `ragdollPush`, `ragdollLift`, `ragdollSpellPush`, `ragdollMeleeDelay`, `cloakPhysics`, `cloakMotion`, `cloakLooseDrawn`), which upstream ignores. |
| Warden (anticheat) | No | As with upstream, use a server with Warden off. |

## Running it

You need the same things as upstream benilla:

- **An English 1.12.1 client (build 5875)** for the game data. benilla only reads it.
- **A 1.12.1 server with Warden off.** cMaNGOS, vmangos and the other 1.12.1 cores all work.
- **Stable Rust and a C compiler**, because the client's Lua is built from source: on macOS the
  Xcode command line tools, on Linux the ALSA and udev development packages and pkg-config, on
  Windows the MSVC build tools that the Rust installer sets up.

The fork's work lives on the `ragdoll` branch:

```sh
git clone -b ragdoll https://github.com/Oddity-git/benilla-physicsproject
cd benilla-physicsproject
WOW_DATA=/path/to/WoW/Data cargo run --release -p benilla
```

On Windows, in PowerShell:

```powershell
$env:WOW_DATA="C:\path\to\WoW\Data"; cargo run --release -p benilla
```

Ragdolls are on by default.

- `WOW_NO_RAGDOLL=1` turns them off for a session.
- `cargo build --release -p benilla --no-default-features --features dev` builds without the fork's
  physics at all.
- In a dev build, the dev chord plus `B` drops a test crate, to check that physics runs. Each drop
  uses the next crate model from your install.

The rest works as in upstream:

- `WOW_DATA` names the install's `Data` folder. A link to the install named `WoW` at the repo root
  does the same: `ln -s /path/to/WoW WoW`, or on Windows a junction, which needs no admin rights
  (`New-Item -ItemType Junction -Path WoW -Target C:\path\to\WoW`).
- The server defaults to `localhost:3724`. Point `WOW_HOST` at another
  (`WOW_HOST=localhost:3725`, or `play.example.com`), or set it from the Realmlist button on the
  login screen, which remembers it.
- `WOW_USER` and `WOW_PASS` skip the login screen.
- Settings, screenshots and addons live in `benilla-config/` at the repo root. An addon goes in
  `benilla-config/AddOns/`.

[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) is upstream's guide to the build and the tests.

## Where the code is

The fork's code is kept in a few places so it stays easy to merge with upstream:

- `crates/benilla-app/src/ragdoll/`
  - `life.rs`: starting, running and freezing ragdolls
  - `rig.rs`: picks the physics bodies from a skeleton
  - `blow.rs`: the last hit, for the death push and the spray
  - `gore.rs`: droplets, splats and pools
  - `lootbag.rs`: the sacks
  - `pusher.rs`: the player capsules
  - `bounds.rs`: keeps a flung ragdoll from being culled
  - `frost.rs`: the frozen body's ice tint
  - `hitstop.rs`: the hold on a landed hit
  - `cloak.rs`: the cloth cloaks
  - `testbox.rs`: the test crate
- `crates/benilla-app/assets/ui/PhysicsOptions.xml`: the Physics options page, which adds itself
  to the options window at load, so upstream's `OptionsFrame.xml` is untouched.
- `crates/benilla-world/src/collision.rs` and `world_plugins.rs` add the ragdoll collision layers
  and turn on avian's solver (the `dynamics` feature).
- Small hooks elsewhere are marked `Fork:` in their comments.

## About benilla

benilla plays the whole game: character creation, questing and professions, dungeons and raids,
battlegrounds and honor, groups, guilds, trade, mail and the auction house, on the stock 1.12
interface and with your addons. It connects to a 1.12.1 server over the original protocol and
reads the game's data from your own 1.12.1 install. Every file format, the network protocol and
the interface engine are written from scratch, with no original client code and no bundled game
assets. [`docs/MAP.md`](docs/MAP.md) maps every crate and subsystem.

For benilla itself (its issues, releases and community), see
[samwhosung/benilla](https://github.com/samwhosung/benilla) and its
[Discord](https://discord.gg/wJSJx467G4). Problems with ragdolls, blood or loot bags belong to
this fork, not upstream.

---

Credit for the client goes to benilla's author and contributors. Early inspiration and file
format guidance for benilla came from the [wowemulation-dev](https://github.com/wowemulation-dev)
community, and [warcraft-rs](https://github.com/wowemulation-dev/warcraft-rs) in particular.

This is an independent fan project, not affiliated with or endorsed by Blizzard Entertainment. It
ships **no Blizzard content**: no art, models, sounds, maps, MPQ contents or FrameXML. You provide
your own legally obtained 1.12.1 client. The few files under `crates/benilla-app/assets/ui/` are
benilla's own, not copies of the stock interface.

World of Warcraft is a trademark of Blizzard Entertainment, Inc. The code is licensed under
[MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option. The two vendored components
under `third_party/`, the kira audio engine and a Lua 5.1 patched to the 1.12 client's dialect,
keep their own upstream licenses, alongside each.
