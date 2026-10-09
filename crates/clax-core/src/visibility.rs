//! Whose words may reach an agent (spec §8). This module holds, for now,
//! only the reader: the person whose agent reads.

/// The person an agent channel reads for (spec §6.1): the owner, for the
/// owner's agents and for agents of a grant of the owner's row; else the
/// connected person, by public ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Reader {
    Owner,
    Person(String),
}
