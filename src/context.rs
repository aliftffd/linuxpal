use crate::ipc::WindowContext;
use crate::sprites::State;

fn is_browser(class: &str) -> bool {
    matches!(
        class,
        "zen" | "firefox" | "chromium" | "google-chrome" | "brave-browser" | "qutebrowser"
    )
}

fn is_player_app(class: &str) -> bool {
    matches!(
        class,
        "mpv" | "vlc" | "spotify" | "spotify_player" | "spotube" | "ncspot" | "cmus"
            | "audacious" | "rhythmbox"
    )
}

/// Should the playing media (jamming/cozy) drive state for THIS deciding window?
/// - Dedicated players (own window) → yes.
/// - Browsers are one MPRIS player across many tabs/screens, so require the
///   playing track title to match the window's shown title (stops a video on
///   another screen's tab from hijacking state).
/// Terminals are deliberately NOT here: focusing a terminal always means
/// Working, even if a TUI music player is running in it.
pub fn media_applies(ctx: &WindowContext, player_title: &str) -> bool {
    let class = ctx.class.as_str();
    if is_player_app(class) {
        return true;
    }
    if is_browser(class) {
        let pt = player_title.trim();
        return pt.len() > 3 && ctx.title.contains(pt);
    }
    false
}

pub fn resolve_state(ctx: &WindowContext) -> State {
    let class = ctx.class.as_str();
    let title = ctx.title.as_str();

    // title-based — ML training run finished (require a training context, not bare "train")
    if (title.contains("training") || title.contains("epoch"))
        && (title.contains("done")
            || title.contains("complete")
            || title.contains("finished"))
    {
        return State::TrainingDone;
    }

    // title-based — build/tests succeeded → celebrate (specific phrases, no bare "success")
    if title.contains("tests passed")
        || title.contains("build succeeded")
        || title.contains("compiled successfully")
        || title.contains("0 failed")
        || title.contains("all tests passing")
    {
        return State::Happy;
    }

    // title-based — nvim inside tmux
    if title.contains("nvim")
        || title.contains("vim")
        || title.contains("cargo build")
        || title.contains("cargo run")
    {
        return State::Working;
    }

    // class-based — editors (incl. VSCode / Insiders) → working
    if matches!(
        class,
        "nvim" | "neovim" | "vim" | "code" | "code-insiders" | "code - insiders"
            | "vscode" | "vscodium" | "codium" | "emacs" | "helix" | "sublime_text" | "zed"
    ) {
        return State::Working;
    }

    // terminals → working (bare shell / tmux counts as "at work" now, not idle)
    if matches!(
        class,
        "kitty" | "alacritty" | "foot" | "wezterm" | "ghostty" | "st" | "xterm" | "konsole"
    ) {
        return State::Working;
    }

    // remote desktop (RDP) → working
    if matches!(
        class,
        "xfreerdp" | "wlfreerdp" | "freerdp" | "remmina" | "org.remmina.remmina"
    ) {
        return State::Working;
    }

    if matches!(
        class,
        "firefox" | "chromium" | "brave-browser" | "google-chrome" | "qutebrowser" | "zen"
    ) {
        return State::Alert;
    }

    if matches!(
        class,
        "thunar" | "nautilus" | "dolphin" | "nemo" | "pcmanfm"
    ) {
        return State::Thinking;
    }

    // media players → jamming
    if matches!(
        class,
        "spotify" | "spotify_player" | "spotube" | "ncspot" | "cmus"
            | "mpv" | "vlc" | "audacious" | "rhythmbox"
    ) {
        return State::Jamming;
    }

    // document / e-readers → cozy
    if matches!(
        class,
        "zathura" | "evince" | "okular" | "foliate" | "calibre" | "sioyek" | "mupdf"
    ) {
        return State::Cozy;
    }

    State::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(class: &str, title: &str) -> WindowContext {
        WindowContext {
            class: class.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn terminal_never_jams() {
        // focusing a terminal is always Working, even with music playing
        assert!(!media_applies(&ctx("kitty", "tmux"), "some song"));
        assert!(!media_applies(&ctx("alacritty", "ncspot"), "track"));
    }

    #[test]
    fn dedicated_player_window_always_applies() {
        assert!(media_applies(&ctx("spotify", "Artist - Song"), ""));
        assert!(media_applies(&ctx("mpv", "video.mkv"), ""));
    }

    #[test]
    fn terminals_and_editors_are_working() {
        // the bug: tmux/bare-shell terminals were Idle; now Working
        assert_eq!(resolve_state(&ctx("kitty", "tmux")), State::Working);
        assert_eq!(resolve_state(&ctx("kitty", "zsh")), State::Working);
        assert_eq!(resolve_state(&ctx("alacritty", "")), State::Working);
        // editors incl. VSCode Insiders
        assert_eq!(resolve_state(&ctx("code-insiders", "x")), State::Working);
        assert_eq!(resolve_state(&ctx("code - insiders", "x")), State::Working);
        assert_eq!(resolve_state(&ctx("nvim", "init.lua")), State::Working);
        // nvim running inside tmux (title carries the program)
        assert_eq!(resolve_state(&ctx("kitty", "nvim main.rs")), State::Working);
        // remote desktop
        assert_eq!(resolve_state(&ctx("xfreerdp", "FreeRDP: host")), State::Working);
    }

    #[test]
    fn empty_context_is_idle() {
        assert_eq!(resolve_state(&ctx("", "")), State::Idle);
    }

    #[test]
    fn browser_requires_title_match() {
        // playing tab is the focused one → applies
        assert!(media_applies(&ctx("zen", "my song - youtube"), "my song"));
        // a different tab title → does not apply
        assert!(!media_applies(&ctx("zen", "github - pull request"), "my song"));
    }
}
