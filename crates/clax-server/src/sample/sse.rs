//! An incremental `text/event-stream` parser: feed it bytes in any chunking,
//! get whole events back. `event:` names the event (default `message`),
//! `data:` lines are joined by `\n`, comments and other fields are ignored.

#[derive(Clone, Debug, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

/// The end of the first event in `b` and the length of its blank-line separator.
fn blank_line(b: &[u8]) -> Option<(usize, usize)> {
    (0..b.len()).find_map(|i| {
        if b[i..].starts_with(b"\r\n\r\n") {
            Some((i, 4))
        } else if b[i..].starts_with(b"\n\n") {
            Some((i, 2))
        } else {
            None
        }
    })
}

impl SseParser {
    /// Adds `chunk` and returns every event it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((end, sep)) = blank_line(&self.buf) {
            let block: Vec<u8> = self.buf.drain(..end + sep).collect();
            let text = String::from_utf8_lossy(&block[..end]);
            let mut event = String::from("message");
            let mut data: Vec<&str> = Vec::new();
            for line in text.split('\n') {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim_start().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push(v.strip_prefix(' ').unwrap_or(v));
                }
            }
            if !data.is_empty() {
                out.push(SseEvent {
                    event,
                    data: data.join("\n"),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_complete_at_a_blank_line_across_any_chunking() {
        let body = b"event: a\ndata: {\"x\":1}\n\n: comment\n\nevent: b\r\ndata: one\r\ndata: two\r\n\r\ndata: plain\n\n";
        for size in [1, 2, 7, body.len()] {
            let mut p = SseParser::default();
            let mut got = Vec::new();
            for chunk in body.chunks(size) {
                got.extend(p.push(chunk));
            }
            assert_eq!(
                got,
                vec![
                    SseEvent {
                        event: "a".into(),
                        data: "{\"x\":1}".into()
                    },
                    SseEvent {
                        event: "b".into(),
                        data: "one\ntwo".into()
                    },
                    SseEvent {
                        event: "message".into(),
                        data: "plain".into()
                    },
                ],
                "chunk size {size}"
            );
        }
    }

    #[test]
    fn multibyte_characters_split_across_chunks_survive() {
        let body = "data: café ☕\n\n".as_bytes();
        let mut p = SseParser::default();
        let mut got = p.push(&body[..9]);
        got.extend(p.push(&body[9..]));
        assert_eq!(
            got,
            vec![SseEvent {
                event: "message".into(),
                data: "café ☕".into()
            }]
        );
    }
}
