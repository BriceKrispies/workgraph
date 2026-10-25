//! Output sinks fed by the reader threads: bounded byte buffers and a line splitter.

use std::collections::VecDeque;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};

use wg_procspec::LineCallback;

/// Receives the bytes a child writes to a pipe, in order.
pub(crate) trait Sink: Send {
    /// A chunk of output.
    fn write(&mut self, chunk: &[u8]);
    /// The pipe reached end of file.
    fn finish(&mut self);
}

/// Keeps the first `limit` bytes and notes whether anything was dropped.
pub(crate) struct ByteBuffer {
    pub(crate) limit: usize,
    pub(crate) data: Vec<u8>,
    pub(crate) truncated: bool,
}

impl Sink for ByteBuffer {
    fn write(&mut self, chunk: &[u8]) {
        let room = self.limit.saturating_sub(self.data.len());
        let take = room.min(chunk.len());
        self.data.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            self.truncated = true;
        }
    }

    fn finish(&mut self) {}
}

/// Splits output on `\n`, decodes each line lossily as UTF-8, strips trailing `\r`/`\n`, keeps the
/// last `tail_lines`, counts lines, appends each to the log file and hands it to the callback.
/// A line longer than `max_line_bytes` is cut there; the rest of it is dropped.
pub(crate) struct LineSink {
    pending: Vec<u8>,
    overflowed: bool,
    max_line_bytes: usize,
    tail_lines: usize,
    pub(crate) tail: VecDeque<String>,
    pub(crate) lines: u64,
    pub(crate) truncated_lines: u64,
    log: Option<std::fs::File>,
    on_line: Option<LineCallback>,
}

impl LineSink {
    pub(crate) fn new(
        max_line_bytes: usize,
        tail_lines: usize,
        log: Option<std::fs::File>,
        on_line: Option<LineCallback>,
    ) -> Self {
        Self {
            pending: Vec::new(),
            overflowed: false,
            max_line_bytes,
            tail_lines,
            tail: VecDeque::new(),
            lines: 0,
            truncated_lines: 0,
            log,
            on_line,
        }
    }

    fn append(&mut self, bytes: &[u8]) {
        let room = self.max_line_bytes.saturating_sub(self.pending.len());
        let take = room.min(bytes.len());
        self.pending.extend_from_slice(&bytes[..take]);
        if take < bytes.len() {
            self.overflowed = true;
        }
    }

    fn emit(&mut self) {
        let text = String::from_utf8_lossy(&self.pending);
        let line = text.trim_end_matches(['\r', '\n']).to_owned();
        self.pending.clear();
        if std::mem::take(&mut self.overflowed) {
            self.truncated_lines += 1;
        }
        self.lines += 1;
        if self.tail_lines > 0 {
            if self.tail.len() == self.tail_lines {
                self.tail.pop_front();
            }
            self.tail.push_back(line.clone());
        }
        if let Some(log) = self.log.as_mut() {
            // A failing log must not stop the capture; the line is still delivered below.
            let mut record = line.clone().into_bytes();
            record.push(b'\n');
            let _ = log.write_all(&record);
            let _ = log.flush();
        }
        if let Some(callback) = self.on_line.as_mut() {
            let _ = catch_unwind(AssertUnwindSafe(|| callback(&line)));
        }
    }
}

impl Sink for LineSink {
    fn write(&mut self, chunk: &[u8]) {
        let mut rest = chunk;
        while let Some(pos) = rest.iter().position(|&b| b == b'\n') {
            self.append(&rest[..pos]);
            self.emit();
            rest = &rest[pos + 1..];
        }
        self.append(rest);
    }

    fn finish(&mut self) {
        if !self.pending.is_empty() || self.overflowed {
            self.emit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn splits_lines_across_chunks_and_strips_cr() {
        let mut sink = LineSink::new(1024, 10, None, None);
        sink.write(b"ab");
        sink.write(b"c\r\nde\n\nf");
        sink.finish();
        assert_eq!(sink.tail, ["abc", "de", "", "f"]);
        assert_eq!(sink.lines, 4);
    }

    #[test]
    fn keeps_only_the_tail_but_counts_everything() {
        let mut sink = LineSink::new(1024, 2, None, None);
        sink.write(b"1\n2\n3\n4\n");
        sink.finish();
        assert_eq!(sink.tail, ["3", "4"]);
        assert_eq!(sink.lines, 4);
    }

    #[test]
    fn long_lines_are_cut_and_counted() {
        let mut sink = LineSink::new(4, 10, None, None);
        sink.write(b"abcdefgh\nxy\n");
        sink.write(b"123456");
        sink.finish();
        assert_eq!(sink.tail, ["abcd", "xy", "1234"]);
        assert_eq!(sink.truncated_lines, 2);
    }

    #[test]
    fn invalid_utf8_is_replaced_and_panicking_callbacks_are_ignored() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_in = Arc::clone(&seen);
        let callback: LineCallback = Box::new(move |line| {
            seen_in.lock().unwrap().push(line.to_owned());
            panic!("consumer bug");
        });
        let mut sink = LineSink::new(64, 10, None, Some(callback));
        sink.write(b"a\xffb\nnext\n");
        sink.finish();
        assert_eq!(*seen.lock().unwrap(), ["a\u{fffd}b", "next"]);
    }

    #[test]
    fn byte_buffer_truncates() {
        let mut buf = ByteBuffer {
            limit: 5,
            data: Vec::new(),
            truncated: false,
        };
        buf.write(b"abc");
        assert!(!buf.truncated);
        buf.write(b"defg");
        assert_eq!(buf.data, b"abcde");
        assert!(buf.truncated);
    }
}
