use super::*;
use crate::doc::Document;

fn doc(id: u32) -> Document {
    Document::ungrouped(ReplicaId::new(id))
}

#[test]
fn hello_and_ops_round_trip() {
    let mut a = doc(10);
    let mut ops = vec![a.edit(0..0, "héllo 🙂\n").unwrap()];
    ops.push(a.edit(1..3, "e").unwrap());
    ops.extend(a.undo());
    let mut b = doc(4_000_000_000);
    b.apply(ops.clone());
    ops.push(b.edit(0..0, "# ").unwrap());

    for message in [Message::Hello(b.version()), Message::Ops(ops)] {
        assert_eq!(Message::decode(&message.encode()), Ok(message));
    }
}

#[test]
fn decoded_ops_rebuild_the_same_document() {
    let mut a = doc(10);
    a.edit(0..0, "one two three").unwrap();
    a.edit(4..7, "2").unwrap();
    a.undo().unwrap();
    a.edit(0..3, "1").unwrap();
    let bytes = Message::Ops(a.ops_since(&Global::new())).encode();
    let Ok(Message::Ops(ops)) = Message::decode(&bytes) else {
        panic!("ops frame did not decode");
    };
    let mut b = doc(20);
    b.apply(ops);
    assert_eq!(b.text(), a.text());
}

#[test]
fn rejects_what_a_peer_cannot_produce() {
    let hello = Message::Hello(doc(10).version()).encode();
    let mut wrong_protocol = hello.clone();
    wrong_protocol[0] = 9;
    assert_eq!(
        Message::decode(&wrong_protocol),
        Err(DecodeError("unsupported protocol version"))
    );

    let mut trailing = hello.clone();
    trailing.push(0);
    assert_eq!(
        Message::decode(&trailing),
        Err(DecodeError("trailing bytes"))
    );

    assert_eq!(Message::decode(&[]), Err(DecodeError("truncated")));

    let reserved_replica = [PROTOCOL, HELLO, 1, 3, 1];
    assert_eq!(
        Message::decode(&reserved_replica),
        Err(DecodeError("reserved timestamp"))
    );

    let huge_count = [PROTOCOL, OPS, 0xff, 0xff, 0xff, 0x0f];
    assert_eq!(
        Message::decode(&huge_count),
        Err(DecodeError("count exceeds frame"))
    );

    let reversed_range = [PROTOCOL, OPS, 1, EDIT, 10, 1, 0, 1, 5, 2, 0];
    assert_eq!(
        Message::decode(&reversed_range),
        Err(DecodeError("edit ranges out of order"))
    );

    let bad_utf8 = [PROTOCOL, OPS, 1, EDIT, 10, 1, 0, 1, 0, 0, 1, 0xff];
    assert_eq!(
        Message::decode(&bad_utf8),
        Err(DecodeError("text is not UTF-8"))
    );
}

#[test]
fn random_and_mutated_bytes_never_panic() {
    let mut a = doc(10);
    a.edit(0..0, "seed text").unwrap();
    a.edit(5..9, "frame").unwrap();
    a.undo().unwrap();
    let valid = Message::Ops(a.ops_since(&Global::new())).encode();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..20_000 {
        let mut bytes = valid.clone();
        for _ in 0..1 + next() % 4 {
            let at = (next() as usize) % bytes.len();
            bytes[at] = next() as u8;
        }
        bytes.truncate(1 + (next() as usize) % bytes.len());
        let _ = Message::decode(&bytes);
        let noise: Vec<u8> = (0..next() % 64).map(|_| next() as u8).collect();
        let _ = Message::decode(&noise);
    }
}
