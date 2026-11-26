use std::sync::Arc;

use iroh::Endpoint;
use iroh::endpoint::presets;
use iroh_tickets::endpoint::EndpointTicket;
use onemd::doc::{Document, random_replica_id};
use onemd::net::{Event, Net};
use onemd::session::{Outbox, Session};
use onemd::splice;
use onemd::wire::Message;
use rune::tok::{self, type_};
use rune::{App, AppHandle, AppSpec, Cx, El, KeyEvent, TextEdit, TextStyle, col, text, text_input};
use tokio::sync::mpsc;

struct State {
    session: Session,
    field: TextEdit,
    net: Option<Arc<Net>>,
    status: String,
    held: Vec<(u64, Message)>,
    unsent: Outbox,
}

enum Mode {
    Offline,
    Host,
    Join(EndpointTicket),
}

impl State {
    fn new(status: &str) -> Self {
        let mut field = TextEdit::new("");
        field.focused = true;
        Self {
            session: Session::new(Document::new(random_replica_id())),
            field,
            net: None,
            status: status.to_owned(),
            held: Vec::new(),
            unsent: Vec::new(),
        }
    }

    fn send(&mut self, outbox: Outbox) {
        self.unsent.extend(outbox);
        if let Some(net) = &self.net {
            for (peer, message) in self.unsent.drain(..) {
                net.send(peer, &message);
            }
        }
    }

    fn local_change(&mut self) {
        let old = self.session.doc().text();
        let s = splice::infer(&old, &self.field.text, self.field.focus);
        let inserted = self.field.text[s.inserted].to_owned();
        match self.session.edit(s.removed, &inserted) {
            Ok(outbox) => self.send(outbox),
            Err(_) => self.show_doc(),
        }
        if self.session.doc().text() != self.field.text {
            self.show_doc();
        }
        self.drain_held();
    }

    fn undo_key(&mut self, key: &KeyEvent) -> bool {
        let outbox = match key.edit_command() {
            Some('z') if key.mods.shift => self.redo(),
            Some('z') => self.undo(),
            Some('y') => self.redo(),
            _ => return false,
        };
        self.send(outbox);
        true
    }

    fn undo(&mut self) -> Outbox {
        self.remapping(|session| session.undo())
    }

    fn redo(&mut self) -> Outbox {
        self.remapping(|session| session.redo())
    }

    fn received(&mut self, peer: u64, message: Message) {
        let composing_over_selection =
            self.field.is_composing() && self.field.anchor != self.field.focus;
        if composing_over_selection {
            self.held.push((peer, message));
            return;
        }
        let outbox = self.remapping(|session| session.received(peer, message));
        self.send(outbox);
    }

    fn drain_held(&mut self) {
        for (peer, message) in std::mem::take(&mut self.held) {
            self.received(peer, message);
        }
    }

    fn remapping(&mut self, change: impl FnOnce(&mut Session) -> Outbox) -> Outbox {
        let doc = self.session.doc();
        let anchor = doc.anchor(self.field.anchor);
        let focus = doc.anchor(self.field.focus);
        let outbox = change(&mut self.session);
        let doc = self.session.doc();
        self.field.text = doc.text();
        self.field.anchor = doc.offset(&anchor);
        self.field.focus = doc.offset(&focus);
        outbox
    }

    fn show_doc(&mut self) {
        self.remapping(|_| Vec::new());
    }

    fn network(&mut self, event: Event) {
        match event {
            Event::Connected(peer) => {
                let outbox = self.session.connected(peer);
                self.send(outbox);
            }
            Event::Received(peer, message) => self.received(peer, message),
            Event::Disconnected(peer) => self.session.disconnected(peer),
        }
        self.status = match self.session.peer_count() {
            0 => "Waiting for peers".to_owned(),
            1 => "Connected to 1 peer".to_owned(),
            n => format!("Connected to {n} peers"),
        };
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let editor = text_input(&s.field, TextStyle::new(15.0, 400.0).lh(22.0))
        .key("editor")
        .label("Note")
        .w_full()
        .grow(1.0)
        .on_edit(|s: &mut State| &mut s.field)
        .on_input_change(|s: &mut State, _| s.local_change())
        .on_input_key(|s: &mut State, key, _| s.undo_key(key))
        .on_submit(|s: &mut State, _| {
            s.field.insert("\n");
            s.local_change();
        });
    col()
        .w_full()
        .h_full()
        .fill(t.bg)
        .px(tok::space::S6)
        .py(tok::space::S6)
        .gap(tok::space::S3)
        .child(
            text(s.status.clone())
                .font(type_::LABEL)
                .color(t.text_secondary),
        )
        .child(editor.color(t.text))
}

fn mode() -> Result<Mode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().position(|a| a == "--join") {
        Some(i) => {
            let ticket = args.get(i + 1).ok_or("--join needs a ticket")?;
            ticket
                .parse()
                .map(Mode::Join)
                .map_err(|e| format!("bad ticket: {e}"))
        }
        None if args.iter().any(|a| a == "--host") => Ok(Mode::Host),
        None => Ok(Mode::Offline),
    }
}

fn start_network(handle: AppHandle<State>, join: Option<EndpointTicket>) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(e) => return handle.update(move |s, _| s.status = format!("Network failed: {e}")),
        };
        runtime.block_on(async move {
            let endpoint = match Endpoint::bind(presets::N0).await {
                Ok(endpoint) => endpoint,
                Err(e) => {
                    return handle.update(move |s, _| s.status = format!("Network failed: {e}"));
                }
            };
            let (tx, mut events) = mpsc::unbounded_channel();
            let net = Arc::new(Net::new(endpoint, tx));
            let ticket = net.ticket().to_string();
            println!("1md --join {ticket}");
            let shared = net.clone();
            handle.update(move |s, _| {
                s.net = Some(shared);
                s.send(Vec::new());
                s.status = "Waiting for peers. The join command is in the terminal.".to_owned();
            });
            if let Some(ticket) = join
                && let Err(e) = net.dial(ticket.endpoint_addr().clone()).await
            {
                handle.update(move |s, _| s.status = format!("Couldn't join: {e}"));
            }
            while let Some(event) = events.recv().await {
                handle.update(move |s, _| s.network(event));
            }
        });
    });
}

fn main() {
    let mode = match mode() {
        Ok(mode) => mode,
        Err(e) => {
            eprintln!("1md: {e}");
            std::process::exit(2);
        }
    };
    let status = match mode {
        Mode::Offline => "Offline. Run with --host to share this note.",
        Mode::Host | Mode::Join(_) => "Starting the network",
    };
    let status = status.to_owned();
    let app = App::new(AppSpec::new("1md", (620.0, 520.0), || State::new(""), view))
        .init(move || State::new(&status));
    match mode {
        Mode::Offline => {}
        Mode::Host => start_network(app.handle(), None),
        Mode::Join(ticket) => start_network(app.handle(), Some(ticket)),
    }
    app.run();
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
