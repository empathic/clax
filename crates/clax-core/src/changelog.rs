//! The version changelog (spec §10 "Version changelog"): bounds and link sources.

/// Longest version note, in characters, after [`crate::working::clean_line`].
pub const MAX_NOTE_CHARS: usize = 280;
/// Most threads one publish may name in `addresses`.
pub const MAX_ADDRESSES: usize = 50;
/// Most artifacts a viewer's seen marks are kept for.
pub const MAX_SEEN_PER_VIEWER: usize = 200;

/// Why a thread is linked to a version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkSource {
    /// The publishing session was marked working on it.
    Working,
    /// Named in `addresses`.
    Explicit,
    /// Resolved by an agent with no earlier link.
    Resolve,
}

impl LinkSource {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkSource::Working => "working",
            LinkSource::Explicit => "explicit",
            LinkSource::Resolve => "resolve",
        }
    }
}
