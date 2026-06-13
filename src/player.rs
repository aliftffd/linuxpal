use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What MPRIS currently reports as playing. `title`/`url` belong to the playing
/// source so the main loop can match it against the deciding window (a browser
/// is one MPRIS player but may have tabs on several screens).
#[derive(Debug, Clone, Default)]
pub struct PlayerState {
    pub music: bool,
    pub video: bool,
    pub title: String,
    pub url: String,
    /// MPRIS player name of the playing source (e.g. `spotify`, `ncspot`,
    /// `firefox`), normalized lowercase with any `.instance…` suffix stripped.
    /// Lets a terminal music TUI be recognized even though its window class is
    /// just the terminal (`kitty`).
    pub player: String,
}

/// Poll MPRIS (via `playerctl`) once a second.
pub fn spawn_player_monitor(state: Arc<Mutex<PlayerState>>) {
    std::thread::spawn(move || loop {
        let s = scan();
        if let Ok(mut g) = state.lock() {
            *g = s;
        }
        std::thread::sleep(Duration::from_secs(1));
    });
}

fn scan() -> PlayerState {
    let out = match Command::new("playerctl")
        .args([
            "--all-players",
            "metadata",
            "--format",
            "{{playerName}}\t{{lc(status)}}\t{{xesam:url}}\t{{xesam:title}}",
        ])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log::debug!("playerctl unavailable: {e}");
            return PlayerState::default();
        }
    };

    let mut st = PlayerState::default();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split('\t');
        let name = parts.next().unwrap_or("").trim();
        let status = parts.next().unwrap_or("").trim();
        let url = parts.next().unwrap_or("").trim();
        let title = parts.next().unwrap_or("").trim();
        if status != "playing" {
            continue;
        }
        if is_music(url) {
            st.music = true;
        } else {
            st.video = true; // plain YouTube watch page
        }
        st.url = url.to_lowercase();
        st.title = title.to_lowercase();
        st.player = norm_player(name);
    }
    st
}

/// Normalize an MPRIS player name: lowercase, drop the `.instanceNNN` suffix
/// playerctl appends (e.g. `firefox.instance_1_15` → `firefox`).
fn norm_player(name: &str) -> String {
    name.split('.').next().unwrap_or(name).trim().to_lowercase()
}

/// A playing source counts as music unless it's a plain YouTube watch page.
/// music.youtube.com, Spotify, local files (mpv), etc → music.
fn is_music(url: &str) -> bool {
    let u = url.to_lowercase();
    let plain_yt = (u.contains("youtube.com") || u.contains("youtu.be"))
        && !u.contains("music.youtube.com");
    !plain_yt
}
