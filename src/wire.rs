use std::ops::Range;
use std::sync::Arc;

use clock::{Global, Lamport, ReplicaId};
use text::{EditOperation, FullOffset, Operation, UndoOperation};

const PROTOCOL: u8 = 0;
const HELLO: u8 = 0;
const OPS: u8 = 1;
const EDIT: u8 = 0;
const UNDO: u8 = 1;

/// One frame of the sync protocol. A peer sends `Hello` with its version when a connection
/// opens, answers a `Hello` with the operations that version lacks, then streams its new
/// operations as `Ops`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Hello(Global),
    Ops(Vec<Operation>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct DecodeError(pub &'static str);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed sync message: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer(vec![PROTOCOL]);
        match self {
            Message::Hello(version) => {
                w.byte(HELLO);
                w.global(version);
            }
            Message::Ops(ops) => {
                w.byte(OPS);
                w.uint(ops.len() as u64);
                for op in ops {
                    w.operation(op);
                }
            }
        }
        w.0
    }

    /// Parses one frame. Rejects anything a well-behaved peer can't produce: another protocol
    /// version, reserved replica IDs, reversed ranges, mismatched edit lists, trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader(bytes);
        if r.byte()? != PROTOCOL {
            return Err(DecodeError("unsupported protocol version"));
        }
        let message = match r.byte()? {
            HELLO => Message::Hello(r.global()?),
            OPS => {
                let count = r.count()?;
                let mut ops = Vec::with_capacity(count);
                for _ in 0..count {
                    ops.push(r.operation()?);
                }
                Message::Ops(ops)
            }
            _ => return Err(DecodeError("unknown message")),
        };
        if !r.0.is_empty() {
            return Err(DecodeError("trailing bytes"));
        }
        Ok(message)
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn byte(&mut self, b: u8) {
        self.0.push(b);
    }

    fn uint(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.0.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.0.push(n as u8);
    }

    fn lamport(&mut self, t: Lamport) {
        self.uint(t.replica_id.as_u32().into());
        self.uint(t.value.into());
    }

    fn global(&mut self, version: &Global) {
        let entries: Vec<Lamport> = version.iter().collect();
        self.uint(entries.len() as u64);
        for t in entries {
            self.lamport(t);
        }
    }

    fn operation(&mut self, op: &Operation) {
        match op {
            Operation::Edit(edit) => {
                self.byte(EDIT);
                self.lamport(edit.timestamp);
                self.global(&edit.version);
                self.uint(edit.ranges.len() as u64);
                for (range, text) in edit.ranges.iter().zip(&edit.new_text) {
                    self.uint(range.start.0 as u64);
                    self.uint(range.end.0 as u64);
                    self.uint(text.len() as u64);
                    self.0.extend_from_slice(text.as_bytes());
                }
            }
            Operation::Undo(undo) => {
                self.byte(UNDO);
                self.lamport(undo.timestamp);
                self.global(&undo.version);
                self.uint(undo.counts.len() as u64);
                for (edit, count) in &undo.counts {
                    self.lamport(*edit);
                    self.uint((*count).into());
                }
            }
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, DecodeError> {
        let (&b, rest) = self.0.split_first().ok_or(DecodeError("truncated"))?;
        self.0 = rest;
        Ok(b)
    }

    fn uint(&mut self) -> Result<u64, DecodeError> {
        let mut n = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            n |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(n);
            }
        }
        Err(DecodeError("varint too long"))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        u32::try_from(self.uint()?).map_err(|_| DecodeError("integer out of range"))
    }

    fn count(&mut self) -> Result<usize, DecodeError> {
        let n = usize::try_from(self.uint()?).map_err(|_| DecodeError("count out of range"))?;
        if n > self.0.len() {
            return Err(DecodeError("count exceeds frame"));
        }
        Ok(n)
    }

    fn offset(&mut self) -> Result<FullOffset, DecodeError> {
        usize::try_from(self.uint()?)
            .map(FullOffset)
            .map_err(|_| DecodeError("offset out of range"))
    }

    fn lamport(&mut self) -> Result<Lamport, DecodeError> {
        let replica = self.u32()?;
        let value = self.u32()?;
        if replica < ReplicaId::FIRST_COLLAB_ID.as_u32() || replica == u32::MAX || value == 0 {
            return Err(DecodeError("reserved timestamp"));
        }
        Ok(Lamport {
            replica_id: ReplicaId::new(replica),
            value,
        })
    }

    fn global(&mut self) -> Result<Global, DecodeError> {
        let count = self.count()?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(self.lamport()?);
        }
        Ok(entries.into_iter().collect())
    }

    fn text(&mut self) -> Result<Arc<str>, DecodeError> {
        let len = self.count()?;
        let (bytes, rest) = self.0.split_at(len);
        self.0 = rest;
        std::str::from_utf8(bytes)
            .map(Arc::from)
            .map_err(|_| DecodeError("text is not UTF-8"))
    }

    fn operation(&mut self) -> Result<Operation, DecodeError> {
        let kind = self.byte()?;
        let timestamp = self.lamport()?;
        let version = self.global()?;
        match kind {
            EDIT => {
                let count = self.count()?;
                let mut ranges: Vec<Range<FullOffset>> = Vec::with_capacity(count);
                let mut new_text = Vec::with_capacity(count);
                for _ in 0..count {
                    let range = self.offset()?..self.offset()?;
                    let ordered = range.start <= range.end
                        && ranges.last().is_none_or(|prev| prev.end <= range.start);
                    if !ordered {
                        return Err(DecodeError("edit ranges out of order"));
                    }
                    ranges.push(range);
                    new_text.push(self.text()?);
                }
                Ok(Operation::Edit(EditOperation {
                    timestamp,
                    version,
                    ranges,
                    new_text,
                }))
            }
            UNDO => {
                let count = self.count()?;
                let mut counts = Vec::with_capacity(count);
                for _ in 0..count {
                    counts.push((self.lamport()?, self.u32()?));
                }
                Ok(Operation::Undo(UndoOperation {
                    timestamp,
                    version,
                    counts: counts.into_iter().collect(),
                }))
            }
            _ => Err(DecodeError("unknown operation")),
        }
    }
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
