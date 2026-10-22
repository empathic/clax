-- A home of the Clax app at this branch's earlier numbering: user_version 43, the app's
-- migrations at 39 to 43 and none of main's 39 and 40, as sqlite3 .dump writes it.
-- Schema: the macOS app's own clax-home (in its sandbox container, written by the clax it
-- shipped), copied on 2026-10-20 with cp and read from that copy with sqlite3 .backup. Every
-- table, index and trigger is as SQLite keeps it there, including the order of artifacts'
-- columns (draft_of and hidden before person, left by the adoption of an earlier layout),
-- which a replay of the migrations does not give.
-- Rows: none of that home's. They were written through the HTTP API by clax built at
-- 79cecdcc6 on a scratch home (two artifacts, three versions, two threads of two comments,
-- one app session, a doc, a db_read_grants row, and what those writes record besides), then
-- copied into the schema above column by column. The artifacts' content files are not here.
-- user_version: 43
/* WARNING: Script requires that SQLITE_DBCONFIG_DEFENSIVE be disabled */
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE artifacts (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        description TEXT,
        icon TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        current_version INTEGER NOT NULL DEFAULT 0,
        owner_session_id TEXT,
        pinned INTEGER NOT NULL DEFAULT 0,
        capabilities_json TEXT NOT NULL DEFAULT '{}',
        contract_version TEXT NOT NULL,
        deleted_at TEXT
    , doc_seq INTEGER NOT NULL DEFAULT 0, kind TEXT NOT NULL DEFAULT 'html'
        CHECK (kind IN ('html', 'live')), draft_of TEXT, hidden INTEGER NOT NULL DEFAULT 0, person TEXT, text_authors_json TEXT, link_access TEXT CHECK (link_access IN ('viewer', 'commenter')), forked_from_id TEXT, forked_from_version INTEGER, link_listed INTEGER NOT NULL DEFAULT 0, draft_inherit_net INTEGER NOT NULL DEFAULT 1);
INSERT INTO artifacts VALUES('f34mv7wr3752','Reading list',NULL,NULL,'2026-10-20T10:46:47.113Z','2026-10-20T10:46:47.279Z',2,'01M5CEQFCNTHGNZ10XVMXTR18P',0,'{}','0.2.61',NULL,0,'html',NULL,0,NULL,NULL,NULL,NULL,NULL,0,1);
INSERT INTO artifacts VALUES('8esgmev6mkyt','Tally',NULL,NULL,'2026-10-20T10:46:47.290Z','2026-10-20T10:46:47.291Z',1,NULL,0,'{}','0.2.61',NULL,1,'html',NULL,0,NULL,NULL,NULL,NULL,NULL,0,1);
CREATE TABLE versions (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        n INTEGER NOT NULL,
        label TEXT,
        created_at TEXT NOT NULL,
        session_id TEXT,
        files_json TEXT NOT NULL, note TEXT, content_sha256 TEXT, person TEXT, by_page INTEGER NOT NULL DEFAULT 0, file_authors_json TEXT,
        PRIMARY KEY (artifact_id, n)
    );
INSERT INTO versions VALUES('f34mv7wr3752',1,NULL,'2026-10-20T10:46:47.114Z','01M5CEQFCNTHGNZ10XVMXTR18P','{"index.html":{"content_type":"text/html","size":74,"sha256":"e463b80a012c3ff5e2f70e2a82051608aa8b8b77968441a885edc6bfef8cc41b"}}',NULL,'sha256:09cdcef93661ca5021fa76d18ab092b94f5bc35f39c4c09aab5c09f1ff4d0dc9',NULL,0,NULL);
INSERT INTO versions VALUES('f34mv7wr3752',2,NULL,'2026-10-20T10:46:47.279Z','01M5CEQFCNTHGNZ10XVMXTR18P','{"index.html":{"content_type":"text/html","size":74,"sha256":"99114961e6fcd8ad954a7cf28dade1a2f32a33b77a9f095f1d8c49067158f916"}}',NULL,'sha256:4f7659e6f331c4532699bd00fd7fdeae5acb25c372f5a9787b04e662e8baa3bb',NULL,0,NULL);
INSERT INTO versions VALUES('8esgmev6mkyt',1,NULL,'2026-10-20T10:46:47.291Z',NULL,'{"index.html":{"content_type":"text/html","size":47,"sha256":"f5461061cd52e44317e1f779d94b2246efd46f8a8b09dc91f808b773f6b53cec"}}',NULL,'sha256:7468a0979e9d8c94b2b9eac462ba393738282ddfcab46c405eb4b9ffcdb5bea2',NULL,0,NULL);
CREATE TABLE assets (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        content_type TEXT NOT NULL,
        size INTEGER NOT NULL,
        ext TEXT NOT NULL,
        created_at TEXT NOT NULL
    , person TEXT);
CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        harness TEXT NOT NULL,
        harness_session_id TEXT,
        cwd TEXT NOT NULL,
        pid INTEGER,
        parent_pid INTEGER,
        started_at TEXT NOT NULL,
        last_seen_at TEXT NOT NULL,
        ended_at TEXT
    , agent_handle TEXT, transcript_path TEXT, grant_id TEXT, person TEXT, name TEXT, name_source TEXT CHECK (name_source IN ('derived', 'agent', 'person')), narrow_json TEXT, ended_reason TEXT
        CHECK (ended_reason IN ('idle', 'exit', 'revoked', 'person')), ended_watches_json TEXT, secret_sha256 TEXT, client_name TEXT, terminal_at TEXT, terminal_inferred INTEGER NOT NULL DEFAULT 0, turn_at TEXT, delivered_at TEXT, delivered_artifact_id TEXT, delivered_thread_id TEXT, delivered_since_turn INTEGER NOT NULL DEFAULT 0, turn_open INTEGER NOT NULL DEFAULT 0, queued_since_turn INTEGER NOT NULL DEFAULT 0, delivered_written_at TEXT, delivered_act TEXT);
INSERT INTO sessions VALUES('01M5CEQFCNTHGNZ10XVMXTR18P','app','install-1','/w',NULL,NULL,'2026-10-20T10:46:46.933Z','2026-10-20T10:46:46.933Z',NULL,'a_f5bffe09adb46f3052e1ba',NULL,NULL,NULL,'app','derived',NULL,NULL,NULL,NULL,NULL,NULL,0,NULL,NULL,NULL,NULL,0,0,0,NULL,NULL);
CREATE TABLE watches (
        session_id TEXT NOT NULL REFERENCES sessions(id),
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        replies_armed INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'direct'
        CHECK (source IN ('direct', 'scope')), relay_name TEXT, relay_harness TEXT,
        PRIMARY KEY (session_id, artifact_id)
    );
INSERT INTO watches VALUES('01M5CEQFCNTHGNZ10XVMXTR18P','f34mv7wr3752',1,'2026-10-20T10:46:47.115Z','direct',NULL,NULL);
CREATE TABLE threads (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        version_n INTEGER NOT NULL,
        anchor_json TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved')),
        sent_to_agent INTEGER NOT NULL DEFAULT 0,
        has_clip INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        resolved_at TEXT,
        resolved_by TEXT
    , target_session_id TEXT, live_path TEXT, resolved_by_person TEXT);
INSERT INTO threads VALUES('01M5CEQFZKB13ZBTTSHS67Z6JK','f34mv7wr3752',2,'{"kind":"element","selector":"body > main > h2","quote":"Quarterly goals","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}','open',0,0,'2026-10-20T10:46:47.539Z',NULL,NULL,NULL,NULL,NULL);
INSERT INTO threads VALUES('01M5CEQG67NT2MA6S5P3V37ZYB','8esgmev6mkyt',1,'{"kind":"element","selector":"body > main > h2","quote":"Quarterly goals","prefix":"","suffix":"","html_hash":"sha256:00","rect":null,"custom_name":null,"file":"index.html"}','open',0,0,'2026-10-20T10:46:47.751Z',NULL,NULL,NULL,NULL,NULL);
CREATE TABLE comments (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        author_kind TEXT NOT NULL CHECK (author_kind IN ('viewer', 'agent')),
        author_name TEXT NOT NULL,
        via_session_id TEXT,
        body TEXT NOT NULL,
        created_at TEXT NOT NULL
    , via_page INTEGER NOT NULL DEFAULT 0, author_public_id TEXT);
INSERT INTO comments VALUES('01M5CEQFZKB13ZBTTSHS67Z6JM','01M5CEQFZKB13ZBTTSHS67Z6JK','viewer','Viewer',NULL,'Make the heading bigger','2026-10-20T10:46:47.539Z',0,'u_5d33d5cccb75cf2adec569');
INSERT INTO comments VALUES('01M5CEQG67NT2MA6S5P3V37ZYC','01M5CEQG67NT2MA6S5P3V37ZYB','viewer','Viewer',NULL,'Add a reset button','2026-10-20T10:46:47.751Z',0,'u_5d33d5cccb75cf2adec569');
INSERT INTO comments VALUES('01M5CEQGDQBPNNQ46SVWWRQ8F2','01M5CEQFZKB13ZBTTSHS67Z6JK','viewer','Viewer',NULL,'And bolder, please','2026-10-20T10:46:47.991Z',0,'u_5d33d5cccb75cf2adec569');
INSERT INTO comments VALUES('01M5CEQGEEVE2Q2FMXG0P09VDF','01M5CEQG67NT2MA6S5P3V37ZYB','viewer','Viewer',NULL,'Below the count','2026-10-20T10:46:48.014Z',0,'u_5d33d5cccb75cf2adec569');
CREATE TABLE feedback (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        comment_id TEXT NOT NULL REFERENCES comments(id),
        target_session_id TEXT,
        created_at TEXT NOT NULL,
        delivered_at TEXT,
        delivery_tier TEXT CHECK (delivery_tier IN
            ('piggyback', 'stop_hook', 'prompt_hook', 'wait', 'queue', 'inject')),
        acknowledged_at TEXT,
        resend_count INTEGER NOT NULL DEFAULT 0,
        last_sent_at TEXT,
        untargeted_at TEXT
    , push_failed_at TEXT, notified_at TEXT, batch_id TEXT, placeholder_at TEXT, no_access_at TEXT);
CREATE TABLE viewers (
        id TEXT PRIMARY KEY,
        display_name TEXT,
        created_at TEXT NOT NULL
    , public_id TEXT, owner INTEGER NOT NULL DEFAULT 0, claimed INTEGER NOT NULL DEFAULT 0, minted_local INTEGER, person INTEGER NOT NULL DEFAULT 0, person_revoked_at TEXT, rights_json TEXT, guest INTEGER NOT NULL DEFAULT 0);
INSERT INTO viewers VALUES('01M5CEQFQTAJ7P5TCRDVFTQA2J',NULL,'2026-10-20T10:46:47.290Z','u_5d33d5cccb75cf2adec569',1,0,NULL,0,NULL,NULL,0);
CREATE TABLE session_env (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        codex_home TEXT
    , push_error TEXT, push_error_at TEXT);
CREATE TABLE docs (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        path TEXT NOT NULL,
        collection TEXT NOT NULL,
        json TEXT NOT NULL,
        version INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, path)
    );
INSERT INTO docs VALUES('8esgmev6mkyt','counts/main','counts','{"n":3}',1,'2026-10-20T10:46:48.026Z');
CREATE TABLE leases (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id),
        path TEXT NOT NULL,
        holder TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, path)
    );
CREATE TABLE version_threads (
        artifact_id TEXT NOT NULL,
        version_n INTEGER NOT NULL,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        source TEXT NOT NULL CHECK (source IN ('working', 'explicit', 'resolve')),
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, version_n, thread_id)
    );
CREATE TABLE viewer_seen (
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        artifact_id TEXT NOT NULL,
        seen_n INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (viewer_id, artifact_id)
    );
CREATE TABLE mentions (
        comment_id TEXT NOT NULL REFERENCES comments(id),
        public_id TEXT NOT NULL,
        PRIMARY KEY (comment_id, public_id)
    );
CREATE TABLE viewer_threads (
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        thread_id TEXT NOT NULL REFERENCES threads(id),
        looked_at TEXT NOT NULL,
        PRIMARY KEY (viewer_id, thread_id)
    );
CREATE TABLE send_batches (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL,
        note TEXT,
        sent_by TEXT NOT NULL,
        size INTEGER NOT NULL,
        created_at TEXT NOT NULL
    , sender TEXT);
CREATE TABLE batch_threads (
        batch_id TEXT NOT NULL REFERENCES send_batches(id),
        thread_id TEXT NOT NULL REFERENCES threads(id),
        PRIMARY KEY (batch_id, thread_id)
    );
CREATE TABLE live_pages (
        artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id),
        origin TEXT NOT NULL,
        path TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE (origin, path)
    );
CREATE TABLE live_watches (
        session_id TEXT NOT NULL REFERENCES sessions(id),
        origin TEXT NOT NULL,
        path TEXT NOT NULL,
        replies_armed INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL,
        PRIMARY KEY (session_id, origin, path)
    );
CREATE TABLE live_pending (
        artifact_id TEXT NOT NULL,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        source TEXT NOT NULL CHECK (source IN ('explicit', 'resolve')),
        harness TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, thread_id)
    );
CREATE TABLE live_picks (
        artifact_id TEXT NOT NULL,
        pick_id TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, pick_id)
    );
CREATE TABLE extension_credentials (
        id TEXT PRIMARY KEY,
        extension_id TEXT NOT NULL,
        secret_sha256 TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL,
        last_used_at TEXT NOT NULL,
        revoked_at TEXT
    );
CREATE TABLE live_rules (
        id TEXT PRIMARY KEY,
        origin TEXT NOT NULL,
        pattern TEXT NOT NULL,
        created_at TEXT NOT NULL,
        deleted_at TEXT,
        UNIQUE (origin, pattern)
    );
CREATE TABLE live_sites (
        origin TEXT PRIMARY KEY,
        site TEXT NOT NULL,
        joined_at TEXT NOT NULL,
        last_used_at TEXT NOT NULL
    );
CREATE TABLE live_merged_pages (
        artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id),
        origin TEXT NOT NULL,
        path TEXT NOT NULL,
        merged_into TEXT,
        merged_at TEXT NOT NULL
    );
CREATE TABLE live_site_answers (
        a TEXT NOT NULL,
        b TEXT NOT NULL,
        answer TEXT NOT NULL CHECK (answer IN ('never', 'later')),
        until TEXT,
        created_at TEXT NOT NULL,
        PRIMARY KEY (a, b),
        CHECK (a < b)
    );
CREATE TABLE IF NOT EXISTS "thread_moves" (
        id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        from_artifact_id TEXT NOT NULL,
        from_url TEXT NOT NULL,
        to_artifact_id TEXT NOT NULL,
        to_url TEXT NOT NULL,
        moved_by TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('move', 'merge', 'unmerge', 'join')),
        rule_id TEXT,
        created_at TEXT NOT NULL
    );
CREATE TABLE questions (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        artifact_id TEXT,
        source TEXT NOT NULL CHECK (source IN ('ask', 'hook')),
        tool_use_id TEXT,
        questions_json TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'open'
            CHECK (status IN ('open', 'answered', 'declined', 'released', 'withdrawn')),
        answers_json TEXT,
        answered_via TEXT CHECK (answered_via IN ('shell', 'extension', 'cli', 'terminal')),
        created_at TEXT NOT NULL,
        closed_at TEXT,
        taken_at TEXT
    , person TEXT, thread_id TEXT, in_terminal INTEGER NOT NULL DEFAULT 0, pulled_at TEXT);
CREATE TABLE IF NOT EXISTS 'inbox_fts_data'(id INTEGER PRIMARY KEY, block BLOB);
INSERT INTO inbox_fts_data VALUES(1,X'0103');
INSERT INTO inbox_fts_data VALUES(10,X'00000000ff00000101010100010101010101000001');
INSERT INTO inbox_fts_data VALUES(137438953473,X'00000063043061707001020401046c697374010203010772656164696e670102020002316101020401016c010203010172010202000332617001020401026c690102030102726501020200043361707001020401036c697301020301037265610102020408090c0706060807070908');
CREATE TABLE IF NOT EXISTS 'inbox_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
INSERT INTO inbox_fts_idx VALUES(1,X'',2);
CREATE TABLE IF NOT EXISTS 'inbox_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB, origin INTEGER);
INSERT INTO inbox_fts_docsize VALUES(1,X'03',1);
CREATE TABLE IF NOT EXISTS 'inbox_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;
INSERT INTO inbox_fts_config VALUES('version',4);
CREATE TABLE audit_events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        at TEXT NOT NULL,
        kind TEXT NOT NULL,
        actor TEXT NOT NULL,
        artifact_id TEXT,
        artifact2_id TEXT,
        thread_id TEXT,
        session_id TEXT,
        question_id TEXT,
        call_id TEXT,
        origin TEXT,
        body TEXT NOT NULL,
        backfilled INTEGER NOT NULL DEFAULT 0
    );
INSERT INTO audit_events VALUES(1,'2026-10-20T10:46:45.652Z','approval.grant','{"type":"system","reason":"daemon"}',NULL,NULL,NULL,NULL,NULL,NULL,NULL,'{"approval_id":"01M5CEQE4MV3ANJRNERSN93FBC","approver":null,"author":"*unconnected","clax_commit":"unknown","clax_version":"0.5.0","created_via":"migration","scope":"everywhere","v":1,"via":"daemon"}',0);
INSERT INTO audit_events VALUES(2,'2026-10-20T10:46:46.933Z','session.start','{"type":"agent","session_id":"01M5CEQFCNTHGNZ10XVMXTR18P","harness":"app","harness_session_id":"install-1","agent_handle":"a_f5bffe09adb46f3052e1ba"}',NULL,NULL,NULL,'01M5CEQFCNTHGNZ10XVMXTR18P',NULL,NULL,NULL,'{"clax_commit":"unknown","clax_version":"0.5.0","cwd":"/w","harness":"app","harness_session_id":"install-1","pid":null,"transcript_path":null,"v":1,"via":"mcp"}',0);
INSERT INTO audit_events VALUES(3,'2026-10-20T10:46:47.114Z','artifact.create','{"type":"agent","session_id":"01M5CEQFCNTHGNZ10XVMXTR18P","harness":"app","harness_session_id":"install-1","agent_handle":"a_f5bffe09adb46f3052e1ba"}','f34mv7wr3752',NULL,NULL,'01M5CEQFCNTHGNZ10XVMXTR18P',NULL,NULL,NULL,'{"capabilities":{},"clax_commit":"unknown","clax_version":"0.5.0","contract_version":"0.2.61","icon":null,"kind":"html","title":"Reading list","v":1,"via":"mcp"}',0);
INSERT INTO audit_events VALUES(4,'2026-10-20T10:46:47.114Z','version.publish','{"type":"agent","session_id":"01M5CEQFCNTHGNZ10XVMXTR18P","harness":"app","harness_session_id":"install-1","agent_handle":"a_f5bffe09adb46f3052e1ba"}','f34mv7wr3752',NULL,NULL,'01M5CEQFCNTHGNZ10XVMXTR18P',NULL,NULL,NULL,'{"addresses":[],"by_page":false,"carried":[],"clax_commit":"unknown","clax_version":"0.5.0","content_sha256":"sha256:09cdcef93661ca5021fa76d18ab092b94f5bc35f39c4c09aab5c09f1ff4d0dc9","files":{"index.html":{"content_type":"text/html","sha256":"sha256:e463b80a012c3ff5e2f70e2a82051608aa8b8b77968441a885edc6bfef8cc41b","size":74}},"label":null,"n":1,"note":null,"title":"Reading list","v":1,"via":"mcp"}',0);
INSERT INTO audit_events VALUES(5,'2026-10-20T10:46:47.115Z','watch.start','{"type":"agent","session_id":"01M5CEQFCNTHGNZ10XVMXTR18P","harness":"app","harness_session_id":"install-1","agent_handle":"a_f5bffe09adb46f3052e1ba"}','f34mv7wr3752',NULL,NULL,'01M5CEQFCNTHGNZ10XVMXTR18P',NULL,NULL,NULL,'{"clax_commit":"unknown","clax_version":"0.5.0","replies_armed":true,"source":"direct","target":"artifact","v":1,"via":"mcp"}',0);
INSERT INTO audit_events VALUES(6,'2026-10-20T10:46:47.279Z','version.publish','{"type":"agent","session_id":"01M5CEQFCNTHGNZ10XVMXTR18P","harness":"app","harness_session_id":"install-1","agent_handle":"a_f5bffe09adb46f3052e1ba"}','f34mv7wr3752',NULL,NULL,'01M5CEQFCNTHGNZ10XVMXTR18P',NULL,NULL,NULL,'{"addresses":[],"by_page":false,"carried":[],"clax_commit":"unknown","clax_version":"0.5.0","content_sha256":"sha256:4f7659e6f331c4532699bd00fd7fdeae5acb25c372f5a9787b04e662e8baa3bb","files":{"index.html":{"content_type":"text/html","sha256":"sha256:99114961e6fcd8ad954a7cf28dade1a2f32a33b77a9f095f1d8c49067158f916","size":74}},"label":null,"n":2,"note":null,"title":"Reading list","v":1,"via":"mcp"}',0);
INSERT INTO audit_events VALUES(7,'2026-10-20T10:46:47.291Z','artifact.create','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','8esgmev6mkyt',NULL,NULL,NULL,NULL,NULL,NULL,'{"capabilities":{},"clax_commit":"unknown","clax_version":"0.5.0","contract_version":"0.2.61","icon":null,"kind":"html","title":"Tally","v":1,"via":"cli"}',0);
INSERT INTO audit_events VALUES(8,'2026-10-20T10:46:47.291Z','version.publish','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','8esgmev6mkyt',NULL,NULL,NULL,NULL,NULL,NULL,'{"addresses":[],"by_page":false,"carried":[],"clax_commit":"unknown","clax_version":"0.5.0","content_sha256":"sha256:7468a0979e9d8c94b2b9eac462ba393738282ddfcab46c405eb4b9ffcdb5bea2","files":{"index.html":{"content_type":"text/html","sha256":"sha256:f5461061cd52e44317e1f779d94b2246efd46f8a8b09dc91f808b773f6b53cec","size":47}},"label":null,"n":1,"note":null,"title":"Tally","v":1,"via":"cli"}',0);
INSERT INTO audit_events VALUES(9,'2026-10-20T10:46:47.539Z','thread.open','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','f34mv7wr3752',NULL,'01M5CEQFZKB13ZBTTSHS67Z6JK',NULL,NULL,NULL,NULL,'{"anchor":{"file":"index.html","html_hash":"sha256:00","kind":"element","prefix":"","quote":"Quarterly goals","route":null,"selector":"body > main > h2","suffix":""},"clax_commit":"unknown","clax_version":"0.5.0","first_comment_id":"01M5CEQFZKB13ZBTTSHS67Z6JM","has_clip":false,"live_path":null,"v":1,"version_n":2,"via":"cli"}',0);
INSERT INTO audit_events VALUES(10,'2026-10-20T10:46:47.539Z','comment.add','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','f34mv7wr3752',NULL,'01M5CEQFZKB13ZBTTSHS67Z6JK',NULL,NULL,NULL,NULL,'{"author_kind":"viewer","author_name":"Viewer","body":"Make the heading bigger","clax_commit":"unknown","clax_version":"0.5.0","comment_id":"01M5CEQFZKB13ZBTTSHS67Z6JM","v":1,"via":"cli","via_harness":null,"via_page":false}',0);
INSERT INTO audit_events VALUES(11,'2026-10-20T10:46:47.751Z','thread.open','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','8esgmev6mkyt',NULL,'01M5CEQG67NT2MA6S5P3V37ZYB',NULL,NULL,NULL,NULL,'{"anchor":{"file":"index.html","html_hash":"sha256:00","kind":"element","prefix":"","quote":"Quarterly goals","route":null,"selector":"body > main > h2","suffix":""},"clax_commit":"unknown","clax_version":"0.5.0","first_comment_id":"01M5CEQG67NT2MA6S5P3V37ZYC","has_clip":false,"live_path":null,"v":1,"version_n":1,"via":"cli"}',0);
INSERT INTO audit_events VALUES(12,'2026-10-20T10:46:47.751Z','comment.add','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','8esgmev6mkyt',NULL,'01M5CEQG67NT2MA6S5P3V37ZYB',NULL,NULL,NULL,NULL,'{"author_kind":"viewer","author_name":"Viewer","body":"Add a reset button","clax_commit":"unknown","clax_version":"0.5.0","comment_id":"01M5CEQG67NT2MA6S5P3V37ZYC","v":1,"via":"cli","via_harness":null,"via_page":false}',0);
INSERT INTO audit_events VALUES(13,'2026-10-20T10:46:47.991Z','comment.add','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','f34mv7wr3752',NULL,'01M5CEQFZKB13ZBTTSHS67Z6JK',NULL,NULL,NULL,NULL,'{"author_kind":"viewer","author_name":"Viewer","body":"And bolder, please","clax_commit":"unknown","clax_version":"0.5.0","comment_id":"01M5CEQGDQBPNNQ46SVWWRQ8F2","v":1,"via":"cli","via_harness":null,"via_page":false}',0);
INSERT INTO audit_events VALUES(14,'2026-10-20T10:46:48.014Z','comment.add','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','8esgmev6mkyt',NULL,'01M5CEQG67NT2MA6S5P3V37ZYB',NULL,NULL,NULL,NULL,'{"author_kind":"viewer","author_name":"Viewer","body":"Below the count","clax_commit":"unknown","clax_version":"0.5.0","comment_id":"01M5CEQGEEVE2Q2FMXG0P09VDF","v":1,"via":"cli","via_harness":null,"via_page":false}',0);
INSERT INTO audit_events VALUES(15,'2026-10-20T10:46:48.026Z','doc.write','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','8esgmev6mkyt',NULL,NULL,NULL,NULL,NULL,NULL,'{"clax_commit":"unknown","clax_version":"0.5.0","collection":"counts","doc_id":"main","op":"set","sha256":"sha256:215ddd5567ca2590efd4ea109b4e56cbe591e2676fbf54a9262692c539166da6","v":1,"version":1,"via":"cli"}',0);
INSERT INTO audit_events VALUES(16,'2026-10-20T10:46:48.036Z','artifact.update','{"type":"owner","public_id":"u_5d33d5cccb75cf2adec569"}','f34mv7wr3752','8esgmev6mkyt',NULL,NULL,NULL,NULL,NULL,'{"clax_commit":"unknown","clax_version":"0.5.0","fields":{"db_read_grants":{"grant":"8esgmev6mkyt"}},"v":1,"via":"cli"}',0);
CREATE TABLE install (k TEXT PRIMARY KEY, v TEXT NOT NULL);
INSERT INTO install VALUES('id','cf8a7b91c778830b79c5ae4e0d41ed3c');
INSERT INTO install VALUES('backfill','2026-10-20T10:46:45.692Z');
INSERT INTO install VALUES('migration_approval','1');
CREATE TABLE db_read_grants (
        reader_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        source_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        granted_at TEXT NOT NULL,
        PRIMARY KEY (reader_id, source_id)
    ) WITHOUT ROWID;
INSERT INTO db_read_grants VALUES('f34mv7wr3752','8esgmev6mkyt','2026-10-20T10:46:48.036Z');
ANALYZE sqlite_schema;
ANALYZE sqlite_schema;
CREATE TABLE net_grants (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        host TEXT NOT NULL,
        granted_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, host)
    ) WITHOUT ROWID;
CREATE TABLE sample_grants (
        artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id) ON DELETE CASCADE,
        granted_at TEXT NOT NULL
    ) WITHOUT ROWID;
CREATE TABLE sample_spend (
        artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        day TEXT NOT NULL,
        micro_usd INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (artifact_id, day)
    ) WITHOUT ROWID;
CREATE TABLE viewer_cookies (
        sha256 TEXT PRIMARY KEY,
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        created_at TEXT NOT NULL,
        legacy INTEGER NOT NULL DEFAULT 0,
        replaced_at TEXT
    );
CREATE TABLE invites (
        id TEXT PRIMARY KEY,
        code_sha256 TEXT NOT NULL UNIQUE,
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        used_at TEXT,
        grant_id TEXT,
        revoked_at TEXT
    );
CREATE TABLE grants (
        id TEXT PRIMARY KEY,
        viewer_id TEXT REFERENCES viewers(id),
        status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'revoked', 'expired')),
        via TEXT NOT NULL CHECK (via IN ('invite', 'request')),
        client_key TEXT NOT NULL,
        client_name TEXT NOT NULL,
        peer_ip TEXT NOT NULL,
        user_code_sha256 TEXT,
        wrong_codes INTEGER NOT NULL DEFAULT 0,
        continue_sha256 TEXT NOT NULL,
        continue_t INTEGER,
        continue_nonce TEXT,
        manage_sha256 TEXT,
        narrow_json TEXT,
        created_at TEXT NOT NULL,
        decided_at TEXT,
        last_used_at TEXT,
        ended_at TEXT,
        ended_by TEXT,
        browser_sha256 TEXT,
        browser_expires_at TEXT
    , bearer INTEGER NOT NULL DEFAULT 0);
CREATE TABLE grant_tokens (
        sha256 TEXT PRIMARY KEY,
        grant_id TEXT NOT NULL REFERENCES grants(id),
        created_at TEXT NOT NULL,
        revoked_at TEXT
    );
CREATE TABLE session_elevations (
        session_id TEXT NOT NULL,
        granter TEXT,
        access_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        version TEXT NOT NULL DEFAULT ''
    );
CREATE TABLE approvals (
        id TEXT PRIMARY KEY,
        approver TEXT,
        author TEXT NOT NULL,
        scope TEXT NOT NULL CHECK (scope IN ('thread', 'artifact', 'everywhere')),
        artifact_id TEXT,
        thread_id TEXT,
        created_at TEXT NOT NULL,
        created_via TEXT NOT NULL CHECK (created_via IN ('shell', 'extension', 'connect', 'migration')),
        request_id TEXT,
        revoked_at TEXT
    );
INSERT INTO approvals VALUES('01M5CEQE4MV3ANJRNERSN93FBC',NULL,'*unconnected','everywhere',NULL,NULL,'2026-10-20T10:46:45.652Z','migration',NULL,NULL);
CREATE TABLE approval_requests (
        id TEXT PRIMARY KEY,
        approver TEXT,
        author TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        thread_id TEXT,
        session_id TEXT,
        cause TEXT NOT NULL CHECK (cause IN ('ask', 'routing')),
        count INTEGER NOT NULL DEFAULT 1,
        status TEXT NOT NULL DEFAULT 'open'
            CHECK (status IN ('open', 'approved', 'declined', 'withdrawn')),
        decided_scope TEXT CHECK (decided_scope IN ('thread', 'artifact', 'everywhere')),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        decided_at TEXT
    );
CREATE TABLE doc_authors (
        artifact_id TEXT NOT NULL,
        path TEXT NOT NULL,
        author TEXT NOT NULL,
        PRIMARY KEY (artifact_id, path, author)
    );
CREATE TABLE page_authors (
        kind TEXT NOT NULL CHECK (kind IN ('comment', 'asset')),
        id TEXT NOT NULL,
        author TEXT NOT NULL,
        PRIMARY KEY (kind, id, author)
    );
CREATE TABLE hosted_setup (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        viewer_id TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
CREATE TABLE browser_sessions (
        sha256 TEXT PRIMARY KEY,
        viewer_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        ended_at TEXT
    );
CREATE TABLE signin_links (
        sha256 TEXT PRIMARY KEY,
        viewer_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        used_at TEXT
    );
CREATE TABLE device_codes (
        id TEXT PRIMARY KEY,
        device_sha256 TEXT NOT NULL UNIQUE,
        user_sha256 TEXT NOT NULL UNIQUE,
        client_name TEXT NOT NULL,
        peer_ip TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending'
            CHECK (status IN ('pending', 'approved', 'denied', 'issued')),
        viewer_id TEXT,
        grant_id TEXT,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        decided_at TEXT,
        polled_at TEXT
    );
CREATE TABLE remotes (
        id TEXT PRIMARY KEY,
        origin TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL UNIQUE,
        status TEXT NOT NULL CHECK (status IN ('pending', 'connected')),
        token TEXT,
        device_code TEXT,
        user_code TEXT,
        verification_url TEXT,
        sign_in_expires_at TEXT,
        poll_interval_s INTEGER,
        person_public_id TEXT,
        person_name TEXT,
        grant_id TEXT,
        created_at TEXT NOT NULL,
        connected_at TEXT
    );
CREATE TABLE remote_links (
        artifact_id TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        url TEXT NOT NULL,
        pushed_n INTEGER NOT NULL,
        remote_n INTEGER NOT NULL,
        auto INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        pushed_at TEXT NOT NULL, download_only INTEGER NOT NULL DEFAULT 0, sync_cursor TEXT, threads_since TEXT,
        PRIMARY KEY (artifact_id, remote_id)
    );
CREATE TABLE remote_assets (
        remote_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        asset_id TEXT NOT NULL,
        url TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (remote_id, remote_artifact_id, asset_id)
    );
CREATE TABLE push_approvals (
        question_id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        upto INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        used_at TEXT
    );
CREATE TABLE publish_keys (
        grant_id TEXT NOT NULL,
        key TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        n INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (grant_id, key)
    );
CREATE TABLE IF NOT EXISTS "inbox_items" (
        seq INTEGER PRIMARY KEY,
        id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL CHECK (kind IN ('reply', 'version', 'published', 'question', 'finished',
                                           'approval', 'connection', 'ended', 'push')),
        key TEXT NOT NULL UNIQUE,
        recipient TEXT,
        artifact_id TEXT,
        thread_id TEXT,
        comment_id TEXT,
        version_n INTEGER,
        question_id TEXT,
        session_id TEXT,
        harness TEXT,
        request_id TEXT,
        grant_id TEXT,
        detail_json TEXT,
        created_at TEXT NOT NULL,
        read_at TEXT
    );
INSERT INTO inbox_items VALUES(1,'01M5CEQFJAK51Q4F4K0BZ6PBXS','published','published:f34mv7wr3752',NULL,'f34mv7wr3752',NULL,NULL,1,NULL,'01M5CEQFCNTHGNZ10XVMXTR18P','app',NULL,NULL,NULL,'2026-10-20T10:46:47.114Z',NULL);
CREATE TABLE github_identities (
        github_id INTEGER PRIMARY KEY,
        viewer_id TEXT NOT NULL UNIQUE,
        login TEXT NOT NULL,
        name TEXT,
        avatar_url TEXT,
        created_at TEXT NOT NULL,
        signed_in_at TEXT NOT NULL,
        blocked_at TEXT
    );
CREATE TABLE artifact_shares (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
        viewer_id TEXT,
        github_login TEXT CHECK (github_login IS NULL OR github_login = lower(github_login)),
        access TEXT NOT NULL CHECK (access IN ('viewer', 'commenter', 'editor')),
        created_at TEXT NOT NULL,
        created_by TEXT,
        CHECK (viewer_id IS NOT NULL OR github_login IS NOT NULL)
    );
CREATE TABLE remote_versions (
        remote_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        local_n INTEGER NOT NULL,
        remote_n INTEGER NOT NULL,
        PRIMARY KEY (remote_id, remote_artifact_id, remote_n)
    );
CREATE TABLE linked_threads (
        thread_id TEXT PRIMARY KEY,
        remote_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        remote_thread_id TEXT,
        origin TEXT NOT NULL CHECK (origin IN ('remote', 'local')),
        remote_version INTEGER,
        twin INTEGER NOT NULL DEFAULT 1,
        synced_status TEXT NOT NULL DEFAULT 'open',
        remote_name TEXT NOT NULL,
        remote_origin TEXT NOT NULL,
        unlinked_at TEXT,
        created_at TEXT NOT NULL
    );
CREATE TABLE linked_comments (
        comment_id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL,
        remote_id TEXT NOT NULL,
        remote_comment_id TEXT,
        idem_key TEXT NOT NULL UNIQUE,
        origin TEXT NOT NULL CHECK (origin IN ('remote', 'local')),
        author_kind TEXT,
        author_public_id TEXT,
        author_name TEXT,
        created_at TEXT NOT NULL
    );
CREATE TABLE linked_outbox (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        remote_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('thread', 'comment', 'resolve', 'reopen')),
        comment_id TEXT,
        idem_key TEXT NOT NULL UNIQUE,
        agent_name TEXT,
        agent_harness TEXT,
        attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        failed_at TEXT,
        created_at TEXT NOT NULL
    );
CREATE TABLE comment_keys (
        grant_id TEXT NOT NULL,
        key TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        comment_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (grant_id, key)
    );
CREATE TABLE linked_questions (
        question_id TEXT PRIMARY KEY,
        remote_id TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        remote_artifact_id TEXT NOT NULL,
        remote_question_id TEXT,
        idem_key TEXT NOT NULL UNIQUE,
        attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        failed_at TEXT,
        answered_there INTEGER NOT NULL DEFAULT 0,
        done_at TEXT,
        remote_name TEXT NOT NULL,
        remote_origin TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
CREATE TABLE question_keys (
        grant_id TEXT NOT NULL,
        key TEXT NOT NULL,
        question_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (grant_id, key)
    );
PRAGMA writable_schema=ON;
INSERT INTO sqlite_schema(type,name,tbl_name,rootpage,sql)VALUES('table','inbox_fts','inbox_fts',0,'CREATE VIRTUAL TABLE inbox_fts USING fts5(
        text, content='''', contentless_delete=1, prefix=''1 2 3'',
        tokenize=''unicode61 remove_diacritics 2'')');
INSERT INTO sqlite_sequence VALUES('audit_events',16);
CREATE TRIGGER db_read_grants_on_delete AFTER UPDATE OF deleted_at ON artifacts
    WHEN NEW.deleted_at IS NOT NULL
    BEGIN
        DELETE FROM db_read_grants WHERE reader_id = NEW.id OR source_id = NEW.id;
    END;
CREATE TRIGGER net_grants_on_delete AFTER UPDATE OF deleted_at ON artifacts
    WHEN NEW.deleted_at IS NOT NULL
    BEGIN
        DELETE FROM net_grants WHERE artifact_id = NEW.id;
    END;
CREATE TRIGGER sample_grants_on_delete AFTER UPDATE OF deleted_at ON artifacts
    WHEN NEW.deleted_at IS NOT NULL
    BEGIN
        DELETE FROM sample_grants WHERE artifact_id = NEW.id;
        DELETE FROM sample_spend WHERE artifact_id = NEW.id;
    END;
CREATE INDEX assets_by_artifact ON assets(artifact_id);
CREATE INDEX watches_by_artifact ON watches(artifact_id);
CREATE INDEX threads_by_artifact ON threads(artifact_id, created_at, id);
CREATE UNIQUE INDEX feedback_comment_target ON feedback(comment_id, target_session_id)
        WHERE target_session_id IS NOT NULL;
CREATE INDEX feedback_by_target ON feedback(target_session_id, delivered_at);
CREATE INDEX feedback_by_thread ON feedback(thread_id, created_at, id);
CREATE UNIQUE INDEX viewers_public_id ON viewers(public_id);
CREATE INDEX docs_by_collection ON docs(artifact_id, collection, path);
CREATE INDEX viewer_seen_by_age ON viewer_seen(viewer_id, updated_at);
CREATE INDEX comments_by_author ON comments(author_public_id);
CREATE INDEX mentions_by_viewer ON mentions(public_id);
CREATE UNIQUE INDEX sessions_by_handle ON sessions(agent_handle);
CREATE INDEX batch_threads_by_thread ON batch_threads(thread_id);
CREATE UNIQUE INDEX viewers_one_owner ON viewers(owner) WHERE owner = 1;
CREATE INDEX live_pending_by_thread ON live_pending(thread_id);
CREATE INDEX live_watches_by_origin ON live_watches(origin, path);
CREATE INDEX live_picks_by_time ON live_picks(created_at);
CREATE INDEX extension_credentials_by_extension
        ON extension_credentials(extension_id, created_at);
CREATE INDEX live_picks_by_thread ON live_picks(thread_id);
CREATE INDEX live_sites_by_site ON live_sites(site, origin);
CREATE INDEX thread_moves_by_thread ON thread_moves(thread_id, created_at, id);
CREATE INDEX questions_by_status ON questions(status, created_at, id);
CREATE INDEX questions_by_session ON questions(session_id, status);
CREATE INDEX questions_by_artifact ON questions(artifact_id, status)
        WHERE artifact_id IS NOT NULL;
CREATE UNIQUE INDEX questions_by_tool_use ON questions(session_id, tool_use_id)
        WHERE tool_use_id IS NOT NULL;
CREATE INDEX questions_closed ON questions(closed_at, id) WHERE status <> 'open';
CREATE INDEX comments_by_thread ON comments(thread_id, created_at, id, author_public_id, via_session_id);
CREATE INDEX version_threads_by_thread ON version_threads(thread_id, created_at, version_n);
CREATE INDEX versions_by_session ON versions(artifact_id, session_id, created_at);
CREATE INDEX audit_events_artifact ON audit_events(artifact_id, seq);
CREATE INDEX audit_events_artifact2 ON audit_events(artifact2_id, seq) WHERE artifact2_id IS NOT NULL;
CREATE INDEX audit_events_session ON audit_events(session_id, seq);
CREATE INDEX audit_events_call ON audit_events(call_id) WHERE call_id IS NOT NULL;
CREATE INDEX audit_events_at ON audit_events(at);
CREATE INDEX db_read_grants_by_source ON db_read_grants(source_id);
CREATE INDEX artifacts_hidden ON artifacts(hidden, created_at) WHERE hidden = 1;
CREATE INDEX viewer_cookies_by_viewer ON viewer_cookies(viewer_id);
CREATE UNIQUE INDEX grants_browser ON grants(browser_sha256) WHERE browser_sha256 IS NOT NULL;
CREATE UNIQUE INDEX grants_live_key ON grants(client_key) WHERE status IN ('pending', 'approved');
CREATE INDEX grants_by_viewer ON grants(viewer_id, status);
CREATE INDEX grants_pending ON grants(created_at) WHERE status = 'pending';
CREATE INDEX grant_tokens_by_grant ON grant_tokens(grant_id);
CREATE UNIQUE INDEX session_elevations_key ON session_elevations(session_id, COALESCE(granter, ''));
CREATE INDEX session_elevations_by_granter ON session_elevations(granter) WHERE granter IS NOT NULL;
CREATE UNIQUE INDEX sessions_live_name ON sessions(name COLLATE NOCASE)
        WHERE ended_at IS NULL AND name IS NOT NULL;
CREATE INDEX sessions_by_grant ON sessions(grant_id, harness, parent_pid) WHERE grant_id IS NOT NULL;
CREATE UNIQUE INDEX sessions_harness_id ON sessions(harness, harness_session_id, COALESCE(grant_id, ''))
        WHERE harness_session_id IS NOT NULL AND ended_at IS NULL;
CREATE UNIQUE INDEX approvals_live ON approvals(
        COALESCE(approver, ''), author, scope, COALESCE(artifact_id, ''), COALESCE(thread_id, ''))
        WHERE revoked_at IS NULL;
CREATE INDEX approvals_by_approver ON approvals(COALESCE(approver, ''), author) WHERE revoked_at IS NULL;
CREATE UNIQUE INDEX approval_requests_open ON approval_requests(
        COALESCE(approver, ''), author, artifact_id, COALESCE(thread_id, ''))
        WHERE status = 'open';
CREATE INDEX approval_requests_by_session ON approval_requests(session_id, status);
CREATE INDEX approval_requests_by_target ON approval_requests(
        COALESCE(approver, ''), artifact_id, author, status);
CREATE INDEX approval_requests_by_thread ON approval_requests(thread_id, status);
CREATE INDEX viewers_people ON viewers(public_id) WHERE person = 1;
CREATE INDEX questions_open_by_person ON questions(COALESCE(person, ''), created_at, id)
        WHERE status = 'open';
CREATE INDEX questions_closed_by_person ON questions(COALESCE(person, ''), closed_at, id)
        WHERE status <> 'open';
CREATE INDEX browser_sessions_by_viewer ON browser_sessions(viewer_id) WHERE ended_at IS NULL;
CREATE INDEX device_codes_by_expiry ON device_codes(expires_at);
CREATE INDEX inbox_unread ON inbox_items(COALESCE(recipient, ''), seq) WHERE read_at IS NULL;
CREATE INDEX inbox_by_recipient ON inbox_items(COALESCE(recipient, ''), seq);
CREATE INDEX inbox_by_artifact ON inbox_items(COALESCE(recipient, ''), artifact_id, seq);
CREATE INDEX inbox_by_kind ON inbox_items(COALESCE(recipient, ''), kind, seq);
CREATE INDEX inbox_by_harness ON inbox_items(COALESCE(recipient, ''), harness, seq);
CREATE INDEX inbox_by_session ON inbox_items(COALESCE(recipient, ''), session_id, seq);
CREATE INDEX inbox_by_created ON inbox_items(created_at, seq);
CREATE INDEX inbox_by_thread ON inbox_items(thread_id) WHERE thread_id IS NOT NULL;
CREATE INDEX inbox_by_question ON inbox_items(question_id) WHERE question_id IS NOT NULL;
CREATE INDEX inbox_by_request ON inbox_items(request_id) WHERE request_id IS NOT NULL;
CREATE UNIQUE INDEX artifact_shares_by_viewer ON artifact_shares(artifact_id, viewer_id)
        WHERE viewer_id IS NOT NULL;
CREATE UNIQUE INDEX artifact_shares_pending ON artifact_shares(artifact_id, github_login)
        WHERE viewer_id IS NULL;
CREATE INDEX artifact_shares_of_viewer ON artifact_shares(viewer_id) WHERE viewer_id IS NOT NULL;
CREATE INDEX artifact_shares_of_login ON artifact_shares(github_login) WHERE viewer_id IS NULL;
CREATE INDEX remote_versions_by_local ON remote_versions(remote_id, artifact_id, local_n);
CREATE UNIQUE INDEX linked_threads_by_remote ON linked_threads(remote_id, remote_thread_id)
        WHERE remote_thread_id IS NOT NULL;
CREATE INDEX linked_threads_by_artifact ON linked_threads(artifact_id, remote_id);
CREATE UNIQUE INDEX linked_comments_by_remote ON linked_comments(remote_id, remote_comment_id)
        WHERE remote_comment_id IS NOT NULL;
CREATE INDEX linked_comments_by_thread ON linked_comments(thread_id);
CREATE INDEX linked_outbox_by_thread ON linked_outbox(thread_id, seq);
CREATE INDEX linked_questions_live ON linked_questions(remote_id, question_id)
        WHERE done_at IS NULL;
CREATE INDEX question_keys_by_question ON question_keys(question_id, grant_id);
PRAGMA writable_schema=OFF;
COMMIT;
