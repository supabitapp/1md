mod system_clock;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::{
    cmp::{self, Ordering},
    fmt,
};

pub use system_clock::*;

/// A unique identifier for each distributed node.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ReplicaId(u32);

impl ReplicaId {
    /// The local replica
    pub const LOCAL: ReplicaId = ReplicaId(0);
    /// The remote replica of the connected remote server.
    pub const REMOTE_SERVER: ReplicaId = ReplicaId(1);
    /// The agent's unique identifier.
    pub const AGENT: ReplicaId = ReplicaId(2);
    /// A local branch.
    pub const LOCAL_BRANCH: ReplicaId = ReplicaId(3);
    /// The first collaborative replica ID, any replica equal or greater than this is a collaborative replica.
    pub const FIRST_COLLAB_ID: ReplicaId = ReplicaId(8);

    pub fn new(id: u32) -> Self {
        ReplicaId(id)
    }

    pub fn as_u32(&self) -> u32 {
        self.0
    }

    pub fn is_remote(self) -> bool {
        self == ReplicaId::REMOTE_SERVER || self >= ReplicaId::FIRST_COLLAB_ID
    }
}

impl fmt::Debug for ReplicaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == ReplicaId::LOCAL {
            write!(f, "<local>")
        } else if *self == ReplicaId::REMOTE_SERVER {
            write!(f, "<remote>")
        } else if *self == ReplicaId::AGENT {
            write!(f, "<agent>")
        } else if *self == ReplicaId::LOCAL_BRANCH {
            write!(f, "<branch>")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

/// A [Lamport sequence number](https://en.wikipedia.org/wiki/Lamport_timestamp).
pub type Seq = u32;

/// A [Lamport timestamp](https://en.wikipedia.org/wiki/Lamport_timestamp),
/// used to determine the ordering of events in the editor.
#[derive(Clone, Copy, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Lamport {
    pub value: Seq,
    pub replica_id: ReplicaId,
}

/// A [version vector](https://en.wikipedia.org/wiki/Version_vector).
#[derive(Clone, Default, Hash, Eq, PartialEq)]
pub struct Global {
    values: SmallVec<[(ReplicaId, Seq); 4]>,
}

impl Global {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, replica_id: ReplicaId) -> Seq {
        match self.values.binary_search_by_key(&replica_id, |(id, _)| *id) {
            Ok(ix) => self.values[ix].1,
            Err(_) => 0,
        }
    }

    pub fn observe(&mut self, timestamp: Lamport) {
        debug_assert_ne!(timestamp.replica_id, Lamport::MAX.replica_id);
        if timestamp.value > 0 {
            match self
                .values
                .binary_search_by_key(&timestamp.replica_id, |(id, _)| *id)
            {
                Ok(ix) => self.values[ix].1 = cmp::max(self.values[ix].1, timestamp.value),
                Err(ix) => self
                    .values
                    .insert(ix, (timestamp.replica_id, timestamp.value)),
            }
        }
    }

    #[doc(alias = "synchronize")]
    pub fn join(&mut self, other: &Self) {
        for timestamp in other.iter() {
            self.observe(timestamp);
        }
    }

    pub fn meet(&mut self, other: &Self) {
        let mut result = Global::new();
        for (id, left) in &self.values {
            let right = other.get(*id);
            let value = if right == 0 { *left } else { cmp::min(*left, right) };
            result.observe(Lamport { replica_id: *id, value });
        }
        for (id, right) in &other.values {
            if self.get(*id) == 0 {
                result.observe(Lamport { replica_id: *id, value: *right });
            }
        }
        *self = result;
    }

    pub fn observed(&self, timestamp: Lamport) -> bool {
        self.get(timestamp.replica_id) >= timestamp.value
    }

    pub fn observed_any(&self, other: &Self) -> bool {
        other
            .values
            .iter()
            .any(|(id, right)| *right > 0 && self.get(*id) >= *right)
    }

    pub fn observed_all(&self, other: &Self) -> bool {
        other.values.iter().all(|(id, right)| self.get(*id) >= *right)
    }

    pub fn changed_since(&self, other: &Self) -> bool {
        self.values.iter().any(|(id, left)| *left > other.get(*id))
    }

    pub fn most_recent(&self) -> Option<Lamport> {
        self.iter().max_by_key(|timestamp| timestamp.value)
    }

    pub fn iter(&self) -> impl Iterator<Item = Lamport> + '_ {
        self.values.iter().map(|(replica_id, value)| Lamport {
            replica_id: *replica_id,
            value: *value,
        })
    }
}

impl FromIterator<Lamport> for Global {
    fn from_iter<T: IntoIterator<Item = Lamport>>(locals: T) -> Self {
        let mut result = Self::new();
        for local in locals {
            result.observe(local);
        }
        result
    }
}

impl Ord for Lamport {
    fn cmp(&self, other: &Self) -> Ordering {
        // Use the replica id to break ties between concurrent events.
        self.value
            .cmp(&other.value)
            .then_with(|| self.replica_id.cmp(&other.replica_id))
    }
}

impl PartialOrd for Lamport {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Lamport {
    pub const MIN: Self = Self {
        replica_id: ReplicaId(u32::MIN),
        value: Seq::MIN,
    };

    pub const MAX: Self = Self {
        replica_id: ReplicaId(u32::MAX),
        value: Seq::MAX,
    };

    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            value: 1,
            replica_id,
        }
    }

    pub fn as_u64(self) -> u64 {
        ((self.value as u64) << 32) | (self.replica_id.0 as u64)
    }

    pub fn tick(&mut self) -> Self {
        let timestamp = *self;
        self.value += 1;
        timestamp
    }

    pub fn observe(&mut self, timestamp: Self) {
        self.value = cmp::max(self.value, timestamp.value) + 1;
    }
}

impl fmt::Debug for Lamport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::MAX {
            write!(f, "Lamport {{MAX}}")
        } else if *self == Self::MIN {
            write!(f, "Lamport {{MIN}}")
        } else {
            write!(f, "Lamport {{{:?}: {}}}", self.replica_id, self.value)
        }
    }
}

impl fmt::Debug for Global {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Global {{")?;
        for (ix, timestamp) in self.iter().enumerate() {
            if ix > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{:?}: {}", timestamp.replica_id, timestamp.value)?;
        }
        write!(f, "}}")
    }
}
