//! linuxpal-ctl — tiny client for LinuxPal's control socket.
//!
//! Meant for Hyprland binds and scripts:
//!   bind = SUPER, P, exec, linuxpal-ctl summon
//!   bind = SUPER, M, exec, linuxpal-ctl morning
//!   bind = SUPER, A, exec, linuxpal-ctl ask "$(fuzzel --dmenu --prompt 'ask> ')"
//!
//! Sends one command line, prints the server's reply, exits non-zero on error.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// Keep in sync with `src/control.rs::socket_path`.
fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    dir.join("linuxpal.sock")
}

fn usage() -> ! {
    eprintln!("usage: linuxpal-ctl <command> [text]");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  summon            pop up and wave");
    eprintln!("  ask <question>    ask the LLM; answer streams into the bubble");
    eprintln!("  say <message>     show a one-off message");
    eprintln!("  morning           run the morning launch routine");
    eprintln!("  state <name>      force an animation state (idle, happy, jamming, ...)");
    eprintln!("  quit              stop linuxpal");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }

    let path = socket_path();
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "linuxpal-ctl: cannot reach LinuxPal at {} ({e})",
                path.display()
            );
            eprintln!("is linuxpal running?");
            std::process::exit(1);
        }
    };

    let line = args.join(" ") + "\n";
    if let Err(e) = stream.write_all(line.as_bytes()) {
        eprintln!("linuxpal-ctl: write failed: {e}");
        std::process::exit(1);
    }

    // surface the server's ok / err reply to the caller
    let mut reply = String::new();
    let _ = BufReader::new(&stream).read_line(&mut reply);
    let reply = reply.trim();
    if reply.starts_with("err") {
        eprintln!("linuxpal-ctl: {reply}");
        std::process::exit(1);
    }
}
