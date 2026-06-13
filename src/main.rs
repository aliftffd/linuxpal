mod bubble;
mod context;
mod ipc;
mod llm;
mod player;
mod renderer;
mod sprites;

use std::os::unix::io::AsFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
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
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use ipc::WindowContext;
use sprites::{Animator, Direction, Sprites, State};

const SPRITE_W: u32 = 520;  // bubble 258 + gap 14 + 230 widest mascot (jamming) + left margin
const SPRITE_H: u32 = 165;  // jamming is tallest at 155
const MARGIN: i32 = 16;
const TICK_HZ: u64 = 10;

// behavior timers (ticks at TICK_HZ=10 → 100ms each)
const CURIOUS_AFTER: u64 = 150;   // idle 15s with no window → curious
const WALK_EVERY: u64 = 100;      // after 10s of a stable window, take a stroll
const WALK_DURATION: u32 = 120;   // non-music roam length (12s), repick on arrival
const WALK_STEP: i32 = 8;         // px moved per tick while walking
const PARK_DURATION: u32 = 30;    // jam in place 3s on arrival (music roam)
const COFFEE_AFTER: u64 = 1800;   // working 3min straight → out of coffee

// free-roam bounds when no output geometry is known yet
const FALLBACK_W: i32 = 1920;
const FALLBACK_H: i32 = 1080;

const WALL_PAD: i32 = 250; // let targets overshoot bounds so it bumps walls
const CROSS_MSG: &str = "a doorway! crossing to the next screen";
const WALL_MSG: &str = "a wall here - cant cross this one";

/// One output's place in the global layout (logical coords).
struct OutputGeom {
    output: wl_output::WlOutput,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

struct LinuxPal {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    seat_state: SeatState,

    layer_surface: Option<LayerSurface>,
    pool: Option<SlotPool>,
    buffer: Option<Buffer>,
    pointer: Option<wl_pointer::WlPointer>,

    sprites: Sprites,
    animator: Animator,
    bubble: bubble::Bubble,
    llm_rx: Option<std::sync::mpsc::Receiver<llm::LlmResponse>>,
    win_ctx: Arc<Mutex<WindowContext>>,
    music_playing: Arc<AtomicBool>,
    video_playing: Arc<AtomicBool>,

    configured: bool,
    width: u32,
    height: u32,

    // drag state: surface-local grab point while left button held
    drag_grab: Option<(f64, f64)>,
    margin_left: i32,
    margin_top: i32,

    // behavior timers — drive curious / working_empty / walk
    idle_ticks: u64,
    working_ticks: u64,
    stable_ticks: u64,
    last_base: Option<State>,
    roaming: bool,
    walk_ticks_left: u32,
    park_ticks_left: u32,
    walk_dir: Direction,

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
    fn draw(&mut self, qh: &QueueHandle<Self>) {
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

        if self.bubble.visible {
            bubble::draw_bubble(
                canvas,
                stride,
                &self.bubble.tip.clone(),
                &self.bubble.joke.clone(),
                x_offset,
            );
        }
        self.bubble.tick();

        renderer::blit_frame(canvas, w, h, stride, frame, x_offset);

        surface.wl_surface().attach(Some(buffer.wl_buffer()), 0, 0);
        surface.wl_surface().damage_buffer(0, 0, w as i32, h as i32);
        surface.wl_surface().commit();

        self.buffer = Some(buffer);
    }

    fn sync_state(&mut self, qh: &QueueHandle<Self>) {
        // poll for LLM response — update bubble if arrived
        if let Some(rx) = &self.llm_rx {
            if let Ok(resp) = rx.try_recv() {
                self.bubble.update(&resp.tip, &resp.joke);
                self.llm_rx = None;
                log::info!("llm response received and applied to bubble");
            }
        }

        let music = self.music_playing.load(Ordering::Relaxed);
        let video = self.video_playing.load(Ordering::Relaxed);

        // roam in progress — walk between spots, jam when parked
        if self.roaming {
            self.advance_walk(music, qh);
            return;
        }

        // read window context → base state + prompt string + media scope
        let (base, context_str, media_here) = match self.win_ctx.lock() {
            Ok(ctx) => {
                let s = context::resolve_state(&ctx);
                let cs = if ctx.title.is_empty() {
                    ctx.class.clone()
                } else {
                    format!("{} - {}", ctx.class, ctx.title)
                };
                (s, cs, context::is_media_window(&ctx))
            }
            Err(_) => return,
        };

        // media (jamming/cozy) only counts when the deciding window IS the player
        let music = music && media_here;
        let video = video && media_here;

        // advance behavior timers off the base state
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

        // generic inactivity: roam whenever the window context hasn't changed
        // for a while — works even with a window focused (terminal, editor…)
        if self.last_base.as_ref() == Some(&base) {
            self.stable_ticks = self.stable_ticks.saturating_add(1);
        } else {
            self.stable_ticks = 0;
            self.last_base = Some(base.clone());
        }

        // priority: music-roam > jamming > video(cozy) > idle-roam > moods > base
        let roam_trigger =
            self.stable_ticks >= WALK_EVERY && self.stable_ticks % WALK_EVERY == 0;
        let idle_or_work = matches!(base, State::Idle | State::Working);

        let effective = if music && roam_trigger {
            self.begin_roam(music)
        } else if music {
            State::Jamming
        } else if video {
            // watching a plain video → settle in and get cozy
            State::Cozy
        } else if idle_or_work && roam_trigger {
            self.begin_roam(music)
        } else if base == State::Idle && self.idle_ticks >= CURIOUS_AFTER {
            State::Curious
        } else if base == State::Working && self.working_ticks >= COFFEE_AFTER {
            State::WorkingEmpty
        } else {
            base
        };

        if effective != self.animator.state {
            log::info!("state → {:?}", effective);

            // walk is ambient — no bubble/LLM churn for it
            if !matches!(effective, State::Walk(_)) {
                self.bubble.show_loading();
                let (tx, rx) = std::sync::mpsc::channel();
                self.llm_rx = Some(rx);
                llm::query_async(context_str, tx);
            }

            self.animator.state = effective;
            self.animator.reset();
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

    /// Bounding box over all outputs (global coords).
    fn union_bounds(&self) -> (i32, i32, i32, i32) {
        if self.outputs.is_empty() {
            return (0, 0, FALLBACK_W, FALLBACK_H);
        }
        let mut minx = i32::MAX;
        let mut miny = i32::MAX;
        let mut maxx = i32::MIN;
        let mut maxy = i32::MIN;
        for g in &self.outputs {
            minx = minx.min(g.x);
            miny = miny.min(g.y);
            maxx = maxx.max(g.x + g.w);
            maxy = maxy.max(g.y + g.h);
        }
        (minx, miny, maxx, maxy)
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

    /// Pick a random global target around the union box. Overshooting the
    /// bounds a little (WALL_PAD) makes it occasionally head into a gap or off
    /// an outer edge, which drives the wall vs doorway behavior.
    fn pick_walk_target(&mut self) {
        let (minx, miny, maxx, maxy) = self.union_bounds();
        self.target_x = self.rand_range(minx - WALL_PAD, maxx - SPRITE_W as i32 + WALL_PAD);
        self.target_y = self.rand_range(miny - WALL_PAD, maxy - SPRITE_H as i32 + WALL_PAD);
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
        self.walk_ticks_left = WALK_DURATION;
        self.pick_walk_target();
        State::Walk(self.walk_dir)
    }

    /// One roam tick in global coords. Travels (walk frames) toward target; on
    /// arrival jams in place if music. Crossing a shared edge hops to the
    /// neighbor output (doorway); an outer edge is a wall — both get a comment.
    fn advance_walk(&mut self, music: bool, qh: &QueueHandle<Self>) {
        // parked: jam in place, don't move the surface
        if self.park_ticks_left > 0 {
            self.park_ticks_left -= 1;
            self.animator.state = State::Jamming;
            if self.park_ticks_left == 0 {
                if music {
                    self.pick_walk_target(); // next leg
                } else {
                    self.stop_roam(false); // music ended while parked
                }
            }
            return;
        }

        let step_axis = |cur: i32, target: i32| -> i32 {
            let d = target - cur;
            if d.abs() <= WALK_STEP {
                target
            } else if d > 0 {
                cur + WALK_STEP
            } else {
                cur - WALK_STEP
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
        let cx = nx + SPRITE_W as i32 / 2;
        let cy = ny + SPRITE_H as i32 / 2;

        match self.output_at(cx, cy) {
            Some(i) if i == self.cur_idx => {
                // still on the same screen — move
                self.pos_x = nx;
                self.pos_y = ny;
                self.apply_margins();
            }
            Some(i) => {
                // sprite center crossed into a neighbor — hop screens
                self.pos_x = nx;
                self.pos_y = ny;
                log::info!("cross → output {i}");
                self.bubble.say(CROSS_MSG);
                self.pin_to(i, qh);
            }
            None => {
                // would leave all screens → outer wall
                self.bubble.say(WALL_MSG);
                self.pos_x = nx;
                self.pos_y = ny;
                self.clamp_to_cur();
                self.apply_margins();
                self.pick_walk_target(); // turn around, head elsewhere
                return;
            }
        }

        // arrived at the spot
        if self.pos_x == self.target_x && self.pos_y == self.target_y {
            if music {
                self.park_ticks_left = PARK_DURATION;
                self.animator.state = State::Jamming;
            } else {
                self.pick_walk_target();
            }
        }

        // session timer governs non-music roam length only
        if !music {
            self.walk_ticks_left = self.walk_ticks_left.saturating_sub(1);
            if self.walk_ticks_left == 0 {
                self.stop_roam(false);
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
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        std::process::exit(0);
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
                    self.drag_grab = Some(event.position);
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    self.drag_grab = None;
                }
                PointerEventKind::Leave { .. } => {
                    self.drag_grab = None;
                }
                PointerEventKind::Motion { .. } => {
                    if let Some((gx, gy)) = self.drag_grab {
                        // anchored TOP|LEFT: growing left/top margin follows cursor delta
                        let dx = event.position.0 - gx;
                        let dy = event.position.1 - gy;
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
                }
                _ => {}
            }
        }
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
delegate_registry!(LinuxPal);

fn main() {
    env_logger::init();

    let asset_dir =
        std::env::var("LINUXPAL_ASSETS").unwrap_or_else(|_| "assets/sprites".to_string());

    let sprites = Sprites::load(&asset_dir).expect("failed to load sprites");
    let animator = Animator::new();

    // shared window context — written by IPC thread, read by main loop
    let win_ctx = Arc::new(Mutex::new(WindowContext::empty()));
    ipc::spawn_ipc_listener(Arc::clone(&win_ctx));

    // background MPRIS poller — music vs plain-video playback flags
    let music_playing = Arc::new(AtomicBool::new(false));
    let video_playing = Arc::new(AtomicBool::new(false));
    player::spawn_player_monitor(Arc::clone(&music_playing), Arc::clone(&video_playing));

    let conn = Connection::connect_to_env().expect("failed to connect to Wayland");
    let (globals, mut queue) =
        registry_queue_init::<LinuxPal>(&conn).expect("failed to init registry");
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");

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
        llm_rx: None,
        win_ctx,
        music_playing,
        video_playing,
        configured: false,
        width: SPRITE_W,
        height: SPRITE_H,
        drag_grab: None,
        margin_left: MARGIN,
        margin_top: MARGIN,
        idle_ticks: 0,
        working_ticks: 0,
        stable_ticks: 0,
        last_base: None,
        roaming: false,
        walk_ticks_left: 0,
        park_ticks_left: 0,
        walk_dir: Direction::Right,
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
