//! A named-pipe server instance speaking newline-delimited text to one client at a time,
//! overlapped and served by the tray thread's wait ([`crate::wait`]): nothing runs until the
//! client writes. Local clients only. What the lines mean is the [`Handler`]'s business. Several
//! instances of one name serve several clients (one browser each).

use anyhow::{bail, Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::Foundation::WAIT_TIMEOUT;
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
use windows::Win32::System::IO::{CancelIo, CancelIoEx, GetOverlappedResult, OVERLAPPED};

const BUFFER: usize = 64 * 1024;
/// How long a send waits for a client that is not reading.
const SEND_PATIENCE_MS: u32 = 250;
/// A line longer than this is a broken client.
const MAX_LINE: usize = 8 * 1024 * 1024;

/// What a pipe's lines mean.
pub trait Handler {
    fn connected(&mut self) {}
    fn line(&mut self, line: &str) -> Result<()>;
    fn disconnected(&mut self) {}
}

pub struct Server<H: Handler> {
    name: &'static str,
    pipe: HANDLE,
    event: HANDLE,
    overlapped: Box<OVERLAPPED>,
    buffer: Box<[u8; BUFFER]>,
    pending: Vec<u8>,
    connected: bool,
    pub handler: H,
}

impl<H: Handler> Server<H> {
    /// Host one of `instances` servers of `\\.\pipe\<name>` (every instance must say the same
    /// number) and wait for its client.
    pub fn instance(name: &'static str, handler: H, instances: u32) -> Result<Self> {
        let path = crate::win::wide(&format!(r"\\.\pipe\{name}"));
        unsafe {
            let pipe = CreateNamedPipeW(
                PCWSTR(path.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                instances,
                BUFFER as u32,
                BUFFER as u32,
                0,
                None,
            );
            if pipe.is_invalid() {
                bail!("CreateNamedPipe({name}): {:?}", GetLastError());
            }
            let event = CreateEventW(None, true, false, None).context("CreateEvent")?;
            let mut server = Self {
                name,
                pipe,
                event,
                overlapped: Box::new(OVERLAPPED { hEvent: event, ..Default::default() }),
                buffer: Box::new([0; BUFFER]),
                pending: Vec::new(),
                connected: false,
                handler,
            };
            server.listen();
            Ok(server)
        }
    }

    pub fn event(&self) -> HANDLE {
        self.event
    }

    fn listen(&mut self) {
        self.reset();
        unsafe {
            match ConnectNamedPipe(self.pipe, Some(&mut *self.overlapped)) {
                Ok(()) => self.on_connected(),
                Err(_) if GetLastError() == ERROR_PIPE_CONNECTED => self.on_connected(),
                Err(_) if GetLastError() == ERROR_IO_PENDING => {}
                Err(e) => eprintln!("{}: cannot listen on the pipe ({e})", self.name),
            }
        }
    }

    fn reset(&mut self) {
        let _ = unsafe { ResetEvent(self.event) };
        *self.overlapped = OVERLAPPED { hEvent: self.event, ..Default::default() };
    }

    fn on_connected(&mut self) {
        self.connected = true;
        self.pending.clear();
        self.handler.connected();
        self.read();
    }

    /// Start the next overlapped read; completions arrive through [`Server::signalled`].
    fn read(&mut self) {
        self.reset();
        unsafe {
            match ReadFile(self.pipe, Some(&mut self.buffer[..]), None, Some(&mut *self.overlapped)) {
                // Completed at once: the event is set too, so the wait loop delivers it.
                Ok(()) => {}
                Err(_) if GetLastError() == ERROR_IO_PENDING => {}
                Err(_) => self.disconnected(),
            }
        }
    }

    /// The event fired: a connection or a read completed.
    pub fn signalled(&mut self) {
        let mut moved = 0u32;
        let done = unsafe { GetOverlappedResult(self.pipe, &*self.overlapped, &mut moved, false) };
        if !self.connected {
            match done {
                Ok(()) => self.on_connected(),
                Err(_) => self.listen(),
            }
            return;
        }
        if done.is_err() {
            return self.disconnected();
        }
        self.pending.extend_from_slice(&self.buffer[..moved as usize]);
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            match std::str::from_utf8(&line) {
                Ok(text) if !text.trim().is_empty() => {
                    if let Err(e) = self.handler.line(text.trim()) {
                        eprintln!("{}: ignoring a line ({e:#})", self.name);
                    }
                }
                Ok(_) => {}
                Err(_) => eprintln!("{}: ignoring a line that is not UTF-8", self.name),
            }
        }
        if self.pending.len() > MAX_LINE {
            eprintln!("{}: a line over {MAX_LINE} bytes; dropping the client", self.name);
            return self.disconnected();
        }
        self.read();
    }

    fn disconnected(&mut self) {
        self.connected = false;
        unsafe {
            let _ = DisconnectNamedPipe(self.pipe);
        }
        self.handler.disconnected();
        self.listen();
    }

    /// Send one line to the client. Best-effort: small writes complete at once, and one that a
    /// client not reading leaves pending is given up after a moment (this is the tray thread).
    pub fn send(&mut self, line: &str) {
        if !self.connected {
            eprintln!("{}: no client connected; {line} not sent", self.name);
            return;
        }
        let line = format!("{line}\n");
        unsafe {
            let Ok(event) = CreateEventW(None, true, false, None) else { return };
            let mut overlapped = OVERLAPPED { hEvent: event, ..Default::default() };
            let mut written = 0u32;
            let result = match WriteFile(self.pipe, Some(line.as_bytes()), None, Some(&mut overlapped)) {
                Ok(()) => Ok(()),
                Err(_) if GetLastError() == ERROR_IO_PENDING => {
                    if WaitForSingleObject(event, SEND_PATIENCE_MS) == WAIT_TIMEOUT {
                        // Cancelled, then waited for: the write reads `line` and `overlapped`.
                        let _ = CancelIoEx(self.pipe, Some(&overlapped));
                    }
                    GetOverlappedResult(self.pipe, &overlapped, &mut written, true)
                }
                Err(e) => Err(e),
            };
            let _ = CloseHandle(event);
            if let Err(e) = result {
                eprintln!("{}: could not send ({e})", self.name);
            }
        }
    }
}

impl<H: Handler> Drop for Server<H> {
    fn drop(&mut self) {
        unsafe {
            let _ = CancelIo(self.pipe);
            // The cancelled read writes into `buffer` and `overlapped` until it completes: wait
            // for that before they are freed.
            let mut moved = 0u32;
            let _ = GetOverlappedResult(self.pipe, &*self.overlapped, &mut moved, true);
            let _ = DisconnectNamedPipe(self.pipe);
            let _ = CloseHandle(self.pipe);
            let _ = CloseHandle(self.event);
        }
    }
}
