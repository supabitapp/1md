use rune::{Env, Ime, Key, Mods, Runtime};

use super::*;

const PEER: u64 = 1;

fn editor() -> Runtime<State> {
    let mut rt = Runtime::new(State::new(""), view, (620.0, 520.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn pair() -> (Runtime<State>, Runtime<State>) {
    let (mut a, mut b) = (editor(), editor());
    let hello = a.state.session.connected(PEER);
    a.state.send(hello);
    let hello = b.state.session.connected(PEER);
    b.state.send(hello);
    pump(&mut a, &mut b);
    (a, b)
}

fn pump(a: &mut Runtime<State>, b: &mut Runtime<State>) {
    loop {
        let to_b = std::mem::take(&mut a.state.unsent);
        let to_a = std::mem::take(&mut b.state.unsent);
        if to_a.is_empty() && to_b.is_empty() {
            break;
        }
        for (_, message) in to_b {
            b.state
                .received(PEER, Message::decode(&message.encode()).unwrap());
        }
        for (_, message) in to_a {
            a.state
                .received(PEER, Message::decode(&message.encode()).unwrap());
        }
        a.frame(1.0 / 60.0);
        b.frame(1.0 / 60.0);
    }
}

fn type_text(rt: &mut Runtime<State>, text: &str) {
    rt.ime(Ime::Commit(text.to_owned()));
    rt.frame(1.0 / 60.0);
}

fn press(rt: &mut Runtime<State>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

fn primary() -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods {
        cmd: mac,
        ctrl: !mac,
        ..Mods::default()
    }
}

#[test]
fn typing_in_one_editor_appears_in_the_other() {
    let (mut a, mut b) = pair();
    type_text(&mut a, "hello");
    pump(&mut a, &mut b);
    assert_eq!(b.state.field.text, "hello");
    assert_eq!(b.state.session.doc().text(), "hello");
}

#[test]
fn return_inserts_a_line_break_on_both_sides() {
    let (mut a, mut b) = pair();
    type_text(&mut a, "# Title");
    press(&mut a, Key::Enter, Mods::default());
    type_text(&mut a, "body");
    pump(&mut a, &mut b);
    assert_eq!(b.state.field.text, "# Title\nbody");
}

#[test]
fn a_remote_edit_keeps_the_local_caret_on_its_text() {
    let (mut a, mut b) = pair();
    type_text(&mut b, "hello");
    pump(&mut a, &mut b);
    a.state.field.set_selection(0, 0);
    type_text(&mut a, "# ");
    pump(&mut a, &mut b);
    assert_eq!(b.state.field.text, "# hello");
    assert_eq!((b.state.field.anchor, b.state.field.focus), (7, 7));
    type_text(&mut b, "!");
    pump(&mut a, &mut b);
    assert_eq!(a.state.field.text, "# hello!");
}

#[test]
fn undo_removes_only_this_editors_text() {
    let (mut a, mut b) = pair();
    type_text(&mut a, "mine");
    pump(&mut a, &mut b);
    b.state.field.set_selection(0, 0);
    type_text(&mut b, "theirs ");
    pump(&mut a, &mut b);
    press(&mut a, Key::Command('z'), primary());
    pump(&mut a, &mut b);
    assert_eq!(a.state.field.text, "theirs ");
    assert_eq!(b.state.field.text, "theirs ");
    press(
        &mut a,
        Key::Command('z'),
        Mods {
            shift: true,
            ..primary()
        },
    );
    pump(&mut a, &mut b);
    assert_eq!(b.state.field.text, "theirs mine");
}

#[test]
fn backspace_among_repeated_characters_syncs_the_right_one() {
    let (mut a, mut b) = pair();
    type_text(&mut a, "aaaa");
    pump(&mut a, &mut b);
    a.state.field.set_selection(2, 2);
    press(&mut a, Key::Backspace, Mods::default());
    pump(&mut a, &mut b);
    assert_eq!(a.state.field.text, "aaa");
    assert_eq!(b.state.field.text, "aaa");
    assert_eq!(a.state.field.focus, 1);
}
