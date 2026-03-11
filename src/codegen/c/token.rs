use crate::codegen::c::CLowerer;
use crate::codegen::token::{Token, TokenInfo, TokenStyle, Tokens};
use crate::util::name::next_name;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::iter;

pub const LEFT_PAREN: Token<'static> = Token::new(Cow::Borrowed("("));
pub const RIGHT_PAREN: Token<'static> = Token::new(Cow::Borrowed(")"));
pub const LEFT_SQUIGGLE: Token<'static> = Token::new(Cow::Borrowed("{"));
pub const RIGHT_SQUIGGLE: Token<'static> = Token::new(Cow::Borrowed("}"));
pub const LEFT_BRACKET: Token<'static> = Token::new(Cow::Borrowed("["));
pub const RIGHT_BRACKET: Token<'static> = Token::new(Cow::Borrowed("]"));
pub const SEMI: Token<'static> = Token::new(Cow::Borrowed(";"));
pub const NEWLINE_REQUIRED: Token<'static> = Token::new(Cow::Borrowed("\n"));

pub const INDENT: Token<'static> = Token::new_fancy(Cow::Borrowed("    "));
pub const NEWLINE: Token<'static> = Token::new_fancy(Cow::Borrowed("\n"));

impl TokenInfo for CLowerer {
    fn needs_space_between<'a>(&self, left: &Token<'a>, right: &Token<'a>) -> bool {
        if left.style == TokenStyle::Marker && right.style == TokenStyle::Marker {
            return false;
        }

        let Some(left_text) = &left.text else {
            return false;
        };
        let Some(right_text) = &right.text else {
            return false;
        };

        if left_text.is_empty() || right_text.is_empty() {
            return false;
        }

        let mut left_char = left_text.chars().last().unwrap();
        let mut right_char = right_text.chars().next().unwrap();

        // Treat markers as identifiers to be conservative about
        // inserting spaces.
        if left.style == TokenStyle::Marker {
            left_char = 'a';
        }
        if right.style == TokenStyle::Marker {
            right_char = 'a';
        }

        // Words must be separated.
        // For example, int main, return 0, variable1 variable2, 123 456
        if is_ident_char(left_char) && is_ident_char(right_char) {
            return true;
        }

        // Spaces are already allowed between operators,
        // and excluding a space here creates a different meaning.
        if is_punct_char(left_char) && is_punct_char(right_char) {
            // Spaces aren't needed if they don't form compound operators.
            return matches!(
                (left_char, right_char),
                ('/', '*')
                    | ('/', '/')
                    | ('+', '+')
                    | ('-', '-')
                    | ('-', '>')
                    | ('<', '<')
                    | ('>', '>')
                    | ('=', '=')
                    | ('<', '=')
                    | ('>', '=')
                    | ('!', '=')
                    | ('&', '&')
                    | ('|', '|')
                    | ('+', '=')
                    | ('-', '=')
                    | ('*', '=')
                    | ('/', '=')
                    | ('%', '=')
                    | ('&', '=')
                    | ('|', '=')
                    | ('^', '=')
            );
        }

        // Literal prefixes.
        if (right_char == '"' || right_char == '\'') && (matches!(left_char, 'L' | 'u' | 'U')) {
            return true;
        }

        false
    }
}

/// Returns true for [a-zA-Z0-9_].
/// Used to detect Identifiers, Keywords, and Numbers.
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Returns true for C operator/separator symbols.
fn is_punct_char(c: char) -> bool {
    "!%^&*-+=|~<>.?/:".contains(c)
}

/// Escapes a string for use in a C string literal.
pub fn escape_string(s: &str) -> String {
    let mut output = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => output.push_str("\\\""),
            c => append_escape(&mut output, c),
        }
    }

    output
}

/// Escapes a char for use in a C char literal.
pub fn escape_char(s: &str) -> String {
    let mut output = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\'' => output.push_str("\\'"),
            c => append_escape(&mut output, c),
        }
    }

    output
}

/// Generic escape for strings and chars.
/// Doesn't include escaping for ' / ".
pub fn append_escape(append: &mut String, c: char) {
    let add = match c {
        '\\' => "\\\\",
        '\n' => "\\n",
        '\r' => "\\r",
        '\t' => "\\t",
        '\0' => "\\0",
        c => {
            if c.is_control() {
                if c.is_ascii() && (c as u32) <= 0o777 {
                    // \nnn in octal
                    &format!("\\{:03o}", c as u32)
                } else {
                    // \uhhhh in hex
                    &format!("\\u{:04x}", c as u32)
                }
            } else {
                append.push(c);
                return;
            }
        }
    };

    append.push_str(add);
}

/// The length we assume a replacement (define name) will be.
/// For simplicity this is kept at 1, as it's more challenging to
/// determine the current next valid name for each define.
///
/// As a result, the cost calculation may be slightly off, and it may
/// incorrectly create a define when it shouldn't.
/// However, this will probably only cause a loss of a few characters,
/// and is unlikely to begin with.
const REPLACE_LEN: usize = 1;

/// The full cost of a define line: `#define {name} {text}\n`.
///
/// If the value contains newlines, each one requires an extra
/// backslash, adding to the cost.
fn define_line_cost(text: &str) -> i32 {
    let newline_cost = text.bytes().filter(|&b| b == b'\n').count();
    ("#define ".len() + REPLACE_LEN + " ".len() + text.len() + newline_cost + "\n".len()) as i32
}

/// How many characters are saved by replacing a token with text `text`,
/// appearing `count` times, with a define.
fn define_savings(text: &str, count: usize) -> i32 {
    let saved_per_use = text.len() as i32 - REPLACE_LEN as i32;
    let line_cost = define_line_cost(text);
    saved_per_use * count as i32 - line_cost
}

/// Merges two token texts into a single string, inserting a space
/// between them if required by the token info.
fn merge_token_text(info: &impl TokenInfo, left: &Token, right: &Token) -> String {
    let left_text = left.text.as_ref().expect("Token text is required");
    let right_text = right.text.as_ref().expect("Token text is required");
    let space = if info.needs_space_between(left, right) {
        " "
    } else {
        ""
    };
    format!("{}{}{}", left_text, space, right_text)
}

/// Returns `(left_text, right_text, merged_text)` for the
/// pair at `body[i]` and `body[i+1]`, or `None` if they can't be
/// merged.
fn pair_at<'a>(
    info: &impl TokenInfo,
    body: &Tokens<'a>,
    i: usize,
) -> Option<(Cow<'a, str>, Cow<'a, str>, String)> {
    let left = &body[i];
    let right = &body[i + 1];
    if left.style == TokenStyle::Marker || right.style == TokenStyle::Marker {
        return None;
    }
    let left_text = left.text.as_ref().expect("Token text is required");
    let right_text = right.text.as_ref().expect("Token text is required");
    let merged = merge_token_text(info, left, right);

    Some((left_text.clone(), right_text.clone(), merged))
}

/// Tracks frequency counts for individual tokens and adjacent pairs
/// during the iterative merge phase.
struct MergeState<'a> {
    /// Token text -> occurrence count in the body.
    token_counts: HashMap<Cow<'a, str>, usize>,
    /// (left text, right text) -> (merged text, occurrence count).
    /// The merged text is used for cost calculations via `define_savings`.
    pair_counts: HashMap<(Cow<'a, str>, Cow<'a, str>), (String, usize)>,
}

impl<'a> MergeState<'a> {
    /// Builds initial counts from the body.
    fn new(info: &impl TokenInfo, body: &Tokens<'a>) -> Self {
        let mut token_counts: HashMap<Cow<'a, str>, usize> = HashMap::new();
        let mut pair_counts: HashMap<(Cow<'a, str>, Cow<'a, str>), (String, usize)> =
            HashMap::new();

        for token in body.iter() {
            if token.style == TokenStyle::Marker {
                continue;
            }

            if let Some(text) = token.text.as_ref() {
                *token_counts.entry(text.clone()).or_default() += 1;
            }
        }

        for i in 0..body.len().saturating_sub(1) {
            if let Some((left, right, merged)) = pair_at(info, body, i) {
                pair_counts
                    .entry((left, right))
                    .and_modify(|v| v.1 += 1)
                    .or_insert((merged, 1));
            }
        }

        Self {
            token_counts,
            pair_counts,
        }
    }

    /// Finds the single most beneficial merge, or `None` if no
    /// profitable merge remains.
    /// Returns the left and right tokens which should be merged.
    ///
    /// A merge is profitable when after > before, i.e. merging
    /// lets us save more text than could be done with both tokens
    /// individually.
    fn find_best_merge(&self) -> Option<(Cow<'a, str>, Cow<'a, str>)> {
        // This is used to keep a consistent sort order
        // when we get the same benefit.
        #[derive(PartialEq, Eq, PartialOrd, Ord)]
        struct MaxKey<'a>(i32, Cow<'a, str>, Cow<'a, str>);

        self.pair_counts
            .iter()
            .filter(|(_, (_, count))| *count > 0)
            .filter_map(|((left, right), (pair_merged, pair_count))| {
                let left_count = *self.token_counts.get(left)?;
                let right_count = *self.token_counts.get(right)?;
                if left_count == 0 || right_count == 0 {
                    return None;
                }

                // We have a min of 0 because if we don't save anything from making
                // this token a define, we wouldn't do so in the first place, i.e.,
                // zero savings.
                let before = define_savings(left, left_count).max(0)
                    + define_savings(right, right_count).max(0);

                let after = define_savings(pair_merged, *pair_count).max(0)
                    // left and right no longer get to claim the savings for this pair.
                    + define_savings(left, left_count - *pair_count).max(0)
                    + define_savings(right, right_count - *pair_count).max(0);

                let benefit = after - before;
                // This makes us stop iterating once we can't compress anymore.
                if benefit > 0 {
                    Some((left.clone(), right.clone(), benefit))
                } else {
                    None
                }
            })
            .max_by_key(|(left, right, b)| MaxKey(*b, left.clone(), right.clone()))
            .map(|(a, b, _)| (a, b))
    }
}

/// Iteratively merges the most beneficial adjacent pair until no
/// profitable merges remain.
fn merge_pairs(info: &impl TokenInfo, body: &mut Tokens<'_>) {
    loop {
        // It's somewhat expensive to recompute the MergeState every iteration,
        // but updating it correctly is very complex.
        let state = MergeState::new(info, body);
        let Some((merge_left, merge_right)) = state.find_best_merge() else {
            break;
        };

        info.merge_tokens(
            body,
            Some(&|left, right| {
                left.style != TokenStyle::Marker
                    && right.style != TokenStyle::Marker
                    && left.text.as_ref() == Some(&merge_left)
                    && right.text.as_ref() == Some(&merge_right)
            }),
        );
    }
}

/// Assigns defines for tokens where the savings are positive, rewrites
/// `body` to use the short names, and prepends define lines to
/// `header`.
fn assign_defines<'a>(
    info: &impl TokenInfo,
    header: &mut Tokens<'a>,
    body: &mut Tokens<'a>,
    used: &HashSet<Cow<'a, str>>,
) {
    // Count occurrences of each distinct token text.
    let mut counts = HashMap::<String, usize>::new();
    for token in body.iter() {
        if token.style == TokenStyle::Marker {
            continue;
        }

        *counts
            .entry(
                token
                    .text
                    .as_ref()
                    .expect("Token text is required")
                    .to_string(),
            )
            .or_default() += 1;
    }

    // Assign short names.
    let mut name_num: usize = 0;
    // Token -> define name.
    let mut defines: Vec<(String, String)> = Vec::new();

    // This is in a loop, rather than a single collect phase, because the replacements
    // which are profitable can depend on whether they're in between identifiers.
    // The neighbors of each token change as we do more replacements.
    // Therefore, we must recompute at each step to determine profitability.
    loop {
        // This is used to keep a consistent sort order
        // when we get the same savings.
        #[derive(PartialEq, Eq, PartialOrd, Ord)]
        struct SortKey<'a>(i32, &'a str);

        // Collect tokens worth defining, sorted by descending savings so
        // that the most valuable tokens get the shortest names first.
        let Some(to_replace) = counts
            .iter()
            .flat_map(|(token, &count)| {
                // Swapping to a define can introduce spaces on either
                // side which weren't previously there.
                // We must factor this into our cost calculation.
                //
                // It may be possible to factor this into merge_pairs as well,
                // although it's likely very complex.
                let num_spaces: i32 = body
                    .array_windows::<3>()
                    .filter(|[_, middle, _]| middle.text.as_deref() == Some(token))
                    .map(|[left, middle, right]| {
                        // Use a dummy token that looks like an identifier (what the define replacement will be).
                        let left_space = info.needs_space_between(left, &Token::new("a".into()))
                            && !info.needs_space_between(left, middle);
                        let right_space = info.needs_space_between(&Token::new("a".into()), right)
                            && !info.needs_space_between(middle, right);

                        left_space as i32 + right_space as i32
                    })
                    .sum();

                let s = define_savings(token, count) - num_spaces;
                if s <= 0 {
                    // This token is not worth compressing.
                    return None;
                }

                Some((token, s))
            })
            .max_by_key(|(text, savings)| SortKey(*savings, text))
            .map(|(text, _)| text.clone())
        else {
            break;
        };
        counts.remove(&to_replace);

        let define_name = next_name(&mut name_num, used);
        defines.push((to_replace.clone(), define_name.to_string()));

        for token in body.iter_mut() {
            if token.style == TokenStyle::Marker {
                continue;
            }
            let text = token.text.as_ref().expect("Token text is required");

            if text.as_ref() == to_replace {
                token.text = Some(define_name.to_string().into());
            }
        }
    }

    // Prepend define lines to header (reversed so the first define
    // ends up at the top).
    //
    // If the original token contains newlines, we must insert backslash
    // characters so it spans multiple lines.
    for (original, name) in defines.iter().rev() {
        // The newline is still inserted literally, just with a backslash behind it.
        let escaped = original.replace('\n', "\\\n");
        // TODO: Replace tokens inside this define with other defines.
        header.push(Token::new(format!("#define {} {}\n", name, escaped).into()));
    }
}

/// Compresses the given body by introducing define macros into the
/// header.
/// This MUST be run before merging any tokens.
pub fn compress_with_defines<'a>(
    info: &impl TokenInfo,
    header: &mut Tokens<'a>,
    body: &mut Tokens<'a>,
) {
    if body.len() <= 1 {
        return;
    }

    // Record used names before merging destroys individual tokens.
    // This lets us avoid conflicts when assigning names to defines.
    let used: HashSet<Cow<'a, str>> = iter::chain(header.iter(), body.iter())
        .flat_map(|token| token.text.clone())
        .collect();

    // Merge adjacent tokens if doing so allows saving more space
    // overall.
    // For example, if a is always followed by b, then we'd want to merge
    // a and b, since it always allows saving more space (1 define instead of 2).
    // However, if a is only followed by c one time, then it doesn't make sense
    // to merge them.
    //
    // The main complexity is with cases in between these, as it may make sense to merge
    // both a+b and a+d.
    merge_pairs(info, body);

    // Now that tokens are maximized, compress the ones with the biggest savings by
    // turning them into defines.
    assign_defines(info, header, body, &used);
}
