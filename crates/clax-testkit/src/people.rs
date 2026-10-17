//! Two people beside the owner, and Mia's words in every author field
//! (spec §8.2, §8.6): the world the canary sweeps run in.
//!
//! [`World::two_people`] starts a daemon reachable from the LAN, connects
//! Sam and Mia by invite, and seeds, as Mia, a distinct canary string in
//! every author field of spec §8.2: an artifact's title, description, icon
//! and `capabilities`; a version's label and note; file names and bytes,
//! and a file the owner's agent carried forward from Mia's version into its
//! own; an asset's bytes; a document's path and fields; a comment body; a
//! thread's anchor quote, prefix, suffix, selector and `html_hash`, and a
//! drawn area's fingerprint; a clip; a batch note;
//! a working message; a question Mia's agent asked. It also seeds a thread
//! on a live page. Mia is connected and approved by no one.
//!
//! The person columns are written as the daemon will write them when Mia
//! publishes herself (`artifacts.person` and `owner_session_id`, Mia's
//! agent's session, `versions.person`,
//! `versions.file_authors_json`, `assets.person`) and writes her document
//! (`doc_authors`); her browser writes the comments, the anchor, the clip,
//! the batch note and the resolution, and her agent the working message
//! and the question.

use crate::TestServer;
use crate::{FAKE_PNG, TestClient};
use serde_json::{Value, json};

/// Every canary string the world seeds, each in one field.
pub const CANARIES: &[&str] = &[
    TITLE,
    DESCRIPTION,
    ICON,
    CAPABILITY,
    LABEL,
    NOTE,
    FILE_NAME,
    FILE_BYTES,
    CARRIED_NAME,
    CARRIED_BYTES,
    ASSET_BYTES,
    DOC_PATH,
    DOC_UNNAMED,
    DOC_FIELD,
    COMMENT,
    QUOTE,
    PREFIX,
    SUFFIX,
    SELECTOR,
    CLIP,
    BATCH_NOTE,
    WORKING,
    QUESTION,
    REPLY,
    HTML_HASH,
    AREA_TAG,
    AREA_TEXT,
    AREA_CHILDREN,
];
pub const TITLE: &str = "MIACANARYtitle";
pub const DESCRIPTION: &str = "MIACANARYdescription";
pub const ICON: &str = "MIACANARYicon";
pub const CAPABILITY: &str = "MIACANARYcapability";
pub const LABEL: &str = "MIACANARYlabel";
pub const NOTE: &str = "MIACANARYnote";
pub const FILE_NAME: &str = "MIACANARYfilename.html";
pub const FILE_BYTES: &str = "MIACANARYfilebytes";
pub const CARRIED_NAME: &str = "MIACANARYcarried.css";
pub const CARRIED_BYTES: &str = "MIACANARYcarriedbytes";
pub const ASSET_BYTES: &str = "MIACANARYassetbytes";
pub const DOC_PATH: &str = "MIACANARYdocpath";
/// A second document of Mia's, at a path no call or command names: its
/// path is never exempt from the sweeps.
pub const DOC_UNNAMED: &str = "MIACANARYunnamed";
pub const DOC_FIELD: &str = "MIACANARYdocfield";
pub const COMMENT: &str = "MIACANARYcomment";
pub const QUOTE: &str = "MIACANARYquote";
pub const PREFIX: &str = "MIACANARYprefix";
pub const SUFFIX: &str = "MIACANARYsuffix";
pub const SELECTOR: &str = "MIACANARYselector";
pub const CLIP: &str = "MIACANARYclip";
pub const BATCH_NOTE: &str = "MIACANARYbatchnote";
pub const WORKING: &str = "MIACANARYworking";
pub const QUESTION: &str = "MIACANARYquestion";
pub const REPLY: &str = "MIACANARYreply";
/// The body of the owner's comment on Mia's page.
pub const OWNER_ASKS: &str = "The owner asks for a bigger chart";
/// An anchor's hash of the anchored element's markup.
pub const HTML_HASH: &str = "sha256:MIACANARYhtmlhash";
/// A drawn area's fingerprint: its element's tag, text and child count
/// (a number, so a value no other field holds).
pub const AREA_TAG: &str = "MIACANARYareatag";
pub const AREA_TEXT: &str = "MIACANARYareatext";
pub const AREA_CHILDREN: &str = "4294967291";

/// One of Mia's artifacts, seeded with her canaries.
#[derive(Clone, Debug)]
pub struct Seeded {
    pub artifact: String,
    /// A thread Mia started (anchor, clip, comment) and sent in a batch.
    pub thread: String,
    /// A thread Mia started and resolved.
    pub resolved: String,
    pub asset: String,
}

/// Who an agent request speaks as.
pub enum As<'a> {
    /// A person's agent: a grant's signed request naming its session.
    Grant(&'a TestClient),
    /// The owner's agent: the token naming a session (and the marker).
    Token,
}

/// An agent the sweeps call routes as.
pub struct AgentOf<'a> {
    pub label: &'static str,
    pub who: As<'a>,
    /// Its session.
    pub session: String,
    /// A question it asked.
    pub question: String,
}

/// The world of the canary sweeps.
pub struct World {
    pub ts: TestServer,
    pub sam: TestClient,
    pub mia: TestClient,
    /// Mia's browser: her viewer cookie and public ID.
    pub mia_cookie: String,
    pub mia_public: String,
    /// What the reads read.
    pub read: Seeded,
    /// What the writes change, a copy seeded the same way.
    pub scratch: Seeded,
    pub sam_session: String,
    pub sam_question: String,
    pub owner_session: String,
    pub owner_question: String,
    pub mia_session: String,
    /// A live page with a thread.
    pub live: String,
}

impl World {
    /// The owner; Sam and Mia connected (`clax:collaborator`); Mia's
    /// canaries in every author field; Sam's agent and an owner's agent,
    /// each with a session and a question.
    pub async fn two_people() -> World {
        World::two_people_with(|_| {}).await
    }

    /// [`World::two_people`] on a daemon whose state `f` configures first.
    pub async fn two_people_with(f: impl FnOnce(&mut clax_http::state::AppState)) -> World {
        let ts = TestServer::spawn_on("0.0.0.0".parse().unwrap(), f).await;
        let sam = ts.connect_person("Sam", "clax:collaborator").await;
        let mia = ts.connect_person("Mia", "clax:collaborator").await;
        let mia_viewer = ts.store.viewer_of_grant(&mia.grant_id()).unwrap();
        let owner_session = ts.register_session("claude", "owner-h1").await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let sam_session = id(&sam.register("claude", "sam-h1").await);
        let mia_session = id(&mia.register("claude", "mia-h1").await);
        let mut w = World {
            read: Seeded {
                artifact: String::new(),
                thread: String::new(),
                resolved: String::new(),
                asset: String::new(),
            },
            scratch: Seeded {
                artifact: String::new(),
                thread: String::new(),
                resolved: String::new(),
                asset: String::new(),
            },
            mia_cookie: ts.browser_cookie(&mia_viewer.id),
            mia_public: mia_viewer.public_id.clone(),
            sam_question: String::new(),
            owner_question: String::new(),
            live: String::new(),
            ts,
            sam,
            mia,
            sam_session,
            owner_session,
            mia_session,
        };
        w.read = w.seed().await;
        w.scratch = w.seed().await;
        w.live = w.live_page().await;
        // Mia's agent works on her artifact and asks about it.
        let r = w
            .mia
            .agent(
                "PUT",
                &format!(
                    "/api/sessions/{}/working/{}",
                    w.mia_session, w.read.artifact
                ),
                Some(&w.mia_session),
                Some(json!({"message": WORKING})),
            )
            .await;
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        let r = w
            .mia
            .agent(
                "POST",
                &format!("/api/sessions/{}/questions", w.mia_session),
                Some(&w.mia_session),
                Some(ask(QUESTION, Some(&w.read.artifact))),
            )
            .await;
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        // Each sweeping agent has a question of its own.
        let r = w
            .sam
            .agent(
                "POST",
                &format!("/api/sessions/{}/questions", w.sam_session),
                Some(&w.sam_session),
                Some(ask("Sam asks", None)),
            )
            .await;
        assert_eq!(r.status(), 201);
        w.sam_question = r.json::<Value>().await.unwrap()["question"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        w.owner_question =
            w.ts.ask(&w.owner_session, ask("The owner's agent asks", None))
                .await["question"]["id"]
                .as_str()
                .unwrap()
                .to_string();
        w
    }

    /// Every canary string seeded.
    pub fn canaries(&self) -> Vec<&'static str> {
        CANARIES.to_vec()
    }

    /// Sam's agent.
    pub fn sam_agent(&self) -> AgentOf<'_> {
        AgentOf {
            label: "Sam's agent",
            who: As::Grant(&self.sam),
            session: self.sam_session.clone(),
            question: self.sam_question.clone(),
        }
    }

    /// The owner's agent.
    pub fn owner_agent(&self) -> AgentOf<'_> {
        AgentOf {
            label: "the owner's agent",
            who: As::Token,
            session: self.owner_session.clone(),
            question: self.owner_question.clone(),
        }
    }

    /// Seeds one artifact of Mia's, as the module comment says.
    async fn seed(&self) -> Seeded {
        let ts = &self.ts;
        let file = |c: &str| json!({"content": c, "encoding": "utf8"});
        let r = ts
            .post_json(
                "/api/artifacts",
                json!({
                    "title": TITLE,
                    "description": DESCRIPTION,
                    "icon": ICON,
                    "label": LABEL,
                    "note": NOTE,
                    "capabilities": {"db": {}, "comments": {}, CAPABILITY: {}},
                    "files": {
                        "index.html": file(&format!("<h2>Quarterly goals</h2><p>{FILE_BYTES}</p>")),
                        FILE_NAME: file("<p>a page</p>"),
                        CARRIED_NAME: file(&format!("/* {CARRIED_BYTES} */")),
                    },
                }),
            )
            .await;
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        let aid = r.json::<Value>().await.unwrap()["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        // The owner's agent publishes a version of its own over Mia's,
        // carrying her files forward unchanged.
        let r = ts
            .authed(
                ts.client
                    .post(format!("{}/api/artifacts/{aid}/versions", ts.base)),
            )
            .header("x-clax-session", &self.owner_session)
            .json(
                &json!({"label": "the owner's agent's", "if_version": 1, "files": {
                    "index.html": file(&format!("<h2>Quarterly goals</h2><p>{FILE_BYTES}</p>")),
                    FILE_NAME: file("<p>a page</p>"),
                    CARRIED_NAME: file(&format!("/* {CARRIED_BYTES} */")),
                    "owner.html": file("<p>mine</p>"),
                }}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        // An asset of Mia's.
        let form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(ASSET_BYTES.as_bytes().to_vec())
                .file_name("notes.txt")
                .mime_str("text/plain")
                .unwrap(),
        );
        let r = ts
            .authed(
                ts.client
                    .post(format!("{}/api/artifacts/{aid}/assets", ts.base)),
            )
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        let asset = r.json::<Value>().await.unwrap()["asset"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        // A document at a path of Mia's naming, with a field of hers.
        let r = ts
            .authed(ts.client.put(format!(
                "{}/api/artifacts/{aid}/docs/notes/{DOC_PATH}",
                ts.base
            )))
            .json(&json!({"data": {"text": DOC_FIELD}}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        let r = ts
            .authed(ts.client.put(format!(
                "{}/api/artifacts/{aid}/docs/notes/{DOC_UNNAMED}",
                ts.base
            )))
            .json(&json!({"data": {"text": DOC_FIELD}}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        // The person columns, as Mia's own publish will write them.
        let mia = self.mia_public.clone();
        let authors = json!({
            "index.html": [mia], FILE_NAME: [mia], CARRIED_NAME: [mia]
        })
        .to_string();
        let authors_v2 = json!({
            "index.html": [mia], FILE_NAME: [mia], CARRIED_NAME: [mia], "owner.html": [null]
        })
        .to_string();
        let db = rusqlite::Connection::open(ts.home.db_path()).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        // Mia's agent made it from its session.
        db.execute(
            "UPDATE artifacts SET person = ?2, owner_session_id = ?3 WHERE id = ?1",
            rusqlite::params![aid, mia, self.mia_session],
        )
        .unwrap();
        db.execute(
            "UPDATE versions SET person = ?2, file_authors_json = ?3 WHERE artifact_id = ?1 AND n = 1",
            rusqlite::params![aid, mia, authors],
        )
        .unwrap();
        db.execute(
            "UPDATE versions SET file_authors_json = ?2 WHERE artifact_id = ?1 AND n = 2",
            rusqlite::params![aid, authors_v2],
        )
        .unwrap();
        db.execute(
            "UPDATE assets SET person = ?2 WHERE id = ?1",
            rusqlite::params![asset, mia],
        )
        .unwrap();
        db.execute(
            "INSERT INTO doc_authors (artifact_id, path, author) VALUES (?1, ?2, ?3)",
            rusqlite::params![aid, format!("notes/{DOC_PATH}"), mia],
        )
        .unwrap();
        db.execute(
            "INSERT INTO doc_authors (artifact_id, path, author) VALUES (?1, ?2, ?3)",
            rusqlite::params![aid, format!("notes/{DOC_UNNAMED}"), mia],
        )
        .unwrap();
        drop(db);
        // Mia's browser: a thread with her anchor, clip and comment, sent
        // in a batch with her note; another she resolves.
        let thread = self.mia_thread(&aid, COMMENT, true).await;
        let resolved = self.mia_thread(&aid, "Mia's other thread", false).await;
        self.mia_area_thread(&aid).await;
        let r = self
            .mia_browser(
                ts.client
                    .post(format!("{}/api/artifacts/{aid}/threads:send", ts.base)),
            )
            .json(&json!({"thread_ids": [thread], "note": BATCH_NOTE}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        let r = self
            .mia_browser(ts.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{resolved}/resolve",
                ts.base
            )))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        // Mia's later reply on her thread.
        let r = self
            .mia_browser(ts.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{thread}/comments",
                ts.base
            )))
            .json(&json!({"body": REPLY}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        Seeded {
            artifact: aid,
            thread,
            resolved,
            asset,
        }
    }

    /// `req` from Mia's browser (her viewer cookie).
    pub fn mia_browser(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("cookie", format!("clax_viewer={}", self.mia_cookie))
    }

    /// A thread Mia's browser starts on version 1 of `aid`, anchored with
    /// her canaries and, when `clip`, with her clip.
    async fn mia_thread(&self, aid: &str, body: &str, clip: bool) -> String {
        let anchor = json!({"kind": "element", "selector": format!("#{SELECTOR}"), "quote": QUOTE,
            "prefix": PREFIX, "suffix": SUFFIX, "html_hash": HTML_HASH, "rect": null,
            "custom_name": null});
        self.mia_thread_at(aid, anchor, body, clip).await
    }

    /// A thread Mia's browser starts on version 1 of `aid` with an area
    /// she drew, fingerprinted with her canaries.
    async fn mia_area_thread(&self, aid: &str) -> String {
        let anchor = json!({"kind": "area", "selector": format!("#{SELECTOR}"),
            "area": {"x": 0.1, "y": 0.1, "w": 0.5, "h": 0.5, "tag": AREA_TAG, "text": AREA_TEXT,
                     "children": AREA_CHILDREN.parse::<u32>().unwrap()}});
        self.mia_thread_at(aid, anchor, "Mia's area", false).await
    }

    /// A thread Mia's browser starts on version 1 of `aid` at `anchor`,
    /// with her clip when `clip`.
    async fn mia_thread_at(&self, aid: &str, anchor: Value, body: &str, clip: bool) -> String {
        let mut form = reqwest::multipart::Form::new()
            .text("anchor", anchor.to_string())
            .text("body", body.to_string())
            .text("version", "1");
        if clip {
            let mut png = FAKE_PNG.to_vec();
            png.extend_from_slice(CLIP.as_bytes());
            form = form.part(
                "clip",
                reqwest::multipart::Part::bytes(png)
                    .file_name("clip.png")
                    .mime_str("image/png")
                    .unwrap(),
            );
        }
        let r = self
            .mia_browser(
                self.ts
                    .client
                    .post(format!("{}/api/artifacts/{aid}/threads", self.ts.base)),
            )
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        let v: Value = r.json().await.unwrap();
        assert!(v.get("clip_error").is_none(), "{v}");
        v["thread"]["id"].as_str().unwrap().to_string()
    }

    /// A comment Mia's browser adds to thread `tid` of `aid` (sent on to
    /// the agents, as the thread was).
    pub async fn mia_comments(&self, aid: &str, tid: &str, body: &str) {
        let r = self
            .mia_browser(self.ts.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/comments",
                self.ts.base
            )))
            .json(&json!({"body": body}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    }

    /// The owner's comment that the owner's agents may read, on Mia's
    /// page (anchored on her words, with a clip of them), sent to the
    /// agents: what the feedback channels deliver to the owner's agent,
    /// each field of Mia's withheld (spec §8.3, M16). Its thread's ID.
    pub async fn owner_thread_on_mias_page(&self) -> String {
        let ts = &self.ts;
        let aid = &self.read.artifact;
        let anchor = json!({"kind": "element", "selector": format!("#{SELECTOR}"), "quote": QUOTE,
            "prefix": PREFIX, "suffix": SUFFIX, "html_hash": HTML_HASH, "rect": null,
            "custom_name": null});
        let mut png = FAKE_PNG.to_vec();
        png.extend_from_slice(CLIP.as_bytes());
        let form = reqwest::multipart::Form::new()
            .text("anchor", anchor.to_string())
            .text("body", OWNER_ASKS)
            .text("version", "1")
            .part(
                "clip",
                reqwest::multipart::Part::bytes(png)
                    .file_name("clip.png")
                    .mime_str("image/png")
                    .unwrap(),
            );
        let r = ts
            .authed(
                ts.client
                    .post(format!("{}/api/artifacts/{aid}/threads", ts.base)),
            )
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        let tid = r.json::<Value>().await.unwrap()["thread"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let r = ts
            .authed(ts.client.post(format!(
                "{}/api/artifacts/{aid}/threads/{tid}/send",
                ts.base
            )))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        tid
    }

    /// A live page with a thread, as the owner's browser makes it.
    async fn live_page(&self) -> String {
        let v = self.ts.viewer(Some("Alex")).await;
        let form = reqwest::multipart::Form::new()
            .text("url", "http://localhost:5173/")
            .text("title", "Settings")
            .text(
                "anchor",
                json!({"kind": "element", "selector": "main", "quote": "Save", "file": "index.html"})
                    .to_string(),
            )
            .text("body", "The button overflows")
            .text("pending", "[]")
            .text("snapshot", "<p>a live page</p>");
        let r = self
            .ts
            .client
            .post(format!("{}/api/live/threads", self.ts.base))
            .header("cookie", format!("clax_viewer={}", v.cookie))
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        let v: Value = r.json().await.unwrap();
        v["page"]["artifact_id"].as_str().unwrap().to_string()
    }

    /// Every call an agent can make: each route of `agent::ROUTES` and
    /// every Denied route of clax-server's `router.rs`, with real IDs (reads on
    /// [`World::read`], writes on [`World::scratch`]), each with a body
    /// that works and the error paths (an unknown thread, a body that is
    /// not JSON). Deletes come last.
    pub fn every_agent_call(&self, agent: &AgentOf<'_>) -> Vec<Call> {
        let mut out = Vec::new();
        let mut deletes = Vec::new();
        for (method, path) in every_route() {
            let write = method != "GET";
            let s = if write { &self.scratch } else { &self.read };
            let concrete = |tid: &str| {
                path.replace("{aid}", &s.artifact)
                    .replace("{tid}", tid)
                    .replace("{id}", &agent.session)
                    .replace("{qid}", &agent.question)
                    .replace("{asset_id}", &s.asset)
                    .replace("{n}", "1")
                    .replace("{gid}", "g")
                    .replace("{code}", "c")
                    .replace("{call}", "c")
                    .replace("{ms}", "1")
                    .replace(
                        "{*path}",
                        if path.contains("/docs/") {
                            "notes/MIACANARYdocpath"
                        } else {
                            "index.html"
                        },
                    )
                    .replace("{*rest}", "index.html")
                    .replace("{asset}", &s.asset)
            };
            let body = body_for(&method, &path, &s.thread);
            let mut calls = vec![
                Call::new(&method, &concrete(&s.thread), body.clone()),
                Call::new(&method, &concrete("tmissing"), body.clone()),
            ];
            if write {
                calls.push(Call::raw(&method, &concrete(&s.thread), b"{".to_vec()));
            }
            if method == "GET" && path.contains("/versions/{n}") {
                calls.push(Call::new(
                    &method,
                    &concrete(&s.thread).replace("/versions/1", "/versions/2"),
                    None,
                ));
            }
            if path.contains("/threads/{tid}") {
                calls.push(Call::new(&method, &concrete(&s.resolved), body.clone()));
            }
            // A blind replace that lands, so the sweep reads its answer.
            if method == "PUT" && path.contains("/docs/") {
                calls.push(Call::new(
                    &method,
                    &concrete(&s.thread),
                    Some(json!({"data": {"a": 1}, "lww": true})),
                ));
            }
            if method == "DELETE" {
                deletes.extend(calls);
            } else {
                out.extend(calls);
            }
        }
        for q in [
            "/api/artifacts?artifact=",
            "/api/artifacts/{aid}/threads?include_resolved=true",
            "/api/threads?include_resolved=true",
            "/api/sessions/{id}/feedback?tier=wait&artifact=",
            "/api/live/site?origin=http%3A%2F%2Flocalhost%3A5173",
            "/api/live/pages?url=http%3A%2F%2Flocalhost%3A5173%2F",
            "/api/artifacts/{aid}/docs?collection=notes",
        ] {
            let p = q
                .replace("{aid}", &self.read.artifact)
                .replace("{id}", &agent.session);
            let p = if p.ends_with('=') {
                format!("{p}{}", self.read.artifact)
            } else {
                p
            };
            out.push(Call::new("GET", &p, None));
        }
        out.push(Call::new(
            "POST",
            "/mcp",
            Some(
                json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "sweep", "version": "0"}}}),
            ),
        ));
        out.extend(deletes);
        out
    }
}

/// A question set of one question, `text`.
pub fn ask(text: &str, artifact: Option<&str>) -> Value {
    let mut b = json!({"source": "ask", "questions": [{"question": text, "header": "Pick",
        "options": [{"label": "A"}, {"label": "B"}]}]});
    if let Some(a) = artifact {
        b["artifact_id"] = json!(a);
    }
    b
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

/// A body that does what the route does, for `method path`.
fn body_for(method: &str, path: &str, thread: &str) -> Option<Value> {
    if method == "GET" || method == "DELETE" {
        return None;
    }
    let file = json!({"content": "<p>x</p>", "encoding": "utf8"});
    Some(match (method, path) {
        (_, "/api/artifacts") => json!({"title": "x", "files": {"index.html": file}}),
        (_, p) if p.ends_with("/versions") => {
            json!({"if_version": 2, "files": {"index.html": file}})
        }
        (_, p) if p.ends_with("/comments") => json!({"body": "x", "author_kind": "agent"}),
        (_, p) if p.ends_with("/resolve") || p.ends_with("/reopen") => json!({"as": "agent"}),
        (_, p) if p.ends_with("threads:send") => json!({"thread_ids": [thread], "note": "n"}),
        (_, p) if p.contains("/docs/") => json!({"data": {"a": 1}}),
        (_, p) if p.ends_with("docs:batch") => {
            json!({"writes": [{"op": "set", "path": "notes/x", "data": {"a": 1}}]})
        }
        (_, p) if p.ends_with("docs:str_replace") => {
            json!({"path": "notes/MIACANARYdocpath", "field": "text", "old_str": "MIA", "new_str": "x"})
        }
        (_, p) if p.ends_with("docs:acquire") => json!({"path": "notes/MIACANARYdocpath"}),
        (_, p) if p.ends_with("/questions") => ask("x", None),
        (_, p) if p.ends_with("/questions:terminal") => json!({"tool_use_id": "t", "answers": {}}),
        (_, p) if p.contains("/working/") && method == "PUT" => json!({"message": "x"}),
        (_, p) if p.ends_with("/feedback/ack") => json!({"thread_ids": [thread]}),
        (_, "/api/sessions") => {
            json!({"harness": "claude", "harness_session_id": "sweep", "cwd": "/w"})
        }
        (_, "/api/sessions/join") => {
            json!({"harness": "claude", "parent_pid": 1, "harness_session_id": "sweep"})
        }
        (_, "/api/sessions/{id}") => json!({"heartbeat": true}),
        (_, p) if p.ends_with("/live-watches") => json!({"url": "http://localhost:5173/"}),
        _ => json!({}),
    })
}

/// Whether `method path` (a concrete path, with any query) is a Denied
/// route: none of `agent::ROUTES` matches it.
fn denied(method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap();
    let parts: Vec<&str> = path.split('/').collect();
    !clax_http::agent::ROUTES.iter().any(|(m, p, _)| {
        let pattern: Vec<&str> = p.split('/').collect();
        let same = |a: &str, b: &str| {
            a == b || (a.starts_with('{') && !a.starts_with("{*")) || {
                // `threads:send`, `docs:batch`: literal after the ID.
                a.contains(':') && a.split(':').nth(1) == b.split(':').nth(1) && a.starts_with('{')
            }
        };
        *m == method
            && match pattern.iter().position(|x| x.starts_with("{*")) {
                Some(i) => {
                    parts.len() > i && pattern[..i].iter().zip(&parts).all(|(a, b)| same(a, b))
                }
                None => {
                    pattern.len() == parts.len()
                        && pattern.iter().zip(&parts).all(|(a, b)| same(a, b))
                }
            }
    })
}

/// One request of the sweep.
#[derive(Clone, Debug)]
pub struct Call {
    pub name: String,
    /// The route refuses a person's agent (no `need!`): a Denied route.
    pub denied: bool,
    pub method: String,
    pub path: String,
    pub body: Option<Vec<u8>>,
}

impl Call {
    fn new(method: &str, path: &str, body: Option<Value>) -> Call {
        Call {
            denied: denied(method, path),
            name: format!("{method} {path}"),
            method: method.into(),
            path: path.into(),
            body: body.map(|b| serde_json::to_vec(&b).unwrap()),
        }
    }

    fn raw(method: &str, path: &str, body: Vec<u8>) -> Call {
        Call {
            denied: denied(method, path),
            name: format!("{method} {path} (not JSON)"),
            method: method.into(),
            path: path.into(),
            body: Some(body),
        }
    }

    /// The document path (`notes/<id>`) this call names, when it reads or
    /// writes one by its path: the docs routes but the listing.
    pub fn named_doc(&self) -> Option<String> {
        if !self.path.contains("/docs") || self.path.contains("/docs?") {
            return None;
        }
        let path = format!("notes/{DOC_PATH}");
        let sent = self.path.contains(&path)
            || self
                .body
                .as_ref()
                .is_some_and(|b| String::from_utf8_lossy(b).contains(&path));
        sent.then_some(path)
    }

    /// Sends the call as `agent` against `w`: the status and every byte of
    /// the response, headers included.
    pub async fn run(&self, w: &World, agent: &AgentOf<'_>) -> String {
        // An event stream resumes from the start of the bus, so a stream
        // that answered would replay the events the world's writes made,
        // up to a marker event published now, which ends the read.
        let epoch = w.ts.events.epoch().to_string();
        let resume = format!("{epoch}-0");
        let marker = self.path.starts_with("/api/events").then(|| {
            w.ts.events.publish(clax_core::Event::ArtifactDeleted {
                artifact_id: "zzzzzzzzzzzz".into(),
            });
            format!("id: {epoch}-{}", w.ts.events.last_id())
        });
        let res = match &agent.who {
            As::Grant(c) => {
                c.call(crate::Call {
                    method: &self.method,
                    path: &self.path,
                    authorization: Some(format!("Clax-Grant {}", c.access())),
                    body: self.body.clone(),
                    headers: vec![
                        ("x-clax-session", agent.session.clone()),
                        ("last-event-id", resume.clone()),
                    ],
                    ..Default::default()
                })
                .await
            }
            As::Token => {
                let mut req =
                    w.ts.authed(w.ts.client.request(
                        reqwest::Method::from_bytes(self.method.as_bytes()).unwrap(),
                        format!("{}{}", w.ts.base, self.path),
                    ))
                    .header("x-clax-channel", "agent")
                    .header("x-clax-session", &agent.session)
                    .header("last-event-id", &resume);
                if let Some(b) = &self.body {
                    req = req
                        .header("content-type", "application/json")
                        .body(b.clone());
                }
                req.send().await.unwrap()
            }
        };
        let status = res.status().as_u16();
        let headers = format!("{:?}", res.headers());
        let streaming = res
            .headers()
            .get("content-type")
            .is_some_and(|v| v.to_str().unwrap_or("").starts_with("text/event-stream"));
        let body = if streaming {
            // An event stream: up to its first whole frame after `ready`
            // (a replayed event, or a keep-alive).
            first_frame(res, &self.name, marker.as_deref()).await
        } else {
            String::from_utf8_lossy(&res.bytes().await.unwrap_or_default()).into_owned()
        };
        format!("{status} {headers}\n{body}")
    }
}

/// What event stream `res` sends up to the end of the frame whose `id:`
/// line is `until` (the replay's last event), or without one, up to its
/// first whole frame (a blank line) that is not `event: ready`
/// (`ready` carries no content). Fails when none comes within 30 s.
async fn first_frame(mut res: reqwest::Response, name: &str, until: Option<&str>) -> String {
    let mut out = Vec::new();
    let got = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Ok(Some(chunk)) = res.chunk().await {
            out.extend_from_slice(&chunk);
            let done = match until {
                Some(id) => whole_frame_with(&out, id),
                None => past_ready(&out),
            };
            if done {
                return true;
            }
        }
        false
    })
    .await;
    assert!(
        matches!(got, Ok(true)),
        "{name}: the stream sent no whole frame: {}",
        String::from_utf8_lossy(&out)
    );
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `bytes` hold a whole frame with the line `id`.
fn whole_frame_with(bytes: &[u8], id: &str) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let mut frames: Vec<&str> = text.split("\n\n").collect();
    frames.pop();
    frames.iter().any(|f| f.lines().any(|l| l.trim_end() == id))
}

/// Whether `bytes` hold a whole frame other than a `ready` event.
fn past_ready(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let mut frames: Vec<&str> = text.split("\n\n").collect();
    frames.pop(); // the unfinished rest
    frames
        .iter()
        .any(|f| !f.lines().any(|l| l.trim_end() == "event: ready"))
}

#[test]
fn a_ready_frame_alone_is_not_enough() {
    assert!(!past_ready(b"id: e-0\nevent: ready\ndata: {}\n\n"));
    assert!(!past_ready(
        b"id: e-0\nevent: ready\ndata: {}\n\nid: e-1\nevent: thread"
    ));
    assert!(past_ready(
        b"id: e-0\nevent: ready\ndata: {}\n\nid: e-1\nevent: thread\ndata: {}\n\n"
    ));
    assert!(past_ready(b": keep-alive\n\n"));
    assert!(past_ready(b"{\"error\":1}\n\n"));
    assert!(!whole_frame_with(
        b"id: e-1\ndata: x\n\nid: e-2\ndata: y",
        "id: e-2"
    ));
    assert!(whole_frame_with(
        b"id: e-1\ndata: x\n\nid: e-2\ndata: y\n\n",
        "id: e-2"
    ));
}

/// Every route of clax-server's `router.rs` (but the test-only ones): method and
/// path, read from the source as the route guards read it.
pub fn every_route() -> Vec<(String, String)> {
    let src = include_str!("../../clax-server/src/router.rs");
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find(".route(") {
        let after = &rest[i + ".route(".len()..];
        let mut depth = 1;
        let mut end = 0;
        for (j, c) in after.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = j;
                        break;
                    }
                }
                _ => {}
            }
        }
        let call = &after[..end];
        rest = &after[end..];
        let open = call.find('"').unwrap();
        let close = open + 1 + call[open + 1..].find('"').unwrap();
        let path = call[open + 1..close].to_string();
        if path.starts_with("/api/_test/") || path == "/api/admin/shutdown" {
            continue;
        }
        let text = &call[close + 1..];
        for m in ["get", "post", "put", "patch", "delete"] {
            let pat = format!("{m}(");
            let mut k = 0;
            while let Some(x) = text[k..].find(&pat) {
                let at = k + x;
                k = at + pat.len();
                if at > 0
                    && (text.as_bytes()[at - 1].is_ascii_alphanumeric()
                        || text.as_bytes()[at - 1] == b'_')
                {
                    continue;
                }
                out.push((m.to_uppercase(), path.clone()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `v` without the fields that echo document `path`, which the request
/// named (spec §8.3, §21.9): a placeholder's or a tool's document ID, a
/// tool's top-level `path`, and an error naming that path (its `path` and
/// `message`). Nothing else is removed, and a listing is never passed here.
pub fn without_named_doc(mut v: Value, path: &str) -> Value {
    let id = path.rsplit('/').next().unwrap_or_default();
    if v["path"] == json!(path) {
        v.as_object_mut().unwrap().remove("path");
    }
    if let Some(d) = v.get_mut("doc").and_then(Value::as_object_mut) {
        for k in ["doc_id", "id"] {
            if d.get(k) == Some(&json!(id)) {
                d.remove(k);
            }
        }
    }
    if let Some(e) = v.get_mut("error").and_then(Value::as_object_mut)
        && e.get("path") == Some(&json!(path))
    {
        e.remove("path");
        e.remove("message");
    }
    v
}
