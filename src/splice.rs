use std::ops::Range;

/// One replacement that turns `old` into `new`: `old[removed]` gives way to `new[inserted]`,
/// and `removed.start == inserted.start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub removed: Range<usize>,
    pub inserted: Range<usize>,
}

/// The edit a text field made, given its text before and after and its caret after.
///
/// A field edit ends with the caret just past the inserted text, so the caret fixes where the
/// edit ends in both strings, and the common prefix, capped there, fixes where it starts. That
/// is exact for typing, deleting and pasting, even among repeated characters. When the caret
/// doesn't fit (an edit the field didn't make), it falls back to the minimal common
/// prefix and suffix diff.
pub fn infer(old: &str, new: &str, caret: usize) -> Splice {
    pinned(old, new, caret).unwrap_or_else(|| minimal(old, new))
}

fn pinned(old: &str, new: &str, caret: usize) -> Option<Splice> {
    let tail = new.len().checked_sub(caret)?;
    let old_end = old.len().checked_sub(tail)?;
    if !new.is_char_boundary(caret)
        || !old.is_char_boundary(old_end)
        || old[old_end..] != new[caret..]
    {
        return None;
    }
    let start = floor_boundary(old, common_prefix(old, new).min(caret).min(old_end));
    Some(Splice {
        removed: start..old_end,
        inserted: start..caret,
    })
}

fn minimal(old: &str, new: &str) -> Splice {
    let start = floor_boundary(old, common_prefix(old, new));
    let max_suffix = old.len().min(new.len()) - start;
    let suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let mut old_end = old.len() - suffix;
    while !old.is_char_boundary(old_end) || !new.is_char_boundary(new.len() - (old.len() - old_end))
    {
        old_end += 1;
    }
    Splice {
        removed: start..old_end,
        inserted: start..new.len() - (old.len() - old_end),
    }
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
#[path = "splice_tests.rs"]
mod tests;
