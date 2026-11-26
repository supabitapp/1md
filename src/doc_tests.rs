use std::time::Duration;

use super::*;

fn peer(id: u32) -> Document {
    let mut doc = Document::new(ReplicaId::new(id));
    doc.buffer.set_group_interval(Duration::ZERO);
    doc
}

fn seeded(text: &str) -> (Document, Document) {
    let mut a = peer(10);
    let op = a.edit(0..0, text).unwrap();
    let mut b = peer(20);
    b.apply([op]);
    (a, b)
}

fn exchange(a: &mut Document, b: &mut Document) {
    let to_b = a.ops_since(&b.version());
    let to_a = b.ops_since(&a.version());
    b.apply(to_b);
    a.apply(to_a);
}

#[test]
fn concurrent_inserts_at_the_same_spot_converge() {
    let (mut a, mut b) = seeded("ac");
    a.edit(1..1, "x").unwrap();
    b.edit(1..1, "y").unwrap();
    exchange(&mut a, &mut b);
    assert_eq!(a.text(), b.text());
    assert!(a.text() == "axyc" || a.text() == "ayxc", "{}", a.text());
}

#[test]
fn undo_keeps_another_peers_insert() {
    let (mut a, mut b) = seeded("hello");
    a.edit(5..5, " world").unwrap();
    b.edit(0..0, ">> ").unwrap();
    exchange(&mut a, &mut b);
    a.undo().unwrap();
    exchange(&mut a, &mut b);
    assert_eq!(a.text(), ">> hello");
    assert_eq!(b.text(), ">> hello");
}

#[test]
fn a_shared_delete_comes_back_only_when_both_peers_undo() {
    let (mut a, mut b) = seeded("abc");
    a.edit(1..2, "").unwrap();
    b.edit(1..2, "").unwrap();
    exchange(&mut a, &mut b);
    a.undo().unwrap();
    exchange(&mut a, &mut b);
    assert_eq!(a.text(), "ac");
    assert_eq!(b.text(), "ac");
    b.undo().unwrap();
    exchange(&mut a, &mut b);
    assert_eq!(a.text(), "abc");
    assert_eq!(b.text(), "abc");
}

#[test]
fn undoing_an_overlapping_delete_keeps_the_other_peers_deletion() {
    let (mut a, mut b) = seeded("abcd");
    a.edit(1..3, "").unwrap();
    b.edit(2..4, "").unwrap();
    exchange(&mut a, &mut b);
    a.undo().unwrap();
    exchange(&mut a, &mut b);
    assert_eq!(a.text(), "ab");
    assert_eq!(b.text(), "ab");
}

#[test]
fn redo_reapplies_an_undone_edit_on_every_peer() {
    let (mut a, mut b) = seeded("note");
    a.edit(4..4, "s").unwrap();
    exchange(&mut a, &mut b);
    a.undo().unwrap();
    a.redo().unwrap();
    exchange(&mut a, &mut b);
    assert_eq!(b.text(), "notes");
}

#[test]
fn edits_must_land_on_char_boundaries() {
    let mut doc = peer(10);
    doc.edit(0..0, "héllo 🙂").unwrap();
    assert_eq!(doc.edit(2..2, "x"), Err(InvalidRange));
    assert_eq!(doc.edit(0..99, ""), Err(InvalidRange));
    assert_eq!(doc.edit(Range { start: 3, end: 1 }, ""), Err(InvalidRange));
    doc.edit(1..3, "e").unwrap();
    doc.edit(6..10, "!").unwrap();
    assert_eq!(doc.text(), "hello !");
}

#[test]
fn a_late_peer_catches_up_from_ops_since_and_duplicates_are_harmless() {
    let mut a = peer(10);
    a.edit(0..0, "# Title\n").unwrap();
    a.edit(8..8, "body").unwrap();
    a.edit(2..7, "Notes").unwrap();
    let mut b = peer(20);
    let ops = a.ops_since(&b.version());
    b.apply(ops.clone());
    b.apply(ops);
    assert_eq!(b.text(), "# Notes\nbody");
    b.edit(12..12, "!").unwrap();
    let missing = b.ops_since(&a.version());
    assert_eq!(missing.len(), 1);
    a.apply(missing);
    assert_eq!(a.text(), "# Notes\nbody!");
    assert!(a.ops_since(&b.version()).is_empty());
}

#[test]
fn random_replica_ids_avoid_reserved_values() {
    for _ in 0..1000 {
        let id = random_replica_id().as_u32();
        assert!(id >= ReplicaId::FIRST_COLLAB_ID.as_u32());
        assert!(id < u32::MAX);
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn boundary_at_or_before(doc: &Document, mut offset: usize) -> usize {
    while !doc.buffer.as_rope().is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

struct Session {
    diverged: bool,
}

fn random_session(seed: u64, peers: usize, steps: usize) -> Session {
    let mut rng = Rng(seed);
    let mut ids = Vec::new();
    while ids.len() < peers {
        let id = (rng.next() as u32).clamp(ReplicaId::FIRST_COLLAB_ID.as_u32(), u32::MAX - 1);
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut docs: Vec<Document> = ids.into_iter().map(peer).collect();
    let mut inbox: Vec<Vec<Operation>> = vec![Vec::new(); peers];
    let words = ["a", "bb", "é", "🙂", "\n", "# ", "**", ""];
    for _ in 0..steps {
        let p = rng.below(peers);
        let sent = match rng.below(10) {
            0..=5 => {
                let len = docs[p].buffer.len();
                let start = boundary_at_or_before(&docs[p], rng.below(len + 1));
                let end = boundary_at_or_before(&docs[p], (start + rng.below(5)).min(len));
                let word = words[rng.below(words.len())];
                docs[p].edit(start..end.max(start), word).ok()
            }
            6 | 7 => docs[p].undo(),
            8 => docs[p].redo(),
            _ => {
                let q = rng.below(peers);
                let pending = inbox[q].len();
                let take = (rng.below(pending) + 1).min(pending);
                let mut batch: Vec<Operation> = inbox[q].drain(..take).collect();
                for i in (1..batch.len()).rev() {
                    batch.swap(i, rng.below(i + 1));
                }
                docs[q].apply(batch);
                None
            }
        };
        if let Some(op) = sent {
            for (q, queue) in inbox.iter_mut().enumerate() {
                if q != p {
                    queue.push(op.clone());
                }
            }
        }
    }
    let diverged = docs.iter().any(|d| d.text() != docs[0].text());
    for (doc, queue) in docs.iter_mut().zip(&mut inbox) {
        doc.apply(queue.drain(..));
    }
    for doc in &docs {
        assert_eq!(doc.text(), docs[0].text(), "seed {seed} diverged");
        assert!(
            !doc.buffer.has_deferred_ops(),
            "seed {seed} left deferred ops"
        );
    }
    Session { diverged }
}

#[test]
fn random_sessions_with_undo_and_shuffled_delivery_converge() {
    let runs: u64 = 100;
    let diverged = (1..=runs)
        .map(|run| {
            random_session(
                run.wrapping_mul(0x9E37_79B9_7F4A_7C15),
                2 + (run as usize % 4),
                150,
            )
        })
        .filter(|s| s.diverged)
        .count();
    assert!(
        diverged > runs as usize / 2,
        "only {diverged} of {runs} sessions diverged before delivery"
    );
}
