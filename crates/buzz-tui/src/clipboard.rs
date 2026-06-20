use std::io::{self, Write};
use std::process::{Command, Stdio};

use base64::Engine;

/// Copy text through terminal-native paths so the TUI can offer scoped copies
/// without adding a platform clipboard dependency. Terminals or tmux configs
/// that disable clipboard writes may still ignore the emitted sequences.
pub fn copy_text(text: &str) -> io::Result<()> {
    if std::env::var_os("TMUX").is_some() {
        let _ = copy_to_tmux_buffer(text);
    }

    let sequence = if std::env::var_os("TMUX").is_some() {
        tmux_passthrough_sequence(text)
    } else {
        osc52_sequence(text)
    };
    let mut stdout = io::stdout();
    write!(stdout, "{sequence}")?;
    stdout.flush()
}

fn copy_to_tmux_buffer(text: &str) -> io::Result<()> {
    let mut child = Command::new("tmux")
        .args(["load-buffer", "-w", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("tmux load-buffer failed"))
    }
}

fn osc52_sequence(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}

fn tmux_passthrough_sequence(text: &str) -> String {
    format!("\x1bPtmux;\x1b{}\x1b\\", osc52_sequence(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_payload_is_base64_encoded() {
        assert_eq!(
            osc52_sequence("hello\nworld"),
            "\x1b]52;c;aGVsbG8Kd29ybGQ=\x07"
        );
    }

    #[test]
    fn tmux_passthrough_wraps_osc52_sequence() {
        assert_eq!(
            tmux_passthrough_sequence("hello"),
            "\x1bPtmux;\x1b\x1b]52;c;aGVsbG8=\x07\x1b\\"
        );
    }
}
