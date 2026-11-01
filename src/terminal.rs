//! Bounded PTY transport + vt100 emulation. Child bytes never reach the host terminal.
use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    sync::mpsc::{SyncSender, TrySendError},
};

const MAX_INPUT: usize = 65_536;
const SCROLLBACK: usize = 500;
const MAX_OSC_BYTES: usize = 4096;
const MAX_CSI_BYTES: usize = 1024;
/// How long an exit waits for the end of its output, and the end of output for an exit. A process
/// left in the background can hold the terminal open, so the end of output may never come.
const EXIT_SETTLE: std::time::Duration = std::time::Duration::from_millis(200);

#[derive(Debug, Default, PartialEq, Eq)]
enum OutputState {
    #[default]
    Ground,
    Escape,
    Csi,
    DiscardCsi,
    Osc(usize),
    DiscardOsc,
}

/// vte's std-enabled OSC buffer is unbounded, even when callbacks ignore OSC.
/// Limit bytes before they reach the parser, retaining only state across reads.
#[derive(Default)]
struct OutputGuard {
    state: OutputState,
    csi: Vec<u8>,
}

impl OutputGuard {
    fn process<CB: vt100::Callbacks>(&mut self, parser: &mut vt100::Parser<CB>, bytes: &[u8]) {
        let mut start = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            if matches!(self.state, OutputState::Csi | OutputState::DiscardCsi) {
                start = index + 1;
                match byte {
                    0x1b => {
                        parser.process(b"\x18\x1b");
                        self.csi.clear();
                        self.state = OutputState::Escape;
                    }
                    0x18 | 0x1a => {
                        parser.process(&[byte]);
                        self.csi.clear();
                        self.state = OutputState::Ground;
                    }
                    0x00..=0x1f => parser.process(&[byte]),
                    0x40..=0x7e => {
                        if self.state == OutputState::Csi {
                            self.finish_csi(parser, byte);
                        }
                        self.csi.clear();
                        self.state = OutputState::Ground;
                    }
                    0x20..=0x3f if self.state == OutputState::Csi => {
                        if self.csi.len() == MAX_CSI_BYTES {
                            parser.process(b"\x18");
                            self.csi.clear();
                            self.state = OutputState::DiscardCsi;
                        } else {
                            self.csi.push(byte);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            self.state = match self.state {
                OutputState::Ground if byte == 0x1b => OutputState::Escape,
                OutputState::Escape => match byte {
                    b']' => OutputState::Osc(0),
                    b'[' => {
                        parser.process(&bytes[start..=index]);
                        start = index + 1;
                        OutputState::Csi
                    }
                    // vte stays in Escape across these controls/ignored bytes.
                    0x00..=0x17 | 0x19 | 0x1b..=0x1f | 0x7f..=0xff => OutputState::Escape,
                    _ => OutputState::Ground,
                },
                OutputState::Osc(_) if byte == 0x1b => OutputState::Escape,
                OutputState::Osc(_) if matches!(byte, 0x07 | 0x18 | 0x1a) => OutputState::Ground,
                OutputState::Osc(count) if count < MAX_OSC_BYTES => OutputState::Osc(count + 1),
                OutputState::Osc(_) => {
                    parser.process(&bytes[start..index]);
                    // CAN ends the current OSC and releases its contents. Do not
                    // reset the screen or interpret the discarded tail as text.
                    parser.process(b"\x18");
                    start = index + 1;
                    OutputState::DiscardOsc
                }
                OutputState::DiscardOsc => {
                    start = index + 1;
                    match byte {
                        // Like vte, any ESC ends OSC and begins a new escape;
                        // preserve it, including when ESC and '\\' span reads.
                        0x1b => {
                            start = index;
                            OutputState::Escape
                        }
                        0x07 | 0x18 | 0x1a => OutputState::Ground,
                        _ => OutputState::DiscardOsc,
                    }
                }
                OutputState::Ground => OutputState::Ground,
                OutputState::Csi | OutputState::DiscardCsi => unreachable!(),
            };
        }
        parser.process(&bytes[start..]);
    }

    fn finish_csi<CB: vt100::Callbacks>(&self, parser: &mut vt100::Parser<CB>, code: u8) {
        // Only plain ICH/IL/SD reach vt100's costly insertion loops. Private
        // and intermediate CSI forms have different meanings and stay intact.
        if matches!(code, b'@' | b'L' | b'T')
            && self
                .csi
                .iter()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b';' | b':'))
        {
            let count = self
                .csi
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .fold(0u16, |count, byte| {
                    count
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'))
                })
                .max(1);
            let (rows, cols) = parser.screen().size();
            let limit = if code == b'@' {
                cols.saturating_sub(parser.screen().cursor_position().1)
                    .max(1)
            } else {
                rows
            };
            parser.process(count.min(limit).to_string().as_bytes());
        } else {
            parser.process(&self.csi);
        }
        parser.process(&[code]);
    }
}

pub enum State {
    Running,
    Exited(u32),
    Closed,
    Disconnected,
    Failed(String),
}
enum Output {
    Data(Vec<u8>),
    Eof,
    Error(String),
}

#[derive(Default)]
pub struct Replies {
    pending: Vec<Vec<u8>>,
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        code: char,
    ) {
        if self.pending.len() >= 8 {
            return;
        }
        let parameter = params
            .first()
            .and_then(|value| value.first())
            .copied()
            .unwrap_or(0);
        let response = match (i1, code, parameter) {
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                Some(format!("\x1b[{};{}R", row + 1, col + 1))
            }
            (None, 'n', 5) => Some("\x1b[0n".into()),
            (None, 'c', _) => Some("\x1b[?1;2c".into()),
            (Some(b'>'), 'c', _) => Some("\x1b[>0;0;0c".into()),
            (None, 't', 18) => {
                let (rows, cols) = screen.size();
                Some(format!("\x1b[8;{rows};{cols}t"))
            }
            _ => None,
        };
        if let Some(response) = response {
            self.pending.push(response.into_bytes());
        }
    }
    // OSC clipboard, window-title, and host-resize requests are deliberately ignored.
}

pub struct Session {
    pub request: crate::exec::Request,
    pub state: State,
    parser: vt100::Parser<Replies>,
    output_guard: OutputGuard,
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn Child + Send + Sync>>,
    input: Option<SyncSender<Vec<u8>>>,
    output: tokio::sync::mpsc::Receiver<Output>,
    output_changed: bool,
    /// When the end of the output was seen.
    output_ended: Option<std::time::Instant>,
    /// The exit status, and when it was collected, until the output has caught up with it.
    exit: Option<(u32, std::time::Instant)>,
    size: (u16, u16),
}

impl Session {
    pub fn start(
        request: crate::exec::Request,
        command: std::process::Command,
        rows: u16,
        cols: u16,
        output_ready: std::sync::Arc<tokio::sync::Notify>,
    ) -> Result<Self> {
        let size = dimensions(rows, cols);
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows: size.0,
                cols: size.1,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Cannot create an embedded terminal")?;
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let mut builder = CommandBuilder::new(command.get_program());
        builder.args(command.get_args());
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                builder.env(key, value);
            } else {
                builder.env_remove(key);
            }
        }
        if let Some(cwd) = command.get_current_dir() {
            builder.cwd(cwd);
        }
        builder.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(builder)
            .context("Cannot start runtime CLI inside the embedded terminal")?;
        drop(pair.slave);
        let (output_tx, output) = tokio::sync::mpsc::channel(64);
        let (input, input_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(128);
        let errors = output_tx.clone();
        std::thread::spawn(move || {
            while let Ok(bytes) = input_rx.recv() {
                if let Err(error) = writer.write_all(&bytes).and_then(|_| writer.flush()) {
                    let _ =
                        errors.blocking_send(Output::Error(format!("Exec input failed: {error}")));
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ = output_tx.blocking_send(Output::Eof);
                        output_ready.notify_one();
                        break;
                    }
                    Ok(count) => {
                        if output_tx
                            .blocking_send(Output::Data(buffer[..count].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                        output_ready.notify_one();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.raw_os_error() == Some(5) => {
                        let _ = output_tx.blocking_send(Output::Eof);
                        output_ready.notify_one();
                        break;
                    }
                    Err(error) => {
                        let _ = output_tx
                            .blocking_send(Output::Error(format!("Exec output failed: {error}")));
                        output_ready.notify_one();
                        break;
                    }
                }
            }
        });
        Ok(Self {
            request,
            state: State::Running,
            parser: vt100::Parser::new_with_callbacks(
                size.0,
                size.1,
                SCROLLBACK,
                Replies::default(),
            ),
            master: pair.master,
            output_guard: OutputGuard::default(),
            child: Some(child),
            input: Some(input),
            output,
            output_changed: false,
            output_ended: None,
            exit: None,
            size,
        })
    }

    pub fn running(&self) -> bool {
        matches!(self.state, State::Running)
    }
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
    pub fn status(&self) -> String {
        match &self.state {
            State::Running => "Connected".into(),
            State::Exited(130) => "Interrupted (130)".into(),
            State::Exited(code) => format!("Exited {code}"),
            State::Closed => "Closed".into(),
            State::Disconnected => "Disconnected".into(),
            State::Failed(error) => format!("Failed: {error}"),
        }
    }
    pub fn same_target(&self, request: &crate::exec::Request) -> bool {
        self.request.connection.key() == request.connection.key()
            && self.request.container.key() == request.container.key()
    }
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        let size = dimensions(rows, cols);
        if size != self.size {
            self.master.resize(PtySize {
                rows: size.0,
                cols: size.1,
                pixel_width: 0,
                pixel_height: 0,
            })?;
            self.parser.screen_mut().set_size(size.0, size.1);
            self.size = size;
        }
        Ok(())
    }
    pub fn send(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.enqueue(bytes)?;
        self.follow();
        Ok(())
    }
    fn enqueue(&self, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if !self.running() {
            bail!("Exec is not connected");
        }
        if bytes.len() > MAX_INPUT {
            bail!("Paste exceeds 64 KiB; nothing was sent");
        }
        match self
            .input
            .as_ref()
            .context("Exec input closed")?
            .try_send(bytes)
        {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                bail!("Exec input queue is busy; this input was not sent")
            }
            Err(TrySendError::Disconnected(_)) => bail!("Exec input disconnected"),
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Result<()> {
        self.send(encode_key(key, self.screen().application_cursor()))
    }
    pub fn paste(&mut self, text: &str) -> Result<()> {
        if text.len() > MAX_INPUT - 12 {
            bail!("Paste exceeds 64 KiB; nothing was sent");
        }
        let bytes = if self.screen().bracketed_paste() {
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.as_bytes().to_vec()
        };
        self.send(bytes)
    }
    pub fn scroll(&mut self, amount: isize) {
        let offset = self.screen().scrollback().saturating_add_signed(amount);
        self.parser
            .screen_mut()
            .set_scrollback(offset.min(SCROLLBACK));
    }
    pub fn follow(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    pub fn poll(&mut self) -> bool {
        let previous_status = self.status();
        for _ in 0..64 {
            match self.output.try_recv() {
                Ok(Output::Data(bytes)) => {
                    self.output_guard.process(&mut self.parser, &bytes);
                    self.output_changed = true;
                }
                Ok(Output::Eof) => self.output_ended = Some(std::time::Instant::now()),
                Ok(Output::Error(error)) => {
                    if self.running() {
                        self.state = State::Failed(error);
                    }
                }
                Err(_) => break,
            }
        }
        for response in std::mem::take(&mut self.parser.callbacks_mut().pending) {
            let _ = self.enqueue(response);
        }
        let running = self.running();
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.exit = Some((status.exit_code(), std::time::Instant::now()));
                    self.child = None;
                    self.input = None;
                }
                Err(error) => {
                    self.state = State::Failed(format!("Cannot read exec status: {error}"))
                }
                // The output of an exiting process ends just before its exit can be collected.
                Ok(None)
                    if running
                        && self
                            .output_ended
                            .is_some_and(|at| at.elapsed() >= EXIT_SETTLE) =>
                {
                    self.state = State::Disconnected;
                }
                _ => {}
            }
        }
        // Whoever reacts to an exit reads the screen, so the output has to be there by then.
        if let Some((code, at)) = self.exit
            && (self.output_ended.is_some() || at.elapsed() >= EXIT_SETTLE)
        {
            self.state = State::Exited(code);
            self.exit = None;
        }
        previous_status != self.status()
    }

    /// Whether new output arrived since the last call, so whether the screen needs drawing.
    pub fn take_output_changed(&mut self) -> bool {
        std::mem::take(&mut self.output_changed)
    }

    pub fn close(&mut self) {
        self.input = None;
        self.exit = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        self.state = State::Closed;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// At least 2 by 2: vt100 panics on a screen with one row or column, and whatever a shell prints
/// reaches it, on the thread that draws the UI. A panel that small shows too little to matter.
fn dimensions(rows: u16, cols: u16) -> (u16, u16) {
    (rows.clamp(2, 256), cols.clamp(2, 512))
}

pub fn encode_key(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let modifier = 1
        + u8::from(key.modifiers.contains(KeyModifiers::SHIFT))
        + u8::from(alt) * 2
        + u8::from(ctrl) * 4;
    let cursor = |letter: char| {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}")
        } else if application_cursor {
            format!("\x1bO{letter}")
        } else {
            format!("\x1b[{letter}")
        }
    };
    let tilde = |number| {
        if modifier > 1 {
            format!("\x1b[{number};{modifier}~")
        } else {
            format!("\x1b[{number}~")
        }
    };
    let mut bytes = match key.code {
        KeyCode::Char(ch) if ctrl && ch.is_ascii() => {
            let ch = ch.to_ascii_uppercase();
            if ('4'..='7').contains(&ch) {
                vec![ch as u8 - b'4' + 0x1c]
            } else if ch == ' ' {
                vec![0]
            } else if ('@'..='_').contains(&ch) {
                vec![(ch as u8) & 0x1f]
            } else {
                vec![]
            }
        }
        KeyCode::Char(ch) => ch.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Backspace => vec![if ctrl { 8 } else { 127 }],
        KeyCode::Up => cursor('A').into_bytes(),
        KeyCode::Down => cursor('B').into_bytes(),
        KeyCode::Right => cursor('C').into_bytes(),
        KeyCode::Left => cursor('D').into_bytes(),
        KeyCode::Home => cursor('H').into_bytes(),
        KeyCode::End => cursor('F').into_bytes(),
        KeyCode::Insert => tilde(2).into_bytes(),
        KeyCode::Delete => tilde(3).into_bytes(),
        KeyCode::PageUp => tilde(5).into_bytes(),
        KeyCode::PageDown => tilde(6).into_bytes(),
        KeyCode::F(n @ 1..=4) => {
            if modifier > 1 {
                format!("\x1b[1;{modifier}{}", (b'P' + n - 1) as char).into_bytes()
            } else {
                vec![0x1b, b'O', b'P' + n - 1]
            }
        }
        KeyCode::F(n @ 5..=12) => {
            tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]).into_bytes()
        }
        _ => vec![],
    };
    if alt
        && matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace
        )
    {
        bytes.insert(0, 0x1b);
    }
    bytes
}

#[cfg(test)]
mod hostile_output {
    use super::*;
    use proptest::prelude::*;

    struct Ignore;
    impl vt100::Callbacks for Ignore {}

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 512,
            // The same inputs each run, so a release cannot fail on an unrelated random find.
            rng_seed: proptest::test_runner::RngSeed::Fixed(1),
            failure_persistence: None,
            ..ProptestConfig::default()
        })]
        // Whatever a shell prints arrives here, split at any point.
        #[test]
        fn any_output_is_processed_without_panicking(
            pieces in proptest::collection::vec(
                proptest::collection::vec(
                    prop_oneof![Just(0x1bu8), Just(b'['), Just(b']'), Just(b';'), Just(0x07), Just(0x18), any::<u8>()],
                    0..300,
                ),
                0..8,
            ),
            rows in 2u16..60,
            columns in 2u16..200,
        ) {
            let mut guard = OutputGuard::default();
            let mut parser = vt100::Parser::new_with_callbacks(rows, columns, 100, Ignore);
            for piece in &pieces {
                guard.process(&mut parser, piece);
            }
            prop_assert!(guard.csi.len() <= MAX_CSI_BYTES);
        }
    }

    #[test]
    fn the_smallest_panel_can_print_anything() {
        let (rows, columns) = dimensions(0, 0);
        let mut guard = OutputGuard::default();
        let mut parser = vt100::Parser::new_with_callbacks(rows, columns, 100, Ignore);
        guard.process(
            &mut parser,
            "[[[[[[[[[[ 日本語 line\r\nnext\n\n".repeat(4).as_bytes(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_keys_are_encoded_instead_of_triggering_application_shortcuts() {
        assert_eq!(
            encode_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                false
            ),
            [3]
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), false),
            [27]
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), true),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL), false),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE), false),
            b"p"
        );
    }

    fn shell(script: &str) -> Session {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", script]);
        Session::start(
            crate::machines::local_tool_request("Test", "test"),
            command,
            24,
            80,
            std::sync::Arc::default(),
        )
        .unwrap()
    }

    /// Polls with no pause, the worst case for noticing an exit before its output has arrived.
    fn poll_until_ended(session: &mut Session, limit: std::time::Duration) {
        let deadline = std::time::Instant::now() + limit;
        while session.running() {
            assert!(std::time::Instant::now() < deadline, "the session must end");
            session.poll();
        }
    }

    #[test]
    fn an_exit_is_reported_only_once_its_output_has_arrived() {
        for _ in 0..50 {
            let mut session = shell("printf finished");
            poll_until_ended(&mut session, std::time::Duration::from_secs(10));
            assert_eq!(session.status(), "Exited 0");
            assert!(
                session.screen().contents().contains("finished"),
                "{:?}",
                session.screen().contents()
            );
        }
    }

    #[test]
    fn a_process_left_holding_the_terminal_cannot_hide_the_exit() {
        let mut session = shell("sleep 3 & exit 4");
        poll_until_ended(&mut session, std::time::Duration::from_secs(2));
        assert_eq!(session.status(), "Exited 4");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn closing_the_terminal_while_still_running_is_a_disconnect() {
        let mut session = shell("exec </dev/null >/dev/null 2>&1; sleep 3");
        poll_until_ended(&mut session, std::time::Duration::from_secs(2));
        assert_eq!(session.status(), "Disconnected");
    }

    #[test]
    fn closing_during_the_wait_for_output_stays_closed() {
        let mut session = shell("sleep 3 & exit 0");
        while session.child.is_some() {
            session.poll();
        }
        session.close();
        std::thread::sleep(EXIT_SETTLE * 2);
        session.poll();
        assert_eq!(session.status(), "Closed");
    }

    #[test]
    fn emulation_contains_cursor_controls_and_ignores_host_clipboard_and_titles() {
        let mut parser = vt100::Parser::new_with_callbacks(3, 20, 10, Replies::default());
        OutputGuard::default().process(
            &mut parser,
            b"hello\r\nworld\x1b[1;1HOK\x1b]52;c;secret\x07\x1b]2;host-title\x07\x1b[6n",
        );
        assert!(parser.screen().contents().starts_with("OKllo"));
        assert!(!parser.screen().contents().contains("secret"));
        assert_eq!(parser.callbacks().pending, [b"\x1b[1;3R".to_vec()]);
    }

    #[derive(Default)]
    struct OscLengths {
        count: usize,
        max: usize,
    }

    impl vt100::Callbacks for OscLengths {
        fn unhandled_osc(&mut self, _screen: &mut vt100::Screen, params: &[&[u8]]) {
            self.count += 1;
            self.max = self.max.max(params.iter().map(|param| param.len()).sum());
        }
    }

    #[test]
    fn oversized_unterminated_osc_is_bounded_across_reads_and_recovers() {
        let mut parser = vt100::Parser::new_with_callbacks(3, 20, 10, OscLengths::default());
        let mut guard = OutputGuard::default();
        guard.process(&mut parser, b"before\x1b");
        guard.process(&mut parser, b"]999;");
        // Eight MiB in small, reused chunks; no large test allocation or PTY.
        for _ in 0..1024 {
            guard.process(&mut parser, &[b'x'; 8192]);
            assert!(parser.callbacks().max <= MAX_OSC_BYTES);
        }
        assert_eq!(guard.state, OutputState::DiscardOsc);
        assert_eq!(parser.callbacks().count, 1);
        assert_eq!(parser.screen().contents(), "before");
        guard.process(&mut parser, b"\x1b");
        guard.process(&mut parser, b"\\after");
        assert_eq!(guard.state, OutputState::Ground);
        assert_eq!(parser.screen().contents(), "beforeafter");
        assert_eq!(parser.callbacks().count, 1);
    }

    #[test]
    fn osc_limit_handles_exact_boundary_and_all_parser_terminators() {
        for terminator in [b"\x07".as_slice(), b"\x18", b"\x1a", b"\x1b\\", b"\x1b[0m"] {
            for length in [MAX_OSC_BYTES - 1, MAX_OSC_BYTES, MAX_OSC_BYTES + 1] {
                let mut parser =
                    vt100::Parser::new_with_callbacks(3, 20, 10, OscLengths::default());
                let mut guard = OutputGuard::default();
                guard.process(&mut parser, b"\x1b]999;");
                guard.process(&mut parser, &vec![b'x'; length - 4]);
                for byte in terminator {
                    guard.process(&mut parser, &[*byte]);
                }
                guard.process(&mut parser, b"ok\x1b]999;next\x07");
                assert_eq!(guard.state, OutputState::Ground);
                assert_eq!(parser.screen().contents(), "ok");
                assert_eq!(parser.callbacks().count, 2);
                assert_eq!(parser.callbacks().max, length.min(MAX_OSC_BYTES) - 1);
            }
        }
    }

    #[test]
    fn osc_guard_preserves_normal_emulation_across_chunk_boundaries() {
        let bytes = concat!(
            "hello\r\n世界\x1b[1;1HOK\x1b[31mred\x1b[0m",
            "\x1b]52;c;secret\x07\x1b]2;title\x1b\\",
            "\x1b\x00\x7f]999;ignored\x07",
            "\x1b ]not-an-osc\x1b[?2004h\x1b[6n"
        )
        .as_bytes();
        let mut expected = vt100::Parser::new_with_callbacks(5, 80, 10, Replies::default());
        expected.process(bytes);
        for chunk_size in [1, 2, 3, 7, bytes.len()] {
            let mut parser = vt100::Parser::new_with_callbacks(5, 80, 10, Replies::default());
            let mut guard = OutputGuard::default();
            for chunk in bytes.chunks(chunk_size) {
                guard.process(&mut parser, chunk);
            }
            assert_eq!(
                parser.screen().contents_formatted(),
                expected.screen().contents_formatted()
            );
            assert_eq!(
                parser.screen().cursor_position(),
                expected.screen().cursor_position()
            );
            assert_eq!(
                parser.screen().bracketed_paste(),
                expected.screen().bracketed_paste()
            );
            assert_eq!(parser.callbacks().pending, expected.callbacks().pending);
        }
    }

    #[test]
    fn repeated_oversized_osc_sequences_in_one_read_remain_bounded() {
        let mut bytes = Vec::new();
        for _ in 0..3 {
            // Repeated ESC and ignored controls must not hide an OSC opener.
            bytes.extend_from_slice(b"\x1b\x1b\x00\x7f]999;");
            bytes.extend(std::iter::repeat_n(b'x', MAX_OSC_BYTES * 2));
            bytes.extend_from_slice(b"\x07ok");
        }
        let mut parser = vt100::Parser::new_with_callbacks(3, 20, 10, OscLengths::default());
        let mut guard = OutputGuard::default();
        guard.process(&mut parser, &bytes);
        assert_eq!(guard.state, OutputState::Ground);
        assert_eq!(parser.callbacks().count, 3);
        assert!(parser.callbacks().max <= MAX_OSC_BYTES);
        assert_eq!(parser.screen().contents(), "okokok");
    }

    #[test]
    fn costly_csi_counts_are_clamped_to_screen_dimensions_across_reads() {
        for code in *b"@LT" {
            for parameter in ["65535", "999999999999999999999999", "65535;2", "65535:2"] {
                let input = format!(
                    "abcdef\r\nsecond\x1b[1;1H\x1b[{parameter}{}ok",
                    char::from(code)
                );
                let mut expected = vt100::Parser::new(3, 20, 10);
                let count = if code == b'@' { 20 } else { 3 };
                expected.process(
                    format!(
                        "abcdef\r\nsecond\x1b[1;1H\x1b[{count}{}ok",
                        char::from(code)
                    )
                    .as_bytes(),
                );
                for chunk_size in [1, 2, 7, input.len()] {
                    let mut parser = vt100::Parser::new(3, 20, 10);
                    let mut guard = OutputGuard::default();
                    for chunk in input.as_bytes().chunks(chunk_size) {
                        guard.process(&mut parser, chunk);
                    }
                    assert_eq!(
                        parser.screen().contents_formatted(),
                        expected.screen().contents_formatted()
                    );
                }
            }
        }
    }

    #[test]
    fn csi_guard_preserves_controls_defaults_and_interrupted_sequences() {
        for bytes in [
            b"hello\x1b[2D\x1b[@!".as_slice(),
            b"hello\x1b[0@!",
            b"hello\x1b[2\r@!",
            b"hello\x1b[?2h\x1b[2 q!",
            b"hello\x1b[65535\x18!",
            b"hello\x1b[65535\x1b]999;ignored\x07!",
        ] {
            let mut expected = vt100::Parser::new(3, 20, 10);
            expected.process(bytes);
            let mut parser = vt100::Parser::new(3, 20, 10);
            let mut guard = OutputGuard::default();
            for byte in bytes {
                guard.process(&mut parser, &[*byte]);
            }
            assert_eq!(
                parser.screen().contents_formatted(),
                expected.screen().contents_formatted()
            );
        }
    }

    #[test]
    fn unterminated_csi_storage_is_bounded_and_recovers_without_rendering_its_tail() {
        let mut parser = vt100::Parser::new(3, 20, 10);
        let mut guard = OutputGuard::default();
        guard.process(&mut parser, b"before\x1b[");
        for _ in 0..1024 {
            guard.process(&mut parser, &[b'9'; 8192]);
            assert!(guard.csi.len() <= MAX_CSI_BYTES);
            assert!(guard.csi.capacity() <= MAX_CSI_BYTES);
        }
        assert_eq!(guard.state, OutputState::DiscardCsi);
        guard.process(&mut parser, b"@after\x1b[2D!");
        assert_eq!(guard.state, OutputState::Ground);
        assert_eq!(parser.screen().contents(), "beforeaft!r");
    }
}
