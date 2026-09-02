// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded-memory exclusive-copy engine shared by Windows create-only writers.

#![cfg_attr(not(windows), allow(dead_code))]

use std::io::{self, ErrorKind, Read, Write};

pub(crate) const COPY_BUFFER_SIZE: usize = 64 * 1024;

pub(crate) fn copy_exclusive<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<u64> {
    let mut buf = [0u8; COPY_BUFFER_SIZE];
    let mut total = 0u64;
    loop {
        let n = loop {
            match reader.read(&mut buf) {
                Ok(0) => return Ok(total),
                Ok(n) => break n,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        };
        total = total.checked_add(n as u64).ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidData, "exclusive copy length overflow")
        })?;
        let mut remaining = &buf[..n];
        while !remaining.is_empty() {
            match writer.write(remaining) {
                Ok(0) => {
                    return Err(io::Error::new(
                        ErrorKind::WriteZero,
                        "exclusive copy write returned zero bytes",
                    ));
                }
                Ok(written) => remaining = &remaining[written..],
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{COPY_BUFFER_SIZE, copy_exclusive};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind, Read, Write};
    use std::rc::Rc;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum CopyEvent {
        ReadRequest { buf_len: usize },
        Read { bytes: usize },
        ReadInterrupted,
        Write { bytes: usize },
        WriteInterrupted,
    }

    struct ScriptedReader {
        chunks: VecDeque<io::Result<Vec<u8>>>,
        events: Rc<RefCell<Vec<CopyEvent>>>,
    }

    impl Read for ScriptedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.events
                .borrow_mut()
                .push(CopyEvent::ReadRequest { buf_len: buf.len() });
            match self.chunks.pop_front() {
                None => Ok(0),
                Some(Ok(chunk)) if chunk.is_empty() => Ok(0),
                Some(Ok(chunk)) => {
                    assert!(chunk.len() <= buf.len());
                    buf[..chunk.len()].copy_from_slice(&chunk);
                    self.events
                        .borrow_mut()
                        .push(CopyEvent::Read { bytes: chunk.len() });
                    Ok(chunk.len())
                }
                Some(Err(error)) if error.kind() == ErrorKind::Interrupted => {
                    self.events.borrow_mut().push(CopyEvent::ReadInterrupted);
                    Err(error)
                }
                Some(Err(error)) => Err(error),
            }
        }
    }

    enum WriteScript {
        Interrupt,
        Partial(usize),
        Full,
    }

    struct ScriptedWriter {
        script: VecDeque<WriteScript>,
        events: Rc<RefCell<Vec<CopyEvent>>>,
        sink: Vec<u8>,
    }

    impl Write for ScriptedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match self.script.pop_front() {
                Some(WriteScript::Interrupt) => {
                    self.events.borrow_mut().push(CopyEvent::WriteInterrupted);
                    Err(io::Error::from(ErrorKind::Interrupted))
                }
                Some(WriteScript::Partial(n)) => {
                    let n = n.min(buf.len());
                    self.sink.extend_from_slice(&buf[..n]);
                    self.events.borrow_mut().push(CopyEvent::Write { bytes: n });
                    Ok(n)
                }
                Some(WriteScript::Full) | None => {
                    self.sink.extend_from_slice(buf);
                    self.events
                        .borrow_mut()
                        .push(CopyEvent::Write { bytes: buf.len() });
                    Ok(buf.len())
                }
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn copy_exclusive_writes_each_chunk_before_the_next_read() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut reader = ScriptedReader {
            chunks: VecDeque::from([
                Ok(b"abc".to_vec()),
                Ok(b"d".to_vec()),
                Err(io::Error::from(ErrorKind::Interrupted)),
                Ok(b"efghi".to_vec()),
            ]),
            events: Rc::clone(&events),
        };
        let mut writer = ScriptedWriter {
            script: VecDeque::from([
                WriteScript::Interrupt,
                WriteScript::Full,
                WriteScript::Full,
                WriteScript::Partial(2),
                WriteScript::Full,
            ]),
            events: Rc::clone(&events),
            sink: Vec::new(),
        };

        let copied = copy_exclusive(&mut reader, &mut writer).unwrap();
        assert_eq!(copied, 9);
        assert_eq!(writer.sink, b"abcdefghi");

        let log = events.borrow().clone();
        assert!(
            log.iter().all(|event| match event {
                CopyEvent::ReadRequest { buf_len } => *buf_len <= COPY_BUFFER_SIZE,
                _ => true,
            }),
            "copy buffer exceeded 64 KiB: {log:?}"
        );
        assert_eq!(
            log,
            vec![
                CopyEvent::ReadRequest {
                    buf_len: COPY_BUFFER_SIZE
                },
                CopyEvent::Read { bytes: 3 },
                CopyEvent::WriteInterrupted,
                CopyEvent::Write { bytes: 3 },
                CopyEvent::ReadRequest {
                    buf_len: COPY_BUFFER_SIZE
                },
                CopyEvent::Read { bytes: 1 },
                CopyEvent::Write { bytes: 1 },
                CopyEvent::ReadRequest {
                    buf_len: COPY_BUFFER_SIZE
                },
                CopyEvent::ReadInterrupted,
                CopyEvent::ReadRequest {
                    buf_len: COPY_BUFFER_SIZE
                },
                CopyEvent::Read { bytes: 5 },
                CopyEvent::Write { bytes: 2 },
                CopyEvent::Write { bytes: 3 },
                CopyEvent::ReadRequest {
                    buf_len: COPY_BUFFER_SIZE
                },
            ]
        );

        let mut outstanding = 0usize;
        for event in &log {
            match event {
                CopyEvent::Read { bytes } => {
                    assert_eq!(
                        outstanding, 0,
                        "read started before prior chunk was fully written: {log:?}"
                    );
                    outstanding = *bytes;
                }
                CopyEvent::Write { bytes } => {
                    assert!(
                        outstanding >= *bytes,
                        "write exceeded unread chunk remainder: {log:?}"
                    );
                    outstanding -= *bytes;
                }
                CopyEvent::ReadInterrupted => {
                    assert_eq!(
                        outstanding, 0,
                        "interrupted read while a chunk was still unwritten: {log:?}"
                    );
                }
                CopyEvent::ReadRequest { .. } | CopyEvent::WriteInterrupted => {}
            }
        }
        assert_eq!(outstanding, 0);
    }

    #[test]
    fn copy_exclusive_streams_more_than_two_buffers_with_a_fixed_read_bound() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut reader = ScriptedReader {
            chunks: VecDeque::from([
                Ok(vec![b'a'; COPY_BUFFER_SIZE]),
                Ok(vec![b'b'; COPY_BUFFER_SIZE]),
                Ok(vec![b'c'; 17]),
            ]),
            events: Rc::clone(&events),
        };
        let mut writer = ScriptedWriter {
            script: VecDeque::new(),
            events: Rc::clone(&events),
            sink: Vec::new(),
        };

        let copied = copy_exclusive(&mut reader, &mut writer).unwrap();
        assert_eq!(copied, (COPY_BUFFER_SIZE * 2 + 17) as u64);
        assert_eq!(writer.sink.len(), COPY_BUFFER_SIZE * 2 + 17);
        assert_eq!(
            &writer.sink[..COPY_BUFFER_SIZE],
            vec![b'a'; COPY_BUFFER_SIZE]
        );
        assert_eq!(
            &writer.sink[COPY_BUFFER_SIZE..COPY_BUFFER_SIZE * 2],
            vec![b'b'; COPY_BUFFER_SIZE]
        );
        assert_eq!(&writer.sink[COPY_BUFFER_SIZE * 2..], vec![b'c'; 17]);

        let log = events.borrow();
        assert!(
            log.iter().all(|event| match event {
                CopyEvent::ReadRequest { buf_len } => *buf_len <= COPY_BUFFER_SIZE,
                _ => true,
            }),
            "copy buffer exceeded 64 KiB: {log:?}"
        );
    }
}
