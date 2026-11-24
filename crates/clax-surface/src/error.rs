//! Why a text is not a surface definition, and where: one error for both
//! notations.

use std::fmt;

/// Where in its text a definition is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// A line of the text, from 1. In a `.d.ts` file (a line ends at a
    /// line feed, a carriage return, both together, U+2028 or U+2029):
    /// the line of the token for a mistake of notation, of the tag for a
    /// mistaken tag, and for a rule a declaration breaks, of the name of
    /// the operation, parameter, interface, field or kind that breaks it
    /// (of the start of the type, for a result). In a manifest: the line
    /// at which the text stops being the JSON the notation reads.
    Line(usize),
    /// A value of a manifest, as a JSON Pointer (RFC 6901) from the
    /// document: `/operations/2/params/0/schema`, and the empty pointer
    /// for the document itself. It is written for a person: a key longer
    /// than 64 characters is cut short, and a character of a key that
    /// would not show is written as its code.
    Pointer(String),
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Place::Line(line) => write!(f, "line {line}"),
            Place::Pointer(p) if p.is_empty() => f.write_str("the manifest"),
            Place::Pointer(p) => f.write_str(p),
        }
    }
}

/// Why a text is not a surface definition. It prints as its place, a
/// colon and its message: `line 12: …` for a `.d.ts` file,
/// `/operations/2/access: …` for a manifest; a message about a manifest
/// as a whole prints alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Where the mistake is.
    pub at: Place,
    /// What is wrong, and what the format allows there.
    pub message: String,
}

impl Error {
    /// The line of the mistake, when its place is a line.
    #[must_use]
    pub fn line(&self) -> Option<usize> {
        match &self.at {
            Place::Line(line) => Some(*line),
            Place::Pointer(_) => None,
        }
    }

    /// The JSON Pointer of the mistake, when its place is a value of a
    /// manifest.
    #[must_use]
    pub fn pointer(&self) -> Option<&str> {
        match &self.at {
            Place::Line(_) => None,
            Place::Pointer(p) => Some(p),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.at {
            Place::Pointer(p) if p.is_empty() => f.write_str(&self.message),
            at => write!(f, "{at}: {}", self.message),
        }
    }
}

impl std::error::Error for Error {}

/// The mistakes found in one text, in the text's order: one at least.
/// A `.d.ts` file's reader stops at its first. A manifest's says the
/// first of each entry it could not read, up to
/// [`crate::Limits::manifest_errors`] of them, and then ends with one
/// more that says how many entries it did not look at
/// ([`Errors::unread`]). It prints as its mistakes, each on a line of
/// its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Errors {
    first: Error,
    rest: Vec<Error>,
    unread: usize,
}

impl Errors {
    pub(crate) fn new(first: Error, rest: Vec<Error>, unread: usize) -> Errors {
        Errors {
            first,
            rest,
            unread,
        }
    }

    /// The first mistake.
    #[must_use]
    pub fn first(&self) -> &Error {
        &self.first
    }

    /// Every mistake, the first first. When [`Errors::unread`] is not
    /// 0, the last is no mistake of the text's but the reader's word
    /// that it stopped there.
    pub fn iter(&self) -> impl Iterator<Item = &Error> {
        std::iter::once(&self.first).chain(&self.rest)
    }

    /// How many there are, [`Errors::iter`]'s last among them: one at
    /// least.
    #[must_use]
    pub fn count(&self) -> usize {
        1 + self.rest.len()
    }

    /// How many entries of a manifest were not looked at, because as
    /// many mistakes as are said had been found before them: 0 when
    /// all were.
    #[must_use]
    pub fn unread(&self) -> usize {
        self.unread
    }

    /// The mistakes themselves, the first first.
    #[must_use]
    pub fn into_vec(self) -> Vec<Error> {
        let mut all = vec![self.first];
        all.extend(self.rest);
        all
    }
}

impl From<Error> for Errors {
    fn from(first: Error) -> Errors {
        Errors {
            first,
            rest: Vec::new(),
            unread: 0,
        }
    }
}

impl fmt::Display for Errors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.first)?;
        for e in &self.rest {
            write!(f, "\n{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Errors {}
