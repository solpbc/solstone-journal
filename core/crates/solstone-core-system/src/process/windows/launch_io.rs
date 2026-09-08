// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded launch-pipe I/O with storage retained through definitive completion.

use std::cell::UnsafeCell;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const CANCELLATION_DRAIN: Duration = Duration::from_millis(250);
const MAX_FRAME: usize = 32 * 1024;

// This registry owns incomplete operations, including their pipe handles. Future
// transactions refuse until completion is observed. It is never interpreted as
// successful cleanup, and an elapsed deadline can never become late admission.
static INCOMPLETE: Mutex<Vec<PendingIo>> = Mutex::new(Vec::new());

struct PendingIo {
    state: Option<Box<OperationState>>,
}

struct OperationState {
    pipe: Arc<OwnedHandle>,
    event: OwnedHandle,
    overlapped: Box<UnsafeCell<OVERLAPPED>>,
    buffer: Vec<u8>,
    pending: bool,
}

// SAFETY: the only pointer retained by Windows points into stable owned storage.
// Moving this owner does not move that allocation. No Rust read of the buffer or
// OVERLAPPED occurs until GetOverlappedResult proves completion. The registry
// mutex serializes ownership; Windows file/event handles have no thread affinity.
#[allow(unsafe_code)]
unsafe impl Send for OperationState {}

impl std::ops::Deref for PendingIo {
    type Target = OperationState;
    fn deref(&self) -> &Self::Target {
        self.state.as_deref().expect("owned operation")
    }
}

impl std::ops::DerefMut for PendingIo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.state.as_deref_mut().expect("owned operation")
    }
}

enum Completion {
    Pending,
    Finished(io::Result<usize>),
}

impl PendingIo {
    fn new(pipe: Arc<OwnedHandle>, buffer: Vec<u8>) -> io::Result<Self> {
        // SAFETY: unnamed, noninheritable manual-reset event; no borrowed input.
        #[allow(unsafe_code)]
        let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful CreateEventW transfers this unique handle to us.
        #[allow(unsafe_code)]
        let event = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut overlapped = OVERLAPPED::default();
        overlapped.hEvent = event.as_raw_handle();
        Ok(Self {
            state: Some(Box::new(OperationState {
                pipe,
                event,
                overlapped: Box::new(UnsafeCell::new(overlapped)),
                buffer,
                pending: false,
            })),
        })
    }

    fn completion(&mut self) -> Completion {
        let mut transferred = 0;
        // SAFETY: both handle and exact OVERLAPPED allocation remain owned. This
        // API is the synchronization boundary; it does not wait or free storage.
        #[allow(unsafe_code)]
        let result = unsafe {
            GetOverlappedResult(
                self.pipe.as_raw_handle(),
                self.overlapped.get(),
                &mut transferred,
                0,
            )
        };
        if result != 0 {
            self.pending = false;
            Completion::Finished(Ok(transferred as usize))
        } else {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(code) if code == ERROR_IO_INCOMPLETE as i32
                || code == windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE as i32
                || code == windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER as i32)
            {
                // A rejected completion query is not completion evidence. Keep
                // the original state, even if an internal invariant was broken.
                Completion::Pending
            } else {
                self.pending = false;
                Completion::Finished(Err(error))
            }
        }
    }

    fn wait(&self, duration: Duration) -> io::Result<()> {
        let milliseconds = duration.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: event remains owned and is never shared with the child.
        #[allow(unsafe_code)]
        match unsafe { WaitForSingleObject(self.event.as_raw_handle(), milliseconds) } {
            WAIT_OBJECT_0 | WAIT_TIMEOUT => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn finish(mut self, deadline: Instant) -> io::Result<Vec<u8>> {
        let mut wait_error = None;
        while Instant::now() < deadline {
            match self.completion() {
                Completion::Finished(result) => {
                    // Completion observed after the deadline cannot admit a child.
                    if Instant::now() >= deadline {
                        break;
                    }
                    let count = result?;
                    if count > self.buffer.len() {
                        return Err(io::Error::other(
                            "launch pipe returned an invalid byte count",
                        ));
                    }
                    self.buffer.truncate(count);
                    return Ok(std::mem::take(&mut self.buffer));
                }
                Completion::Pending => {
                    if let Err(error) =
                        self.wait(deadline.saturating_duration_since(Instant::now()))
                    {
                        wait_error = Some(error);
                        break;
                    }
                }
            }
        }
        self.cancel_and_drain();
        let incomplete = self.pending;
        if incomplete {
            retain(self);
        }
        let cleanup = if incomplete {
            "I/O cleanup incomplete and retained"
        } else {
            "I/O cancellation completed"
        };
        match wait_error {
            Some(error) => Err(io::Error::new(
                error.kind(),
                format!("launch I/O wait failed: {error}; {cleanup}"),
            )),
            None => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("launch deadline elapsed; {cleanup}"),
            )),
        }
    }

    fn cancel_and_drain(&mut self) {
        if !self.pending {
            return;
        }
        // SAFETY: cancellation targets only this exact owned operation. Even
        // ERROR_NOT_FOUND is not completion; the subsequent query is required.
        #[allow(unsafe_code)]
        unsafe {
            CancelIoEx(self.pipe.as_raw_handle(), self.overlapped.get())
        };
        if matches!(self.completion(), Completion::Pending) {
            let _ = self.wait(CANCELLATION_DRAIN);
            let _ = self.completion();
        }
    }
}

impl Drop for PendingIo {
    fn drop(&mut self) {
        if self.state.as_ref().is_some_and(|state| state.pending) {
            self.cancel_and_drain();
            if self.pending {
                // Transfer the original event, handle, and allocations together.
                // Duplicating an event then closing its original handle would
                // invalidate OVERLAPPED.hEvent while the operation is pending.
                retain(PendingIo {
                    state: self.state.take(),
                });
            }
        }
    }
}

fn retain(operation: PendingIo) {
    INCOMPLETE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(operation);
}

pub(super) fn require_completed_cleanup() -> io::Result<()> {
    let mut retained = INCOMPLETE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    retained.retain_mut(|operation| matches!(operation.completion(), Completion::Pending));
    if retained.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(
            "previous launch I/O cleanup remains incomplete",
        ))
    }
}

pub(super) fn connect(pipe: &Arc<OwnedHandle>, deadline: Instant) -> io::Result<()> {
    ensure_time(deadline)?;
    let mut operation = PendingIo::new(Arc::clone(pipe), Vec::new())?;
    // SAFETY: pipe was opened overlapped; stable state remains owned until completion.
    #[allow(unsafe_code)]
    let result = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), operation.overlapped.get()) };
    if result != 0 {
        return ensure_time(deadline);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) {
        return ensure_time(deadline);
    }
    if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
        return Err(error);
    }
    operation.pending = true;
    // Connect's byte count is undefined: provide capacity only for the generic
    // completion owner, then discard the count in the dedicated path below.
    operation.finish_connect(deadline)
}

impl PendingIo {
    fn finish_connect(mut self, deadline: Instant) -> io::Result<()> {
        let mut wait_error = None;
        while Instant::now() < deadline {
            if let Completion::Finished(result) = self.completion() {
                ensure_time(deadline)?;
                return result.map(|_| ());
            }
            if let Err(error) = self.wait(deadline.saturating_duration_since(Instant::now())) {
                wait_error = Some(error);
                break;
            }
        }
        self.cancel_and_drain();
        let incomplete = self.pending;
        if incomplete {
            retain(self);
        }
        let cleanup = if incomplete {
            "I/O cleanup incomplete and retained"
        } else {
            "I/O cancellation completed"
        };
        match wait_error {
            Some(error) => Err(io::Error::new(
                error.kind(),
                format!("launch connection wait failed: {error}; {cleanup}"),
            )),
            None => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("launch connection deadline elapsed; {cleanup}"),
            )),
        }
    }
}

fn ensure_time(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "launch deadline elapsed",
        ))
    } else {
        Ok(())
    }
}

fn transfer(
    pipe: &Arc<OwnedHandle>,
    buffer: Vec<u8>,
    read: bool,
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    ensure_time(deadline)?;
    let mut operation = PendingIo::new(Arc::clone(pipe), buffer)?;
    let count = operation.buffer.len() as u32;
    let buffer = operation.buffer.as_mut_ptr();
    let overlapped = operation.overlapped.get();
    // SAFETY: no allocation moves after submission; buffers are not read or
    // modified by Rust until definitive completion. No stack output pointer is
    // supplied to asynchronous I/O. Pipe uses overlapped mode.
    #[allow(unsafe_code)]
    let result = unsafe {
        if read {
            ReadFile(
                pipe.as_raw_handle(),
                buffer,
                count,
                std::ptr::null_mut(),
                overlapped,
            )
        } else {
            WriteFile(
                pipe.as_raw_handle(),
                buffer,
                count,
                std::ptr::null_mut(),
                overlapped,
            )
        }
    };
    if result != 0 {
        operation.pending = true;
        return operation.finish(deadline);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
        return Err(error);
    }
    operation.pending = true;
    operation.finish(deadline)
}

pub(super) fn write_frame(
    pipe: &Arc<OwnedHandle>,
    data: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    if data.is_empty() || data.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid launch frame size",
        ));
    }
    let mut frame = Vec::with_capacity(data.len() + 4);
    frame.extend_from_slice(&(data.len() as u32).to_le_bytes());
    frame.extend_from_slice(data);
    let mut offset = 0;
    while offset < frame.len() {
        let written = transfer(pipe, frame[offset..].to_vec(), false, deadline)?.len();
        if written == 0 {
            return Err(io::Error::from(io::ErrorKind::WriteZero));
        }
        offset += written;
    }
    Ok(())
}

fn read_exact(pipe: &Arc<OwnedHandle>, length: usize, deadline: Instant) -> io::Result<Vec<u8>> {
    let mut result = Vec::with_capacity(length);
    while result.len() < length {
        let chunk = transfer(pipe, vec![0; length - result.len()], true, deadline)?;
        if chunk.is_empty() {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        result.extend(chunk);
    }
    Ok(result)
}

pub(super) fn read_frame(pipe: &Arc<OwnedHandle>, deadline: Instant) -> io::Result<Vec<u8>> {
    let header: [u8; 4] = read_exact(pipe, 4, deadline)?
        .try_into()
        .expect("exact header length");
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid launch frame size",
        ));
    }
    read_exact(pipe, length, deadline)
}
