# LinuxPal

A tiny pixel-art desktop mascot for **Hyprland / Wayland**. It lives on your screen as a
`wlr-layer-shell` overlay, reads what you're doing (coding, browsing, watching, listening),
roams across your monitors, and speaks tips + jokes from a local LLM.

<p align="center">
  <img src="assets/sprites/happy_sparks.png" width="120" alt="happy">
  <img src="assets/sprites/working_steam0.png" width="160" alt="working">
  <img src="assets/sprites/jamming_0.png" width="190" alt="jamming">
</p>

---

## Features

- **Context-aware moods** — picks a state from the active window (editor → working, browser →
  alert, media → jamming/cozy).
- **Dual-monitor priority** — on two screens the **HDMI** monitor decides state; no flicker when
  focus bounces between displays.
- **Free roaming** — wanders in 2D, hops monitors at shared edges (*"a doorway!"*), bumps outer
  walls (*"a wall here…"*).
- **Music & video aware** — real playback via MPRIS (`playerctl`): music → dances, plain video →
  cozy. Scoped to the deciding window so off-screen audio can't hijack the mood.
- **Local LLM** — speech bubble with a TIP + JOKE from Ollama; falls back to an offline bank if
  Ollama is down.
- **Ask anything** — `linuxpal-ctl ask "…"` streams a short answer into the bubble.
- **Control socket** — summon, ask, force a mood, or run a routine from a Hyprland keybind or
  script.
- **Morning routine** — one trigger launches your daily apps in order.
- **Robust** — startup greeting; survives a monitor power-off by re-pinning to a live screen.

---

## States

| State | Sprite | Trigger |
|-------|--------|---------|
| Idle | <img src="assets/sprites/idle_open.png" width="60"> | empty workspace |
| Working | <img src="assets/sprites/working_steam0.png" width="85"> | editor or terminal (nvim, code, kitty, …) |
| Alert | <img src="assets/sprites/alert.png" width="60"> | browser focused |
| Thinking | <img src="assets/sprites/thinking.png" width="60"> | file manager |
| Happy | <img src="assets/sprites/happy_sparks.png" width="60"> | build/tests passed · greeting |
| Jamming | <img src="assets/sprites/jamming_0.png" width="100"> | music playing |
| Cozy | <img src="assets/sprites/cozy_0.png" width="60"> | plain video · PDF/e-reader |
| Curious | <img src="assets/sprites/curious_0.png" width="60"> | idle on the desktop a while |
| WorkingEmpty | <img src="assets/sprites/working_empty.png" width="85"> | working too long ("more coffee?") |
| TrainingDone | <img src="assets/sprites/training_done_0.png" width="60"> | window title says a run finished |
| Walk | <img src="assets/sprites/walk_right_0.png" width="60"> | free-roam travel |

---

## Requirements

- **Hyprland** — window context via `hyprctl`
- `playerctl` — playback detection (`mpv-mpris` too if you want mpv on MPRIS)
- **Ollama** running locally — optional, for bubble tips
- Rust toolchain — to build

---

## Build & install

```sh
./install.sh
```

Builds release, installs `linuxpal` + `linuxpal-ctl` + `linuxpal-toggle` to `~/.local/bin`,
sprites to `~/.local/share/linuxpal/sprites/`, and adds the Hyprland autostart entry once. Re-run
after any change.

Run without installing:

```sh
LINUXPAL_ASSETS=assets/sprites cargo run --release
```

---

## Configuration

Edit `~/.config/linuxpal/config.toml` (written with defaults on first run), then restart. No
rebuild.

| Key | Meaning | Default |
|-----|---------|---------|
| `hdmi_match` | monitor substring that decides state on dual screens | `"hdmi"` |
| `model` | Ollama model for tips + ask | `"qwen2.5:1.5b"` |
| `greet_msg` | startup greeting text | — |
| `curious_after` | idle ticks before Curious | `150` |
| `walk_every` | stable ticks before a roam starts | `100` |
| `walk_duration` | non-music roam length | `120` |
| `walk_step` | px moved per tick | `8` |
| `park_duration` | dance-in-place ticks per spot during music | `30` |
| `coffee_after` | working ticks before "out of coffee" | `1800` |

Timings are in **ticks** — 10 ticks = 1 second.

Env: `LINUXPAL_ASSETS` (sprite dir), `RUST_LOG=info` (verbose logs).

---

## Controls

- **Tap** the mascot → action menu (morning, terminal, browser, ask, quit). Tap a row to run, tap
  away to dismiss.
- **Hold + drag** (~350ms, or drag past a few px) → reposition.
- **Ask** (menu → ask) → type in the bubble; **Enter** sends, **Esc** cancels. Keyboard grabbed
  only while typing.
- **Toggle** without a terminal → bind `linuxpal-toggle` (quits if running, launches if not), e.g.
  `bindd = $mainMod SHIFT, P, toggle LinuxPal, exec, $HOME/.local/bin/linuxpal-toggle`.

### Control socket

Listens on `$XDG_RUNTIME_DIR/linuxpal.sock`. Drive it with `linuxpal-ctl`:

```sh
linuxpal-ctl summon                          # pop up and wave
linuxpal-ctl ask "how do I list open ports?" # streamed LLM answer
linuxpal-ctl say "build done"                # one-off message
linuxpal-ctl morning                         # run the morning routine
linuxpal-ctl state jamming                   # force a mood briefly
linuxpal-ctl quit                            # stop it
```

Any socket client works too:

```sh
echo 'ask what is a tmpfs?' | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/linuxpal.sock"
```

Wire to Hyprland keybinds:

```ini
bind = SUPER, P, exec, linuxpal-ctl summon
bind = SUPER, M, exec, linuxpal-ctl morning
bind = SUPER, A, exec, linuxpal-ctl ask "$(fuzzel --dmenu --prompt 'ask> ')"
```

### Morning routine

`linuxpal-ctl morning` launches your daily apps in order via `hyprctl dispatch exec`. First run
writes a commented default to `~/.config/linuxpal/morning.toml`:

```toml
[[apps]]
cmd = "zen-browser"

[[apps]]
cmd = "kitty"
args = "-e tmux new-session -As main"

# optional per-app pause before the next launch
# [[apps]]
# cmd = "spotify"
# delay_ms = 2000
```

---

## How it works

```
hyprctl poll   (ipc.rs) ─┐
playerctl poll (player.rs)┤
                          ├─> resolve_state + priority ─> Animator ─> renderer ─> wl buffer
wayland outputs (main.rs)─┘                                   │
                                                     bubble (LLM tip/joke)
```

| Module | Role |
|--------|------|
| `ipc.rs` | polls Hyprland (~3 Hz) → `WindowContext` (class + title) |
| `player.rs` | polls MPRIS (~1 Hz) → music/video flags + track title |
| `context.rs` | maps a window to a `State`; `media_applies` scopes audio to the deciding window |
| `sprites.rs` | sprite loading + per-state frame timing (`Animator`) |
| `main.rs` | layer-shell surface, tick state machine, global-coord roaming, monitor hop, drag, render |
| `bubble.rs` / `renderer.rs` | speech bubble (bitmap font) + ARGB blitting |
| `llm.rs` | async Ollama queries (tips + streamed answers) over one bubble channel |
| `control.rs` | Unix control socket → `ControlEvent`s; single entry for external triggers |
| `morning.rs` | reads `morning.toml`, launches daily apps |

---

## Notes & limits

- Layer-shell surfaces are per-output, so a monitor hop is a *jump* at the seam, not a smooth
  straddle.
- A browser is one MPRIS player across all tabs, so media detection matches the foreground tab
  title only.
</content>
</invoke>
