use std::collections::{BTreeSet, HashSet};
use std::ops::Range;

use clock::Lamport;

use crate::doc::{Document, InvalidRange, Operation};
use crate::wire::Message;

/// A transport's handle for one connected peer.
pub type PeerId = u64;

/// Messages to send, each addressed to one peer.
pub type Outbox = Vec<(PeerId, Message)>;

/// One replica and the peers it is connected to. Every local edit, undo and redo goes out to
/// every peer, and operations from one peer are applied once and relayed to the others, so a
/// star of peers around one host keeps every replica up to date.
pub struct Session {
    doc: Document,
    peers: BTreeSet<PeerId>,
    seen: HashSet<Lamport>,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            peers: BTreeSet::new(),
            seen: HashSet::new(),
        }
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    pub fn edit(&mut self, range: Range<usize>, text: &str) -> Result<Outbox, InvalidRange> {
        let op = self.doc.edit(range, text)?;
        Ok(self.local(op))
    }

    #[must_use]
    pub fn undo(&mut self) -> Outbox {
        self.doc.undo().map(|op| self.local(op)).unwrap_or_default()
    }

    #[must_use]
    pub fn redo(&mut self) -> Outbox {
        self.doc.redo().map(|op| self.local(op)).unwrap_or_default()
    }

    /// Greets a new peer with this replica's version, so the peer answers with what it lacks.
    #[must_use]
    pub fn connected(&mut self, peer: PeerId) -> Outbox {
        self.peers.insert(peer);
        vec![(peer, Message::Hello(self.doc.version()))]
    }

    pub fn disconnected(&mut self, peer: PeerId) {
        self.peers.remove(&peer);
    }

    #[must_use]
    pub fn received(&mut self, from: PeerId, message: Message) -> Outbox {
        match message {
            Message::Hello(version) => {
                let missing = self.doc.ops_since(&version);
                if missing.is_empty() {
                    Vec::new()
                } else {
                    vec![(from, Message::Ops(missing))]
                }
            }
            Message::Ops(ops) => {
                let fresh: Vec<Operation> = ops
                    .into_iter()
                    .filter(|op| self.seen.insert(op.timestamp()))
                    .collect();
                if fresh.is_empty() {
                    return Vec::new();
                }
                self.doc.apply(fresh.clone());
                self.relay(Some(from), fresh)
            }
        }
    }

    fn local(&mut self, op: Operation) -> Outbox {
        self.seen.insert(op.timestamp());
        self.relay(None, vec![op])
    }

    fn relay(&self, except: Option<PeerId>, ops: Vec<Operation>) -> Outbox {
        self.peers
            .iter()
            .filter(|&&peer| Some(peer) != except)
            .map(|&peer| (peer, Message::Ops(ops.clone())))
            .collect()
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
