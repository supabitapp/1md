use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr};
use iroh_tickets::endpoint::EndpointTicket;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::session::PeerId;
use crate::wire::Message;

pub const ALPN: &[u8] = b"1md/sync/0";
const MAX_FRAME: usize = 16 << 20;

#[derive(Debug)]
pub enum Event {
    Connected(PeerId),
    Received(PeerId, Message),
    Disconnected(PeerId),
}

/// Sync connections over iroh. Every connection, inbound or dialed, carries one stream of
/// length-prefixed frames each way, and its frames arrive as [`Event`]s.
pub struct Net {
    router: Router,
    links: Links,
}

#[derive(Clone, Debug)]
struct Links(Arc<Shared>);

#[derive(Debug)]
struct Shared {
    next: AtomicU64,
    peers: Mutex<HashMap<PeerId, mpsc::UnboundedSender<Vec<u8>>>>,
    events: mpsc::UnboundedSender<Event>,
}

impl Net {
    pub fn new(endpoint: Endpoint, events: mpsc::UnboundedSender<Event>) -> Self {
        let links = Links(Arc::new(Shared {
            next: AtomicU64::new(1),
            peers: Mutex::new(HashMap::new()),
            events,
        }));
        let router = Router::builder(endpoint)
            .accept(ALPN, links.clone())
            .spawn();
        Self { router, links }
    }

    /// A ticket other peers can dial to join this replica's document.
    pub fn ticket(&self) -> EndpointTicket {
        EndpointTicket::new(self.router.endpoint().addr())
    }

    pub async fn dial(&self, addr: impl Into<EndpointAddr>) -> Result<()> {
        let conn = self.router.endpoint().connect(addr, ALPN).await?;
        let (send, recv) = conn.open_bi().await?;
        tokio::spawn(self.links.clone().run(conn, send, recv));
        Ok(())
    }

    /// Queues `message` for `peer`. A peer that has disconnected drops it.
    pub fn send(&self, peer: PeerId, message: &Message) {
        if let Some(frames) = self.links.0.peers.lock().get(&peer) {
            let _ = frames.send(message.encode());
        }
    }

    pub async fn shutdown(self) -> Result<()> {
        self.router.shutdown().await?;
        Ok(())
    }
}

impl ProtocolHandler for Links {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let (send, recv) = conn.accept_bi().await?;
        self.clone().run(conn, send, recv).await;
        Ok(())
    }
}

impl Links {
    async fn run(self, conn: Connection, mut send: SendStream, mut recv: RecvStream) {
        let peer = self.0.next.fetch_add(1, Ordering::Relaxed);
        let (frames, mut outgoing) = mpsc::unbounded_channel();
        self.0.peers.lock().insert(peer, frames);
        let _ = self.0.events.send(Event::Connected(peer));
        let writer = async {
            while let Some(frame) = outgoing.recv().await {
                write_frame(&mut send, &frame).await?;
            }
            anyhow::Ok(())
        };
        let reader = async {
            loop {
                let message = Message::decode(&read_frame(&mut recv).await?)?;
                self.0.events.send(Event::Received(peer, message))?;
            }
        };
        let ended: Result<()> = tokio::select! {
            r = writer => r,
            r = reader => r,
        };
        self.0.peers.lock().remove(&peer);
        let _ = self.0.events.send(Event::Disconnected(peer));
        let reason: &[u8] = if ended.is_ok() { b"done" } else { b"error" };
        conn.close(0u32.into(), reason);
    }
}

async fn write_frame(send: &mut SendStream, frame: &[u8]) -> Result<()> {
    let len = u32::try_from(frame.len()).context("frame too large")?;
    send.write_all(&len.to_be_bytes()).await?;
    send.write_all(frame).await?;
    Ok(())
}

async fn read_frame(recv: &mut RecvStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        bail!("frame of {len} bytes exceeds the {MAX_FRAME} byte limit");
    }
    let mut frame = vec![0u8; len];
    recv.read_exact(&mut frame).await?;
    Ok(frame)
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod tests;
