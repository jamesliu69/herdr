use std::io::{self, BufRead};
use std::sync::atomic::AtomicBool;

use base64::Engine;
use interprocess::local_socket::traits::Stream as _;
use interprocess::TryClone as _;
use tracing::{info, warn};

use crate::ipc::LocalStream;
use crate::protocol::{
    self, AttachScrollDirection, AttachScrollSource, ClientMessage, ClientMouseButton,
    ClientMouseKind, ClientMousePosition, RenderEncoding, ServerMessage, MAX_GRAPHICS_FRAME_SIZE,
};
use crate::server::socket_paths::client_socket_path;

use super::transport::ClientMessageSink;
use super::{do_handshake, init_logging, write_to_server, ClientError, HandshakePixelCapabilities};

/// Runs a read-only terminal session observer and prints one JSON envelope per frame.
pub fn run_terminal_session_observe(target: String, cols: u16, rows: u16) -> io::Result<()> {
    let mut stream =
        connect_terminal_session_stream(target.clone(), cols, rows, "observing terminal session")?;
    write_to_server(&mut stream, &ClientMessage::ObserveTerminal { target })?;
    write_terminal_session_output(stream)
}

/// Runs a writable terminal session controller.
pub fn run_terminal_session_control(
    target: String,
    takeover: bool,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let mut stream = connect_terminal_session_stream(
        target.clone(),
        cols,
        rows,
        "controlling terminal session",
    )?;
    write_to_server(
        &mut stream,
        &ClientMessage::ControlTerminal { target, takeover },
    )?;

    let mut write_stream = stream.try_clone()?;
    let _input_thread = std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        forward_terminal_control_commands(&mut input, &mut write_stream);
    });

    write_terminal_session_output(stream)
}

/// Hard cap for one JSON control command line read from stdin.
///
/// The server already caps decoded terminal input at 1 MiB, so no valid command
/// comes close to this. The cap keeps a malformed stdin stream that never emits
/// a newline from growing the line buffer without bound.
const MAX_CONTROL_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlLine {
    /// A complete line (or the final unterminated line) was read.
    Read,
    /// The line exceeded [`MAX_CONTROL_LINE_BYTES`]; only the capped prefix is buffered.
    TooLong,
    /// End of input was reached before any byte of a new line was read.
    Eof,
}

/// Reads one newline-terminated command from `input` into `buffer`, never
/// letting `buffer` grow past [`MAX_CONTROL_LINE_BYTES`].
fn read_bounded_control_line(
    input: &mut impl BufRead,
    buffer: &mut Vec<u8>,
) -> io::Result<ControlLine> {
    buffer.clear();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(if buffer.is_empty() {
                ControlLine::Eof
            } else {
                ControlLine::Read
            });
        }
        let (has_newline, chunk_len) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (true, index + 1),
            None => (false, available.len()),
        };
        let remaining = MAX_CONTROL_LINE_BYTES.saturating_sub(buffer.len());
        if chunk_len > remaining {
            buffer.extend_from_slice(&available[..remaining]);
            input.consume(remaining);
            return Ok(ControlLine::TooLong);
        }
        buffer.extend_from_slice(&available[..chunk_len]);
        input.consume(chunk_len);
        if has_newline {
            return Ok(ControlLine::Read);
        }
    }
}

/// Forwards JSON command lines from `input` to the server until stdin ends, a
/// line exceeds the cap, or the server write fails.
fn forward_terminal_control_commands(
    input: &mut impl BufRead,
    write_stream: &mut impl ClientMessageSink,
) {
    let mut buffer = Vec::new();
    loop {
        match read_bounded_control_line(input, &mut buffer) {
            Ok(ControlLine::Read) => {}
            Ok(ControlLine::Eof) => break,
            Ok(ControlLine::TooLong) => {
                warn!(
                    limit = MAX_CONTROL_LINE_BYTES,
                    "terminal session control input line exceeded the limit; detaching"
                );
                break;
            }
            Err(err) => {
                warn!(error = %err, "terminal session control input read failed");
                break;
            }
        }
        let mut line = buffer.as_slice();
        line = line.strip_suffix(b"\n").unwrap_or(line);
        line = line.strip_suffix(b"\r").unwrap_or(line);
        let Ok(line) = std::str::from_utf8(line) else {
            warn!("terminal session control input was not valid UTF-8; detaching");
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        match terminal_control_command_from_json(line) {
            Ok(message) => {
                let release = matches!(message, ClientMessage::Detach);
                if write_to_server(write_stream, &message).is_err() {
                    return;
                }
                if release {
                    return;
                }
            }
            Err(err) => eprintln!("herdr: terminal session control input ignored: {err}"),
        }
    }
    let _ = write_to_server(write_stream, &ClientMessage::Detach);
}

fn connect_terminal_session_stream(
    target: String,
    cols: u16,
    rows: u16,
    log_message: &'static str,
) -> io::Result<LocalStream> {
    init_logging();

    let socket_path = client_socket_path();
    crate::logging::startup("client");
    info!(path = %socket_path.display(), target = %target, cols, rows, "{log_message}");

    let mut stream = match crate::ipc::connect_local_stream(&socket_path) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("herdr: {}", ClientError::ConnectionFailed(err));
            std::process::exit(1);
        }
    };

    match do_handshake(
        &mut stream,
        cols,
        rows,
        0,
        0,
        HandshakePixelCapabilities {
            exact_geometry: false,
            sgr_mouse: false,
        },
        None,
        false,
        false,
        true,
        true,
        &AtomicBool::new(false),
    ) {
        Ok(handshake) if handshake.encoding == RenderEncoding::TerminalAnsi => {}
        Ok(handshake) => {
            eprintln!(
                "herdr: terminal session observe negotiated unsupported encoding {:?}",
                handshake.encoding
            );
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("herdr: {err}");
            std::process::exit(1);
        }
    }

    stream.set_nonblocking(false)?;
    Ok(stream)
}

fn write_terminal_session_output(stream: LocalStream) -> io::Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    forward_terminal_session_output(stream, &mut stdout)
}

/// Streams server output to `stdout` until the server shuts down or disconnects.
fn forward_terminal_session_output(
    mut stream: impl io::Read,
    stdout: &mut impl io::Write,
) -> io::Result<()> {
    loop {
        match protocol::read_message(&mut stream, MAX_GRAPHICS_FRAME_SIZE) {
            Ok(ServerMessage::Terminal(frame)) => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(&frame.bytes);
                let line = serde_json::json!({
                    "type": "terminal.frame",
                    "seq": frame.seq,
                    "encoding": "ansi",
                    "width": frame.width,
                    "height": frame.height,
                    "full": frame.full,
                    "bytes": encoded,
                });
                serde_json::to_writer(&mut *stdout, &line)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }
            Ok(ServerMessage::ServerShutdown { reason }) => {
                let line = serde_json::json!({
                    "type": "terminal.closed",
                    "reason": reason,
                });
                serde_json::to_writer(&mut *stdout, &line)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
                return Ok(());
            }
            Ok(ServerMessage::Graphics { .. }) => {}
            Ok(_) => {}
            Err(protocol::FramingError::UnexpectedEof) => return Ok(()),
            Err(err) => return Err(io::Error::other(err.to_string())),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "type")]
enum TerminalControlCommand {
    #[serde(rename = "terminal.input")]
    Input {
        text: Option<String>,
        bytes: Option<String>,
    },
    #[serde(rename = "terminal.resize")]
    Resize {
        cols: u16,
        rows: u16,
        #[serde(default)]
        cell_width_px: u32,
        #[serde(default)]
        cell_height_px: u32,
    },
    #[serde(rename = "terminal.scroll")]
    Scroll {
        direction: TerminalControlScrollDirection,
        lines: u16,
        #[serde(default)]
        source: TerminalControlScrollSource,
        #[serde(default)]
        column: Option<u16>,
        #[serde(default)]
        row: Option<u16>,
        #[serde(default)]
        modifiers: u8,
    },
    #[serde(rename = "terminal.mouse")]
    Mouse {
        action: TerminalControlMouseAction,
        #[serde(default)]
        button: TerminalControlMouseButton,
        column: u16,
        row: u16,
        #[serde(default)]
        modifiers: u8,
    },
    #[serde(rename = "terminal.release")]
    Release {},
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlMouseAction {
    Down,
    Up,
    Drag,
    Move,
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlMouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalControlScrollSource {
    #[default]
    Wheel,
    PageKey,
}

pub(super) fn terminal_control_command_from_json(raw: &str) -> Result<ClientMessage, String> {
    let command = serde_json::from_str::<TerminalControlCommand>(raw)
        .map_err(|err| format!("invalid json command: {err}"))?;
    match command {
        TerminalControlCommand::Input { text, bytes } => {
            let data = match (text, bytes) {
                (Some(_), Some(_)) => {
                    return Err("terminal.input accepts text or bytes, not both".into())
                }
                (Some(text), None) => text.into_bytes(),
                (None, Some(bytes)) => base64::engine::general_purpose::STANDARD
                    .decode(bytes)
                    .map_err(|err| format!("invalid terminal.input bytes: {err}"))?,
                (None, None) => Vec::new(),
            };
            Ok(ClientMessage::Input { data })
        }
        TerminalControlCommand::Resize {
            cols,
            rows,
            cell_width_px,
            cell_height_px,
        } => {
            if cols == 0 || rows == 0 {
                return Err("terminal.resize cols and rows must be greater than 0".into());
            }
            Ok(ClientMessage::Resize {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse: false,
            })
        }
        TerminalControlCommand::Scroll {
            direction,
            lines,
            source,
            column,
            row,
            modifiers,
        } => {
            if lines == 0 {
                return Err("terminal.scroll lines must be greater than 0".into());
            }
            let direction = match direction {
                TerminalControlScrollDirection::Up => AttachScrollDirection::Up,
                TerminalControlScrollDirection::Down => AttachScrollDirection::Down,
            };
            let source = match source {
                TerminalControlScrollSource::Wheel => AttachScrollSource::Wheel,
                TerminalControlScrollSource::PageKey => AttachScrollSource::PageKey {
                    input: match direction {
                        AttachScrollDirection::Up => b"\x1b[5~".to_vec(),
                        AttachScrollDirection::Down => b"\x1b[6~".to_vec(),
                    },
                },
            };
            Ok(ClientMessage::AttachScroll {
                source,
                direction,
                lines,
                column,
                row,
                modifiers,
            })
        }
        TerminalControlCommand::Mouse {
            action,
            button,
            column,
            row,
            modifiers,
        } => {
            let button = match button {
                TerminalControlMouseButton::Left => ClientMouseButton::Left,
                TerminalControlMouseButton::Right => ClientMouseButton::Right,
                TerminalControlMouseButton::Middle => ClientMouseButton::Middle,
            };
            let kind = match action {
                TerminalControlMouseAction::Down => ClientMouseKind::Down(button),
                TerminalControlMouseAction::Up => ClientMouseKind::Up(button),
                TerminalControlMouseAction::Drag => ClientMouseKind::Drag(button),
                TerminalControlMouseAction::Move => ClientMouseKind::Moved,
            };
            // The server encodes the event for the pane's mouse mode and drops it
            // when the application has not enabled mouse reporting.
            Ok(ClientMessage::AttachMouse {
                kind,
                position: ClientMousePosition::Cell { column, row },
                geometry: None,
                modifiers,
                lines: 1,
            })
        }
        TerminalControlCommand::Release {} => Ok(ClientMessage::Detach),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TerminalFrame;

    #[derive(Default)]
    struct RecordingSink {
        messages: Vec<ClientMessage>,
    }

    impl ClientMessageSink for RecordingSink {
        fn send_client_message(&mut self, message: &ClientMessage) -> io::Result<()> {
            self.messages.push(message.clone());
            Ok(())
        }
    }

    #[test]
    fn bounded_control_line_caps_unterminated_input() {
        let data = vec![b'x'; MAX_CONTROL_LINE_BYTES * 2 + 1];
        let mut input = io::Cursor::new(data);
        let mut buffer = Vec::new();
        assert_eq!(
            read_bounded_control_line(&mut input, &mut buffer).unwrap(),
            ControlLine::TooLong
        );
        assert_eq!(buffer.len(), MAX_CONTROL_LINE_BYTES);
        assert_eq!(input.position(), MAX_CONTROL_LINE_BYTES as u64);
    }

    #[test]
    fn bounded_control_line_reads_whole_lines_and_trailing_eof() {
        let mut input = io::Cursor::new(b"one\ntwo".to_vec());
        let mut buffer = Vec::new();
        assert_eq!(
            read_bounded_control_line(&mut input, &mut buffer).unwrap(),
            ControlLine::Read
        );
        assert_eq!(buffer, b"one\n");
        assert_eq!(
            read_bounded_control_line(&mut input, &mut buffer).unwrap(),
            ControlLine::Read
        );
        assert_eq!(buffer, b"two");
        assert_eq!(
            read_bounded_control_line(&mut input, &mut buffer).unwrap(),
            ControlLine::Eof
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn control_commands_stop_at_the_line_cap_and_detach() {
        let mut input = io::Cursor::new(vec![b'x'; MAX_CONTROL_LINE_BYTES + 64]);
        let mut sink = RecordingSink::default();
        forward_terminal_control_commands(&mut input, &mut sink);
        assert_eq!(sink.messages, vec![ClientMessage::Detach]);
    }

    #[test]
    fn control_commands_forward_before_eof() {
        let mut input =
            io::Cursor::new(b"{\"type\":\"terminal.input\",\"text\":\"hi\"}\n".to_vec());
        let mut sink = RecordingSink::default();
        forward_terminal_control_commands(&mut input, &mut sink);
        assert_eq!(
            sink.messages,
            vec![
                ClientMessage::Input {
                    data: b"hi".to_vec()
                },
                ClientMessage::Detach
            ]
        );
    }

    #[test]
    fn terminal_session_output_returns_on_empty_stream() {
        let mut input: &[u8] = &[];
        let mut output = Vec::new();
        forward_terminal_session_output(&mut input, &mut output).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn terminal_session_output_reports_server_shutdown() {
        let mut frames = Vec::new();
        protocol::write_message(
            &mut frames,
            &ServerMessage::ServerShutdown {
                reason: Some("server stopped".into()),
            },
        )
        .unwrap();
        let mut input = frames.as_slice();
        let mut output = Vec::new();
        forward_terminal_session_output(&mut input, &mut output).unwrap();
        let line: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(line["type"], "terminal.closed");
        assert_eq!(line["reason"], "server stopped");
    }

    #[test]
    fn terminal_session_output_encodes_frames_as_json_lines() {
        let frame = TerminalFrame {
            seq: 7,
            width: 80,
            height: 24,
            full: true,
            bytes: vec![b'A', 0x1b],
        };
        let mut frames = Vec::new();
        protocol::write_message(&mut frames, &ServerMessage::Terminal(frame)).unwrap();
        let mut input = frames.as_slice();
        let mut output = Vec::new();
        forward_terminal_session_output(&mut input, &mut output).unwrap();
        let line: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(line["seq"], 7);
        assert_eq!(line["encoding"], "ansi");
        assert_eq!(line["width"], 80);
        assert_eq!(line["height"], 24);
        assert_eq!(line["full"], true);
        assert_eq!(line["bytes"], "QRs=");
    }
}
