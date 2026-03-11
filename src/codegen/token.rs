use std::borrow::Cow;

pub type Tokens<'a> = smallvec::SmallVec<Token<'a>, 8>;

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
///
/// A valid program is produced if all tokens are separated by a space,
/// excluding edge cases like preprocessor macros (which require newlines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    /// The raw text of the token.
    ///
    /// Some types of tokens, such as those replacing
    /// a quine array, aren't defined until further codegen
    /// passes.
    pub text: Option<Cow<'a, str>>,

    /// Gets the token's style (what purpose it serves in the output).
    pub style: TokenStyle,
}

impl<'a> Token<'a> {
    pub const fn new(text: Cow<'a, str>) -> Self {
        Self {
            text: Some(text),
            style: TokenStyle::Required,
        }
    }

    pub const fn new_fancy(text: Cow<'a, str>) -> Self {
        Self {
            text: Some(text),
            style: TokenStyle::Fancy,
        }
    }
}

/// The style represents why the token exists, and is used to strip out
/// unneeded tokens when different compiler flags are passed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TokenStyle {
    /// A standard part of the output.
    Required,

    /// A non-essential token used for fancy output, likely incompatible with quines.
    Fancy,

    /// A marker that designates all tokens until the next marker
    /// as within it.
    /// The text part of the marker shouldn't be output.
    Marker,
}

/// Contains target-specific rules for how tokens need to be handled.
pub trait TokenInfo {
    /// Determines if a space is required between this token and the next
    /// to prevent accidental merging.
    #[must_use]
    fn needs_space_between<'a>(&self, left: &Token<'a>, right: &Token<'a>) -> bool;

    /// Tries to merge next to the right of this token.
    /// Returns whether merging succeeded.
    #[must_use]
    fn try_merge<'a>(&self, left: &mut Token<'a>, right: &Token<'a>) -> bool {
        if left.style == TokenStyle::Marker {
            return false;
        }

        // If the write is a marker, we need space between the two
        // to safely merge it during runtime, even though we shouldn't
        // merge it here.
        if right.style == TokenStyle::Marker {
            // Optimization: if it's safe to put the left side directly
            // against an identifier without any space, then it's probably
            // safe to do so with whatever is placed within the marker.
            // This is fragile, but I can't think of any good cases against it.
            if self.needs_space_between(left, right) {
                *left.text.as_mut().expect("Token text is required") += " ";
            }
            return false;
        }

        let needs_space = self.needs_space_between(left, right);

        // This is required because otherwise, markers don't have a consistent
        // index.
        let left = left.text.as_mut().expect("Token text is required");
        let right = right.text.as_ref().expect("Token text is required");

        // Spaces need to be added to disambiguate.
        if needs_space {
            *left.to_mut() += " ";
        }

        *left.to_mut() += right;
        true
    }

    /// Combines multiple tokens into a single token while preserving their rules correctly.
    /// Returns the original token array.
    ///
    /// If `should_merge` is provided, two adjacent tokens will only be merged if
    /// it returns true.
    fn merge_tokens(
        &self,
        tokens: &mut Tokens<'_>,
        should_merge: Option<&dyn Fn(&Token<'_>, &Token<'_>) -> bool>,
    ) {
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

            let this_should_merge = should_merge.map(|f| f(write, read)).unwrap_or(true);

            // Merge if possible.
            // On failure, this creates a split / new token.
            if this_should_merge && self.try_merge(write, read) {
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
}

/// Removes all fancy tokens from the given list of tokens.
pub fn strip_fancy_tokens(tokens: &mut Tokens<'_>) {
    tokens.retain(|token| token.style != TokenStyle::Fancy);
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
