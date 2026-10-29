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
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn Child + Send + Sync>>,
    input: Option<SyncSender<Vec<u8>>>,
    output: tokio::sync::mpsc::Receiver<Output>,
    size: (u16, u16),
}

impl Session {
    pub fn start(
        request: crate::exec::Request,
        command: std::process::Command,
        rows: u16,
        cols: u16,
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
                        break;
                    }
                    Ok(count) => {
                        if output_tx
                            .blocking_send(Output::Data(buffer[..count].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.raw_os_error() == Some(5) => {
                        let _ = output_tx.blocking_send(Output::Eof);
                        break;
                    }
                    Err(error) => {
                        let _ = output_tx
                            .blocking_send(Output::Error(format!("Exec output failed: {error}")));
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
            child: Some(child),
            input: Some(input),
            output,
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
        let mut eof = false;
        for _ in 0..64 {
            match self.output.try_recv() {
                Ok(Output::Data(bytes)) => self.parser.process(&bytes),
                Ok(Output::Eof) => eof = true,
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
                    self.state = State::Exited(status.exit_code());
                    self.child = None;
                    self.input = None;
                }
                Err(error) => {
                    self.state = State::Failed(format!("Cannot read exec status: {error}"))
                }
                Ok(None) if eof && running => self.state = State::Disconnected,
                _ => {}
            }
        }
        previous_status != self.status()
    }

    pub fn close(&mut self) {
        self.input = None;
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

fn dimensions(rows: u16, cols: u16) -> (u16, u16) {
    (rows.clamp(1, 256), cols.clamp(1, 512))
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

    #[test]
    fn emulation_contains_cursor_controls_and_ignores_host_clipboard_and_titles() {
        let mut parser = vt100::Parser::new_with_callbacks(3, 20, 10, Replies::default());
        parser
            .process(b"hello\r\nworld\x1b[1;1HOK\x1b]52;c;secret\x07\x1b]2;host-title\x07\x1b[6n");
        assert!(parser.screen().contents().starts_with("OKllo"));
        assert!(!parser.screen().contents().contains("secret"));
        assert_eq!(parser.callbacks().pending, [b"\x1b[1;3R".to_vec()]);
    }
}
