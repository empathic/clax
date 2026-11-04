//! A refused request's body, read on to its end before the refusal is
//! answered.
//!
//! An answer sent while the client is still sending its body can reach it
//! as a reset rather than as the answer, and under `wrangler dev` its proxy
//! loses the connection to the Worker (a 500, "Network connection lost",
//! for that request or the next one on it). So a refusal of a body it has
//! not read whole (413 past a limit, 429 past the instance's budget) reads
//! the rest first, keeping none of it, while the body ends within
//! [`REFUSED_READ_BYTES`]: the git routes (`repos::js::read_body_capped`),
//! and the instance's, in the Worker and in its object
//! (`instance::refuse_declared`, `instance::http_request`).

use futures_util::{Stream, StreamExt};

/// How far into a refused body it is still read, to its end, before the
/// refusal is answered. A body that ends within this many bytes is read to
/// its end and dropped as it streams in; a longer one is cancelled once
/// past it, and one that declares a longer length is refused unread.
pub const REFUSED_READ_BYTES: usize = 32 << 20;

/// Whether a refused body that declares `length` bytes is read to its end
/// first: when it ends within [`REFUSED_READ_BYTES`].
pub fn drains(length: u64) -> bool {
    length <= REFUSED_READ_BYTES as u64
}

/// Reads a refused body, `seen` bytes of it already read, on to its end
/// while that comes within [`REFUSED_READ_BYTES`], keeping nothing:
/// `chunks` yields each chunk's length as it is read. `true` once the body
/// ended; `false` once it passed the bound or a read failed, when the
/// caller cancels the rest.
pub async fn drain<E>(chunks: impl Stream<Item = Result<usize, E>>, mut seen: usize) -> bool {
    let mut chunks = std::pin::pin!(chunks);
    while seen <= REFUSED_READ_BYTES {
        match chunks.next().await {
            None => return true,
            Some(Ok(n)) => seen += n,
            Some(Err(_)) => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::cell::Cell;

    const MIB: usize = 1 << 20;

    /// Drains `chunks` of `seen` bytes in, counting the chunks it pulled.
    fn drained(chunks: Vec<Result<usize, ()>>, seen: usize) -> (bool, usize) {
        let pulled = Cell::new(0);
        let stream = futures_util::stream::iter(chunks).inspect(|_| pulled.set(pulled.get() + 1));
        let ended = drain(stream, seen)
            .now_or_never()
            .expect("a ready stream drains without waiting");
        (ended, pulled.get())
    }

    #[test]
    fn a_refused_body_within_the_bound_is_read_to_its_end() {
        // 17 MiB refused at 16: the last MiB is read, then the end.
        assert_eq!(drained(vec![Ok(MIB)], 16 * MIB), (true, 1));
        // Ending exactly at the bound is within it.
        let rest = vec![Ok(MIB); 16];
        assert_eq!(drained(rest, 16 * MIB), (true, 16));
        // A body refused before any of it was read (a declared length, the
        // instance's budget) is read from its start.
        assert_eq!(drained(vec![Ok(MIB); 3], 0), (true, 3));
    }

    #[test]
    fn a_refused_body_past_the_bound_is_left_once_past_it() {
        // 40 MiB, refused 16 MiB in: read until past 32 MiB, no further.
        let (ended, pulled) = drained(vec![Ok(MIB); 24], 16 * MIB);
        assert!(!ended);
        // 16 MiB more reach the bound, and one more passes it.
        assert_eq!(pulled, 17);
        // Already past it: nothing more is read.
        assert_eq!(drained(vec![Ok(MIB)], REFUSED_READ_BYTES + 1), (false, 0));
    }

    #[test]
    fn a_failed_read_ends_the_drain() {
        let (ended, pulled) = drained(vec![Ok(MIB), Err(()), Ok(MIB)], 0);
        assert!(!ended);
        assert_eq!(pulled, 2);
    }

    #[test]
    fn a_declared_length_past_the_bound_is_not_read() {
        assert!(drains(17 << 20));
        assert!(drains(REFUSED_READ_BYTES as u64));
        assert!(!drains(REFUSED_READ_BYTES as u64 + 1));
    }
}
