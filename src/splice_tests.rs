use super::*;

fn apply(old: &str, new: &str, s: &Splice) -> String {
    format!(
        "{}{}{}",
        &old[..s.removed.start],
        &new[s.inserted.clone()],
        &old[s.removed.end..]
    )
}

fn check(old: &str, new: &str, caret: usize, removed: Range<usize>, inserted: Range<usize>) {
    let s = infer(old, new, caret);
    assert_eq!(
        s,
        Splice { removed, inserted },
        "{old:?} -> {new:?} caret {caret}"
    );
    assert_eq!(apply(old, new, &s), new);
}

#[test]
fn typing_lands_at_the_caret_among_repeated_characters() {
    check("aa", "aaa", 1, 0..0, 0..1);
    check("aa", "aaa", 2, 1..1, 1..2);
    check("aa", "aaa", 3, 2..2, 2..3);
}

#[test]
fn backspace_and_delete_remove_the_right_repeated_character() {
    check("aa", "a", 0, 0..1, 0..0);
    check("aa", "a", 1, 1..2, 1..1);
    check("aaaa", "aaa", 1, 1..2, 1..1);
}

#[test]
fn return_between_blank_lines_inserts_at_the_caret() {
    check("\n\n", "\n\n\n", 1, 0..0, 0..1);
    check("\n\n", "\n\n\n", 2, 1..1, 1..2);
}

#[test]
fn paste_over_a_selection_replaces_it() {
    check("hello world", "hello there", 11, 6..11, 6..11);
    check("abc", "aXYc", 3, 1..2, 1..3);
}

#[test]
fn multibyte_edits_stay_on_char_boundaries() {
    check("é", "è", 2, 0..2, 0..2);
    check("a🙂b", "ab", 1, 1..5, 1..1);
    check("日本", "日本語", 9, 6..6, 6..9);
}

#[test]
fn a_caret_that_does_not_fit_falls_back_to_the_minimal_diff() {
    let s = infer("hello", "help", 0);
    assert_eq!(apply("hello", "help", &s), "help");
    assert_eq!(
        s,
        Splice {
            removed: 3..5,
            inserted: 3..4
        }
    );
    let s = infer("aé", "é", 99);
    assert_eq!(apply("aé", "é", &s), "é");
}

#[test]
fn every_single_edit_round_trips() {
    let samples = ["", "a", "aa", "abba", "é🙂\n\n#", "x y  z"];
    for old in samples {
        for (start, end) in boundaries(old) {
            for text in ["", "a", "é", "\n", "🙂a"] {
                let new = format!("{}{}{}", &old[..start], text, &old[end..]);
                let s = infer(old, &new, start + text.len());
                assert_eq!(
                    apply(old, &new, &s),
                    new,
                    "{old:?} [{start}..{end}] <- {text:?}"
                );
            }
        }
    }
}

fn boundaries(s: &str) -> Vec<(usize, usize)> {
    let points: Vec<usize> = (0..=s.len()).filter(|&i| s.is_char_boundary(i)).collect();
    points
        .iter()
        .flat_map(|&a| {
            points
                .iter()
                .filter(move |&&b| b >= a)
                .map(move |&b| (a, b))
        })
        .collect()
}
