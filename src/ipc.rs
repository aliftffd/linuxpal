use serde_json::Value;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct WindowContext {
    pub class: String,
    pub title: String,
}

impl WindowContext {
    pub fn empty() -> Self {
        Self {
            class: String::new(),
            title: String::new(),
        }
    }
}

/// Poll Hyprland for the window that should drive the mascot's state.
/// Dual screen → the HDMI monitor's active-workspace window (stable, ignores
/// focus bouncing to the other display). Single screen → the focused monitor's
/// active window (workspace-based, as before).
pub fn spawn_ipc_listener(ctx: Arc<Mutex<WindowContext>>, hdmi_match: String) {
    std::thread::spawn(move || loop {
        if let Some(new) = poll_context(&hdmi_match) {
            if let Ok(mut c) = ctx.lock() {
                if *c != new {
                    log::info!("ctx → class={} title={}", new.class, new.title);
                    *c = new;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    });
}

fn poll_context(hdmi_match: &str) -> Option<WindowContext> {
    let monitors = hyprctl_json(&["-j", "monitors"])?;
    let mons = monitors.as_array()?;

    // enabled monitors only
    let enabled: Vec<&Value> = mons
        .iter()
        .filter(|m| !m["disabled"].as_bool().unwrap_or(false))
        .collect();
    if enabled.is_empty() {
        return Some(WindowContext::empty());
    }

    // pick the deciding monitor
    let target = if enabled.len() >= 2 {
        // dual → HDMI screen drives state
        enabled
            .iter()
            .find(|m| {
                m["name"]
                    .as_str()
                    .map(|n| n.to_lowercase().contains(hdmi_match))
                    .unwrap_or(false)
            })
            .copied()
            .unwrap_or(enabled[0])
    } else {
        // single → focused monitor (workspace-based)
        enabled
            .iter()
            .find(|m| m["focused"].as_bool().unwrap_or(false))
            .copied()
            .unwrap_or(enabled[0])
    };

    let mon_id = target["id"].as_i64()?;
    let ws_id = target["activeWorkspace"]["id"].as_i64()?;

    // active window on that monitor's active workspace = lowest focusHistoryID
    let clients = hyprctl_json(&["-j", "clients"])?;
    let win = clients
        .as_array()?
        .iter()
        .filter(|c| {
            c["monitor"].as_i64() == Some(mon_id) && c["workspace"]["id"].as_i64() == Some(ws_id)
        })
        .min_by_key(|c| c["focusHistoryID"].as_i64().unwrap_or(i64::MAX));

    Some(match win {
        Some(c) => WindowContext {
            // trim: hyprctl titles often carry trailing whitespace (e.g. "tmux "),
            // which would break exact matches like `title == "tmux"`
            class: c["class"].as_str().unwrap_or("").trim().to_lowercase(),
            title: c["title"].as_str().unwrap_or("").trim().to_lowercase(),
        },
        None => WindowContext::empty(), // empty workspace → idle
    })
}

fn hyprctl_json(args: &[&str]) -> Option<Value> {
    let out = Command::new("hyprctl").args(args).output().ok()?;
    serde_json::from_slice(&out.stdout).ok()
}
