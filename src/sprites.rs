use image::RgbaImage;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum State {
    Idle,
    Alert,
    Thinking,
    Happy,
    Working,
    Jamming,
    Cozy,
    Curious,
    WorkingEmpty,
    TrainingDone,
    Walk(Direction),
}

#[derive(Clone)]
pub struct Frame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Frame {
    fn load(path: &str) -> Result<Self, String> {
        let img: RgbaImage = image::open(path)
            .map_err(|e| format!("failed to load {path}: {e}"))?
            .into_rgba8();
        Ok(Self {
            width: img.width(),
            height: img.height(),
            data: img.into_raw(),
        })
    }
}

pub struct Sprites {
    frames: HashMap<String, Frame>,
}

impl Sprites {
    pub fn load(assert_dir: &str) -> Result<Self, String> {
        let mut frames = HashMap::new();
        let names = [
            "idle_open",
            "idle_blink",
            "alert",
            "thinking",
            "happy_static",
            "happy_sparks",
            "working_steam0",
            "working_steam1",
            "jamming_0",
            "jamming_1",
            "jamming_2",
            "jamming_3",
            "cozy_0",
            "cozy_1",
            "curious_0",
            "curious_1",
            "working_empty",
            "training_done_0",
            "training_done_1",
            "walk_left_0",
            "walk_left_1",
            "walk_right_0",
            "walk_right_1",
        ];

        for name in names {
            let path = format!("{assert_dir}/{name}.png");
            let frame = Frame::load(&path)?;
            log::info!("loaded {name} ({}x{})", frame.width, frame.height);
            frames.insert(name.to_string(), frame);
        }
        Ok(Self { frames })
    }

    pub fn get(&self, name: &str) -> &Frame {
        self.frames
            .get(name)
            .unwrap_or_else(|| panic!("sprite not found: {name}"))
    }
}

pub struct Animator {
    pub state: State,
    tick: u64,
}

impl Animator {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            tick: 0,
        }
    }

    pub fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn reset(&mut self) {
        self.tick = 0;
    }

    pub fn current_frame(&self) -> &'static str {
        match self.state {
            //blink
            State::Idle => {
                if self.tick % 32 < 2 {
                    "idle_blink"
                } else {
                    "idle_open"
                }
            }
            State::Alert => "alert",
            State::Thinking => "thinking",
            // Happy state
            State::Happy => {
                if (self.tick / 4) % 2 == 0 {
                    "happy_static"
                } else {
                    "happy_sparks"
                }
            }
            // working coffee
            State::Working => {
                if (self.tick / 8) % 2 == 0 {
                    "working_steam0"
                } else {
                    "working_steam1"
                }
            }
            // jamming — 200ms/frame, 4-frame bob cycle
            State::Jamming => match (self.tick / 2) % 4 {
                0 => "jamming_0",
                1 => "jamming_1",
                2 => "jamming_2",
                _ => "jamming_3",
            },
            // cozy — 600ms alternating
            State::Cozy => {
                if (self.tick / 6) % 2 == 0 {
                    "cozy_0"
                } else {
                    "cozy_1"
                }
            }
            // curious — 900ms alternating
            State::Curious => {
                if (self.tick / 9) % 2 == 0 {
                    "curious_0"
                } else {
                    "curious_1"
                }
            }
            // working but out of coffee — static
            State::WorkingEmpty => "working_empty",
            // training run finished — 400ms alternating
            State::TrainingDone => {
                if (self.tick / 4) % 2 == 0 {
                    "training_done_0"
                } else {
                    "training_done_1"
                }
            }
            // walk — 150ms/frame (≈200ms on the 100ms tick grid), directional
            State::Walk(dir) => match (dir, (self.tick / 2) % 2 == 0) {
                (Direction::Left, true) => "walk_left_0",
                (Direction::Left, false) => "walk_left_1",
                (Direction::Right, true) => "walk_right_0",
                (Direction::Right, false) => "walk_right_1",
            },
        }
    }
}

pub struct TipBank {
    tips: HashMap<String, Vec<&'static str>>,
}

impl TipBank {
    pub fn new() -> Self {
        let mut tips = HashMap::new();

        tips.insert("working".into(), vec![
            "try :Telescope find_files in nvim",
            "cargo check is faster than build",
            "use zoxide for fast dir jumping",
            "tmux prefix + z to zoom a pane",
            "rg is faster than grep, try it",
            "cargo clippy catches common bugs",
        ]);

        tips.insert("alert".into(), vec![
            "zen supports vertical tabs",
            "ctrl+l to focus the address bar",
            "use uBlock Origin filter lists",
            "middle click to close browser tabs",
        ]);

        tips.insert("thinking".into(), vec![
            "try: man <command> for any tool",
            "use tldr for quick command refs",
            "apropos <keyword> finds commands",
            "info coreutils has detailed docs",
        ]);

        tips.insert("idle".into(), vec![
            "hyprctl dispatch workspace 2",
            "try: hyprctl clients | grep class",
            "wofi --show run for app launcher",
            "super+shift+q to close a window",
            "hyprctl reload reloads config",
        ]);

        tips.insert("jamming".into(), vec![
            "playerctl play-pause from any term",
            "mpv --no-video plays audio only",
            "spotify_player is a TUI client",
            "ncspot is rust-native spotify TUI",
        ]);

        tips.insert("cozy".into(), vec![
            "zathura: J/K scroll, gg jumps top",
            "use foliate for epub reading",
            "okular can annotate PDFs",
            "sioyek is built for papers",
        ]);

        Self { tips }
    }

    pub fn get(&self, state: &State) -> &'static str {
        let key = match state {
            State::Working      => "working",
            State::Alert        => "alert",
            State::Thinking     => "thinking",
            State::Happy        => "idle",
            State::Idle         => "idle",
            State::Jamming      => "jamming",
            State::Cozy         => "cozy",
            State::Curious      => "idle",
            State::WorkingEmpty => "working",
            State::TrainingDone => "working",
            State::Walk(_)      => "idle",
        };
        let list = self.tips.get(key).unwrap();
        let idx = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() / 5) as usize % list.len();
        list[idx]
    }
}
