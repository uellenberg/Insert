use crate::mir::MIRContext;
use ariadne::{Label, Report, ReportKind};
use std::fmt::Display;
use std::ops::Range;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Span<'a>(&'a Path, Range<usize>, &'a str);

impl<'a> Span<'a> {
    pub fn empty() -> Self {
        Span(&Path::new(""), 0..0, "")
    }
}

/// Converts a pest Span to an ariadne Span.
pub fn to_span<'a>(file: &'a Path, span: pest::Span<'a>) -> Span<'a> {
    Span(file, span.start()..span.end(), span.as_str())
}

impl<'a> ariadne::Span for Span<'a> {
    type SourceId = Path;

    fn source(&self) -> &Self::SourceId {
        self.0
    }

    fn start(&self) -> usize {
        self.1.start
    }

    fn end(&self) -> usize {
        self.1.end
    }
}

impl<'a> Display for Span<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.2)
    }
}

pub fn print_with_span<'a>(file_cache: &FileCache, span: Option<Span<'a>>, msg: &str) {
    let Some(span) = span else {
        eprintln!("{msg}");
        return;
    };

    Report::build(ReportKind::Error, span.clone())
        .with_message(msg)
        .with_label(Label::new(span).with_message("The error occurred here"))
        .finish()
        .eprint(file_cache.clone())
        .unwrap();
}

/// Prints a message alongside the given span.
/// Pass the span as an option.
macro_rules! eprintln_span {
    ($ctx:expr, $span:expr, $($arg:tt)*) => {
        $crate::parser::span::print_with_span(&$ctx.file_cache, $span, &format!($($arg)*));
    };
}

use crate::parser::file_cache::FileCache;
pub(crate) use eprintln_span;
