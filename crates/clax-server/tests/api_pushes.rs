//! A commit pushed to an artifact's repository becomes a version the API
//! lists and the streams hear of as a publish, and a publish that did not
//! see it is a conflict.

use crate::common;
use common::TestServer;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;

use clax_content_git::MemGitContent;
use clax_core::working::SystemClock;
use clax_git::tree::{Changes, build_tree};
use clax_git::versions::repo_name;
use clax_git::{Commit, MAIN, Person, RefUpdate, Repo, Signature};

const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Pushes one commit on `main` of `aid`'s repository putting `index.html`.
async fn push(content: &MemGitContent, aid: &str, html: &str, message: &str) {
    let repo = content.host().repo(&repo_name(aid)).unwrap();
    let head = repo.refs().await.unwrap()[MAIN];
    let base = repo.read_commit_meta(&head).await.unwrap().unwrap().tree;
    let mut changes = Changes::new();
    changes.insert("index.html".into(), Some(html.as_bytes().to_vec()));
    let (tree, mut objects) = build_tree(&repo, Some(base), &changes).await.unwrap();
    let sig = Signature::utc(
        &Person {
            name: "Rae".into(),
            email: "rae@example.com".into(),
        },
        1_800_000_000,
    );
    let commit = Commit {
        tree,
        parents: vec![head],
        author: sig.clone(),
        committer: sig,
        message: format!("{message}\n"),
    }
    .to_object()
    .unwrap();
    let new = commit.id();
    objects.push(commit);
    let st = repo
        .apply(
            objects,
            &[RefUpdate::new(MAIN, Some(head), Some(new))],
            true,
        )
        .await
        .unwrap();
    assert!(st.iter().all(|s| s.is_ok()), "{st:?}");
}

/// The next `version` event's data on an SSE stream.
async fn next_version(
    stream: &mut (impl StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin),
    buf: &mut String,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + READ_TIMEOUT;
    loop {
        if let Some(end) = buf.find("\n\n") {
            let block = buf[..end].to_string();
            buf.drain(..end + 2);
            if block.lines().any(|l| l == "event: version") {
                let data = block
                    .lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .unwrap();
                return serde_json::from_str(data).unwrap();
            }
            continue;
        }
        // Keep-alive comments arrive meanwhile: the bound is on the event.
        let chunk = tokio::time::timeout_at(deadline, stream.next())
            .await
            .expect("a version event within 20 s")
            .expect("the stream still open")
            .unwrap();
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
    }
}

#[tokio::test]
async fn a_pushed_commit_is_a_version_the_stream_announces_and_a_stale_publish_conflicts() {
    let content = Arc::new(MemGitContent::in_memory(
        Person {
            name: "Clax".into(),
            email: "clax@users.clax.dev".into(),
        },
        Arc::new(SystemClock),
    ));
    let ts = TestServer::spawn_on_one_connection(content.clone()).await;
    let a = ts.publish("Pushed", &[("index.html", "<p>one</p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let res = ts.get(&format!("/api/events?artifact={aid}")).await;
    assert_eq!(res.status(), 200);
    let mut stream = res.bytes_stream();
    let mut buf = String::new();

    push(&content, &aid, "<p>pushed</p>", "Pushed with git").await;
    let stale = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            serde_json::json!({"if_version": 1, "files": {"index.html": {"content": "<p>mine</p>"}}}),
        )
        .await;
    assert_eq!(stale.status(), 409);
    let v = next_version(&mut stream, &mut buf).await;
    assert_eq!(
        (v["artifact_id"].as_str(), v["n"].as_u64()),
        (Some(aid.as_str()), Some(2))
    );

    let versions: serde_json::Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/versions"))
        .await
        .json()
        .await
        .unwrap();
    let versions = versions["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[1]["note"], "Pushed with git");
    let page = ts
        .get_authed(&format!("/api/artifacts/{aid}/versions/2"))
        .await;
    assert_eq!(page.status(), 200);

    let next = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            serde_json::json!({"if_version": 2, "files": {"index.html": {"content": "<p>three</p>"}}}),
        )
        .await;
    assert_eq!(next.status(), 201);
    let v = next_version(&mut stream, &mut buf).await;
    assert_eq!(v["n"], 3);
}

/// A push ledger naming one person, unknown here, as every commit's pusher.
struct Stranger;

#[async_trait::async_trait]
impl clax_core::content::PushLedger for Stranger {
    async fn pushers(
        &self,
        _id: &clax_core::ArtifactId,
        commits: &[String],
    ) -> clax_core::Result<Vec<Option<clax_core::content::Pusher>>> {
        Ok(commits
            .iter()
            .map(|_| {
                Some(clax_core::content::Pusher {
                    person: Some("u_str4ng".into()),
                    session_id: None,
                })
            })
            .collect())
    }
    async fn tokens(
        &self,
        _id: &clax_core::ArtifactId,
    ) -> clax_core::Result<Vec<clax_core::content::RepoToken>> {
        Ok(Vec::new())
    }
    async fn revoke(&self, _id: &clax_core::ArtifactId, _token: &str) -> clax_core::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_publish_over_a_push_whose_pusher_may_not_publish_is_refused_409() {
    let content = Arc::new(
        MemGitContent::in_memory(
            Person {
                name: "Clax".into(),
                email: "clax@users.clax.dev".into(),
            },
            Arc::new(SystemClock),
        )
        .with_ledger(Arc::new(Stranger)),
    );
    let ts = TestServer::spawn_on_one_connection(content.clone()).await;
    let a = ts.publish("Pushed", &[("index.html", "<p>one</p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    push(&content, &aid, "<p>pushed</p>", "Pushed by a stranger").await;
    let res = ts
        .post_json(
            &format!("/api/artifacts/{aid}/versions"),
            serde_json::json!({"if_version": 1, "files": {"index.html": {"content": "<p>mine</p>"}}}),
        )
        .await;
    assert_eq!(res.status(), 409);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "push_refused", "{body}");
    let versions: serde_json::Value = ts
        .get_authed(&format!("/api/artifacts/{aid}/versions"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(versions["versions"].as_array().unwrap().len(), 1);
}
