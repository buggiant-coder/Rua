use std::io::{Error, ErrorKind, Result, Write};

use rua_channel::core::console::{AnsiRenderer, TerminalInput, TerminalOutput};

#[test]
fn terminal_input_trims_empty_lines_and_recognizes_commands() {
    assert_eq!(
        TerminalInput::parse_line("  hello terminal  "),
        Some("hello terminal".into())
    );
    assert_eq!(TerminalInput::parse_line("\n\t "), None);

    for input in ["exit", " quit ", "Q", ":q", ":quit"] {
        assert!(TerminalInput::is_exit(input), "{input} should exit");
    }

    assert!(TerminalInput::is_command(" /status"));
    assert!(!TerminalInput::is_command("not /status"));
}

#[test]
fn terminal_output_writes_expected_frames_to_usable_writer() {
    let mut buffer = Vec::new();
    {
        let mut output = TerminalOutput::new(&mut buffer);
        output.write_user_input("hello").expect("write user input");
        output
            .write_assistant_response("world")
            .expect("write assistant response");
        output.write_error("boom").expect("write error");
        output.write_thinking("working").expect("write thinking");
        output.write_raw("raw").expect("write raw");
        output.flush().expect("flush output");
    }

    let rendered = String::from_utf8(buffer).expect("utf8 output");
    let plain = AnsiRenderer::strip_ansi(&rendered);
    assert!(plain.contains("You: hello\n"));
    assert!(plain.contains("Assistant: world\n"));
    assert!(plain.contains("Error: boom\n"));
    assert!(plain.contains("Thinking: working\n"));
    assert!(plain.ends_with("raw"));
}

#[test]
fn terminal_output_propagates_broken_writer_errors() {
    let mut output = TerminalOutput::new(BrokenWriter);

    let error = output
        .write_assistant_response("will fail")
        .expect_err("broken writer should fail");
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
}

struct BrokenWriter;

impl Write for BrokenWriter {
    fn write(&mut self, _buf: &[u8]) -> Result<usize> {
        Err(Error::new(ErrorKind::BrokenPipe, "broken terminal"))
    }

    fn flush(&mut self) -> Result<()> {
        Err(Error::new(ErrorKind::BrokenPipe, "broken terminal"))
    }
}
