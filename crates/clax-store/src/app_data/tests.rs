//! The artifact store's tests: the document calls (through [`Docs`], as
//! the instance once made them), its move from the instance, its folds and
//! its wire.

use super::shim::Docs;
use super::*;
use crate::Store;
use crate::docs::{FilterOp, MAX_DEPTH, MAX_DOC_BYTES, MAX_IN_VALUES, parse_where};
use crate::test_util::DAEMON;
use crate::test_util::{artifact_with_caps, store};
use clax_core::ArtifactId;
use clax_core::db::Level;
use serde_json::json;

const A: &str = "u_00000000000000000000aa";
const B: &str = "u_00000000000000000000bb";

fn who(level: Level, viewer: Option<&str>) -> Caller {
    Caller::new(level, viewer.map(str::to_string))
}
fn admin() -> Caller {
    who(Level::Admin, None)
}
fn page() -> Pin {
    Pin {
        if_version: None,
        lww: true,
    }
}
fn pinned(v: u64) -> Pin {
    Pin {
        if_version: Some(v),
        lww: false,
    }
}

#[test]
fn set_get_update_delete_round_trip() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({"db": {}}));
    let w = st
        .doc_set(
            DAEMON,
            &id,
            "tasks/t1",
            json!({"title": "Ship", "meta": {"a": 1}}),
            Pin::default(),
            &admin(),
        )
        .unwrap();
    assert!(w.created);
    assert_eq!(w.doc.as_ref().unwrap().version, 1);
    assert_eq!(
        w.change,
        Some(DocChange {
            path: "tasks/t1".into(),
            version: Some(1),
            private_to: None,
            read_level: Level::View,
            self_read: None,
        })
    );
    let d = st.doc_get(&id, "tasks/t1", &admin()).unwrap().unwrap();
    assert_eq!((d.id.as_str(), d.collection.as_str()), ("t1", "tasks"));
    let w = st
        .doc_update(
            DAEMON,
            &id,
            "tasks/t1",
            json!({"meta": {"b": 2}}),
            pinned(1),
            &admin(),
        )
        .unwrap();
    assert_eq!(
        w.doc.unwrap().data,
        json!({"title": "Ship", "meta": {"a": 1, "b": 2}})
    );
    let w = st
        .doc_delete(DAEMON, &id, "tasks/t1", pinned(2), &admin())
        .unwrap();
    assert!(w.deleted);
    assert_eq!(w.change.unwrap().version, None);
    assert_eq!(st.doc_get(&id, "tasks/t1", &admin()).unwrap(), None);
    assert!(
        !st.doc_delete(DAEMON, &id, "tasks/t1", Pin::default(), &admin())
            .unwrap()
            .deleted,
        "deleting nothing succeeds"
    );
}

#[test]
fn existing_documents_need_a_pin_unless_last_writer_wins() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({"db": {}}));
    st.doc_set(
        DAEMON,
        &id,
        "tasks/t1",
        json!({"n": 1}),
        Pin::default(),
        &admin(),
    )
    .unwrap();
    match st.doc_set(
        DAEMON,
        &id,
        "tasks/t1",
        json!({"n": 2}),
        Pin::default(),
        &admin(),
    ) {
        Err(CoreError::DocPinRequired { path, current }) => {
            assert_eq!((path.as_str(), current), ("tasks/t1", 1))
        }
        other => panic!("{other:?}"),
    }
    match st.doc_set(
        DAEMON,
        &id,
        "tasks/t1",
        json!({"n": 2}),
        pinned(7),
        &admin(),
    ) {
        Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, Some(1)),
        other => panic!("{other:?}"),
    }
    match st.doc_set(DAEMON, &id, "tasks/none", json!({}), pinned(1), &admin()) {
        Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, None),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        st.doc_set(DAEMON, &id, "tasks/t1", json!({"n": 3}), page(), &admin())
            .unwrap()
            .doc
            .unwrap()
            .version,
        2
    );
}

#[test]
fn update_merges_nested_objects_and_removes_marked_fields() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    st.doc_set(
        DAEMON,
        &id,
        "c/d",
        json!({"a": {"x": 1, "y": 2}, "b": [1, 2], "c": "keep"}),
        page(),
        &admin(),
    )
    .unwrap();
    let d = st.doc_update(DAEMON, &id, "c/d", json!({"a": {"y": {"__delete__": true}, "z": 3}, "b": [3], "n": {"m": {"__delete__": true}}}), page(), &admin())
        .unwrap().doc.unwrap();
    assert_eq!(
        d.data,
        json!({"a": {"x": 1, "z": 3}, "b": [3], "c": "keep", "n": {}})
    );
    match st.doc_update(DAEMON, &id, "c/missing", json!({"a": 1}), page(), &admin()) {
        Err(CoreError::Invalid {
            code: "invalid_argument",
            message,
        }) => assert_eq!(message, "c/missing does not exist; use set to create it"),
        other => panic!("update needs an existing document: {other:?}"),
    }
    let batch = vec![BatchWrite {
        path: "c/missing".into(),
        op: BatchOp::Update(json!({"a": 1})),
        if_version: None,
    }];
    match st.doc_batch(DAEMON, &id, batch, true, &admin()) {
        Err(CoreError::InBatch { op: 0, error, .. }) => assert!(
            matches!(
                *error,
                CoreError::Invalid {
                    code: "invalid_argument",
                    ..
                }
            ),
            "a batched update needs an existing document too: {error:?}"
        ),
        other => panic!("{other:?}"),
    }
    match st.doc_update(
        DAEMON,
        &id,
        "c/d",
        json!({"__delete__": true}),
        page(),
        &admin(),
    ) {
        Err(CoreError::Invalid {
            code: "invalid_argument",
            message,
        }) => assert!(message.contains("use delete"), "{message}"),
        other => panic!("a whole-body marker is refused: {other:?}"),
    }
    assert_eq!(st.doc_get(&id, "c/missing", &admin()).unwrap(), None);
    assert!(
        matches!(
            st.doc_update(
                DAEMON,
                &id,
                "c/d",
                json!({"b": [{"__delete__": true}]}),
                page(),
                &admin()
            ),
            Err(CoreError::Invalid {
                code: "invalid_argument",
                ..
            })
        ),
        "no markers inside arrays"
    );
}

#[test]
fn set_refuses_markers_non_objects_deep_and_oversized_bodies() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    let mut deep = json!(1);
    for _ in 0..MAX_DEPTH {
        deep = json!({"x": deep});
    }
    for body in [
        json!([1]),
        json!("s"),
        json!({"a": {"__delete__": true}}),
        deep,
        json!({"s": "x".repeat(MAX_DOC_BYTES)}),
    ] {
        assert!(matches!(
            st.doc_set(DAEMON, &id, "c/d", body, page(), &admin()),
            Err(CoreError::Invalid {
                code: "invalid_argument",
                ..
            })
        ));
    }
    assert!(
        matches!(
            st.doc_set(DAEMON, &id, "c", json!({}), page(), &admin()),
            Err(CoreError::Invalid {
                code: "invalid_argument",
                ..
            })
        ),
        "odd path"
    );
}

#[test]
fn refused_writes_read_as_missing_and_view_writes_nothing() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(
        &st,
        json!({"db": {"rules": [{"path": "", "write": "admin"}]}}),
    );
    assert!(matches!(
        st.doc_set(
            DAEMON,
            &id,
            "t/1",
            json!({}),
            page(),
            &who(Level::Interact, Some(A))
        ),
        Err(CoreError::DocNotFound { .. })
    ));
    st.doc_set(DAEMON, &id, "t/1", json!({}), page(), &admin())
        .unwrap();
    assert!(
        st.doc_get(&id, "t/1", &who(Level::View, None))
            .unwrap()
            .is_some(),
        "view reads shared documents"
    );
    let open = artifact_with_caps(&st, json!({}));
    assert!(matches!(
        st.doc_set(
            DAEMON,
            &open,
            "t/1",
            json!({}),
            page(),
            &who(Level::View, Some(A))
        ),
        Err(CoreError::DocNotFound { .. })
    ));
}

#[test]
fn private_subtrees_are_invisible_to_siblings_and_the_owner() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({"db": {}}));
    let mine = format!("data/users/{A}/profile");
    let w = st
        .doc_set(
            DAEMON,
            &id,
            &mine,
            json!({"pick": 3}),
            page(),
            &who(Level::Interact, Some(A)),
        )
        .unwrap();
    assert_eq!(w.change.unwrap().private_to.as_deref(), Some(A));
    for c in [
        who(Level::Interact, Some(B)),
        who(Level::Admin, None),
        who(Level::Owner, Some(B)),
    ] {
        assert_eq!(st.doc_get(&id, &mine, &c).unwrap(), None, "{c:?}");
        let q = DocQuery {
            collection: format!("data/users/{A}"),
            ..Default::default()
        };
        assert!(st.doc_query(&id, &q, &c).unwrap().0.is_empty(), "{c:?}");
        assert!(
            matches!(
                st.doc_set(DAEMON, &id, &mine, json!({}), page(), &c),
                Err(CoreError::DocNotFound { .. })
            ),
            "{c:?}"
        );
    }
    let q = DocQuery {
        collection: format!("data/users/{A}"),
        ..Default::default()
    };
    assert_eq!(
        st.doc_query(&id, &q, &who(Level::Interact, Some(A)))
            .unwrap()
            .0
            .len(),
        1
    );
}

#[test]
fn an_ordered_query_without_a_limit_returns_every_match() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    let n = MAX_LIMIT + 5;
    for start in (0..n).step_by(MAX_BATCH) {
        let chunk = (start..n.min(start + MAX_BATCH))
            .map(|i| BatchWrite {
                path: format!("log/e{i:05}"),
                op: BatchOp::Set(json!({"at": n - i})),
                if_version: None,
            })
            .collect();
        st.doc_batch(DAEMON, &id, chunk, true, &admin()).unwrap();
    }
    let q = DocQuery {
        collection: "log".into(),
        order_by: Some("at".into()),
        ..Default::default()
    };
    let (docs, next) = st.doc_query(&id, &q, &admin()).unwrap();
    assert_eq!((docs.len(), next), (n, None));
    assert_eq!(docs[0].data["at"], 1);
    // Unordered, the page is the default size and a cursor follows.
    let (docs, next) = st
        .doc_query(
            &id,
            &DocQuery {
                order_by: None,
                ..q
            },
            &admin(),
        )
        .unwrap();
    assert_eq!((docs.len(), next.is_some()), (DEFAULT_LIMIT, true));
}

#[test]
fn an_ordered_query_without_a_limit_has_a_byte_budget() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    // Bodies just under the document cap; enough of them to pass the budget.
    let big = "x".repeat(MAX_DOC_BYTES - 64);
    let n = MAX_UNLIMITED_QUERY_BYTES / (MAX_DOC_BYTES - 64) + 1;
    for i in 0..n {
        st.doc_set(
            DAEMON,
            &id,
            &format!("big/d{i:04}"),
            json!({"at": i, "s": big}),
            page(),
            &admin(),
        )
        .unwrap();
    }
    let q = DocQuery {
        collection: "big".into(),
        order_by: Some("at".into()),
        ..Default::default()
    };
    let e = st.doc_query(&id, &q, &admin()).unwrap_err();
    assert!(
        matches!(
            &e,
            CoreError::Invalid {
                code: "resource_exhausted",
                ..
            }
        ),
        "{e:?}"
    );
    // With a limit, or narrowed below the budget, it answers.
    let limited = DocQuery {
        limit: Some(3),
        ..q.clone()
    };
    assert_eq!(st.doc_query(&id, &limited, &admin()).unwrap().0.len(), 3);
    let narrowed = DocQuery {
        filters: parse_where(&json!([["at", "<", 10]])).unwrap(),
        ..q
    };
    assert_eq!(st.doc_query(&id, &narrowed, &admin()).unwrap().0.len(), 10);
}

#[test]
fn query_filters_orders_limits_and_pages() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    for (k, v) in [
        ("a", json!({"n": 3, "tags": ["x"], "s": "open"})),
        ("b", json!({"n": 1, "s": "done"})),
        ("c", json!({"n": 2, "tags": ["x", "y"], "s": "open"})),
        ("d", json!({"s": "open"})),
    ] {
        st.doc_set(DAEMON, &id, &format!("tasks/{k}"), v, page(), &admin())
            .unwrap();
    }
    st.doc_set(
        DAEMON,
        &id,
        "tasks/a/sub/z",
        json!({"n": 0}),
        page(),
        &admin(),
    )
    .unwrap();
    let ids = |q: DocQuery| {
        st.doc_query(&id, &q, &admin())
            .unwrap()
            .0
            .into_iter()
            .map(|d| d.id)
            .collect::<Vec<_>>()
    };
    let base = || DocQuery {
        collection: "tasks".into(),
        ..Default::default()
    };
    assert_eq!(
        ids(base()),
        ["a", "b", "c", "d"],
        "document ID order; nested collections excluded"
    );
    assert_eq!(
        ids(DocQuery {
            filters: parse_where(&json!([["s", "==", "open"], ["n", ">=", 2]])).unwrap(),
            ..base()
        }),
        ["a", "c"]
    );
    assert_eq!(
        ids(DocQuery {
            filters: parse_where(&json!([["tags", "array-contains", "y"]])).unwrap(),
            ..base()
        }),
        ["c"]
    );
    assert_eq!(
        ids(DocQuery {
            filters: parse_where(&json!([["n", "in", [1, 3]]])).unwrap(),
            ..base()
        }),
        ["a", "b"]
    );
    assert_eq!(
        ids(DocQuery {
            filters: parse_where(&json!([["n", "not-in", [1]]])).unwrap(),
            ..base()
        }),
        ["a", "c"],
        "a missing field matches nothing"
    );
    assert_eq!(
        ids(DocQuery {
            filters: parse_where(&json!([["n", "<", "z"]])).unwrap(),
            ..base()
        }),
        Vec::<String>::new(),
        "ranges compare within one type"
    );
    assert_eq!(
        ids(DocQuery {
            order_by: Some("n".into()),
            ..base()
        }),
        ["b", "c", "a", "d"],
        "missing sorts last"
    );
    assert_eq!(
        ids(DocQuery {
            order_by: Some("n".into()),
            descending: true,
            limit: Some(2),
            ..base()
        }),
        ["a", "c"]
    );
    let (page1, next) = st
        .doc_query(
            &id,
            &DocQuery {
                limit: Some(3),
                ..base()
            },
            &admin(),
        )
        .unwrap();
    assert_eq!((page1.len(), next.as_deref()), (3, Some("c")));
    let (page2, next) = st
        .doc_query(
            &id,
            &DocQuery {
                limit: Some(3),
                cursor: Some("c".into()),
                ..base()
            },
            &admin(),
        )
        .unwrap();
    assert_eq!((page2.len(), next), (1, None));
    for bad in [
        json!([["n", "~", 1]]),
        json!([["n", "in", 1]]),
        json!([["n", "=="]]),
        json!(vec![json!(["n", "==", 1]); MAX_FILTERS + 1]),
    ] {
        assert!(parse_where(&bad).is_err(), "{bad}");
    }
    assert!(
        st.doc_query(
            &id,
            &DocQuery {
                limit: Some(MAX_LIMIT + 1),
                ..base()
            },
            &admin()
        )
        .is_err()
    );
    assert!(
        st.doc_query(
            &id,
            &DocQuery {
                order_by: Some("n".into()),
                cursor: Some("a".into()),
                ..base()
            },
            &admin()
        )
        .is_err()
    );
}

#[test]
fn batch_is_atomic_and_names_the_failing_path() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    st.doc_set(DAEMON, &id, "t/1", json!({"n": 1}), page(), &admin())
        .unwrap();
    let writes = vec![
        BatchWrite {
            path: "t/2".into(),
            op: BatchOp::Set(json!({"n": 2})),
            if_version: None,
        },
        BatchWrite {
            path: "t/1".into(),
            op: BatchOp::Update(json!({"n": 9})),
            if_version: Some(5),
        },
    ];
    match st.doc_batch(DAEMON, &id, writes, false, &admin()) {
        Err(CoreError::InBatch { op, path, error }) => {
            assert_eq!((op, path.as_str()), (1, "t/1"));
            assert!(
                matches!(
                    *error,
                    CoreError::DocConflict {
                        current: Some(1),
                        ..
                    }
                ),
                "{error:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        st.doc_get(&id, "t/2", &admin()).unwrap(),
        None,
        "nothing landed"
    );
    let ok = st
        .doc_batch(
            DAEMON,
            &id,
            vec![
                BatchWrite {
                    path: "t/2".into(),
                    op: BatchOp::Set(json!({"n": 2})),
                    if_version: None,
                },
                BatchWrite {
                    path: "t/1".into(),
                    op: BatchOp::Delete,
                    if_version: Some(1),
                },
            ],
            false,
            &admin(),
        )
        .unwrap();
    assert_eq!((ok[0].created, ok[1].deleted), (true, true));
    let dup = vec![
        BatchWrite {
            path: "t/3".into(),
            op: BatchOp::Delete,
            if_version: None,
        },
        BatchWrite {
            path: "t/3".into(),
            op: BatchOp::Delete,
            if_version: None,
        },
    ];
    match st.doc_batch(DAEMON, &id, dup, false, &admin()) {
        Err(CoreError::InBatch { op: 1, path, error }) => {
            assert_eq!(path, "t/3");
            assert!(matches!(
                *error,
                CoreError::Invalid {
                    code: "invalid_argument",
                    ..
                }
            ));
        }
        other => panic!("{other:?}"),
    }
    let many = (0..=MAX_BATCH)
        .map(|i| BatchWrite {
            path: format!("t/x{i}"),
            op: BatchOp::Delete,
            if_version: None,
        })
        .collect();
    assert!(matches!(
        st.doc_batch(DAEMON, &id, many, false, &admin()),
        Err(CoreError::Invalid {
            code: "invalid_argument",
            ..
        })
    ));
}

#[test]
fn str_replace_requires_one_occurrence_unless_replace_all() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    st.doc_set(
        DAEMON,
        &id,
        "p/1",
        json!({"html": "a-b-a", "n": 1}),
        page(),
        &admin(),
    )
    .unwrap();
    let r = |old: &str, all: bool, field: &str| StrReplace {
        field: field.into(),
        old_str: old.into(),
        new_str: "Z".into(),
        replace_all: all,
    };
    assert!(matches!(
        st.doc_str_replace(
            DAEMON,
            &id,
            "p/1",
            r("a", false, "html"),
            pinned(1),
            &admin()
        ),
        Err(CoreError::Invalid {
            code: "old_str_not_unique",
            ..
        })
    ));
    assert!(matches!(
        st.doc_str_replace(
            DAEMON,
            &id,
            "p/1",
            r("q", false, "html"),
            pinned(1),
            &admin()
        ),
        Err(CoreError::Invalid {
            code: "old_str_not_found",
            ..
        })
    ));
    assert!(matches!(
        st.doc_str_replace(DAEMON, &id, "p/1", r("a", false, "n"), pinned(1), &admin()),
        Err(CoreError::Invalid {
            code: "invalid_argument",
            ..
        })
    ));
    let d = st
        .doc_str_replace(
            DAEMON,
            &id,
            "p/1",
            r("b", false, "html"),
            pinned(1),
            &admin(),
        )
        .unwrap()
        .doc
        .unwrap();
    assert_eq!((d.data["html"].as_str(), d.version), (Some("a-Z-a"), 2));
    let d = st
        .doc_str_replace(
            DAEMON,
            &id,
            "p/1",
            r("a", true, "html"),
            pinned(2),
            &admin(),
        )
        .unwrap()
        .doc
        .unwrap();
    assert_eq!(d.data["html"], "Z-Z-Z");
}

#[test]
fn creating_past_the_document_cap_is_a_quota_error() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    st.with_data(&id, |c| {
        let tx = c.unchecked_transaction()?;
        for i in 0..MAX_DOCS {
            tx.execute(
                "INSERT INTO docs (path, collection, json, version, updated_at) VALUES (?1, 'f', '{}', 1, 'x')",
                params![format!("f/{i}")],
            )?;
        }
        tx.commit()?;
        Ok(())
    }).unwrap();
    assert!(matches!(
        st.doc_set(DAEMON, &id, "f/new", json!({}), page(), &admin()),
        Err(CoreError::Invalid {
            code: "quota_exceeded",
            ..
        })
    ));
    assert!(
        st.doc_set(DAEMON, &id, "f/0", json!({"n": 1}), page(), &admin())
            .is_ok(),
        "existing documents stay writable"
    );
}

#[test]
fn acquire_grants_one_holder_until_the_lease_lapses() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    let acq = |holder: &str, data: Option<Value>| {
        st.doc_acquire(
            DAEMON,
            &id,
            "locks/editor",
            Acquire {
                holder: holder.into(),
                ttl_ms: Some(5),
                data,
            },
            &admin(),
        )
        .unwrap()
    };
    let (a, change) = acq("tab-a", Some(json!({"by": "tab-a"})));
    assert!(
        a.acquired
            && a.holder.as_deref() == Some("tab-a")
            && a.version == Some(1)
            && change.is_some()
    );
    let (b, _) = acq("tab-b", None);
    assert!(
        !b.acquired && b.holder.is_none() && b.expires_at.is_some(),
        "busy reveals only the expiry"
    );
    let (renew, _) = acq("tab-a", None);
    assert!(
        renew.acquired && renew.version == Some(1),
        "renewal without data writes nothing"
    );
    st.with_data(&id, |c| {
        Ok(c.execute(
            "UPDATE leases SET expires_at = '2000-01-01T00:00:00.000Z'",
            [],
        )?)
    })
    .unwrap();
    assert!(acq("tab-b", None).0.acquired, "a lapsed lease is free");
    let (none, _) = st
        .doc_acquire(
            DAEMON,
            &id,
            "locks/empty",
            Acquire {
                holder: "x".into(),
                ttl_ms: None,
                data: None,
            },
            &admin(),
        )
        .unwrap();
    assert!(
        none.acquired && none.version.is_none(),
        "no data: no document is created"
    );
    assert!(
        st.doc_acquire(
            DAEMON,
            &id,
            "locks/e",
            Acquire {
                holder: String::new(),
                ttl_ms: None,
                data: None
            },
            &admin()
        )
        .is_err()
    );
}

#[test]
fn deleting_an_artifact_removes_its_data_file() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    st.doc_set(DAEMON, &id, "t/1", json!({}), page(), &admin())
        .unwrap();
    st.doc_acquire(
        DAEMON,
        &id,
        "t/1",
        Acquire {
            holder: "h".into(),
            ttl_ms: None,
            data: None,
        },
        &admin(),
    )
    .unwrap();
    let file = st.home().app_data_path(&id);
    assert!(file.exists());
    st.delete_artifact(DAEMON, &id).unwrap();
    // The home's files go with the artifact (`FsContent::delete_artifact`);
    // a store on one connection keeps its content elsewhere.
    let files = !crate::test_util::on_one_connection();
    assert!(!files || !file.exists(), "its store goes with it");
    assert!(matches!(
        st.doc_get(&id, "t/1", &admin()),
        Err(CoreError::NotFound)
    ));
    assert!(
        !files || !file.exists(),
        "a call on a deleted artifact makes no store"
    );
}

#[test]
fn versions_rise_across_the_artifact_and_survive_deletes() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    let v = |w: Written| w.doc.unwrap().version;
    let a1 = v(st
        .doc_set(DAEMON, &id, "t/a", json!({"n": 1}), page(), &admin())
        .unwrap());
    let b1 = v(st
        .doc_set(DAEMON, &id, "t/b", json!({"n": 1}), page(), &admin())
        .unwrap());
    assert!(b1 > a1, "two documents get distinct increasing versions");
    st.doc_delete(DAEMON, &id, "t/a", pinned(a1), &admin())
        .unwrap();
    let a2 = v(st
        .doc_set(DAEMON, &id, "t/a", json!({"n": 2}), page(), &admin())
        .unwrap());
    assert!(
        a2 > b1 && a2 > a1,
        "a recreated document never reuses a version: {a1} {b1} {a2}"
    );
    match st.doc_set(DAEMON, &id, "t/a", json!({"n": 3}), pinned(a1), &admin()) {
        Err(CoreError::DocConflict { current, .. }) => assert_eq!(current, Some(a2)),
        other => panic!("a pin from before the delete must conflict: {other:?}"),
    }
    let batch = st
        .doc_batch(
            DAEMON,
            &id,
            vec![BatchWrite {
                path: "t/c".into(),
                op: BatchOp::Set(json!({})),
                if_version: None,
            }],
            true,
            &admin(),
        )
        .unwrap();
    let c1 = batch[0].doc.as_ref().unwrap().version;
    assert!(c1 > a2);
    let (acq, _) = st
        .doc_acquire(
            DAEMON,
            &id,
            "t/d",
            Acquire {
                holder: "h".into(),
                ttl_ms: None,
                data: Some(json!({"x": 1})),
            },
            &admin(),
        )
        .unwrap();
    assert!(acq.version.unwrap() > c1);
    let other = artifact_with_caps(&st, json!({}));
    assert_eq!(
        v(
            st.doc_set(DAEMON, &other, "t/a", json!({}), page(), &admin())
                .unwrap()
        ),
        1,
        "each artifact has its own sequence"
    );
}

#[test]
fn paths_are_checked_before_bodies() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    let path_err = |r: Result<Written>| match r {
        Err(CoreError::Invalid { message, .. }) => {
            assert!(message.contains("document path"), "{message}")
        }
        other => panic!("{other:?}"),
    };
    path_err(st.doc_update(DAEMON, &id, "odd", json!([1]), page(), &admin()));
    let r = StrReplace {
        field: "f".into(),
        old_str: String::new(),
        new_str: "x".into(),
        replace_all: false,
    };
    path_err(st.doc_str_replace(DAEMON, &id, "odd", r, page(), &admin()));
    let writes = vec![
        BatchWrite {
            path: "t/1".into(),
            op: BatchOp::Delete,
            if_version: None,
        },
        BatchWrite {
            path: "odd".into(),
            op: BatchOp::Update(json!([1])),
            if_version: None,
        },
    ];
    match st.doc_batch(DAEMON, &id, writes, true, &admin()) {
        Err(CoreError::InBatch { op: 1, path, error }) => {
            assert_eq!(path, "odd");
            assert!(error.to_string().contains("document path"), "{error}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn batch_failures_name_the_op_for_refusals_and_bodies() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(
        &st,
        json!({"db": {"rules": [{"path": "locked", "write": "admin"}]}}),
    );
    let writes = vec![
        BatchWrite {
            path: "t/1".into(),
            op: BatchOp::Set(json!({})),
            if_version: None,
        },
        BatchWrite {
            path: "locked/x".into(),
            op: BatchOp::Set(json!({})),
            if_version: None,
        },
    ];
    match st.doc_batch(DAEMON, &id, writes, true, &who(Level::Interact, Some(A))) {
        Err(CoreError::InBatch { op: 1, path, error }) => {
            assert_eq!(path, "locked/x");
            assert!(matches!(&*error, CoreError::DocNotFound { path } if path == "locked/x"));
        }
        other => panic!("{other:?}"),
    }
    let writes = vec![
        BatchWrite {
            path: "t/1".into(),
            op: BatchOp::Set(json!({})),
            if_version: None,
        },
        BatchWrite {
            path: "t/2".into(),
            op: BatchOp::Update(json!([1])),
            if_version: None,
        },
    ];
    assert!(matches!(
        st.doc_batch(DAEMON, &id, writes, true, &admin()),
        Err(CoreError::InBatch { op: 1, .. })
    ));
    let writes = vec![BatchWrite {
        path: "t/9".into(),
        op: BatchOp::Set(json!("s")),
        if_version: None,
    }];
    assert!(matches!(
        st.doc_batch(DAEMON, &id, writes, true, &admin()),
        Err(CoreError::InBatch { op: 0, .. })
    ));
    assert_eq!(st.count(&id, "docs"), 0);
}

#[test]
fn built_queries_are_held_to_the_in_limit() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    let q = |value: Value| DocQuery {
        collection: "t".into(),
        filters: vec![Filter {
            field: "n".into(),
            op: FilterOp::In,
            value,
        }],
        ..Default::default()
    };
    assert!(
        st.doc_query(&id, &q(json!(vec![1; MAX_IN_VALUES + 1])), &admin())
            .is_err()
    );
    assert!(st.doc_query(&id, &q(json!(1)), &admin()).is_err());
    assert!(
        st.doc_query(&id, &q(json!(vec![1; MAX_IN_VALUES])), &admin())
            .is_ok()
    );
}

#[test]
fn leases_are_pruned_and_capped_per_artifact() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    let acq = |path: &str| {
        st.doc_acquire(
            DAEMON,
            &id,
            path,
            Acquire {
                holder: "h".into(),
                ttl_ms: None,
                data: None,
            },
            &admin(),
        )
    };
    for i in 0..MAX_LEASES {
        assert!(acq(&format!("l/{i}")).unwrap().0.acquired);
    }
    match acq("l/extra") {
        Err(CoreError::Invalid {
            code: "resource_exhausted",
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    assert!(
        acq("l/0").unwrap().0.acquired,
        "renewing a held lease is not a new one"
    );
    st.with_data(&id, |c| {
        Ok(c.execute(
            "UPDATE leases SET expires_at = '2000-01-01T00:00:00.000Z' WHERE path <> 'l/0'",
            [],
        )?)
    })
    .unwrap();
    assert!(
        acq("l/extra").unwrap().0.acquired,
        "lapsed leases do not count"
    );
    assert_eq!(st.count(&id, "leases"), 2, "lapsed leases are pruned");
}

/// Documents per reader (spec §8.2, §8.3): Sam and Mia, two connected
/// people, on a store with a `db` artifact.
mod per_reader {
    use super::*;
    use crate::people::tests::{approved, store};
    use crate::visibility::Visibility;
    use clax_core::visibility::Reader;

    struct Two {
        _dir: tempfile::TempDir,
        st: Store,
        sam: String,
        mia: String,
        aid: ArtifactId,
    }

    fn two() -> Two {
        let (dir, _clock, st) = store();
        let (g, _, _) = approved(&st, "Sam", [1; 32]);
        let sam = st.viewer_of_grant(&g).unwrap().public_id;
        let (g, _, _) = approved(&st, "Mia", [2; 32]);
        let mia = st.viewer_of_grant(&g).unwrap().public_id;
        let aid = artifact_with_caps(&st, json!({"db": {}}));
        Two {
            _dir: dir,
            st,
            sam,
            mia,
            aid,
        }
    }

    fn as_(p: &str) -> Caller {
        Caller::new(Level::Admin, Some(p.to_string()))
    }

    impl Two {
        fn docs(&self) -> Docs<'_> {
            Docs::of(&self.st)
        }
        fn set(&self, by: &str, path: &str, data: Value) -> Written {
            self.docs()
                .doc_set(DAEMON, &self.aid, path, data, page(), &as_(by))
                .unwrap()
        }
        fn view(&self, p: &str) -> Visibility {
            Visibility::for_reader(
                &self.st,
                &Reader::Person(p.to_string()),
                self.st.visibility_generation(),
            )
            .unwrap()
        }
        fn query(&self, reader: &str, q: DocQuery) -> DocPage {
            self.docs()
                .doc_query_visible(&self.aid, &q, &as_(reader), Some(&self.view(reader)))
                .unwrap()
        }
    }

    fn filters(w: Value) -> Vec<Filter> {
        parse_where(&w).unwrap()
    }

    #[test]
    fn where_runs_over_visible_documents_only() {
        let t = two();
        t.set(&t.mia, "c/1", json!({"secret": "aaa"}));
        t.set(&t.mia, "c/2", json!({"secret": "zzz"}));
        t.set(&t.sam, "c/3", json!({"secret": "mmm"}));
        for w in [
            json!([["secret", "<", "b"]]),
            json!([["secret", ">", "y"]]),
            json!([["secret", "==", "aaa"]]),
            json!([["secret", "!=", "mmm"]]),
            json!([]),
        ] {
            for order_by in [None, Some("secret".to_string())] {
                for descending in [false, true] {
                    let page = t.query(
                        &t.sam,
                        DocQuery {
                            collection: "c".into(),
                            filters: filters(w.clone()),
                            order_by: order_by.clone(),
                            descending,
                            ..DocQuery::default()
                        },
                    );
                    assert!(
                        page.docs.iter().all(|d| d.path == "c/3"),
                        "{w}: the result never depends on Mia's values"
                    );
                    assert_eq!(page.withheld, Some(2), "{w}: counted whatever the filter");
                    assert_eq!(page.next_cursor, None, "{w}");
                }
            }
        }
        // Mia's own reader sees hers and counts Sam's.
        let page = t.query(
            &t.mia,
            DocQuery {
                collection: "c".into(),
                ..DocQuery::default()
            },
        );
        let paths: Vec<&str> = page.docs.iter().map(|d| d.path.as_str()).collect();
        assert_eq!((paths, page.withheld), (vec!["c/1", "c/2"], Some(1)));
        // Without a view (a person's browser), every document as before.
        let page = t
            .docs()
            .doc_query_visible(
                &t.aid,
                &DocQuery {
                    collection: "c".into(),
                    ..DocQuery::default()
                },
                &as_(&t.sam),
                None,
            )
            .unwrap();
        assert_eq!((page.docs.len(), page.withheld), (3, None));
    }

    #[test]
    fn the_count_leaves_out_what_the_rules_hide() {
        let t = two();
        // Mia's private subtree: the rules hide it from Sam, so the
        // count never tells him it holds anything.
        let mine = format!("data/users/{}/profile", t.mia);
        t.set(&t.mia, &mine, json!({"pick": 3}));
        let got = t.query(
            &t.sam,
            DocQuery {
                collection: format!("data/users/{}", t.mia),
                ..DocQuery::default()
            },
        );
        assert_eq!((got.docs.len(), got.withheld), (0, Some(0)));
        assert!(
            matches!(
                t.docs().doc_str_replace_visible(
                    DAEMON,
                    &t.aid,
                    &mine,
                    StrReplace {
                        field: "pick".into(),
                        old_str: "3".into(),
                        new_str: "4".into(),
                        replace_all: false
                    },
                    page(),
                    &as_(&t.sam),
                    Some(&t.view(&t.sam)),
                ),
                Err(CoreError::DocNotFound { .. })
            ),
            "a document the rules hide is missing, never withheld"
        );
    }

    #[test]
    fn pages_of_visible_documents_have_no_gaps() {
        let t = two();
        t.set(&t.sam, "c/a", json!({"n": 1}));
        t.set(&t.mia, "c/b", json!({"n": 2}));
        t.set(&t.mia, "c/c", json!({"n": 3}));
        t.set(&t.sam, "c/d", json!({"n": 4}));
        let page = |cursor: Option<&str>| {
            t.query(
                &t.sam,
                DocQuery {
                    collection: "c".into(),
                    limit: Some(1),
                    cursor: cursor.map(str::to_string),
                    ..DocQuery::default()
                },
            )
        };
        let first = page(None);
        assert_eq!(first.docs[0].path, "c/a");
        assert_eq!(first.next_cursor.as_deref(), Some("a"));
        let second = page(Some("a"));
        assert_eq!(second.docs[0].path, "c/d");
        assert_eq!(second.next_cursor, None, "nothing withheld paginates");
        assert_eq!((first.withheld, second.withheld), (Some(2), Some(2)));
        // An ordered page of one: Sam's highest, never Mia's.
        let top = t.query(
            &t.sam,
            DocQuery {
                collection: "c".into(),
                order_by: Some("n".into()),
                descending: true,
                limit: Some(1),
                ..DocQuery::default()
            },
        );
        assert_eq!(top.docs[0].path, "c/d");
    }

    #[test]
    fn str_replace_on_a_withheld_document_is_refused_before_it_is_evaluated() {
        let t = two();
        t.set(&t.mia, "c/1", json!({"text": "Mia's secret"}));
        let before = t
            .docs()
            .doc_get(&t.aid, "c/1", &as_(&t.mia))
            .unwrap()
            .unwrap();
        let vis = t.view(&t.sam);
        for (old, all) in [
            ("secret", false),
            ("absent", false),
            ("s", false),
            ("s", true),
        ] {
            for pin in [page(), pinned(before.version), pinned(999)] {
                let r = t.docs().doc_str_replace_visible(
                    DAEMON,
                    &t.aid,
                    "c/1",
                    StrReplace {
                        field: "text".into(),
                        old_str: old.into(),
                        new_str: "x".into(),
                        replace_all: all,
                    },
                    pin,
                    &as_(&t.sam),
                    Some(&vis),
                );
                match r {
                    Err(CoreError::DocWithheld { path, authors }) => {
                        assert_eq!(path, "c/1");
                        assert_eq!(authors, vec![Author::Person(t.mia.clone())]);
                    }
                    other => panic!("{old}: {other:?}"),
                }
            }
        }
        // Nothing was written, and a missing document is missing.
        assert_eq!(
            t.docs()
                .doc_get(&t.aid, "c/1", &as_(&t.mia))
                .unwrap()
                .unwrap(),
            before
        );
        assert!(matches!(
            t.docs().doc_str_replace_visible(
                DAEMON,
                &t.aid,
                "c/none",
                StrReplace {
                    field: "text".into(),
                    old_str: "a".into(),
                    new_str: "b".into(),
                    replace_all: false
                },
                page(),
                &as_(&t.sam),
                Some(&vis),
            ),
            Err(CoreError::DocNotFound { .. })
        ));
        // Sam's own document is his to edit.
        t.set(&t.sam, "c/2", json!({"text": "Sam's"}));
        let w = t
            .docs()
            .doc_str_replace_visible(
                DAEMON,
                &t.aid,
                "c/2",
                StrReplace {
                    field: "text".into(),
                    old_str: "Sam".into(),
                    new_str: "S".into(),
                    replace_all: false,
                },
                page(),
                &as_(&t.sam),
                Some(&vis),
            )
            .unwrap();
        assert_eq!(w.doc.unwrap().data, json!({"text": "S's"}));
    }

    fn pad_to_cap(base: &Value) -> Value {
        let n = MAX_DOC_BYTES - serde_json::to_vec(base).unwrap().len();
        json!({"pad": "a".repeat(n)})
    }

    #[test]
    fn a_merge_on_a_withheld_document_is_refused_before_it_is_evaluated() {
        let t = two();
        t.set(&t.mia, "c/t", json!({"flag": true, "text": "hello"}));
        t.set(&t.mia, "c/f", json!({"flag": false, "text": "hello"}));
        let vis = t.view(&t.sam);
        // A patch that fits beside {flag: true} but not {flag: false}.
        let patch = pad_to_cap(&json!({"flag": true, "text": "hello", "pad": ""}));
        let before: Vec<Doc> = ["c/t", "c/f"]
            .iter()
            .map(|p| t.docs().doc_get(&t.aid, p, &as_(&t.mia)).unwrap().unwrap())
            .collect();
        let refused = |r: Result<Written>, what: &str| match r {
            Err(CoreError::DocWithheld { authors, .. }) => {
                assert_eq!(authors, vec![Author::Person(t.mia.clone())], "{what}")
            }
            other => panic!("{what}: {other:?}"),
        };
        for p in ["c/t", "c/f"] {
            for pin in [page(), pinned(1), pinned(999)] {
                refused(
                    t.docs().doc_update_visible(
                        DAEMON,
                        &t.aid,
                        p,
                        patch.clone(),
                        pin,
                        &as_(&t.sam),
                        Some(&vis),
                    ),
                    p,
                );
            }
            match t.docs().doc_batch_visible(
                DAEMON,
                &t.aid,
                vec![
                    BatchWrite {
                        path: "c/sam".into(),
                        op: BatchOp::Set(json!({"a": 1})),
                        if_version: None,
                    },
                    BatchWrite {
                        path: p.into(),
                        op: BatchOp::Update(patch.clone()),
                        if_version: None,
                    },
                ],
                true,
                &as_(&t.sam),
                Some(&vis),
            ) {
                Err(CoreError::InBatch { op: 1, error, .. }) => {
                    assert!(matches!(*error, CoreError::DocWithheld { .. }), "{error:?}")
                }
                other => panic!("batch {p}: {other:?}"),
            }
            match t.docs().doc_acquire_visible(
                DAEMON,
                &t.aid,
                p,
                Acquire {
                    holder: "h".into(),
                    ttl_ms: None,
                    data: Some(patch.clone()),
                },
                &as_(&t.sam),
                Some(&vis),
            ) {
                Err(CoreError::DocWithheld { .. }) => {}
                other => panic!("acquire {p}: {other:?}"),
            }
        }
        // Nothing was written, not even the batch's other write.
        let after: Vec<Doc> = ["c/t", "c/f"]
            .iter()
            .map(|p| t.docs().doc_get(&t.aid, p, &as_(&t.mia)).unwrap().unwrap())
            .collect();
        assert_eq!(after, before);
        assert_eq!(
            t.docs().doc_get(&t.aid, "c/sam", &as_(&t.sam)).unwrap(),
            None
        );
        assert_eq!(t.docs().count(&t.aid, "leases"), 0);
        // Blind writes stay allowed: set and delete in a batch, and a
        // lease without data.
        let w = t
            .docs()
            .doc_batch_visible(
                DAEMON,
                &t.aid,
                vec![
                    BatchWrite {
                        path: "c/t".into(),
                        op: BatchOp::Set(json!({"x": 1})),
                        if_version: None,
                    },
                    BatchWrite {
                        path: "c/f".into(),
                        op: BatchOp::Delete,
                        if_version: None,
                    },
                ],
                true,
                &as_(&t.sam),
                Some(&vis),
            )
            .unwrap();
        assert!(w[1].deleted);
        let (a, _) = t
            .docs()
            .doc_acquire_visible(
                DAEMON,
                &t.aid,
                "c/t",
                Acquire {
                    holder: "h".into(),
                    ttl_ms: None,
                    data: None,
                },
                &as_(&t.sam),
                Some(&vis),
            )
            .unwrap();
        assert!(a.acquired);
        // Sam's own document merges as before.
        t.set(&t.sam, "c/mine", json!({"a": 1}));
        let w = t
            .docs()
            .doc_update_visible(
                DAEMON,
                &t.aid,
                "c/mine",
                json!({"b": 2}),
                page(),
                &as_(&t.sam),
                Some(&vis),
            )
            .unwrap();
        assert_eq!(w.doc.unwrap().data, json!({"a": 1, "b": 2}));
    }

    #[test]
    fn a_lease_on_a_withheld_document_never_confirms_its_holder() {
        let t = two();
        t.set(&t.mia, "c/1", json!({"a": 1}));
        let acquire = |by: &str, holder: &str, vis: Option<&Visibility>| {
            t.docs()
                .doc_acquire_visible(
                    DAEMON,
                    &t.aid,
                    "c/1",
                    Acquire {
                        holder: holder.into(),
                        ttl_ms: Some(600_000),
                        data: None,
                    },
                    &as_(by),
                    vis,
                )
                .unwrap()
                .0
        };
        let mine = acquire(&t.mia, "mia-laptop", Some(&t.view(&t.mia)));
        assert!(mine.acquired);
        let vis = t.view(&t.sam);
        let miss = acquire(&t.sam, "guess", Some(&vis));
        let hit = acquire(&t.sam, "mia-laptop", Some(&vis));
        assert_eq!(miss, hit, "a guessed holder answers as any other");
        assert_eq!((hit.acquired, hit.holder), (false, None));
        assert_eq!(hit.expires_at, mine.expires_at, "her lease is untouched");
        // Mia renews her own lease as before.
        assert!(acquire(&t.mia, "mia-laptop", Some(&t.view(&t.mia))).acquired);
    }

    #[test]
    fn withheld_documents_before_the_cursor_are_counted() {
        let t = two();
        t.set(&t.mia, "c/a", json!({"n": 1}));
        t.set(&t.sam, "c/b", json!({"n": 2}));
        t.set(&t.mia, "c/c", json!({"n": 3}));
        t.set(&t.sam, "c/d", json!({"n": 4}));
        t.set(&t.mia, "c/e", json!({"n": 5}));
        for cursor in [None, Some("b"), Some("c"), Some("d"), Some("zz")] {
            let got = t.query(
                &t.sam,
                DocQuery {
                    collection: "c".into(),
                    cursor: cursor.map(str::to_string),
                    ..DocQuery::default()
                },
            );
            assert_eq!(got.withheld, Some(3), "{cursor:?}");
            assert!(
                got.docs.iter().all(|d| d.id == "b" || d.id == "d"),
                "{cursor:?}"
            );
        }
    }

    #[test]
    fn the_byte_budget_counts_visible_bytes_only() {
        let t = two();
        t.set(&t.mia, "c/big", json!({"text": "m".repeat(1000)}));
        t.set(&t.sam, "c/small", json!({"text": "s"}));
        let q = DocQuery {
            collection: "c".into(),
            order_by: Some("text".into()),
            ..DocQuery::default()
        };
        let got = t
            .docs()
            .doc_query_within(&t.aid, &q, &as_(&t.sam), Some(&t.view(&t.sam)), 500)
            .unwrap();
        assert_eq!((got.docs.len(), got.withheld), (1, Some(1)));
        // Mia's own reader is held to the budget by her bytes.
        assert!(matches!(
            t.docs()
                .doc_query_within(&t.aid, &q, &as_(&t.mia), Some(&t.view(&t.mia)), 500),
            Err(CoreError::Invalid {
                code: "resource_exhausted",
                ..
            })
        ));
    }

    #[test]
    fn a_write_answers_with_the_authors_it_leaves() {
        let t = two();
        let w = t.set(&t.mia, "c/1", json!({"a": 1}));
        assert_eq!(w.authors, vec![Author::Person(t.mia.clone())]);
        // Sam's blind write over Mia's document joins her; it never
        // makes it his.
        let w = t.set(&t.sam, "c/1", json!({"b": 2}));
        assert_eq!(
            w.authors,
            vec![Author::Person(t.mia.clone()), Author::Person(t.sam.clone())]
        );
        let (doc, authors) = t
            .docs()
            .doc_get_authored(&t.aid, "c/1", &as_(&t.sam))
            .unwrap()
            .unwrap();
        assert_eq!((doc.data, authors), (json!({"b": 2}), w.authors.clone()));
        let w = t
            .docs()
            .doc_delete(DAEMON, &t.aid, "c/1", page(), &as_(&t.sam))
            .unwrap();
        assert!(w.deleted && w.authors.is_empty());
        let w = t.set(&t.sam, "c/1", json!({"c": 3}));
        assert_eq!(w.authors, vec![Author::Person(t.sam.clone())]);
    }
}

/// Rows of artifact `id` as an instance before the move held them: two
/// documents, one of them also A's, a lease, and the sequence at `seq`.
fn seed_instance(st: &Store, id: &ArtifactId, seq: i64) {
    st.with_tx(|tx| {
        tx.execute_batch(&format!(
            "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
                VALUES ('{id}', 't/a', 't', '{{\"n\":1}}', 3, 'x'), ('{id}', 't/b', 't', '{{\"n\":2}}', 5, 'x');
             INSERT INTO doc_authors (artifact_id, path, author) VALUES ('{id}', 't/b', '*owner'), ('{id}', 't/b', '{A}');
             INSERT INTO leases (artifact_id, path, holder, expires_at) VALUES ('{id}', 't/a', 'h', '2999-01-01T00:00:00.000Z');
             UPDATE artifacts SET doc_seq = {seq} WHERE id = '{id}';"
        ))?;
        Ok(())
    })
    .unwrap();
}

fn instance_rows(st: &Store, id: &ArtifactId) -> i64 {
    st.with_read(|c| {
        Ok(c.query_row(
            "SELECT (SELECT COUNT(*) FROM docs WHERE artifact_id = ?1)
                  + (SELECT COUNT(*) FROM doc_authors WHERE artifact_id = ?1)
                  + (SELECT COUNT(*) FROM leases WHERE artifact_id = ?1)",
            params![id.as_str()],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

#[test]
fn the_first_open_moves_an_artifacts_documents_once() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({"db": {}}));
    seed_instance(&st, &id, 9);
    assert_eq!(instance_rows(&st, &id), 5);
    assert!(!st.app_data_state(&id).unwrap().moved);
    let (b, authors) = st.doc_get_authored(&id, "t/b", &admin()).unwrap().unwrap();
    assert_eq!((b.version, b.data.clone()), (5, json!({"n": 2})));
    assert_eq!(authors, vec![Author::Owner, Author::Person(A.into())]);
    assert_eq!(instance_rows(&st, &id), 0, "the instance forgot them");
    assert!(st.app_data_state(&id).unwrap().moved);
    let busy = st
        .doc_acquire(
            DAEMON,
            &id,
            "t/a",
            Acquire {
                holder: "other".into(),
                ttl_ms: None,
                data: None,
            },
            &admin(),
        )
        .unwrap()
        .0;
    assert!(!busy.acquired, "the lease moved with its document");
    // A second open moves nothing: rows the instance gains now (as a build
    // from before the move would write them) stay there.
    seed_instance(&st, &id, 9);
    let q = DocQuery {
        collection: "t".into(),
        ..Default::default()
    };
    assert_eq!(st.doc_query(&id, &q, &admin()).unwrap().0.len(), 2);
    assert_eq!(instance_rows(&st, &id), 5);
    // Versions keep increasing past the old sequence.
    let w = st
        .doc_set(DAEMON, &id, "t/c", json!({}), page(), &admin())
        .unwrap();
    assert_eq!(w.doc.unwrap().version, 10);
}

#[test]
fn importing_the_same_rows_again_changes_nothing() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    seed_instance(&st, &id, 9);
    let rows = st.export_app_data(&id).unwrap();
    let ad = AppData::open(&st.home().app_data_path(&id), st.clock()).unwrap();
    ad.import(rows.clone()).unwrap();
    let w = ad
        .doc_set(
            &WriteCtx::of(DAEMON),
            &Rules::default(),
            "t/a",
            json!({"n": 7}),
            page(),
            &admin(),
        )
        .unwrap();
    // A move that stopped before the instance forgot its rows imports
    // them again: what the store holds since stays.
    ad.import(rows).unwrap();
    let a = ad
        .doc_get(&Rules::default(), "t/a", &admin())
        .unwrap()
        .unwrap();
    assert_eq!(
        (a.data, a.version),
        (json!({"n": 7}), w.doc.unwrap().version)
    );
    assert_eq!(ad.doc_authors("t/b").unwrap().len(), 2);
}

#[test]
fn a_fold_moves_authorship_and_private_documents_once() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({"db": {}}));
    let ad = AppData::open(&st.home().app_data_path(&id), st.clock()).unwrap();
    ad.import(Exported {
        artifact_id: id.to_string(),
        ..Exported::default()
    })
    .unwrap();
    let rules = Rules::from_capabilities(&json!({"db": {}})).unwrap();
    let w = WriteCtx::of(DAEMON);
    let mine = format!("data/users/{A}/pick");
    let a = who(Level::Interact, Some(A));
    ad.doc_set(&w, &rules, &mine, json!({"p": 1}), page(), &a)
        .unwrap();
    ad.doc_set(&w, &rules, "t/1", json!({}), page(), &a)
        .unwrap();
    let fold = Fold {
        seq: 4,
        old: A.into(),
        new: B.into(),
        into_owner: false,
        audit: AuditEnv::of(DAEMON),
    };
    assert_eq!(ad.fold(std::slice::from_ref(&fold)).unwrap(), 4);
    assert_eq!(
        ad.doc_authors("t/1").unwrap(),
        vec![Author::Person(B.into())]
    );
    let moved = format!("data/users/{B}/pick");
    let d = ad
        .doc_get(&rules, &moved, &who(Level::Interact, Some(B)))
        .unwrap()
        .unwrap();
    assert_eq!(d.version, 3, "a move takes the next version");
    assert_eq!(
        ad.doc_authors(&moved).unwrap(),
        vec![Author::Person(B.into())]
    );
    let kinds: Vec<String> = ad
        .audit_since(0)
        .unwrap()
        .into_iter()
        .map(|r| r.kind)
        .collect();
    assert_eq!(kinds, ["doc.write", "doc.write", "doc.move"]);
    // Applied once: the same fold again, at or below the mark, does nothing.
    assert_eq!(ad.fold(&[fold]).unwrap(), 4);
    assert_eq!(ad.audit_since(0).unwrap().len(), 3);
    let into_owner = Fold {
        seq: 5,
        old: B.into(),
        new: "u_00000000000000000000cc".into(),
        into_owner: true,
        audit: AuditEnv::of(DAEMON),
    };
    assert_eq!(ad.fold(&[into_owner]).unwrap(), 5);
    assert_eq!(ad.doc_authors("t/1").unwrap(), vec![Author::Owner]);
    assert_eq!(ad.fold_mark().unwrap(), 5);
}

#[test]
fn doc_write_records_carry_the_writers_envelope_and_the_artifact() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    st.doc_set(
        DAEMON,
        &id,
        "notes/n1",
        json!({"text": "secret"}),
        page(),
        &admin(),
    )
    .unwrap();
    let rows = st.data(&id).unwrap().audit_since(0).unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(
        (r.kind.as_str(), r.ids.artifact.as_deref()),
        ("doc.write", Some(id.as_str()))
    );
    let body: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(
        (&body["op"], &body["v"], &body["collection"]),
        (&json!("set"), &json!(1), &json!("notes"))
    );
    assert!(!r.body.contains("secret"));
    assert!(st.data(&id).unwrap().audit_since(r.seq).unwrap().is_empty());
}

#[test]
fn an_op_and_its_reply_cross_the_wire_and_errors_come_back() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let caps = json!({"db": {"rules": [{"path": "locked", "write": "admin"}]}});
    let id = artifact_with_caps(&st, caps.clone());
    let ad = st.data(&id).unwrap();
    let send = |op: AppDataOp| -> AppDataReply {
        let op: AppDataOp = serde_json::from_str(&serde_json::to_string(&op).unwrap()).unwrap();
        let reply = ad.apply(op);
        serde_json::from_str(&serde_json::to_string(&reply).unwrap()).unwrap()
    };
    let caller = WireCaller::from(&who(Level::Interact, Some(A)));
    let set = |data: Value| AppDataOp::Set {
        ctx: WriteCtx::of(DAEMON),
        caps: caps.clone(),
        path: "t/1".into(),
        data,
        pin: Pin::default(),
        caller: caller.clone(),
    };
    let w = send(set(json!({"n": 1}))).into_written().unwrap();
    assert_eq!(w.authors, vec![Author::Person(A.into())]);
    let got = send(AppDataOp::Get {
        caps: caps.clone(),
        path: "t/1".into(),
        caller: caller.clone(),
    })
    .into_doc()
    .unwrap()
    .unwrap();
    assert_eq!(got.0.data, json!({"n": 1}));
    match send(set(json!({}))).into_written() {
        Err(CoreError::DocPinRequired { path, current }) => {
            assert_eq!((path.as_str(), current), ("t/1", 1))
        }
        other => panic!("{other:?}"),
    }
    match send(AppDataOp::Batch {
        ctx: WriteCtx::of(DAEMON),
        caps: caps.clone(),
        writes: vec![BatchWrite {
            path: "locked/x".into(),
            op: BatchOp::Set(json!({})),
            if_version: None,
        }],
        lww: true,
        caller: caller.clone(),
        view: None,
    })
    .into_batch()
    {
        Err(CoreError::InBatch { op: 0, error, .. }) => {
            assert!(matches!(*error, CoreError::DocNotFound { .. }), "{error:?}")
        }
        other => panic!("{other:?}"),
    }
    match send(AppDataOp::Query {
        caps,
        query: DocQuery {
            collection: "t".into(),
            limit: Some(MAX_LIMIT + 1),
            ..Default::default()
        },
        caller,
        view: None,
    })
    .into_page()
    {
        Err(CoreError::Invalid {
            code: "invalid_argument",
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(
        send(AppDataOp::Authors).into_strings().unwrap(),
        vec![A.to_string()]
    );
    assert_eq!(
        send(AppDataOp::AuthoredBy {
            author: A.into(),
            is_owner: false
        })
        .into_strings()
        .unwrap(),
        vec!["t/1".to_string()]
    );
    assert_eq!(
        send(AppDataOp::AuditSince { seq: 0 })
            .into_audit()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_document_view_withholds_as_the_readers_visibility_does() {
    let owner = "u_00000000000000000000ow";
    let view = DocView {
        reader: Some(A.into()),
        owner: Some(owner.into()),
        approved_keys: [UNCONNECTED.to_string()].into_iter().collect(),
        people: Some([B.to_string()].into_iter().collect()),
    };
    let p = |x: &str| Author::Person(x.into());
    assert!(view.sees(&[p(A), Author::System, Author::Unconnected]));
    assert!(
        view.sees(&[p("u_00000000000000000000zz")]),
        "an unconnected viewer is approved"
    );
    assert!(
        !view.sees(&[p(B)]),
        "a person is not, by the unconnected approval"
    );
    assert_eq!(
        view.withheld_authors(&[p(owner), p(B)]),
        vec![Author::Owner, p(B)]
    );
    let owner_reads = DocView {
        reader: None,
        ..view
    };
    assert!(owner_reads.sees(&[Author::Owner, p(owner)]));
}

/// A move that imported its rows but failed to forget them in the
/// instance, a viewer fold, and the move again: the second import adds
/// nothing, and the fold applies to the store once, from its own mark.
#[test]
fn a_reimport_after_a_fold_keeps_the_stores_rows_and_the_fold_applies_once() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({"db": {}}));
    let mine = format!("data/users/{A}/p");
    st.with_tx(|tx| {
        tx.execute_batch(&format!(
            "INSERT INTO docs (artifact_id, path, collection, json, version, updated_at)
                VALUES ('{id}', '{mine}', 'data/users/{A}', '{{}}', 3, 'x'), ('{id}', 't/1', 't', '{{}}', 5, 'x');
             INSERT INTO doc_authors (artifact_id, path, author) VALUES ('{id}', '{mine}', '{A}'), ('{id}', 't/1', '{A}');
             UPDATE artifacts SET doc_seq = 5 WHERE id = '{id}';"
        ))?;
        Ok(())
    })
    .unwrap();
    let ad = AppData::open(&st.home().app_data_path(&id), st.clock()).unwrap();
    ad.import(st.export_app_data(&id).unwrap()).unwrap();
    // The instance still holds the rows, and a fold of A into B rewrites
    // them there, as `viewers::fold` does, and logs it.
    st.with_tx(|tx| {
        log_fold(tx, DAEMON, A, B, false)?;
        tx.execute_batch(&format!(
            "UPDATE docs SET path = 'data/users/{B}/p', collection = 'data/users/{B}' WHERE path = '{mine}';
             UPDATE doc_authors SET path = 'data/users/{B}/p' WHERE path = '{mine}';
             UPDATE doc_authors SET author = '{B}' WHERE author = '{A}';"
        ))?;
        Ok(())
    })
    .unwrap();
    ad.import(st.export_app_data(&id).unwrap()).unwrap();
    ad.fold(&st.app_data_folds_after(ad.fold_mark().unwrap()).unwrap())
        .unwrap();
    let paths: Vec<String> = st
        .with_data(&id, |c| {
            Ok(c.prepare("SELECT path FROM docs ORDER BY path")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(paths, [format!("data/users/{B}/p"), "t/1".to_string()]);
    for path in [format!("data/users/{B}/p"), "t/1".to_string()] {
        assert_eq!(
            ad.doc_authors(&path).unwrap(),
            vec![Author::Person(B.into())],
            "{path}"
        );
    }
    assert_eq!(ad.fold_mark().unwrap(), 1);
}

#[test]
fn the_document_sequence_moves_with_every_change_and_only_then() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({}));
    let ad = AppData::open(&st.home().app_data_path(&id), st.clock()).unwrap();
    let seq = |ad: &AppData| match ad.apply(AppDataOp::Seq) {
        AppDataReply::Seq(n) => n,
        r => panic!("{r:?}"),
    };
    assert_eq!(seq(&ad), 0);
    let ctx = WriteCtx::of(DAEMON);
    let rules = Rules::default();
    ad.doc_set(&ctx, &rules, "t/a", json!({"n": 1}), page(), &admin())
        .unwrap();
    let after_write = seq(&ad);
    assert_eq!(after_write, 1);
    // A read, and a delete of nothing, leave it.
    ad.doc_get(&rules, "t/a", &admin()).unwrap();
    ad.doc_delete(&ctx, &rules, "t/none", page(), &admin())
        .unwrap();
    assert_eq!(seq(&ad), after_write);
    ad.doc_delete(&ctx, &rules, "t/a", page(), &admin())
        .unwrap();
    assert_eq!(seq(&ad), after_write + 1);
}

/// A write under an operation ID made again (the instance's record of it
/// lost to a crash, the call retried) answers as the first did and changes
/// nothing; another write under the ID is `key_reused`; a write that failed
/// records nothing, so its retry runs.
#[test]
fn a_write_made_again_under_its_operation_id_lands_once() {
    let (_d, store) = store();
    let st = Docs::of(&store);
    let id = artifact_with_caps(&st, json!({"db": {}}));
    let ad = st.data(&id).unwrap();
    let rules = Rules::from_capabilities(&json!({"db": {}})).unwrap();
    let op = |digest: &str| {
        WriteCtx::of(DAEMON).with_op(OpId {
            id: "job:j1:publish:1".into(),
            digest: digest.into(),
        })
    };
    let writes = || {
        vec![BatchWrite {
            path: "tasks/t1".into(),
            op: BatchOp::Set(json!({"n": 1})),
            if_version: None,
        }]
    };
    let first = ad
        .doc_batch(&op("d1"), &rules, writes(), false, &admin())
        .unwrap();
    let audit = ad.audit_seq().unwrap();
    let again = ad
        .doc_batch(&op("d1"), &rules, writes(), false, &admin())
        .unwrap();
    assert_eq!(again, first, "the first answer");
    assert_eq!(ad.doc_seq().unwrap(), 1, "the sequence moved once");
    assert_eq!(ad.audit_seq().unwrap(), audit, "nothing more is recorded");
    assert!(matches!(
        ad.doc_batch(&op("d2"), &rules, writes(), false, &admin()),
        Err(CoreError::Invalid {
            code: "key_reused",
            ..
        })
    ));
    // A set under another ID; a refused write under a third, then its retry.
    let set = |o: &str, pin: Pin| {
        ad.doc_set(
            &WriteCtx::of(DAEMON).with_op(OpId {
                id: o.into(),
                digest: "x".into(),
            }),
            &rules,
            "tasks/t2",
            json!({"n": 2}),
            pin,
            &admin(),
        )
    };
    assert!(matches!(
        set("bot:r:1:2", pinned(9)),
        Err(CoreError::DocConflict { .. })
    ));
    let w = set("bot:r:1:2", Pin::default()).unwrap();
    assert_eq!(set("bot:r:1:2", Pin::default()).unwrap(), w);
    assert_eq!(ad.doc_seq().unwrap(), 2);
}
