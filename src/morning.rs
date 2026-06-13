//! Morning routine — one trigger launches your daily apps in order.
//!
//! Zero LLM: reads `~/.config/linuxpal/morning.toml` and spawns each app via
//! `hyprctl dispatch exec`, honouring optional per-app delays. Triggered by
//! `linuxpal-ctl morning` (or a Hyprland keybind). On first run it writes a
//! commented default config you can edit.

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::llm::BubbleUpdate;

/// One launch entry from the config.
struct App {
    cmd: String,
    args: Option<String>,
    /// Pause AFTER launching this app before the next (e.g. let a server settle).
    delay_ms: u64,
}

/// `$XDG_CONFIG_HOME/linuxpal/morning.toml`, falling back to `~/.config`.
fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("linuxpal").join("morning.toml")
}

const DEFAULT_TOML: &str = r#"# ~/.config/linuxpal/morning.toml
# LinuxPal morning routine. Trigger it with:  linuxpal-ctl morning
# Each [[apps]] block runs `hyprctl dispatch exec <cmd> <args>` in order.
# Optional delay_ms pauses AFTER launching that app before the next one.

[[apps]]
cmd = "zen-browser"

[[apps]]
cmd = "kitty"
args = "-e tmux new-session -As main"

# [[apps]]
# cmd = "spotify"

# [[apps]]
# cmd = "xfreerdp"
# args = "/v:YOUR_RDP_TARGET /u:YOUR_USER +clipboard"
# delay_ms = 2000
"#;

/// Load the config, writing the default on first run.
fn load() -> Vec<App> {
    let path = config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&path, DEFAULT_TOML) {
                Ok(_) => log::info!("wrote default morning config to {}", path.display()),
                Err(e) => log::warn!("could not write default morning config: {e}"),
            }
            DEFAULT_TOML.to_string()
        }
    };
    parse(&text)
}

/// Minimal parser for our own `[[apps]]` schema (`cmd` / `args` / `delay_ms`).
/// Deliberately not a general TOML parser — this keeps the build dependency-free
/// while covering exactly the config shape we document.
fn parse(text: &str) -> Vec<App> {
    let mut apps: Vec<App> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[apps]]" {
            apps.push(App {
                cmd: String::new(),
                args: None,
                delay_ms: 0,
            });
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let Some(app) = apps.last_mut() else {
            continue; // key before any [[apps]] header — ignore
        };
        let val = unquote(val.trim());
        match key.trim() {
            "cmd" => app.cmd = val,
            "args" => app.args = Some(val),
            "delay_ms" => app.delay_ms = val.parse().unwrap_or(0),
            _ => {}
        }
    }
    apps.retain(|a| !a.cmd.is_empty());
    apps
}

/// Strip surrounding double quotes if present.
fn unquote(s: &str) -> String {
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apps_with_args_and_delay() {
        let cfg = r#"
            # a comment
            [[apps]]
            cmd = "zen-browser"

            [[apps]]
            cmd = "kitty"
            args = "-e tmux new-session -As main"
            delay_ms = 1500
        "#;
        let apps = parse(cfg);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].cmd, "zen-browser");
        assert_eq!(apps[0].args, None);
        assert_eq!(apps[0].delay_ms, 0);
        assert_eq!(apps[1].cmd, "kitty");
        assert_eq!(apps[1].args.as_deref(), Some("-e tmux new-session -As main"));
        assert_eq!(apps[1].delay_ms, 1500);
    }

    #[test]
    fn skips_blocks_without_cmd_and_stray_keys() {
        let cfg = "args = \"orphan\"\n[[apps]]\ndelay_ms = 100\n[[apps]]\ncmd = \"spotify\"\n";
        let apps = parse(cfg);
        // first block has no cmd → dropped; stray key before any block → ignored
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].cmd, "spotify");
    }

    #[test]
    fn default_config_is_parseable() {
        assert!(!parse(DEFAULT_TOML).is_empty());
    }
}

/// Launch the routine on a background thread so per-app delays never stall
/// rendering. Progress is reported into the bubble via `status`.
pub fn run(status: Sender<BubbleUpdate>) {
    std::thread::spawn(move || {
        let apps = load();
        if apps.is_empty() {
            let _ = status.send(BubbleUpdate::Plain(
                "morning: no apps configured in morning.toml".into(),
            ));
            return;
        }

        let _ = status.send(BubbleUpdate::Plain(format!(
            "morning! launching {} apps",
            apps.len()
        )));

        for app in apps {
            let mut exec = app.cmd.clone();
            if let Some(args) = &app.args {
                exec.push(' ');
                exec.push_str(args);
            }
            let _ = status.send(BubbleUpdate::Plain(format!("launching {}", app.cmd)));

            match Command::new("hyprctl")
                .args(["dispatch", "exec", &exec])
                .status()
            {
                Ok(s) if !s.success() => {
                    log::warn!("morning: hyprctl exec '{}' exited {:?}", exec, s.code())
                }
                Err(e) => log::warn!("morning: failed to launch '{}': {e}", app.cmd),
                _ => {}
            }

            if app.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(app.delay_ms));
            }
        }

        let _ = status.send(BubbleUpdate::Plain("morning routine done".into()));
    });
}
