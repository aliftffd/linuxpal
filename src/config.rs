//! User config — `~/.config/linuxpal/config.toml`.
//!
//! Moves the knobs people actually tweak out of source `const`s so they can be
//! changed without a rebuild. Hand-parsed flat `key = value` (no TOML dep, same
//! approach as `morning.rs`). A commented default is written on first run.

use std::path::PathBuf;

pub struct Config {
    /// Substring identifying the monitor that drives state on dual-screen setups.
    pub hdmi_match: String,
    /// Ollama model for ambient tips + `ask`.
    pub model: String,
    /// Startup greeting text.
    pub greet_msg: String,
    /// Idle ticks (10 = 1s) before going Curious.
    pub curious_after: u64,
    /// Ticks of a stable window before a roam starts.
    pub walk_every: u64,
    /// Length of a non-music roam, in ticks.
    pub walk_duration: u32,
    /// Pixels moved per tick while walking.
    pub walk_step: i32,
    /// Ticks spent dancing in place per spot while music plays.
    pub park_duration: u32,
    /// Working ticks before "out of coffee" (WorkingEmpty).
    pub coffee_after: u64,
    /// Max px the mascot leans toward the cursor (0 disables the lean).
    pub lean_max: i32,
    /// Cursor must be within this many px (horizontally) to trigger a lean.
    pub lean_radius: i32,
    /// Seconds of real input-idle before "away" (ext-idle-notify); enables
    /// accurate Curious + welcome-back.
    pub idle_secs: u32,
    /// Companion mode: roam continuously in every situation across all screens,
    /// ignoring the mood state machine (Cozy/Jamming/Curious/WorkingEmpty). The
    /// only pauses are the periodic tip/joke park and screen-edge slides.
    pub always_roam: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hdmi_match: "hdmi".into(),
            model: "qwen2.5:1.5b".into(),
            greet_msg: "hello! lets get some ideas and tasks done!".into(),
            curious_after: 150,
            walk_every: 50,
            walk_duration: 120,
            walk_step: 8,
            park_duration: 30,
            coffee_after: 1800,
            lean_max: 6,
            lean_radius: 160,
            idle_secs: 60,
            always_roam: true,
        }
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("linuxpal").join("config.toml")
}

const DEFAULT_TOML: &str = r#"# ~/.config/linuxpal/config.toml
# LinuxPal settings. Edit and restart the pet (super+shift+P twice) to apply.
# Timings are in ticks: 10 ticks = 1 second.

# Monitor that decides state on a dual-screen setup (substring of the name).
hdmi_match = "hdmi"

# Ollama model for ambient tips and the "ask" feature.
model = "qwen2.5:1.5b"

# Greeting shown at launch.
greet_msg = "hello! lets get some ideas and tasks done!"

curious_after = 150    # idle 15s with no window -> curious
walk_every    = 50     # 5s of a stable window -> take a stroll
walk_duration = 120    # roam-leg length before it pauses for a tip/joke (12s)
walk_step     = 8       # px moved per tick while walking
park_duration = 30      # pause time per stop (tip/joke delivery, music dance) (3s)
coffee_after  = 1800    # working 3min straight -> out of coffee
lean_max      = 6       # px the pet leans toward your cursor (0 = off)
lean_radius   = 160     # cursor must be within this many px to react
idle_secs     = 60      # real input-idle (s) before "away" -> curious + welcome-back
always_roam   = true    # companion mode: roam every situation, all screens;
                        # only pauses for tip/joke + screen edges (false = moods)
"#;

/// Load config, writing the documented default on first run. Unknown keys and
/// malformed values are ignored (defaults kept).
pub fn load() -> Config {
    let path = config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&path, DEFAULT_TOML) {
                Ok(_) => log::info!("wrote default config to {}", path.display()),
                Err(e) => log::warn!("could not write default config: {e}"),
            }
            DEFAULT_TOML.to_string()
        }
    };

    let mut cfg = Config::default();
    parse_into(&text, &mut cfg);
    cfg
}

fn parse_into(text: &str, cfg: &mut Config) {
    for raw in text.lines() {
        // strip trailing `# comment`, then trim
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        apply(cfg, key.trim(), &unquote(val.trim()));
    }
}

fn apply(cfg: &mut Config, key: &str, val: &str) {
    match key {
        "hdmi_match" => cfg.hdmi_match = val.to_string(),
        "model" => cfg.model = val.to_string(),
        "greet_msg" => cfg.greet_msg = val.to_string(),
        "curious_after" => set(&mut cfg.curious_after, val),
        "walk_every" => set(&mut cfg.walk_every, val),
        "walk_duration" => set(&mut cfg.walk_duration, val),
        "walk_step" => set(&mut cfg.walk_step, val),
        "park_duration" => set(&mut cfg.park_duration, val),
        "coffee_after" => set(&mut cfg.coffee_after, val),
        "lean_max" => set(&mut cfg.lean_max, val),
        "lean_radius" => set(&mut cfg.lean_radius, val),
        "idle_secs" => set(&mut cfg.idle_secs, val),
        "always_roam" => set(&mut cfg.always_roam, val),
        _ => {}
    }
}

/// Parse a value into a field, keeping the existing default on error.
fn set<T: std::str::FromStr>(field: &mut T, val: &str) {
    if let Ok(v) = val.parse() {
        *field = v;
    }
}

fn unquote(s: &str) -> String {
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        let mut c = Config::default();
        parse_into("", &mut c);
        assert_eq!(c.hdmi_match, "hdmi");
        assert_eq!(c.coffee_after, 1800);
    }

    #[test]
    fn overrides_and_comments() {
        let mut c = Config::default();
        parse_into(
            "# header\nhdmi_match = \"DP-1\"\nwalk_step = 12  # inline comment\nmodel = qwen2.5:3b\nbogus = 9\n",
            &mut c,
        );
        assert_eq!(c.hdmi_match, "DP-1");
        assert_eq!(c.walk_step, 12);
        assert_eq!(c.model, "qwen2.5:3b");
        // malformed/unknown keys leave defaults intact
        assert_eq!(c.curious_after, 150);
    }

    #[test]
    fn bad_number_keeps_default() {
        let mut c = Config::default();
        parse_into("coffee_after = not_a_number", &mut c);
        assert_eq!(c.coffee_after, 1800);
    }

    #[test]
    fn default_toml_parses() {
        let mut c = Config::default();
        parse_into(DEFAULT_TOML, &mut c);
        assert_eq!(c.walk_every, 50);
    }
}
