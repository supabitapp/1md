use std::collections::VecDeque;

use super::*;
use crate::doc::ReplicaId;

struct Mesh {
    sessions: Vec<Session>,
    links: Vec<(usize, usize)>,
    queue: VecDeque<(usize, usize, Vec<u8>)>,
}

impl Mesh {
    fn star(peers: usize) -> Self {
        let sessions = (0..peers)
            .map(|i| Session::new(Document::ungrouped(ReplicaId::new(10 + i as u32))))
            .collect();
        let links = (1..peers).map(|i| (0, i)).collect();
        Self {
            sessions,
            links,
            queue: VecDeque::new(),
        }
    }

    fn peer_id(from: usize, to: usize) -> PeerId {
        (from * 1000 + to) as PeerId
    }

    fn target(peer: PeerId) -> usize {
        peer as usize % 1000
    }

    fn send(&mut self, from: usize, outbox: Outbox) {
        for (peer, message) in outbox {
            self.queue
                .push_back((from, Self::target(peer), message.encode()));
        }
    }

    fn connect_all(&mut self) {
        for (a, b) in self.links.clone() {
            let to_b = self.sessions[a].connected(Self::peer_id(a, b));
            self.send(a, to_b);
            let to_a = self.sessions[b].connected(Self::peer_id(b, a));
            self.send(b, to_a);
        }
    }

    fn deliver_all(&mut self) {
        while let Some((from, to, bytes)) = self.queue.pop_front() {
            let message = Message::decode(&bytes).unwrap();
            let reply = self.sessions[to].received(Self::peer_id(to, from), message);
            self.send(to, reply);
        }
    }

    fn texts(&self) -> Vec<String> {
        self.sessions.iter().map(|s| s.doc().text()).collect()
    }
}

#[test]
fn a_late_joiner_gets_everything_from_the_hello_exchange() {
    let mut mesh = Mesh::star(2);
    let none = mesh.sessions[0]
        .edit(0..0, "written before anyone joined")
        .unwrap();
    assert!(none.is_empty());
    mesh.connect_all();
    mesh.deliver_all();
    assert_eq!(mesh.texts(), vec!["written before anyone joined"; 2]);
}

#[test]
fn the_host_relays_edits_between_joiners() {
    let mut mesh = Mesh::star(3);
    mesh.connect_all();
    mesh.deliver_all();
    let out = mesh.sessions[1].edit(0..0, "from one").unwrap();
    mesh.send(1, out);
    let out = mesh.sessions[2].edit(0..0, "from two ").unwrap();
    mesh.send(2, out);
    mesh.deliver_all();
    let texts = mesh.texts();
    assert!(texts.iter().all(|t| t == &texts[0]), "{texts:?}");
    assert!(texts[0].contains("from one") && texts[0].contains("from two"));
}

#[test]
fn undo_and_redo_reach_every_peer() {
    let mut mesh = Mesh::star(3);
    mesh.connect_all();
    let out = mesh.sessions[2].edit(0..0, "draft").unwrap();
    mesh.send(2, out);
    mesh.deliver_all();
    let out = mesh.sessions[2].undo();
    mesh.send(2, out);
    mesh.deliver_all();
    assert_eq!(mesh.texts(), vec![""; 3]);
    let out = mesh.sessions[2].redo();
    mesh.send(2, out);
    mesh.deliver_all();
    assert_eq!(mesh.texts(), vec!["draft"; 3]);
}

#[test]
fn reconnecting_catches_up_edits_made_while_apart() {
    let mut mesh = Mesh::star(2);
    mesh.connect_all();
    mesh.deliver_all();
    mesh.sessions[0].disconnected(Mesh::peer_id(0, 1));
    mesh.sessions[1].disconnected(Mesh::peer_id(1, 0));
    let none = mesh.sessions[0].edit(0..0, "host offline edit. ").unwrap();
    assert!(none.is_empty());
    let none = mesh.sessions[1]
        .edit(0..0, "joiner offline edit. ")
        .unwrap();
    assert!(none.is_empty());
    mesh.connect_all();
    mesh.deliver_all();
    let texts = mesh.texts();
    assert_eq!(texts[0], texts[1]);
    assert!(texts[0].contains("host offline") && texts[0].contains("joiner offline"));
}

#[test]
fn duplicate_frames_are_applied_once_and_not_relayed_again() {
    let mut mesh = Mesh::star(3);
    mesh.connect_all();
    mesh.deliver_all();
    let out = mesh.sessions[1].edit(0..0, "once").unwrap();
    let (_, message) = out[0].clone();
    let first = mesh.sessions[0].received(Mesh::peer_id(0, 1), message.clone());
    assert_eq!(first.len(), 1, "relayed to the other joiner");
    let again = mesh.sessions[0].received(Mesh::peer_id(0, 1), message);
    assert!(again.is_empty());
    assert_eq!(mesh.sessions[0].doc().text(), "once");
}
