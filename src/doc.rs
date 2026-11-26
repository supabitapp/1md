use std::num::NonZeroU64;
use std::ops::Range;

pub use clock::{Global, ReplicaId};
pub use text::Operation;

use text::{Buffer, BufferId};

/// One Markdown document as a CRDT replica. Every replica starts empty, so a peer that
/// joins late needs only the operations it hasn't seen.
pub struct Document {
    buffer: Buffer,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidRange;

/// A replica ID for this launch, drawn at random so peers need no coordinator. IDs below
/// `FIRST_COLLAB_ID` are reserved by the clock, and `u32::MAX` is `Lamport::MAX`.
pub fn random_replica_id() -> ReplicaId {
    ReplicaId::new(rand::random_range(
        ReplicaId::FIRST_COLLAB_ID.as_u32()..u32::MAX,
    ))
}

impl Document {
    pub fn new(replica: ReplicaId) -> Self {
        Self {
            buffer: Buffer::new(replica, BufferId::from(NonZeroU64::MIN), ""),
        }
    }

    #[cfg(test)]
    pub(crate) fn ungrouped(replica: ReplicaId) -> Self {
        let mut doc = Self::new(replica);
        doc.buffer.set_group_interval(std::time::Duration::ZERO);
        doc
    }

    pub fn replica(&self) -> ReplicaId {
        self.buffer.replica_id()
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    /// Replaces the UTF-8 byte `range` with `text`. Both ends must lie on char boundaries.
    pub fn edit(&mut self, range: Range<usize>, text: &str) -> Result<Operation, InvalidRange> {
        let rope = self.buffer.as_rope();
        let valid = range.start <= range.end
            && range.end <= rope.len()
            && rope.is_char_boundary(range.start)
            && rope.is_char_boundary(range.end);
        if !valid {
            return Err(InvalidRange);
        }
        Ok(self.buffer.edit([(range, text)]))
    }

    /// Undoes this replica's latest transaction. Other replicas' edits are never undone.
    pub fn undo(&mut self) -> Option<Operation> {
        self.buffer.undo().map(|(_, op)| op)
    }

    pub fn redo(&mut self) -> Option<Operation> {
        self.buffer.redo().map(|(_, op)| op)
    }

    /// Applies operations from other replicas in any order, any number of times. Operations
    /// whose causes haven't arrived wait until they do.
    pub fn apply(&mut self, ops: impl IntoIterator<Item = Operation>) {
        self.buffer.apply_ops(ops);
    }

    pub fn version(&self) -> Global {
        self.buffer.version()
    }

    /// Every operation this replica holds that `version` hasn't observed, oldest first.
    pub fn ops_since(&self, version: &Global) -> Vec<Operation> {
        self.buffer
            .operations()
            .iter()
            .filter(|(_, op)| !version.observed(op.timestamp()))
            .map(|(_, op)| op.clone())
            .collect()
    }
}

#[cfg(test)]
#[path = "doc_tests.rs"]
mod tests;
