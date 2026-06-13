use crate::ipc::WindowContext;
use crate::sprites::State;

/// True if the deciding window is itself a media app (browser or player).
/// Media (jamming/cozy) overrides only apply when this window is the source —
/// stops a YouTube tab on another screen from hijacking the state.
pub fn is_media_window(ctx: &WindowContext) -> bool {
    matches!(
        ctx.class.as_str(),
        "zen" | "firefox"
            | "chromium"
            | "google-chrome"
            | "brave-browser"
            | "qutebrowser"
            | "mpv"
            | "vlc"
            | "spotify"
            | "spotify_player"
            | "spotube"
            | "ncspot"
            | "cmus"
            | "audacious"
            | "rhythmbox"
    )
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
