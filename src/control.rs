//! Control socket — the single "way in" for external triggers.
//!
//! Binds a Unix socket and turns one-line text commands into [`ControlEvent`]s
//! delivered over an mpsc channel the main loop drains each tick. A Hyprland
//! `bind` (via the `linuxpal-ctl` helper) or any `socat`/`nc` can drive it:
//!
//! ```text
//! echo summon            | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/linuxpal.sock
//! echo 'ask how to ...'  | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/linuxpal.sock
//! ```
//!
//! Everything else in the roadmap (hotkey summon, action menu, notification
//! bridge) reuses this one entry point.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// A command received over the control socket.
#[derive(Debug, Clone)]
pub enum ControlEvent {
    /// Pop up and wave — focus attention on the pet.
    Summon,
    /// Show a one-off message in the bubble.
    Say(String),
    /// Ask the LLM a free-form question; answer streams into the bubble.
    Ask(String),
    /// Run the configured morning launch routine (no LLM).
    Morning,
    /// Force a named animation state for a few seconds (debug / fun).
    SetState(String),
    /// Exit the process cleanly.
    Quit,
}

/// Path to the control socket. Prefers `$XDG_RUNTIME_DIR` (per-user, tmpfs,
/// cleaned on logout); falls back to `/tmp`. Keep in sync with the copy in
/// `src/bin/linuxpal-ctl.rs`.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    dir.join("linuxpal.sock")
}

/// Bind the socket and spawn an accept loop. Failure to bind is non-fatal —
/// the pet keeps running, just without remote control.
pub fn spawn_control_listener(tx: Sender<ControlEvent>) {
    let path = socket_path();
    // a stale socket from a previous run blocks bind — clear it first.
    let _ = std::fs::remove_file(&path);

    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("control socket bind failed at {}: {e}", path.display());
            return;
        }
    };
    log::info!("control socket listening at {}", path.display());

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => handle_client(s, &tx),
                Err(e) => log::warn!("control accept error: {e}"),
            }
        }
    });
}

/// Read one command line, reply `ok` / `err: ...`, forward the parsed event.
fn handle_client(mut stream: UnixStream, tx: &Sender<ControlEvent>) {
    let reader = match stream.try_clone() {
        Ok(s) => BufReader::new(s),
        Err(_) => return,
    };
    let mut lines = reader.lines();
    let line = match lines.next() {
        Some(Ok(l)) => l,
        _ => return,
    };

    match parse_command(&line) {
        Ok(ev) => {
            let _ = stream.write_all(b"ok\n");
            if tx.send(ev).is_err() {
                log::warn!("control: main loop gone, dropping event");
            }
        }
        Err(msg) => {
            let _ = stream.write_all(format!("err: {msg}\n").as_bytes());
        }
    }
}

/// Parse one command line into a [`ControlEvent`]. Pure (no IO) so it can be
/// unit-tested. The first whitespace run splits the command from its argument.
fn parse_command(line: &str) -> Result<ControlEvent, String> {
    let line = line.trim();
    if line.is_empty() {
        return Err("empty command".to_string());
    }
    let (cmd, arg) = match line.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    match cmd.to_lowercase().as_str() {
        "summon" => Ok(ControlEvent::Summon),
        "morning" => Ok(ControlEvent::Morning),
        "quit" | "exit" => Ok(ControlEvent::Quit),
        "say" if !arg.is_empty() => Ok(ControlEvent::Say(arg.to_string())),
        "say" => Err("say needs a message".to_string()),
        "ask" if !arg.is_empty() => Ok(ControlEvent::Ask(arg.to_string())),
        "ask" => Err("ask needs a question".to_string()),
        "state" if !arg.is_empty() => Ok(ControlEvent::SetState(arg.to_lowercase())),
        "state" => Err("state needs a name".to_string()),
        other => Err(format!("unknown command '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_argless_commands() {
        assert!(matches!(parse_command("summon"), Ok(ControlEvent::Summon)));
        assert!(matches!(parse_command("morning"), Ok(ControlEvent::Morning)));
        assert!(matches!(parse_command("quit"), Ok(ControlEvent::Quit)));
        assert!(matches!(parse_command("exit"), Ok(ControlEvent::Quit)));
    }

    #[test]
    fn parses_commands_with_args() {
        match parse_command("ask how do I list ports?") {
            Ok(ControlEvent::Ask(q)) => assert_eq!(q, "how do I list ports?"),
            other => panic!("expected Ask, got {other:?}"),
        }
        match parse_command("say  hello world  ") {
            Ok(ControlEvent::Say(m)) => assert_eq!(m, "hello world"),
            other => panic!("expected Say, got {other:?}"),
        }
        // command keyword is case-insensitive; state arg is lowercased
        match parse_command("STATE Jamming") {
            Ok(ControlEvent::SetState(s)) => assert_eq!(s, "jamming"),
            other => panic!("expected SetState, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_command("").is_err());
        assert!(parse_command("   ").is_err());
        assert!(parse_command("ask").is_err()); // missing question
        assert!(parse_command("say").is_err()); // missing message
        assert!(parse_command("bogus").is_err());
    }

    // Full IO path over a connected socket pair: client writes a command,
    // handle_client parses it, replies `ok`, and forwards the event.
    #[test]
    fn handle_client_roundtrip() {
        use std::io::{BufRead as _, Write as _};
        use std::sync::mpsc::channel;

        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(b"ask hello there\n").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();

        let (tx, rx) = channel();
        handle_client(server, &tx);

        match rx.try_recv() {
            Ok(ControlEvent::Ask(q)) => assert_eq!(q, "hello there"),
            other => panic!("expected Ask, got {other:?}"),
        }

        let mut reply = String::new();
        BufReader::new(&client).read_line(&mut reply).unwrap();
        assert_eq!(reply.trim(), "ok");
    }
}
