use crate::codegen::c::CLowerer;
use crate::codegen::token::{Token, TokenInfo};
use std::borrow::Cow;

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
        let Some(left) = &left.text else {
            return false;
        };
        let Some(right) = &right.text else {
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
