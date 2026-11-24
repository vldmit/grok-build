//! Callers must pass an already-sanitized payload.
//! These helpers only double ESC (`0x1b`).
//! They do not strip CAN (`0x18`), SUB (`0x1a`), C1 ST (`0x9c`), or BEL (`0x07`), any of which abort or terminate DCS/OSC in tmux/xterm.

use super::TerminalContext;

/// Wrap `sequence` in `\x1bPtmux;…\x1b\\`, doubling embedded ESC bytes.
#[must_use]
pub fn tmux_passthrough(sequence: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sequence.len() + 16);
    out.extend_from_slice(b"\x1bPtmux;");
    for &byte in sequence {
        if byte == 0x1b {
            out.push(0x1b);
        }
        out.push(byte);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// String form of [`tmux_passthrough`].
/// Builds the envelope in UTF-8; only ASCII is inserted and ESC doubling cannot split a multibyte sequence.
#[must_use]
pub fn tmux_passthrough_str(sequence: &str) -> String {
    let mut out = String::with_capacity(sequence.len() + 16);
    out.push_str("\x1bPtmux;");
    for ch in sequence.chars() {
        if ch == '\x1b' {
            out.push('\x1b');
        }
        out.push(ch);
    }
    out.push_str("\x1b\\");
    out
}

/// True when the immediate emulator is tmux 3.3 or later, the minimum for reliable DCS passthrough.
#[must_use]
pub fn passthrough_available(ctx: &TerminalContext) -> bool {
    ctx.is_tmux_backed() && ctx.is_tmux_version_or_later(3, 3)
}

/// OSC 11 queries are wrapped only in passthrough-capable tmux that is not an editor `:terminal`.
/// A bare query is answered by tmux itself. The wrap is only the fallback that asks the outer terminal.
#[must_use]
pub fn should_wrap_osc11(ctx: &TerminalContext) -> bool {
    passthrough_available(ctx) && ctx.embedded_editor.is_none()
}

/// Set from startup once tmux control mode has been probed.
/// Session-color OSC must be bare in `tmux -CC`: control mode copies pane bytes into `%output` verbatim and does not unwrap a DCS `tmux;` envelope, whether or not `allow-passthrough` is on.
static SESSION_COLOR_CONTROL_MODE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_session_color_control_mode(control_mode: bool) {
    SESSION_COLOR_CONTROL_MODE.store(control_mode, std::sync::atomic::Ordering::Release);
}

#[must_use]
pub fn session_color_control_mode() -> bool {
    SESSION_COLOR_CONTROL_MODE.load(std::sync::atomic::Ordering::Acquire)
}

/// Wrap OSC 10/11/110/111 only for a normal tmux client that can passthrough.
/// `tmux -CC` must receive the sequence bare. An editor `:terminal` must not be wrapped either.
#[must_use]
pub fn should_wrap_session_color(ctx: &TerminalContext, control_mode: bool) -> bool {
    !control_mode && passthrough_available(ctx) && ctx.embedded_editor.is_none()
}

/// Bare sequence, or the same bytes inside the tmux DCS passthrough envelope.
#[must_use]
pub fn encode_session_color(sequence: &str, wrap: bool) -> String {
    if wrap {
        tmux_passthrough_str(sequence)
    } else {
        sequence.to_string()
    }
}

/// DEC 2026 synchronized output is skipped only when tmux is the immediate terminal, in any tmux version: tmux repaints
/// the whole pane when a block closes and already synchronizes its own output toward the outer terminal, so the
/// pane-level wrapper only multiplies traffic. Inside an editor `:terminal` the editor's emulator is the immediate
/// terminal even though `TMUX` is inherited, so the wrapper stays.
#[must_use]
pub fn should_emit_synchronized_output(ctx: &TerminalContext) -> bool {
    !(ctx.is_tmux_backed() && ctx.embedded_editor.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{EmbeddedEditor, MultiplexerKind, TerminalContext};

    #[test]
    fn doubles_esc_and_wraps() {
        assert_eq!(
            tmux_passthrough(b"\x1b]11;?\x07"),
            b"\x1bPtmux;\x1b\x1b]11;?\x07\x1b\\"
        );
    }

    #[test]
    fn plain_text_has_no_esc_to_double() {
        assert_eq!(tmux_passthrough(b"plain"), b"\x1bPtmux;plain\x1b\\");
    }

    #[test]
    fn doubles_every_esc_including_st() {
        assert_eq!(
            tmux_passthrough(b"\x1b]777;notify;t;b\x1b\\"),
            b"\x1bPtmux;\x1b\x1b]777;notify;t;b\x1b\x1b\\\x1b\\"
        );
    }

    #[test]
    fn str_wrapper_matches_byte_wrapper() {
        let seq = "\x1b]9;task done\x07";
        assert_eq!(
            tmux_passthrough_str(seq).as_bytes(),
            tmux_passthrough(seq.as_bytes())
        );
        assert_eq!(tmux_passthrough_str("plain"), "\x1bPtmux;plain\x1b\\");
    }

    #[test]
    fn passthrough_available_requires_tmux_33() {
        let tmux_33 = TerminalContext {
            multiplexer: MultiplexerKind::Tmux,
            tmux_version: Some("tmux 3.3".into()),
            ..Default::default()
        };
        assert!(passthrough_available(&tmux_33));
        assert!(should_wrap_osc11(&tmux_33));

        let tmux_32 = TerminalContext {
            multiplexer: MultiplexerKind::Tmux,
            tmux_version: Some("tmux 3.2".into()),
            ..Default::default()
        };
        assert!(!passthrough_available(&tmux_32));
        assert!(!should_wrap_osc11(&tmux_32));

        let tmux_version_unknown = TerminalContext {
            multiplexer: MultiplexerKind::Tmux,
            tmux_version: None,
            ..Default::default()
        };
        assert!(!passthrough_available(&tmux_version_unknown));

        assert!(!passthrough_available(&TerminalContext::default()));
    }

    #[test]
    fn should_wrap_osc11_skips_embedded_editor() {
        let nvim = TerminalContext {
            multiplexer: MultiplexerKind::Tmux,
            tmux_version: Some("tmux 3.4".into()),
            embedded_editor: Some(EmbeddedEditor::Neovim),
            ..Default::default()
        };
        assert!(passthrough_available(&nvim));
        assert!(!should_wrap_osc11(&nvim));
        assert!(!should_wrap_session_color(&nvim, false));
    }

    #[test]
    fn session_color_stays_bare_in_control_mode_and_before_tmux_33() {
        let tmux_38 = TerminalContext {
            multiplexer: MultiplexerKind::Tmux,
            tmux_version: Some("tmux 3.8".into()),
            ..Default::default()
        };
        assert!(should_wrap_session_color(&tmux_38, false));
        assert!(!should_wrap_session_color(&tmux_38, true));

        let tmux_32 = TerminalContext {
            multiplexer: MultiplexerKind::Tmux,
            tmux_version: Some("tmux 3.2".into()),
            ..Default::default()
        };
        assert!(!should_wrap_session_color(&tmux_32, false));
        assert!(!should_wrap_session_color(
            &TerminalContext::default(),
            false
        ));
    }

    #[test]
    fn encode_session_color_wraps_only_when_asked() {
        let bare = "\x1b]11;rgb:14/14/14\x07";
        assert_eq!(encode_session_color(bare, false), bare);
        assert_eq!(
            encode_session_color(bare, true),
            "\x1bPtmux;\x1b\x1b]11;rgb:14/14/14\x07\x1b\\"
        );
    }

    #[test]
    fn synchronized_output_skipped_in_any_tmux_version() {
        for tmux_version in [None, Some("tmux 3.2".into()), Some("tmux 3.6".into())] {
            let tmux = TerminalContext {
                multiplexer: MultiplexerKind::Tmux,
                tmux_version,
                ..Default::default()
            };
            assert!(!should_emit_synchronized_output(&tmux));
        }
    }

    #[test]
    fn synchronized_output_kept_in_editor_terminal_inside_tmux() {
        for editor in [
            EmbeddedEditor::Neovim,
            EmbeddedEditor::Vim,
            EmbeddedEditor::Emacs,
        ] {
            let ctx = TerminalContext {
                multiplexer: MultiplexerKind::Tmux,
                tmux_version: Some("tmux 3.6".into()),
                embedded_editor: Some(editor),
                ..Default::default()
            };
            assert!(
                should_emit_synchronized_output(&ctx),
                "{editor:?} is the immediate terminal, not tmux"
            );
        }
    }

    #[test]
    fn synchronized_output_emitted_outside_tmux() {
        assert!(should_emit_synchronized_output(&TerminalContext::default()));

        for multiplexer in [
            MultiplexerKind::Zellij,
            MultiplexerKind::Screen,
            MultiplexerKind::Cmux,
            MultiplexerKind::Herdr,
        ] {
            let ctx = TerminalContext {
                multiplexer,
                ..Default::default()
            };
            assert!(
                should_emit_synchronized_output(&ctx),
                "{multiplexer:?} must keep synchronized output"
            );
        }
    }
}
