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
    if let Some(ctx) = poll_hyprland(hdmi_match) {
        return Some(ctx);
    }
    if let Some(ctx) = poll_mutter() {
        return Some(ctx);
    }
    Some(WindowContext::empty())
}

fn poll_hyprland(hdmi_match: &str) -> Option<WindowContext> {
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

fn poll_mutter() -> Option<WindowContext> {
    // 1. Try Focused Window D-Bus extension
    if let Some(ctx) = poll_mutter_focused_window_extension() {
        return Some(ctx);
    }
    // 2. Try Window Calls Extended extension
    if let Some(ctx) = poll_mutter_window_calls_extension() {
        return Some(ctx);
    }
    // 3. Try xdotool fallback
    if let Some(ctx) = poll_x11_xdotool() {
        return Some(ctx);
    }
    None
}

fn poll_mutter_focused_window_extension() -> Option<WindowContext> {
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/shell/extensions/FocusedWindow",
            "--method",
            "org.gnome.shell.extensions.FocusedWindow.Get",
        ])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let json_str = extract_gdbus_string(&out.stdout)?;
    let val: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    
    let wm_class = val.get("wm_class")?.as_str()?.to_lowercase();
    let title = val.get("title")?.as_str()?.to_lowercase();

    if wm_class.is_empty() && title.is_empty() {
        Some(WindowContext::empty())
    } else {
        Some(WindowContext {
            class: wm_class,
            title,
        })
    }
}

fn poll_mutter_window_calls_extension() -> Option<WindowContext> {
    let out_class = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/Shell/Extensions/WindowsExt",
            "--method",
            "org.gnome.Shell.Extensions.WindowsExt.FocusClass",
        ])
        .output()
        .ok()?;

    if !out_class.status.success() {
        return None;
    }

    let out_title = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/Shell/Extensions/WindowsExt",
            "--method",
            "org.gnome.Shell.Extensions.WindowsExt.FocusTitle",
        ])
        .output()
        .ok()?;

    let wm_class = extract_gdbus_string(&out_class.stdout)?.to_lowercase();
    let title = extract_gdbus_string(&out_title.stdout)?.to_lowercase();

    Some(WindowContext {
        class: wm_class,
        title,
    })
}

fn extract_gdbus_string(stdout: &[u8]) -> Option<String> {
    let raw = String::from_utf8_lossy(stdout);
    let trimmed = raw.trim();
    
    // gdbus replies wrap strings/JSON inside single quotes: ( 'content', )
    let start_idx = trimmed.find('\'')? + 1;
    let end_idx = trimmed.rfind('\'')?;
    if start_idx >= end_idx {
        return None;
    }
    Some(trimmed[start_idx..end_idx].to_string())
}

fn poll_x11_xdotool() -> Option<WindowContext> {
    let active_win = Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()?;

    if !active_win.status.success() {
        return None;
    }

    let win_id = String::from_utf8_lossy(&active_win.stdout).trim().to_string();
    if win_id.is_empty() {
        return None;
    }

    let class_out = Command::new("xdotool")
        .args(["getwindowclassname", &win_id])
        .output()
        .ok()?;

    let title_out = Command::new("xdotool")
        .args(["getwindowname", &win_id])
        .output()
        .ok()?;

    let wm_class = String::from_utf8_lossy(&class_out.stdout).trim().to_lowercase();
    let title = String::from_utf8_lossy(&title_out.stdout).trim().to_lowercase();

    Some(WindowContext {
        class: wm_class,
        title,
    })
}

fn hyprctl_json(args: &[&str]) -> Option<Value> {
    let out = Command::new("hyprctl").args(args).output().ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_gdbus_string() {
        assert_eq!(
            extract_gdbus_string(b"( 'hello', )"),
            Some("hello".to_string())
        );
        assert_eq!(
            extract_gdbus_string(b"('{\n  \"wm_class\": \"code\",\n  \"title\": \"main.rs\"\n}')"),
            Some("{\n  \"wm_class\": \"code\",\n  \"title\": \"main.rs\"\n}".to_string())
        );
        assert_eq!(extract_gdbus_string(b"no quotes"), None);
        assert_eq!(extract_gdbus_string(b"''"), None);
    }
}

