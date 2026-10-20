//! Reading Server-Sent Events from the daemon's event and stream routes.

/// Reads Server-Sent Events from one `/api/events` or `/api/stream` response.
pub struct EventReader {
    pub(crate) stream:
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<Vec<u8>, String>> + Send>>,
    pub(crate) buf: String,
    /// Whether `ready` events are returned rather than skipped.
    pub(crate) keep_ready: bool,
}

impl EventReader {
    /// A reader of `res`'s body that returns every event, `ready` included
    /// (an `/api/stream` response opens with `ready`, naming the stream).
    pub fn from_response(res: reqwest::Response) -> EventReader {
        use futures::StreamExt;
        let stream = res
            .bytes_stream()
            .map(|r| r.map(|b| b.to_vec()).map_err(|e| e.to_string()));
        EventReader {
            stream: Box::pin(stream),
            buf: String::new(),
            keep_ready: true,
        }
    }

    /// The next event other than keep-alive comments (and `ready`, unless
    /// made by [`EventReader::from_response`]), as (name, data); panics
    /// when none comes within the [`HANG`](crate::HANG) guard.
    pub async fn next(&mut self) -> (String, serde_json::Value) {
        use futures::StreamExt;
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block = self.buf[..end].to_string();
                self.buf.drain(..end + 2);
                if block.starts_with(':') {
                    continue;
                }
                let name = block
                    .lines()
                    .find_map(|l| l.strip_prefix("event: "))
                    .unwrap_or("message")
                    .to_string();
                let data = block
                    .lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .unwrap_or("null");
                if name == "ready" && !self.keep_ready {
                    continue;
                }
                return (
                    name,
                    serde_json::from_str(data).expect("event data is JSON"),
                );
            }
            let chunk = tokio::time::timeout(crate::HANG, self.stream.next())
                .await
                .unwrap_or_else(|_| panic!("no SSE chunk within {:?} (hang guard)", crate::HANG))
                .expect("stream still open")
                .expect("chunk readable");
            self.buf
                .push_str(std::str::from_utf8(&chunk).expect("UTF-8 events"));
        }
    }

    /// The names of the events (keep-alive comments aside) until the body
    /// ends; panics when it has not ended within the [`HANG`](crate::HANG)
    /// guard.
    pub async fn rest(&mut self) -> Vec<String> {
        use futures::StreamExt;
        let mut names = vec![];
        let deadline = tokio::time::Instant::now() + crate::HANG;
        loop {
            while let Some(end) = self.buf.find("\n\n") {
                let block = self.buf[..end].to_string();
                self.buf.drain(..end + 2);
                if let Some(n) = block.lines().find_map(|l| l.strip_prefix("event: ")) {
                    names.push(n.to_string());
                }
            }
            let chunk = tokio::time::timeout_at(deadline, self.stream.next())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "the stream has not ended within {:?} (hang guard)",
                        crate::HANG
                    )
                });
            match chunk {
                Some(Ok(c)) => self
                    .buf
                    .push_str(std::str::from_utf8(&c).expect("UTF-8 events")),
                _ => return names,
            }
        }
    }

    /// Skips events until one named `name`; returns its data.
    pub async fn next_named(&mut self, name: &str) -> serde_json::Value {
        loop {
            let (n, d) = self.next().await;
            if n == name {
                return d;
            }
        }
    }
}
