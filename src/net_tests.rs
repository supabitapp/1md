use std::time::Duration;

use iroh::RelayMode;
use iroh::endpoint::presets;
use tokio::sync::mpsc::UnboundedReceiver;

use super::*;
use crate::doc::{Document, random_replica_id};
use crate::session::{Outbox, Session};

struct Node {
    net: Net,
    events: UnboundedReceiver<Event>,
    session: Session,
}

impl Node {
    async fn new() -> Self {
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
            .unwrap();
        let (tx, events) = mpsc::unbounded_channel();
        Self {
            net: Net::new(endpoint, tx),
            events,
            session: Session::new(Document::new(random_replica_id())),
        }
    }

    fn send(&self, outbox: Outbox) {
        for (peer, message) in outbox {
            self.net.send(peer, &message);
        }
    }

    fn handle(&mut self, event: Event) {
        let outbox = match event {
            Event::Connected(peer) => self.session.connected(peer),
            Event::Received(peer, message) => self.session.received(peer, message),
            Event::Disconnected(peer) => {
                self.session.disconnected(peer);
                Vec::new()
            }
        };
        self.send(outbox);
    }

    fn edit(&mut self, at: usize, text: &str) {
        let outbox = self.session.edit(at..at, text).unwrap();
        self.send(outbox);
    }

    fn text(&self) -> String {
        self.session.doc().text()
    }
}

async fn pump_until(nodes: &mut [&mut Node], done: impl Fn(&[&mut Node]) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !done(nodes) {
        let mut handled = false;
        for node in nodes.iter_mut() {
            while let Ok(event) = node.events.try_recv() {
                node.handle(event);
                handled = true;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out: {:?}",
            nodes.iter().map(|n| n.text()).collect::<Vec<_>>()
        );
        if !handled {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

fn joined(ticket: &EndpointTicket) -> EndpointAddr {
    let parsed: EndpointTicket = ticket.to_string().parse().unwrap();
    parsed.endpoint_addr().clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_joiner_receives_the_document_and_both_edit_live() {
    let mut host = Node::new().await;
    let mut guest = Node::new().await;
    host.edit(0, "# Shared note\n");

    guest.net.dial(joined(&host.net.ticket())).await.unwrap();
    pump_until(&mut [&mut host, &mut guest], |n| {
        n[1].text() == "# Shared note\n"
    })
    .await;

    host.edit(14, "host line\n");
    guest.edit(0, "guest line\n");
    pump_until(&mut [&mut host, &mut guest], |n| {
        n[0].text() == n[1].text()
            && n[0].text().contains("host line")
            && n[0].text().contains("guest line")
    })
    .await;

    host.net.shutdown().await.unwrap();
    guest.net.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_relays_between_two_guests() {
    let mut host = Node::new().await;
    let mut a = Node::new().await;
    let mut b = Node::new().await;
    a.net.dial(joined(&host.net.ticket())).await.unwrap();
    b.net.dial(joined(&host.net.ticket())).await.unwrap();

    a.edit(0, "from a");
    pump_until(&mut [&mut host, &mut a, &mut b], |n| {
        n[2].text() == "from a"
    })
    .await;
    b.edit(0, "from b, ");
    pump_until(&mut [&mut host, &mut a, &mut b], |n| {
        n.iter().all(|node| node.text() == "from b, from a")
    })
    .await;

    for node in [host, a, b] {
        node.net.shutdown().await.unwrap();
    }
}
