use std::borrow::Borrow;
use std::collections::HashSet;
use std::hash::Hash;
use std::ops::Deref;

/// Maps a sequential number to a short identifier string.
/// First character is [a-zA-Z], remaining characters are [a-zA-Z0-9].
fn name_num_to_str(mut num: usize) -> String {
    const FIRST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const REST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    // The actual conversion algorithm doesn't matter very much, so long as it's consistent
    // for the length-one characters and uses the space as efficiently as possible.
    // We could make this a bit nicer, since it can look strange sometimes, but it's not that important.

    // 2 is arbitrary here, but realistically every identifier will sit within it.
    let mut buf = String::with_capacity(2);
    loop {
        if buf.is_empty() {
            buf.push(FIRST[num % FIRST.len()] as char);
            num /= FIRST.len();
        } else {
            buf.push(REST[num % REST.len()] as char);
            num /= REST.len();
        }

        if num == 0 {
            return buf;
        }
    }
}

/// Returns the next valid name that isn't in the `used` set,
/// advancing `name_num` past it.
pub fn next_name<S: Borrow<str> + Eq + Hash>(name_num: &mut usize, used: &HashSet<S>) -> String {
    loop {
        let name = name_num_to_str(*name_num);
        *name_num += 1;
        if !used.contains(&name) {
            return name;
        }
    }
}
