mod bubble;
mod config;
mod context;
mod control;
mod ipc;
mod llm;
mod menu;
mod morning;
mod player;
mod renderer;
mod sprites;

use std::os::unix::io::AsFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::{self, ExtIdleNotifierV1},
};

use ipc::WindowContext;
use sprites::{Animator, Direction, Sprites, State};

const SPRITE_W: u32 = 520;  // bubble 258 + gap 14 + 230 widest mascot (jamming) + left margin
const SPRITE_H: u32 = 165;  // jamming is tallest at 155
const MARGIN: i32 = 16;
const TICK_HZ: u64 = 10;

// behavior timers (CURIOUS_AFTER / WALK_EVERY / WALK_DURATION / WALK_STEP /
// PARK_DURATION / COFFEE_AFTER) now live in config.toml — see config.rs.

// free-roam bounds when no output geometry is known yet
const FALLBACK_W: i32 = 1920;
const FALLBACK_H: i32 = 1080;

const WALL_PAD: i32 = 250; // let targets overshoot bounds so it bumps walls
const GREET_TICKS: u32 = 80; // 8s welcome on launch (greet text → config.greet_msg)
// representative mascot width (sprites are 120–230); the mascot blits flush-right
// in the SPRITE_W surface, so its visual centre is offset from the surface centre.
const MASCOT_W: i32 = 188;
const STATE_DEBOUNCE: u32 = 5; // ticks a base state must hold before committing (anti-flap)
const FORCE_TICKS: u32 = 80;   // hold a summoned / forced state ~8s before the state machine resumes
// click model: a quick tap toggles the menu; a long press (or a clear drag)
// moves the pet. Distinguishes "tap to act" from "hold to drag".
const LONG_PRESS: Duration = Duration::from_millis(350);
const DRAG_SLOP: f64 = 36.0; // px of motion that promotes a press to a drag immediately
const MENU_GAP: usize = 6; // px between the action menu's right edge and the mascot
const CROSS_MSG: &str = "a doorway! crossing to the next screen";
const WALL_MSG: &str = "a wall here - cant cross this one";
const WELCOME_MSG: &str = "welcome back! ready when you are"; // on real input resume
const WALL_COOLDOWN: u32 = 30;  // min ticks between WALL_MSG (~3s @ 10Hz) — anti-spam
const WALL_SLIDE_MAX: u32 = 20; // slide along a wall this long before repicking a target

/// One output's place in the global layout (logical coords).
struct OutputGeom {
    output: wl_output::WlOutput,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// An in-progress left-button press, tracked to tell a tap from a drag.
struct Press {
    start: (f64, f64), // where the press began (surface-local)
    last: (f64, f64),  // last motion position, for incremental drag delta
    time: Instant,     // when it began, for the long-press threshold
    dragging: bool,    // promoted to a move once held long / moved far enough
}

struct LinuxPal {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    seat_state: SeatState,

    cfg: config::Config,

    layer_surface: Option<LayerSurface>,
    pool: Option<SlotPool>,
    buffer: Option<Buffer>,
    pointer: Option<wl_pointer::WlPointer>,

    sprites: Sprites,
    animator: Animator,
    bubble: bubble::Bubble,
    // persistent async bubble channel — drained every tick. Ambient tips,
    // streamed answers, and morning-routine status all post here (event-bus seed).
    bubble_tx: std::sync::mpsc::Sender<llm::BubbleUpdate>,
    bubble_rx: std::sync::mpsc::Receiver<llm::BubbleUpdate>,
    // commands from the control socket (summon / ask / morning / say / state / quit)
    control_rx: std::sync::mpsc::Receiver<control::ControlEvent>,
    win_ctx: Arc<Mutex<WindowContext>>,
    player_state: Arc<Mutex<player::PlayerState>>,

    configured: bool,
    width: u32,
    height: u32,

    // pointer: current press (tap vs drag) + last cursor pos for menu hover
    press: Option<Press>,
    pointer_pos: (f64, f64),
    lean_px: i32, // eased horizontal lean toward the cursor (alive reaction)
    menu_open: bool,
    // keyboard: in-surface "ask" typing. Modal Exclusive focus only while typing,
    // so normal taps/drags never steal keys from the rest of the desktop.
    keyboard: Option<wl_keyboard::WlKeyboard>,
    input_mode: bool,
    input_buf: String,
    margin_left: i32,
    margin_top: i32,

    // behavior timers — drive curious / working_empty / walk
    idle_ticks: u64,
    working_ticks: u64,
    stable_ticks: u64,
    last_base: Option<State>,
    // live main-screen mood + context string, refreshed every tick; the roam
    // pause poses as `roam_mood` and generates its tip/joke for `last_ctx`
    roam_mood: State,
    last_ctx: String,
    roaming: bool,
    walk_ticks_left: u32,
    park_ticks_left: u32,
    walk_dir: Direction,
    // wall handling during roam: throttle the WALL_MSG and bound how long the
    // sprite slides along an edge before giving up on the current target
    wall_cooldown: u32,
    wall_ticks: u32,

    // real input-idle (ext-idle-notify-v1): notifier global + the armed
    // notification, plus away/return flags driven by its idled/resumed events
    idle_notifier: Option<ExtIdleNotifierV1>,
    idle_notification: Option<ExtIdleNotificationV1>,
    user_idle: bool,
    idle_returned: bool,

    // startup greeting
    greet_ticks_left: u32,
    greeted: bool,

    // forced state from a control command (summon / state) — held briefly
    // so the window-driven state machine doesn't immediately override it
    force_state: Option<State>,
    force_ticks_left: u32,

    // anti-flap debounce for base-state transitions
    pending_state: Option<State>,
    pending_count: u32,

    // multi-output free roam: layout + global sprite position/target + PRNG
    outputs: Vec<OutputGeom>,
    cur_idx: usize,
    pos_init: bool,
    pos_x: i32,
    pos_y: i32,
    target_x: i32,
    target_y: i32,
    rng: u64,
}

impl LinuxPal {
    fn draw(&mut self, _qh: &QueueHandle<Self>) {
        let surface = match &self.layer_surface {
            Some(s) => s,
            None => return,
        };
        let pool = match &mut self.pool {
            Some(p) => p,
            None => return,
        };

        let w = self.width;
        let h = self.height;
        let stride = w as usize * 4;

        let (buffer, canvas) = pool
            .create_buffer(w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888)
            .expect("failed to create shm buffer");

        renderer::clear(canvas);

        let frame_name = self.animator.current_frame();
        let frame = self.sprites.get(frame_name);
        // blit mascot flush-right so variable-width sprites stay anchored to screen corner
        let x_offset = (w.saturating_sub(frame.width)) as usize;

        if self.menu_open {
            let ox = menu_origin_x();
            let hover = menu::hit_row(ox, self.pointer_pos.0, self.pointer_pos.1);
            menu::draw(canvas, stride, ox, hover);
        } else if self.input_mode {
            // small bubble showing the question as it's typed (cursor = "_")
            let text = format!("ask> {}_", self.input_buf);
            bubble::draw_bubble(canvas, stride, true, &text, "", x_offset);
        } else if self.bubble.visible {
            bubble::draw_bubble(
                canvas,
                stride,
                self.bubble.plain,
                &self.bubble.tip.clone(),
                &self.bubble.joke.clone(),
                x_offset,
            );
        }
        self.bubble.tick();

        // lean toward the cursor when it's over our surface and the pet is
        // otherwise still — a subtle "I notice you" reaction. Eased 1px/frame
        // so it glides. Applied only to the mascot blit (menu/bubble above use
        // the static x_offset so their hit-tests stay valid).
        // "still" = neutral pose only. Greeting/forced (welcome-back, summon,
        // set-state) poses are exclusive too — don't let the lean leak in.
        // (Thinking is gated by menu_open/input_mode already.)
        let still = !self.menu_open
            && !self.input_mode
            && !self.roaming
            && self.greet_ticks_left == 0
            && self.force_ticks_left == 0;
        let mascot_cx = x_offset as f64 + frame.width as f64 / 2.0;
        let dx = self.pointer_pos.0 - mascot_cx;
        let near = still
            && self.cfg.lean_max > 0
            && self.pointer_pos.1 >= 0.0
            && self.pointer_pos.1 <= h as f64
            && dx.abs() < self.cfg.lean_radius as f64;
        let target = if near { (dx.signum() as i32) * self.cfg.lean_max } else { 0 };
        self.lean_px += (target - self.lean_px).clamp(-1, 1);
        // clamp into the surface so a rightward lean can't push the flush-right
        // sprite past the right edge and clip it
        let max_off = w.saturating_sub(frame.width) as i32;
        let x_offset = (x_offset as i32 + self.lean_px).clamp(0, max_off.max(0)) as usize;

        renderer::blit_frame(canvas, w, h, stride, frame, x_offset);

        surface.wl_surface().attach(Some(buffer.wl_buffer()), 0, 0);
        surface.wl_surface().damage_buffer(0, 0, w as i32, h as i32);
        surface.wl_surface().commit();

        self.buffer = Some(buffer);
    }

    fn sync_state(&mut self, qh: &QueueHandle<Self>) {
        // drain async bubble updates: ambient tips, streamed answers, status lines
        while let Ok(update) = self.bubble_rx.try_recv() {
            match update {
                llm::BubbleUpdate::TipJoke { tip, joke } => self.bubble.update(&tip, &joke),
                llm::BubbleUpdate::Plain(text) => self.bubble.show_answer(&text),
            }
        }

        // drain control-socket commands (summon / ask / morning / say / state / quit)
        while let Ok(ev) = self.control_rx.try_recv() {
            self.handle_control(ev);
        }

        // arm the real input-idle notification once the seat + notifier exist
        // (seat arrives async). Drives accurate away/Curious + welcome-back.
        if self.idle_notification.is_none() {
            if let Some(n) = self.idle_notifier.clone() {
                if let Some(seat) = self.seat_state.seats().next() {
                    let ms = self.cfg.idle_secs.saturating_mul(1000);
                    self.idle_notification = Some(n.get_idle_notification(ms, &seat, qh, ()));
                    log::info!("idle notification armed ({}s)", self.cfg.idle_secs);
                }
            }
        }

        // a still hold (no motion to trigger it) becomes a drag after the threshold
        if let Some(p) = self.press.as_mut() {
            if !p.dragging && p.time.elapsed() >= LONG_PRESS {
                p.dragging = true;
            }
        }

        // clicking the pet (menu open) or typing a question → show the
        // "thinking" pose and freeze the window-driven state machine (no roam,
        // no LLM churn) until it's dismissed
        if self.menu_open || self.input_mode {
            if self.menu_open {
                self.bubble.visible = false; // panel replaces the bubble
            }
            if self.animator.state != State::Thinking {
                self.animator.state = State::Thinking;
                self.animator.reset();
            }
            return;
        }

        // startup greeting — cheer with happy state for a few seconds
        if self.greet_ticks_left > 0 {
            if !self.greeted {
                let greet = self.cfg.greet_msg.clone();
                self.bubble.say(&greet);
                self.animator.state = State::Happy;
                self.animator.reset();
                self.greeted = true;
                log::info!("greeting");
            }
            self.greet_ticks_left -= 1;
            return;
        }

        // real input resumed after being away (ext-idle-notify) → welcome back,
        // cheer briefly via the same forced-state path a control command uses
        if self.idle_returned {
            self.idle_returned = false;
            // don't clobber an in-flight forced command (summon/state) that
            // landed in the same window — let it finish its hold first
            if self.force_ticks_left == 0 {
                self.bubble.say(WELCOME_MSG);
                self.force(State::Happy);
                log::info!("welcome back");
            }
        }

        // a control command forced a state — hold it briefly, suppressing the
        // window-driven state machine, then let normal behavior resume
        if self.force_ticks_left > 0 {
            if let Some(s) = self.force_state.clone() {
                if self.animator.state != s {
                    self.animator.state = s;
                    self.animator.reset();
                }
            }
            self.force_ticks_left -= 1;
            return;
        }

        // snapshot what MPRIS is playing
        let (pmusic, pvideo, ptitle) = match self.player_state.lock() {
            Ok(p) => (p.music, p.video, p.title.clone()),
            Err(_) => (false, false, String::new()),
        };

        // read window context → base state + prompt string + media scope.
        // media applies only if the playing track matches the deciding window.
        let (base, context_str, media_ok) = match self.win_ctx.lock() {
            Ok(ctx) => {
                let s = context::resolve_state(&ctx);
                let cs = if ctx.title.is_empty() {
                    ctx.class.clone()
                } else {
                    format!("{} - {}", ctx.class, ctx.title)
                };
                (s, cs, context::media_applies(&ctx, &ptitle))
            }
            Err(_) => return,
        };

        // gate BEFORE the roam check so a video on another screen can't make it dance
        let music = pmusic && media_ok;
        let video = pvideo && media_ok;

        // advance behavior timers off the MAIN-SCREEN base state EVERY tick (even
        // while roaming) so the mood always reflects what's open right now.
        match base {
            State::Idle => {
                self.idle_ticks = self.idle_ticks.saturating_add(1);
                self.working_ticks = 0;
            }
            State::Working => {
                self.working_ticks = self.working_ticks.saturating_add(1);
                self.idle_ticks = 0;
            }
            _ => {
                self.idle_ticks = 0;
                self.working_ticks = 0;
            }
        }
        if self.last_base.as_ref() == Some(&base) {
            self.stable_ticks = self.stable_ticks.saturating_add(1);
        } else {
            self.stable_ticks = 0;
            self.last_base = Some(base.clone());
        }

        // current mood from the main screen (HDMI-decided deciding window):
        // Jamming/Cozy/WorkingEmpty/Curious/base. This is the pose the pet wears
        // whenever it pauses, and the context the tip/joke is generated for.
        // Roam is travel BETWEEN moods — it never replaces them.
        let mood = self.mood_state(&base, music, video);
        self.roam_mood = mood.clone();
        self.last_ctx = context_str.clone();

        // roam in progress — walk between spots; each pause poses as `mood`
        if self.roaming {
            self.advance_walk(music, qh);
            return;
        }

        // companion mode: always be roaming, every situation, all screens. The
        // mood (above) still drives every pause pose + tip/joke; roam just carries
        // the pet around between them. Reached only when not already roaming.
        if self.cfg.always_roam {
            let s = self.begin_roam(music);
            self.animator.state = s;
            self.animator.reset();
            return;
        }

        // non-companion: stroll only after a stable window, else hold the mood.
        let roam_trigger =
            self.stable_ticks >= self.cfg.walk_every && self.stable_ticks % self.cfg.walk_every == 0;
        let idle_or_work = matches!(base, State::Idle | State::Working);
        let effective = if roam_trigger && (music || idle_or_work) {
            self.begin_roam(music)
        } else {
            mood
        };

        if effective != self.animator.state {
            // roam / media apply instantly; base+mood states must hold STATE_DEBOUNCE
            // ticks first, so window-recency flapping doesn't churn state + LLM calls
            let immediate = matches!(
                effective,
                State::Walk(_) | State::Jamming | State::Cozy
            );
            let ready = if immediate {
                true
            } else if self.pending_state.as_ref() == Some(&effective) {
                self.pending_count += 1;
                self.pending_count >= STATE_DEBOUNCE
            } else {
                self.pending_state = Some(effective.clone());
                self.pending_count = 1;
                false
            };

            if ready {
                log::info!("state → {:?}", effective);
                if !matches!(effective, State::Walk(_)) {
                    self.bubble.show_loading();
                    llm::query_async(context_str, effective.clone(), self.bubble_tx.clone());
                }
                self.animator.state = effective;
                self.animator.reset();
                self.pending_state = None;
                self.pending_count = 0;
            }
        } else {
            self.pending_state = None;
            self.pending_count = 0;
        }
    }

    /// Act on a command from the control socket.
    /// Force a state for FORCE_TICKS, suppressing the window-driven machine.
    /// Single writer for the force channel (welcome-back, summon, set-state).
    fn force(&mut self, s: State) {
        self.force_state = Some(s);
        self.force_ticks_left = FORCE_TICKS;
    }

    fn handle_control(&mut self, ev: control::ControlEvent) {
        use control::ControlEvent::*;
        match ev {
            Summon => {
                self.bubble.say("hi! ask me:  linuxpal-ctl ask \"...\"");
                self.force(State::Happy);
                self.greet_ticks_left = 0; // don't let a pending greet stomp this
            }
            Say(msg) => self.bubble.say(&msg),
            Ask(question) => {
                log::info!("control: ask {question:?}");
                self.bubble.show_loading();
                llm::query_ask(question, self.bubble_tx.clone());
            }
            Morning => {
                log::info!("control: morning routine");
                morning::run(self.bubble_tx.clone());
            }
            SetState(name) => match parse_state(&name) {
                Some(s) => self.force(s),
                None => self.bubble.say(&format!("unknown state: {name}")),
            },
            Quit => {
                log::info!("control: quit");
                std::process::exit(0);
            }
        }
    }

    /// Move the surface by a cursor delta (anchored TOP|LEFT → grow margins).
    fn drag_by(&mut self, dx: f64, dy: f64) {
        self.margin_left = (self.margin_left + dx as i32).max(0);
        self.margin_top = (self.margin_top + dy as i32).max(0);
        if let Some(g) = self.outputs.get(self.cur_idx) {
            self.pos_x = g.x + self.margin_left;
            self.pos_y = g.y + self.margin_top;
        }
        if let Some(surface) = &self.layer_surface {
            surface.set_margin(self.margin_top, 0, 0, self.margin_left);
            surface.commit();
        }
    }

    /// A quick tap: open the menu, or pick a row if it's already open.
    fn handle_tap(&mut self, pos: (f64, f64)) {
        if self.input_mode {
            self.cancel_input(); // tapping the pet while typing backs out
            return;
        }
        if self.menu_open {
            let ox = menu_origin_x();
            if let Some(action) = menu::hit_row(ox, pos.0, pos.1).and_then(menu::action) {
                self.do_menu_action(action);
            }
            self.menu_open = false; // any tap dismisses the menu
        } else {
            self.menu_open = true;
            self.bubble.visible = false; // menu replaces the bubble while open
        }
    }

    /// Run the action behind a menu row — reuses the same plumbing as the socket.
    fn do_menu_action(&mut self, action: menu::MenuAction) {
        use menu::MenuAction::*;
        log::info!("menu action: {action:?}");
        match action {
            Morning => morning::run(self.bubble_tx.clone()),
            Terminal => spawn_exec("kitty"),
            Browser => spawn_exec("zen-browser"),
            Ask => self.begin_input(),
            Quit => std::process::exit(0),
        }
    }

    /// Grab (or release) modal keyboard focus for in-surface typing.
    fn set_keyboard(&self, on: bool) {
        if let Some(s) = &self.layer_surface {
            s.set_keyboard_interactivity(if on {
                KeyboardInteractivity::Exclusive
            } else {
                KeyboardInteractivity::None
            });
            s.commit();
        }
    }

    /// Enter "ask" input mode: clear the buffer and grab the keyboard.
    fn begin_input(&mut self) {
        self.input_mode = true;
        self.input_buf.clear();
        self.menu_open = false;
        self.set_keyboard(true);
    }

    /// Abandon input without asking.
    fn cancel_input(&mut self) {
        self.input_mode = false;
        self.input_buf.clear();
        self.set_keyboard(false);
    }

    /// Submit the typed question: release the keyboard and stream the answer.
    fn submit_input(&mut self) {
        let question = self.input_buf.trim().to_string();
        self.input_mode = false;
        self.input_buf.clear();
        self.set_keyboard(false);
        if !question.is_empty() {
            log::info!("ask (in-surface): {question:?}");
            self.bubble.show_loading();
            llm::query_ask(question, self.bubble_tx.clone());
        }
    }

    /// (Re)build the global output layout from sctk's output info.
    fn rebuild_outputs(&mut self) {
        let mut v = Vec::new();
        for o in self.output_state.outputs() {
            if let Some(info) = self.output_state.info(&o) {
                let size = info.logical_size.or_else(|| {
                    info.modes.iter().find(|m| m.current).map(|m| m.dimensions)
                });
                if let (Some((x, y)), Some((w, h))) = (info.logical_position, size) {
                    if w > 0 && h > 0 {
                        v.push(OutputGeom { output: o, x, y, w, h });
                    }
                }
            }
        }
        if v.is_empty() {
            return;
        }
        v.sort_by_key(|g| (g.x, g.y));
        self.outputs = v;
        if self.cur_idx >= self.outputs.len() {
            self.cur_idx = 0;
        }
        if !self.pos_init {
            let g = &self.outputs[self.cur_idx];
            self.pos_x = g.x + MARGIN;
            self.pos_y = g.y + MARGIN;
            self.pos_init = true;
        }
        log::info!("outputs: {} detected", self.outputs.len());
    }

    /// xorshift64 — cheap PRNG, no external crate.
    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// Index of the output whose rect contains the global point, if any.
    fn output_at(&self, x: i32, y: i32) -> Option<usize> {
        self.outputs
            .iter()
            .position(|g| x >= g.x && x < g.x + g.w && y >= g.y && y < g.y + g.h)
    }

    fn rand_range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.rand() % (hi - lo + 1) as u64) as i32
    }

    /// Pick a random global target inside a real output — never a dead zone in
    /// the union box (areas inside the bounding box no monitor covers, common
    /// when screens differ in size/offset). x overshoots a chosen output's
    /// left/right edge by WALL_PAD so it still bumps outer side walls and heads
    /// toward a neighbor's overlap band; y stays strictly inside that output so
    /// it never walks at a row no monitor covers.
    fn pick_walk_target(&mut self) {
        if self.outputs.is_empty() {
            self.target_x = self.rand_range(MARGIN, FALLBACK_W - SPRITE_W as i32);
            self.target_y = self.rand_range(MARGIN, FALLBACK_H - SPRITE_H as i32);
            return;
        }
        // global reachable x-range so the whole sprite stays on real screen.
        // Overshoot (WALL_PAD) toward an interior seam still drives crossings,
        // but it's clamped at the OUTER edges so the pet can't aim past them
        // and bonk the outermost monitor edge repeatedly.
        let min_x = self.outputs.iter().map(|o| o.x).min().unwrap_or(0);
        let max_x = self
            .outputs
            .iter()
            .map(|o| o.x + o.w)
            .max()
            .unwrap_or(FALLBACK_W);
        let i = (self.rand() as usize) % self.outputs.len();
        let g = &self.outputs[i];
        let (gx, gy, gw, gh) = (g.x, g.y, g.w, g.h);
        let lo = (gx - WALL_PAD).max(min_x);
        let hi = (gx + (gw - SPRITE_W as i32) + WALL_PAD).min(max_x - SPRITE_W as i32);
        self.target_x = self.rand_range(lo, hi.max(lo));
        let yhi = gy + (gh - SPRITE_H as i32);
        self.target_y = if yhi >= gy {
            self.rand_range(gy, yhi)
        } else {
            // output shorter than the sprite — aim the center onto it, else the
            // sprite center lands off-screen and output_at rejects every step
            // into a perpetual wall-slide for the whole leg
            gy + gh / 2 - SPRITE_H as i32 / 2
        };
    }

    /// Push current global pos to the surface as TOP|LEFT margins.
    fn apply_margins(&mut self) {
        if let Some(g) = self.outputs.get(self.cur_idx) {
            self.margin_left = (self.pos_x - g.x).max(0);
            self.margin_top = (self.pos_y - g.y).max(0);
        }
        if let Some(s) = &self.layer_surface {
            s.set_margin(self.margin_top, 0, 0, self.margin_left);
            s.commit();
        }
    }

    /// Clamp global pos so the whole sprite stays inside the current output.
    fn clamp_to_cur(&mut self) {
        if let Some(g) = self.outputs.get(self.cur_idx) {
            let maxx = g.x + (g.w - SPRITE_W as i32).max(0);
            let maxy = g.y + (g.h - SPRITE_H as i32).max(0);
            self.pos_x = self.pos_x.clamp(g.x, maxx);
            self.pos_y = self.pos_y.clamp(g.y, maxy);
        }
    }

    /// Destroy + recreate the layer surface on another output (the hop).
    fn pin_to(&mut self, idx: usize, qh: &QueueHandle<Self>) {
        let out = match self.outputs.get(idx) {
            Some(g) => g.output.clone(),
            None => return,
        };
        let surface = self.compositor.create_surface(qh);
        let ls = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("linuxpal"),
            Some(&out),
        );
        ls.set_anchor(Anchor::TOP | Anchor::LEFT);
        ls.set_size(SPRITE_W, SPRITE_H);
        let g = &self.outputs[idx];
        let ml = (self.pos_x - g.x).max(0);
        let mt = (self.pos_y - g.y).max(0);
        ls.set_margin(mt, 0, 0, ml);
        ls.set_keyboard_interactivity(KeyboardInteractivity::None);
        ls.commit();

        self.layer_surface = Some(ls);
        self.cur_idx = idx;
        self.margin_left = ml;
        self.margin_top = mt;
        self.configured = false;
        self.buffer = None;
    }

    /// Kick off a free roam: walk toward a random spot. Returns the initial state.
    fn begin_roam(&mut self, music: bool) -> State {
        log::info!("roam start (stable {} ticks, music={music})", self.stable_ticks);
        self.roaming = true;
        self.park_ticks_left = 0;
        self.walk_ticks_left = self.cfg.walk_duration;
        self.pick_walk_target();
        State::Walk(self.walk_dir)
    }

    /// Mood from the main-screen context, minus roam: what the pet poses as and
    /// what tips/jokes are generated for. music → Jamming, video → Cozy, long
    /// work → WorkingEmpty, away/long-idle → Curious, else the base state.
    fn mood_state(&self, base: &State, music: bool, video: bool) -> State {
        if music {
            State::Jamming
        } else if video {
            State::Cozy
        } else if *base == State::Working && self.working_ticks >= self.cfg.coffee_after {
            State::WorkingEmpty
        } else if self.user_idle
            || (*base == State::Idle && self.idle_ticks >= self.cfg.curious_after)
        {
            State::Curious
        } else {
            base.clone()
        }
    }

    /// Pause mid-roam: stand in the LIVE main-screen mood and deliver a tip/joke
    /// generated for that context, then the park branch resumes wandering. The
    /// roam's one intentional, content-bearing stop (companion mode).
    fn roam_tip(&mut self) {
        let mood = self.roam_mood.clone();
        let ctx = self.last_ctx.clone();
        self.park_ticks_left = self.cfg.park_duration;
        self.animator.state = mood.clone();
        self.animator.reset();
        self.bubble.show_loading();
        llm::query_async(ctx, mood, self.bubble_tx.clone());
    }

    /// One roam tick in global coords. Travels (walk frames) toward target; on
    /// arrival jams in place if music. Crossing a shared edge hops to the
    /// neighbor output (doorway); an outer edge is a wall — both get a comment.
    fn advance_walk(&mut self, music: bool, qh: &QueueHandle<Self>) {
        // parked: hold the pose set when the pause began (Jamming for a music
        // stop, Idle while a tip/joke shows). Don't move the surface.
        if self.park_ticks_left > 0 {
            self.park_ticks_left -= 1;
            if self.park_ticks_left == 0 {
                // pause over → resume wandering with a fresh leg + target
                self.walk_ticks_left = self.cfg.walk_duration;
                self.pick_walk_target();
                if !self.cfg.always_roam && !music {
                    self.stop_roam(false); // music ended while parked
                }
            }
            return;
        }

        // leg timer governs non-music roam length. Tick at the top so every tick
        // counts even while sliding along a wall (the wall branch returns early).
        if !music {
            self.walk_ticks_left = self.walk_ticks_left.saturating_sub(1);
            if self.walk_ticks_left == 0 {
                if self.cfg.always_roam {
                    self.roam_tip(); // pause, deliver a tip/joke, then resume
                    return;
                }
                self.stop_roam(false);
                return;
            }
        }
        self.wall_cooldown = self.wall_cooldown.saturating_sub(1);

        let walk_step = self.cfg.walk_step;
        let step_axis = |cur: i32, target: i32| -> i32 {
            let d = target - cur;
            if d.abs() <= walk_step {
                target
            } else if d > 0 {
                cur + walk_step
            } else {
                cur - walk_step
            }
        };

        // face horizontal travel direction
        if self.target_x > self.pos_x {
            self.walk_dir = Direction::Right;
        } else if self.target_x < self.pos_x {
            self.walk_dir = Direction::Left;
        }
        self.animator.state = State::Walk(self.walk_dir);

        let nx = step_axis(self.pos_x, self.target_x);
        let ny = step_axis(self.pos_y, self.target_y);
        // mascot is flush-right in the surface → its centre, not the surface centre
        let cx = nx + SPRITE_W as i32 - MASCOT_W / 2;
        let cy = ny + SPRITE_H as i32 / 2;
        // "keep current axis" centers — let the wall branch test sliding one
        // axis at a time (move x while holding y, or vice versa)
        let cx_keep = self.pos_x + SPRITE_W as i32 - MASCOT_W / 2;
        let cy_keep = self.pos_y + SPRITE_H as i32 / 2;

        match self.output_at(cx, cy) {
            Some(i) if i == self.cur_idx => {
                // still on the same screen — move
                self.pos_x = nx;
                self.pos_y = ny;
                self.apply_margins();
                self.wall_ticks = 0; // clean step clears any wall-slide streak
            }
            Some(i) => {
                // sprite center crossed into a neighbor — hop screens
                self.pos_x = nx;
                self.pos_y = ny;
                log::info!("cross → output {i}");
                self.bubble.say(CROSS_MSG);
                self.pin_to(i, qh);
                self.wall_ticks = 0;
            }
            None => {
                // diagonal step would leave every screen. Don't bounce off
                // randomly — slide along whichever axis keeps the center on the
                // current output, so it can reach a neighbor's overlap band
                // (doorway). A true corner pins it until the slide budget runs
                // out, then it repicks.
                let x_ok = self.output_at(cx, cy_keep) == Some(self.cur_idx);
                let y_ok = self.output_at(cx_keep, cy) == Some(self.cur_idx);
                if x_ok {
                    self.pos_x = nx;
                }
                if y_ok {
                    self.pos_y = ny;
                }
                self.clamp_to_cur();
                self.apply_margins();
                self.wall_ticks += 1;

                if self.wall_cooldown == 0 {
                    self.bubble.say(WALL_MSG);
                    self.wall_cooldown = WALL_COOLDOWN;
                }
                if self.wall_ticks >= WALL_SLIDE_MAX {
                    self.wall_ticks = 0;
                    log::info!("wall edge at ({},{}) — repick", self.pos_x, self.pos_y);
                    self.pick_walk_target(); // give up this target, head elsewhere
                }
                return;
            }
        }

        // arrived at the spot
        if self.pos_x == self.target_x && self.pos_y == self.target_y {
            if music {
                self.park_ticks_left = self.cfg.park_duration;
                self.animator.state = State::Jamming;
            } else {
                self.pick_walk_target();
            }
        }
    }

    /// End a roam: drop back to jamming (music still on) or idle.
    fn stop_roam(&mut self, music: bool) {
        self.roaming = false;
        self.park_ticks_left = 0;
        self.animator.state = if music { State::Jamming } else { State::Idle };
        self.animator.reset();
        self.idle_ticks = 0;
    }
}

impl CompositorHandler for LinuxPal {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.animator.advance();
        self.draw(qh);
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for LinuxPal {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.rebuild_outputs();
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.rebuild_outputs();
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.rebuild_outputs();
    }
}

impl LayerShellHandler for LinuxPal {
    fn closed(&mut self, _: &Connection, qh: &QueueHandle<Self>, layer: &LayerSurface) {
        // a screen hop destroys the old surface → ignore its close event.
        let is_current = self
            .layer_surface
            .as_ref()
            .map(|s| s.wl_surface() == layer.wl_surface())
            .unwrap_or(false);
        if !is_current {
            log::info!("old layer surface closed (re-pin), ignoring");
            return;
        }

        // current surface gone — usually a monitor powered off / unplugged.
        // survive by hopping to a remaining output instead of exiting.
        self.rebuild_outputs();
        if self.outputs.is_empty() {
            log::warn!("no outputs left, exiting");
            std::process::exit(0);
        }
        self.cur_idx = 0;
        let (x, y) = (self.outputs[0].x, self.outputs[0].y);
        self.pos_x = x + MARGIN;
        self.pos_y = y + MARGIN;
        log::warn!("current surface closed (monitor off?) → re-pinning to remaining output");
        self.pin_to(0, qh);
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        if self.pool.is_none() {
            let pool = SlotPool::new((self.width * self.height * 4 * 2) as usize, &self.shm)
                .expect("failed to create slot pool");
            self.pool = Some(pool);
        }
        self.configured = true;
        self.draw(qh);
    }
}

impl ShmHandler for LinuxPal {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for LinuxPal {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(p) => self.pointer = Some(p),
                Err(e) => log::warn!("failed to get pointer: {e}"),
            }
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard(qh, &seat, None) {
                Ok(k) => self.keyboard = Some(k),
                Err(e) => log::warn!("failed to get keyboard: {e}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

const BTN_LEFT: u32 = 0x110;

impl PointerHandler for LinuxPal {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match event.kind {
                PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                    self.press = Some(Press {
                        start: event.position,
                        last: event.position,
                        time: Instant::now(),
                        dragging: false,
                    });
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    // released without ever dragging → it was a tap
                    if let Some(p) = self.press.take() {
                        if !p.dragging {
                            self.handle_tap(p.start);
                        }
                    }
                }
                PointerEventKind::Leave { .. } => {
                    self.press = None;
                    self.pointer_pos = (-1.0, -1.0); // cursor gone → ease lean back
                }
                PointerEventKind::Motion { .. } => {
                    self.pointer_pos = event.position;
                    // decide drag promotion + delta without holding the press
                    // borrow across the self.drag_by() call
                    let mut delta = None;
                    if let Some(p) = self.press.as_mut() {
                        let tx = event.position.0 - p.start.0;
                        let ty = event.position.1 - p.start.1;
                        if !p.dragging && (tx * tx + ty * ty).sqrt() > DRAG_SLOP {
                            p.dragging = true;
                        }
                        if p.dragging {
                            delta = Some((event.position.0 - p.last.0, event.position.1 - p.last.1));
                        }
                        p.last = event.position;
                    }
                    if let Some((dx, dy)) = delta {
                        self.drag_by(dx, dy);
                    }
                }
                _ => {}
            }
        }
    }
}

// keysym raw values for the keys we special-case during input
const KEY_BACKSPACE: u32 = 0xff08;
const KEY_RETURN: u32 = 0xff0d;
const KEY_KP_ENTER: u32 = 0xff8d;
const KEY_ESCAPE: u32 = 0xff1b;

impl KeyboardHandler for LinuxPal {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[smithay_client_toolkit::seat::keyboard::Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        // lost keyboard focus mid-type → abandon the input
        if self.input_mode {
            self.cancel_input();
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if !self.input_mode {
            return;
        }
        match event.keysym.raw() {
            KEY_ESCAPE => self.cancel_input(),
            KEY_RETURN | KEY_KP_ENTER => self.submit_input(),
            KEY_BACKSPACE => {
                self.input_buf.pop();
            }
            _ => {
                if let Some(text) = &event.utf8 {
                    for ch in text.chars() {
                        if !ch.is_control() {
                            self.input_buf.push(ch);
                        }
                    }
                }
            }
        }
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: smithay_client_toolkit::seat::keyboard::Modifiers,
        _: u32,
    ) {
    }
}

impl ProvidesRegistryState for LinuxPal {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(LinuxPal);
delegate_output!(LinuxPal);
delegate_layer!(LinuxPal);
delegate_shm!(LinuxPal);
delegate_seat!(LinuxPal);
delegate_pointer!(LinuxPal);
delegate_keyboard!(LinuxPal);
delegate_registry!(LinuxPal);

// ext-idle-notify-v1 lives outside sctk's delegate macros, so wire its two
// objects by hand. The notifier has no events; the notification fires
// idled/resumed, which drive away-detection + the welcome-back greeting.
impl Dispatch<ExtIdleNotifierV1, ()> for LinuxPal {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: ext_idle_notifier_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for LinuxPal {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {
                state.user_idle = true;
                log::info!("user idle (away)");
            }
            ext_idle_notification_v1::Event::Resumed => {
                // only a genuine away→back transition triggers welcome-back;
                // a Resumed with no prior Idled (some compositors emit one at
                // arm time) must not fire a spurious greeting on startup
                if state.user_idle {
                    state.idle_returned = true;
                }
                state.user_idle = false;
                log::info!("user resumed (back)");
            }
            _ => {}
        }
    }
}

/// Action-menu panel left-x: anchored just left of the mascot (which blits
/// flush-right, nominal left `SPRITE_W - MASCOT_W`). Stable across animation
/// frames so draw and hit-test agree.
fn menu_origin_x() -> usize {
    (SPRITE_W as usize)
        .saturating_sub(MASCOT_W as usize)
        .saturating_sub(menu::PANEL_W + MENU_GAP)
        .max(4)
}

/// Launch an app through Hyprland, off-thread so it never stalls rendering.
fn spawn_exec(cmd: &str) {
    let cmd = cmd.to_string();
    std::thread::spawn(move || {
        let _ = std::process::Command::new("hyprctl")
            .args(["dispatch", "exec", &cmd])
            .status();
    });
}

/// Map a control-socket state name to an animation state. `Walk` is excluded —
/// it needs a direction and is driven by roaming, not by command.
fn parse_state(name: &str) -> Option<State> {
    Some(match name {
        "idle" => State::Idle,
        "alert" => State::Alert,
        "thinking" => State::Thinking,
        "happy" => State::Happy,
        "working" => State::Working,
        "jamming" | "jam" => State::Jamming,
        "cozy" => State::Cozy,
        "curious" => State::Curious,
        "working_empty" | "workingempty" => State::WorkingEmpty,
        "training_done" | "trainingdone" => State::TrainingDone,
        _ => return None,
    })
}

fn main() {
    env_logger::init();

    // user config (~/.config/linuxpal/config.toml) — written with defaults on first run
    let cfg = config::load();
    llm::set_model(cfg.model.clone());

    let asset_dir =
        std::env::var("LINUXPAL_ASSETS").unwrap_or_else(|_| "assets/sprites".to_string());

    let sprites = Sprites::load(&asset_dir).expect("failed to load sprites");
    let animator = Animator::new();

    // shared window context — written by IPC thread, read by main loop
    let win_ctx = Arc::new(Mutex::new(WindowContext::empty()));
    ipc::spawn_ipc_listener(Arc::clone(&win_ctx), cfg.hdmi_match.clone());

    // background MPRIS poller — exposes playing music/video + track title/url
    let player_state = Arc::new(Mutex::new(player::PlayerState::default()));
    player::spawn_player_monitor(Arc::clone(&player_state));

    // persistent bubble channel — async producers (LLM tips, streamed answers,
    // morning-routine status) post updates the main loop drains each tick
    let (bubble_tx, bubble_rx) = std::sync::mpsc::channel();

    // control socket — external triggers (summon / ask / morning / say / state)
    let (control_tx, control_rx) = std::sync::mpsc::channel();
    control::spawn_control_listener(control_tx);

    let conn = Connection::connect_to_env().expect("failed to connect to Wayland");
    let (globals, mut queue) =
        registry_queue_init::<LinuxPal>(&conn).expect("failed to init registry");
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
    // optional: real input-idle. Absent on non-supporting compositors → feature
    // just stays off (notifier None).
    let idle_notifier = globals
        .bind::<ExtIdleNotifierV1, _, _>(&qh, 1..=2, ())
        .map_err(|e| log::warn!("ext-idle-notify unavailable, idle detection off: {e}"))
        .ok();

    let surface = compositor.create_surface(&qh);
    let layer_surface =
        layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("linuxpal"), None);

    layer_surface.set_anchor(Anchor::TOP | Anchor::LEFT);
    layer_surface.set_size(SPRITE_W, SPRITE_H);
    layer_surface.set_margin(MARGIN, 0, 0, MARGIN);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer_surface.commit();

    let mut app = LinuxPal {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        cfg,
        compositor,
        layer_shell,
        shm,
        layer_surface: Some(layer_surface),
        pool: None,
        buffer: None,
        pointer: None,
        sprites,
        animator,
        bubble: bubble::Bubble::new(),
        bubble_tx,
        bubble_rx,
        control_rx,
        win_ctx,
        player_state,
        configured: false,
        width: SPRITE_W,
        height: SPRITE_H,
        press: None,
        pointer_pos: (-1.0, -1.0),
        lean_px: 0,
        menu_open: false,
        keyboard: None,
        input_mode: false,
        input_buf: String::new(),
        margin_left: MARGIN,
        margin_top: MARGIN,
        idle_ticks: 0,
        working_ticks: 0,
        stable_ticks: 0,
        last_base: None,
        roam_mood: State::Idle,
        last_ctx: String::new(),
        roaming: false,
        walk_ticks_left: 0,
        park_ticks_left: 0,
        walk_dir: Direction::Right,
        wall_cooldown: 0,
        wall_ticks: 0,
        idle_notifier,
        idle_notification: None,
        user_idle: false,
        idle_returned: false,
        greet_ticks_left: GREET_TICKS,
        greeted: false,
        force_state: None,
        force_ticks_left: 0,
        pending_state: None,
        pending_count: 0,
        outputs: Vec::new(),
        cur_idx: 0,
        pos_init: false,
        pos_x: MARGIN,
        pos_y: MARGIN,
        target_x: MARGIN,
        target_y: MARGIN,
        rng: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64 | 1)
            .unwrap_or(0x9e3779b97f4a7c15),
    };

    let tick_duration = Duration::from_millis(1000 / TICK_HZ);
    let mut last_tick = Instant::now();

    loop {
        queue.flush().expect("wayland flush failed");
        queue.dispatch_pending(&mut app).expect("dispatch failed");

        // sync mascot state from IPC context every tick
        if last_tick.elapsed() >= tick_duration {
            last_tick = Instant::now();
            app.sync_state(&qh);
            app.animator.advance();
            if app.configured {
                app.draw(&qh);
            }
        }

        let timeout = tick_duration
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::ZERO);

        if let Some(guard) = queue.prepare_read() {
            let fd = conn.as_fd();
            let mut fds = [rustix::event::PollFd::new(
                &fd,
                rustix::event::PollFlags::IN,
            )];
            let _ = rustix::event::poll(&mut fds, timeout.as_millis() as i32);
            let _ = guard.read();
        }
    }
}
