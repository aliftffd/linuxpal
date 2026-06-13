use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Poll MPRIS (via `playerctl`) once a second. `music` = a music source is
/// playing (mpv/spotify/music.youtube.com/…); `video` = a plain YouTube watch
/// page is playing. Detected even when the player window isn't focused.
pub fn spawn_player_monitor(music: Arc<AtomicBool>, video: Arc<AtomicBool>) {
    std::thread::spawn(move || loop {
        let (m, v) = scan();
        music.store(m, Ordering::Relaxed);
        video.store(v, Ordering::Relaxed);
        std::thread::sleep(Duration::from_secs(1));
    });
}

/// Returns (music_playing, video_playing).
fn scan() -> (bool, bool) {
    // one call → "status<TAB>url" per player
    let out = match Command::new("playerctl")
        .args(["--all-players", "metadata", "--format", "{{lc(status)}}\t{{xesam:url}}"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log::debug!("playerctl unavailable: {e}");
            return (false, false);
        }
    };

    let mut music = false;
    let mut video = false;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let (status, url) = line.split_once('\t').unwrap_or((line.trim(), ""));
        if status.trim() != "playing" {
            continue;
        }
        if is_music(url.trim()) {
            music = true;
        } else {
            video = true; // plain YouTube watch page
        }
    }
    (music, video)
}

/// A playing source counts as music unless it's a plain YouTube watch page.
/// music.youtube.com, Spotify, local files (mpv), etc → music.
fn is_music(url: &str) -> bool {
    let u = url.to_lowercase();
    let plain_yt = (u.contains("youtube.com") || u.contains("youtu.be"))
        && !u.contains("music.youtube.com");
    !plain_yt
}
