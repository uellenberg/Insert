use std::borrow::Cow;

pub mod c;

type Tokens<T> = smallvec::SmallVec<T, 8>;

/// The style represents why the token exists, and is used to strip out
/// unneeded tokens when different compiler flags are passed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TokenStyle {
    Required,
    Fancy,
}

/// A single token in the source code to be output.
/// The final source will be constructed from concatenated tokens.
///
/// The exact definition for what is and isn't a token is somewhat vague,
/// and very specific to the output language.
/// A token could be anything from a single number in a larger number literal,
/// all the way to the entire text of a multiline comment.
///
/// However, text cannot be inserted within a token, and other rules apply
/// for when and how tokens can be appended depending on the language.
///
/// In general, space and comments can be inserted between tokens (depending on the language),
/// but not within a token.
pub trait Token<'a> {
    /// The raw text of the token.
    ///
    /// Some types of tokens, such as those replacing
    /// a quine array, aren't defined until further codegen
    /// passes.
    fn text(&self) -> &Option<Cow<'a, str>>;

    /// Gets the token's style (what purpose it serves in the output).
    #[must_use]
    fn style(&self) -> TokenStyle;

    /// Determines if a space is required between this token and the next
    /// to prevent accidental merging.
    #[must_use]
    fn needs_space_between(&self, next: &Self) -> bool;

    /// Tries to merge next to the right of this token.
    /// Returns whether merging succeeded.
    #[must_use]
    fn try_merge(&mut self, next: &Self) -> bool;
}

/// Options passed to the lowering process, controlling
/// how the output should be formatted.
#[derive(Clone, Debug, Default)]
pub struct LowerOptions {
    /// Should fancy tokens be included in the output?
    pub fancy: bool,
}

/// Combines multiple tokens into a single token while preserving their rules correctly.
/// Returns the original token array.
pub fn merge_tokens<'a, T: Token<'a>>(tokens: &mut Tokens<T>) {
    if tokens.is_empty() {
        return;
    }

    // Skip the first token so we can merge with it.
    let mut read_index = 1;
    // This refers to the index we've previously written to.
    // Initially, we act like we've already written to index 0.
    let mut write_index = 0;

    while read_index < tokens.len() {
        debug_assert!(read_index > write_index);

        let [read, write] = tokens.get_disjoint_mut([read_index, write_index]).unwrap();

        // Merge if possible.
        // On failure, this creates a split / new token.
        if write.try_merge(read) {
            read_index += 1;
            continue;
        }

        // Merging failed, so write this token out and continue.
        // write_index points to the previous token's spot, so we need to increment it.
        write_index += 1;
        // It's okay to swap a token with itself is less efficient.
        if write_index != read_index {
            tokens.swap(write_index, read_index);
        }

        read_index += 1;
    }

    // write_index points to the last token's index, so decrease the total length to match.
    tokens.truncate(write_index + 1);
}

/// Removes all fancy tokens from the given list of tokens.
pub fn strip_fancy_tokens<'a, T: Token<'a>>(tokens: &mut Tokens<T>) {
    tokens.retain(|token| token.style() != TokenStyle::Fancy);
}

/// Creates a new instance of `Tokens` filled with the given elements.
/// Other `Tokens` can be spread into this new instance by writing `...tokens`.
/// Elements may be evaluated multiple times, and therefore MUST not have side effects.
macro_rules! spread {
    // -- Count --

    (@count ... $collection:expr) => {
        $collection.len()
    };
    (@count ... $collection:expr, $( $rest:tt )* ) => {
        $collection.len() + spread!(@count $( $rest )* )
    };

    (@count) => { 0 };
    (@count $e:expr) => { 1 };
    (@count $e:expr, $( $rest:tt )* ) => {
        1 + spread!(@count $( $rest )* )
    };

    // -- Fill --

    (@fill $v:ident, ... $collection:expr) => {
        $v.extend($collection);
    };
    (@fill $v:ident, ... $collection:expr, $( $rest:tt )* ) => {
        $v.extend($collection);
        spread!(@fill $v, $( $rest )*);
    };

    (@fill $v:ident, $e:expr) => {
        $v.push($e);
    };
    (@fill $v:ident, $e:expr, $( $rest:tt )* ) => {
        $v.push($e);
        spread!(@fill $v, $( $rest )*);
    };
    (@fill $v:ident, ) => {};

    // -- Entry --

    [ $( $tt:tt )* ] => {
        {
            let cap = spread!(@count $( $tt )*);
            #[allow(unused_mut)]
            let mut v = Tokens::with_capacity(cap);

            spread!(@fill v, $( $tt )*);

            v
        }
    };
}

pub(crate) use spread;
