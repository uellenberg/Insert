use crate::codegen::{Token, TokenStyle, Tokens};
use std::borrow::Cow;

pub const LEFT_PAREN: CToken<'static> = CToken::new(Cow::Borrowed("("));
pub const RIGHT_PAREN: CToken<'static> = CToken::new(Cow::Borrowed(")"));
pub const LEFT_SQUIGGLE: CToken<'static> = CToken::new(Cow::Borrowed("{"));
pub const RIGHT_SQUIGGLE: CToken<'static> = CToken::new(Cow::Borrowed("}"));
pub const SEMI: CToken<'static> = CToken::new(Cow::Borrowed(";"));

pub const INDENT: CToken<'static> = CToken::new_fancy(Cow::Borrowed("    "));
pub const NEWLINE: CToken<'static> = CToken::new_fancy(Cow::Borrowed("\n"));

pub type CTokens<'a> = Tokens<CToken<'a>>;

/// A valid program is produced if all tokens are separated by a space,
/// excluding edge cases like preprocessor macros (which require newlines).
#[derive(Debug, Clone)]
pub struct CToken<'a> {
    text: Option<Cow<'a, str>>,
    style: TokenStyle,
}

impl<'a> CToken<'a> {
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

impl<'a> Token<'a> for CToken<'a> {
    fn text(&self) -> &Option<Cow<'a, str>> {
        &self.text
    }

    fn style(&self) -> TokenStyle {
        self.style
    }

    fn needs_space_between(&self, next: &Self) -> bool {
        let Some(left) = &self.text else {
            return false;
        };
        let Some(right) = &next.text else {
            return false;
        };

        if left.is_empty() || right.is_empty() {
            return false;
        }

        let last_char = left.chars().last().unwrap();
        let first_char = right.chars().next().unwrap();

        // Words must be separated.
        // For example, int main, return 0, variable1 variable2, 123 456
        if is_ident_char(last_char) && is_ident_char(first_char) {
            return true;
        }

        // Spaces are already allowed between operators,
        // and excluding a space here creates a different meaning.
        if is_punct_char(last_char) && is_punct_char(first_char) {
            return true;
        }

        false
    }

    fn try_merge(&mut self, next: &Self) -> bool {
        let needs_space = self.needs_space_between(next);

        let Some(left) = &mut self.text else {
            return false;
        };
        let Some(right) = &next.text else {
            return false;
        };

        // We can't merge if spaces must be inserted, as these
        // are incompatible tokens.
        if needs_space {
            return false;
        }

        *left.to_mut() += right;
        true
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
        let add = match c {
            '\"' => "\\\"",
            '\\' => "\\",
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
                    &c.to_string()
                }
            }
        };

        output.push_str(add);
    }

    output
}
