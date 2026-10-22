//! An artifact store's schema versions ([`super::AppData`]), applied in
//! order on open; a list of its own, apart from the instance's
//! (`crate::migrations`).

pub const MIGRATIONS: &[&str] = &[
    // 1: the artifact's documents, leases and document authors, as the
    // instance kept them per artifact (its migrations 7, 8 and 25), the
    // artifact's one document sequence and the last viewer fold applied
    // (`doc_seq`, one row), and its `doc.write` and `doc.move` audit
    // records in the instance's `audit_events` shape (its migration 23).
    // `doc_seq.artifact_id` is the artifact's, set by the first import.
    "CREATE TABLE docs (
        path TEXT PRIMARY KEY,
        collection TEXT NOT NULL,
        json TEXT NOT NULL,
        version INTEGER NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX docs_by_collection ON docs(collection, path);
    CREATE TABLE leases (
        path TEXT PRIMARY KEY,
        holder TEXT NOT NULL,
        expires_at TEXT NOT NULL
    );
    CREATE TABLE doc_authors (
        path TEXT NOT NULL,
        author TEXT NOT NULL,
        PRIMARY KEY (path, author)
    );
    CREATE INDEX doc_authors_by_author ON doc_authors(author);
    CREATE TABLE doc_seq (
        one INTEGER PRIMARY KEY CHECK (one = 1),
        artifact_id TEXT,
        seq INTEGER NOT NULL DEFAULT 0,
        folded INTEGER NOT NULL DEFAULT 0
    );
    INSERT INTO doc_seq (one) VALUES (1);
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
    );",
    // 2: the operation IDs writes carried (`super::OpId`), with the digest
    // of what each asked and its answer: a write made again under an ID
    // answers as the first did and changes nothing, so the instance can
    // record a write after it (a job's step, ClaxBot's call journal) and a
    // retry after a crash between the two applies the write once.
    "CREATE TABLE applied_ops (
        op_id TEXT PRIMARY KEY,
        digest TEXT NOT NULL,
        reply TEXT NOT NULL,
        at TEXT NOT NULL
    );",
];
