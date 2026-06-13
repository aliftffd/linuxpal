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
/// Dedicated players → yes (single window). Browsers are one MPRIS player across
/// many tabs/screens, so require the playing track title to match the window's
/// shown title — stops a video on another screen's tab from hijacking state.
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

    // title-based — ML training run finished
    if (title.contains("training") || title.contains("epoch") || title.contains("train"))
        && (title.contains("done")
            || title.contains("complete")
            || title.contains("finished")
            || title.contains("✓"))
    {
        return State::TrainingDone;
    }

    // title-based — build/tests succeeded → celebrate
    if title.contains("passed")
        || title.contains("success")
        || title.contains("compiled successfully")
        || title.contains("0 failed")
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

    // bare tmux/shell → idle
    if title == "tmux" || title.is_empty() {
        return State::Idle;
    }

    // class-based — direct window
    if matches!(
        class,
        "nvim" | "neovim" | "vim" | "code" | "vscodium" | "emacs" | "helix"
    ) {
        return State::Working;
    }

    if matches!(
        class,
        "kitty" | "alacritty" | "foot" | "wezterm" | "ghostty"
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
