//! A pull request's preview: where it will be, and whether it serves the
//! commit yet. A preview runs the proposed code, so what it answers is
//! treated as hostile: every look at it is bounded in time and in size, and
//! a redirect is not followed.

use crate::runtime::bounded;
use std::time::{Duration, Instant};

/// The longest one look at a preview may take, and the shortest it is
/// given however little of the wait is left.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SHORTEST_LOOK: Duration = Duration::from_secs(1);
/// The most of a preview's answer that is read for the commit.
const MAX_BODY: usize = 64 * 1024;

/// How a target's previews work.
#[derive(Clone, Debug)]
pub struct Preview {
    /// The preview's URL, with `{pr}` for the pull request's number.
    pub url_template: String,
    /// Fetched to tell whether the preview is up, such as `/healthz`.
    pub ready_path: String,
    /// Whether that answer must name the commit. A preview still serving
    /// its last deployment answers 200 too.
    pub ready_names_commit: bool,
    pub timeout: Duration,
    pub poll: Duration,
}

impl Preview {
    pub fn url(&self, pr: u64) -> String {
        self.url_template.replace("{pr}", &pr.to_string())
    }

    /// Waits until the preview of pull request `pr` serves `commit`;
    /// answers whether it did in time. It looks at least once and ends a
    /// look past the timeout at the latest.
    pub async fn wait(&self, http: &reqwest::Client, pr: u64, commit: &str) -> bool {
        let url = format!("{}{}", self.url(pr).trim_end_matches('/'), self.ready_path);
        let deadline = Instant::now() + self.timeout;
        loop {
            if self.ready(http, &url, commit).await {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    async fn ready(&self, http: &reqwest::Client, url: &str, commit: &str) -> bool {
        let limit = self.timeout.clamp(SHORTEST_LOOK, REQUEST_TIMEOUT);
        let look = http.get(url).timeout(limit).send();
        let Ok(r) = look.await else {
            return false;
        };
        if !r.status().is_success() {
            return false;
        }
        if !self.ready_names_commit {
            return true;
        }
        match bounded(r, MAX_BODY).await {
            Ok(Some(body)) => String::from_utf8_lossy(&body).contains(commit),
            _ => false,
        }
    }
}
