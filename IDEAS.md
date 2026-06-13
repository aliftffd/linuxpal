# LinuxPal — Assistant Roadmap

Ideas to grow LinuxPal from a reactive mascot into a real desktop assistant. Ordered roughly by
effort and dependency. Each item notes *what*, *why*, and *where it touches the code*.

---

## Phase 1 — Make it talk back (interaction)

Right now LinuxPal is one-way: it reacts, you watch. An assistant needs a way in.

- **Click-to-ask** — left-click already drags; add a click (or a hotkey) that opens a small input
  so you can type a question; answer streams into the bubble.
  *Where:* `main.rs` pointer handler + a text-input layer surface; reuse `llm.rs`.
  *Status:* ◐ asking works now via the control socket (`linuxpal-ctl ask "…"`, e.g. bound to a
  `fuzzel` prompt) and the answer **streams** into the bubble. In-surface typing (Wayland keyboard
  + text layer) is still TODO.
- **Global hotkey summon** — Hyprland `bind` → IPC to LinuxPal (`SIGUSR1` or a unix socket) to
  pop it up and focus the input from anywhere.
  *Where:* new control socket in `ipc.rs`; `bind` line in Hyprland config.
  *Status:* ✅ done — `control.rs` binds `$XDG_RUNTIME_DIR/linuxpal.sock`; `linuxpal-ctl summon`
  (or any socket client) pops it up. See README → *Talk to it*.
- **Conversation, not one-shot** — keep a short message history so follow-ups have context.
  *Where:* `llm.rs` (message array instead of single prompt).
- **Bigger bubble / scrollable** — current bubble is 2 lines. Add a resizable panel for real
  answers, code snippets.
  *Where:* `bubble.rs` (layout) — consider switching text rendering to a real font crate
  (`fontdue`/`cosmic-text`) instead of the 5×7 bitmap.

---

## Phase 2 — Give it senses (system awareness)

An assistant should know your machine's state, not just the focused window.

- **Resource watch** — CPU/RAM/battery/temp. React: low battery → worried state + "plug me in";
  high CPU → "something's churning". Proactive nudge.
  *Where:* new `system.rs` poller (read `/proc`, `/sys/class/power_supply`).
- **Time / calendar awareness** — greet by time of day; remind of events. Optional `khal` or
  `.ics` read.
- **Notification bridge** — listen to D-Bus `org.freedesktop.Notifications`, let LinuxPal mirror
  or summarize notifications in-character.
- **Idle / away detection** — real input-idle via the `ext-idle-notify` Wayland protocol (would
  finally make `Curious` accurate and enable "welcome back").

---

## Phase 2.5 — Two-model LLM system (local AI assistant)

> Hardware constraint: RTX 3050 Mobile 4GB VRAM. Never load both models simultaneously.
> During active PyTorch training (3–3.5GB VRAM used), disable phi4-mini entirely — pet runs
> display-only mode.

*Offline fallback:* ◐ done — when Ollama is unreachable (off / GPU busy training), the
bubble shows a curated per-state tip + joke (`sprites::offline_tip` / `offline_joke`)
instead of going blank. Two-model swap + intent routing still TODO.

### Two-model architecture

```
SIMPLE — tips, jokes, state-triggered, intent classify
        ↓
qwen2.5:1.5b  ~750ms  ~1.2GB VRAM  always loaded

COMPLEX — paper extract, email, planning, debugging
        ↓
phi4-mini  ~2-3s  ~2.5GB VRAM  loaded on demand, evicted after task
```

### VRAM budget

| Process | VRAM |
|---|---|
| Display (Hyprland) | ~200MB |
| qwen2.5:1.5b | ~1200MB |
| phi4-mini | ~2500MB |
| PyTorch training (active) | 3000–3500MB |

- qwen + display = ~1400MB — safe always
- phi4-mini + display = ~2700MB — safe when not training
- Training active → evict everything, pet falls back to static tips/jokes

### Model swap flow

When a complex task is requested:
1. Bubble shows "switching to phi4-mini..."
2. POST `/api/generate` with `keep_alive=0` → evicts qwen from VRAM
3. POST `/api/generate` with `keep_alive=-1` → loads phi4-mini
4. phi4-mini handles the task, streams response to bubble
5. Task complete → evict phi4-mini, reload qwen silently in background

*Where:* `llm.rs` — add `ModelState` enum, `evict_model()`, `load_model()`,
`query_complex()` alongside existing `query_async()`.

### Intent routing via qwen

qwen classifies every user input before routing. It never executes — only classifies.
A Rust handler always does the actual execution.

```
user types input
        ↓
qwen: INTENT: <class>  PARAM: <key value>
        ↓
simple intent  →  shell command, done (<100ms)
complex intent →  model swap + phi4-mini
```

Supported intents:
`OPEN_APP`, `OPEN_TERMINAL`, `TMUX_SESSION`, `WALLPAPER`,
`EMAIL`, `EXPLAIN`, `SCHEDULE`, `SUMMARIZE_PDF`, `DEBUG`, `UNKNOWN`

### Complex tasks (phi4-mini)

- **PDF paper extract** — user provides path → `markitdown` converts PDF to
  markdown → phi4-mini extracts **Problem / Method / Result / Research Gap**
  in structured format → bubble shows summary.
  *Why:* reading papers is a daily activity; structured extraction beats manual
  skimming for a quick orientation pass.
  *Where:* new `papers.rs` — runs `markitdown` via `std::process::Command`,
  passes markdown to phi4-mini with extraction prompt.

- **Error explanation** — paste a Rust compiler error, Python traceback, or
  CUDA OOM message → phi4-mini explains what went wrong and suggests a fix.
  *Where:* `llm.rs` complex query with error-specific system prompt.

- **Email drafting** — describe the email in plain language → phi4-mini writes
  a professional draft → result copied to clipboard. User sends manually.
  *Note:* pet never sends email directly — no SMTP credentials stored.
  *Where:* `llm.rs` + clipboard write via `wl-copy`.

- **Work / study planning** — describe what you need to get done today →
  phi4-mini structures a plan → bubble shows it.
  *Where:* `llm.rs` complex query with scheduling prompt.

- **Deep learning debugging** — describe a training issue (loss not converging,
  OOM, wrong output shape) → phi4-mini suggests diagnosis and fixes.
  *Where:* `llm.rs` complex query with DL-specific system prompt.

### Fine-tuning consideration

Do NOT fine-tune immediately. Collect training data naturally first:
- Every time the model gives a wrong/bad answer, save `{prompt, ideal_response}`
  to a local JSONL file
- After 500–1000 examples (~2–3 months of daily use), fine-tune with
  **Unsloth QLoRA** — fits in 4GB VRAM, ~20–30min training time
- Priority domains for fine-tuning: AMC/wireless thesis domain + your Rust/PyTorch stack

---

## Phase 3 — Make it useful (actions)

Move from talking to doing.

- **Morning routine** — one-click launch sequence for daily startup. No LLM needed.
  Default sequence: Zen Browser → Kitty + tmux (Spotify / xfreerdp commented, opt in).
  Configurable via `~/.config/linuxpal/morning.toml` with per-app delays.
  *Where:* new `morning.rs` — reads TOML config, spawns apps via
  `hyprctl dispatch exec` with configurable delays between launches.
  *Status:* ✅ done — `morning.rs` + `linuxpal-ctl morning`. Writes a default config on first run;
  progress shows in the bubble. (Minimal in-house parser for the `[[apps]]` subset — no TOML dep.)

  ```toml
  # ~/.config/linuxpal/morning.toml
  [[apps]]
  cmd = "zen-browser"

  [[apps]]
  cmd = "kitty"
  args = "-e tmux new-session -As main"

  [[apps]]
  cmd = "spotify"

  [[apps]]
  cmd = "xfreerdp"
  args = "/v:YOUR_RDP_TARGET /u:YOUR_USER +clipboard"
  delay_ms = 2000
  ```

- **Quick action menu** — click the mascot → pixel-art panel with two columns.
  *Where:* new layer surface in `main.rs`; action dispatch in `actions.rs`.

  *Simple column (shell commands, instant):*
  - Morning routine (launch all configured apps)
  - Open Zen Browser
  - Open terminal + tmux session picker
  - Open Spotify
  - Open WhatsApp Web (`zen-browser https://web.whatsapp.com`)
  - Change wallpaper (`swww img`)
  - xfreerdp connect

  *Smart column (phi4-mini, shows input box first):*
  - Summarize paper (PDF path → markitdown → phi4-mini)
  - Draft email (describe → phi4-mini → clipboard)
  - Plan my day (describe → phi4-mini → bubble)
  - Debug error (paste → phi4-mini → explanation)
  - Explain concept (paste → phi4-mini → bubble)

- **Tool calling** — let the LLM run whitelisted shell tools (open app, search
  files with `rg`, `hyprctl dispatch`, set a timer). Confirm-before-run for
  anything destructive.
  *Where:* `llm.rs` (tool schema) + a sandboxed executor module.
- **Clipboard helper** — watch clipboard, offer "explain / translate / summarize
  this" actions.
- **Timers & reminders** — "remind me in 20 min" → LinuxPal walks over and nags.
- **Pomodoro / focus coach** — ties into `WorkingEmpty` ("you've coded 50 min,
  break?").

---

## Phase 4 — Give it memory (personalization)

- **Persistent memory** — small local store (SQLite or JSON) of facts,
  preferences, recurring tasks. "What was that command you showed me yesterday?"
- **Habit learning** — log which apps/states dominate; surface gentle insights
  ("you context-switch a lot in the afternoon").
- **Profiles** — work vs gaming vs chill; different state mappings + personality
  per profile.

---

## Phase 5 — More life (polish & character)

> Text-only by design — no TTS/voice. All interaction stays visual (bubble + input).

- **Reactions to events** — git commit success → cheer; failed build → concerned;
  long download done → celebrate.
- **Richer animation** — emote on demand, eye-tracking toward the cursor, idle
  micro-animations.
- **Theming** — palette swaps, alternate sprite packs, day/night skins.
- **Config file** — move the `const`s into `~/.config/linuxpal/config.toml`
  (states, timings, personality, HDMI name) so no rebuild needed.
  *Status:* ✅ done — `config.rs` loads `hdmi_match`, `model`, `greet_msg`, and the
  roam/idle/coffee timings; writes a commented default on first run. (App-class lists
  still in code — could move to config later.)

---

## Architecture changes this implies

- **Split the brain from the body.** Today state logic lives in `main.rs`'s tick
  loop. As actions + memory + tools grow, extract an `assistant`/`agent` module
  that owns intent, and keep `main.rs` for Wayland + rendering only.
- **Event bus.** Pollers (window, player, system, notifications) and inputs
  (click, hotkey) should feed a single event channel the agent consumes —
  cleaner than more flags on the struct.
  *Status:* ◐ seeded — control inputs flow through `control_rx`, and all async
  bubble producers (tips, streamed answers, morning status) share one
  `bubble_tx`/`bubble_rx` channel drained each tick. Pollers still use their own threads.
- **Async runtime.** `tokio` is already a dependency; lean on it for the LLM
  streaming, tool execution, and pollers instead of manual threads.
- **Stream LLM output.** Switch Ollama calls to streaming so the bubble fills
  in live.
  *Status:* ◐ done for `ask` (`stream_ask` reads Ollama's NDJSON and pushes growing
  text to the bubble). Ambient tips are still one-shot.

---

## Suggested next step

Smallest high-value slice: **Phase 1 click-to-ask + Phase 3 morning routine**.
Morning routine needs zero LLM and is immediately useful every day.
Click-to-ask makes it feel like a real assistant. Build the control socket first;
everything else (hotkey, action menu, notifications) reuses it.

### Done (first slice)

The control socket is built (`control.rs` + `linuxpal-ctl`), plus the morning routine
(`morning.rs`) and streamed `ask`. The "build the control socket first" plan worked — the
remaining items below now have a clean entry point to hook into.

**Next, in reuse order:**
1. **Quick action menu** (Phase 3) — a layer-surface panel whose buttons just send the existing
   control commands (`summon`, `morning`, `ask`, …). No new backend.
2. **In-surface click-to-ask** — Wayland keyboard + text layer so you can type at the pet instead
   of via a launcher prompt (the streaming answer path already exists).
3. **Notification bridge** (Phase 2) — D-Bus listener feeding the same `bubble_tx`.
4. **Two-model swap** (Phase 2.5) — route `ask` to `phi4-mini` for complex intents inside
   `query_ask`; the bubble/stream plumbing is ready.
