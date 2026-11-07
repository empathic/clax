use super::*;
use crate::queues::NewQueue;
use crate::test_util::{DAEMON, store};
use clax_core::queue::ReportersSee;

const READ: &str = "github_pat_READ_0123456789-read";
const WRITE: &str = "github_pat_WRITE_0123456789-rite";
const PEM: &str =
    "-----BEGIN RSA PRIVATE KEY-----\nMIIEsecretsecret\n-----END RSA PRIVATE KEY-----\n";

fn kek() -> Kek {
    Kek::from_bytes([7; 32])
}

fn queue(st: &Store, name: &str) -> String {
    st.create_queue(
        DAEMON,
        None,
        NewQueue {
            name: name.into(),
            origins: vec![],
            reporters_see: ReportersSee::Own,
        },
    )
    .unwrap()
    .id
}

fn fields(access: RepoAccess) -> RepoLinkFields {
    RepoLinkFields {
        repo: "acme/web".into(),
        base: None,
        repo_url: None,
        forge_api_url: None,
        protected_paths: vec![],
        preview_url: "https://pr-{pr}.acme.dev".into(),
        preview_ready_path: None,
        preview_names_commit: None,
        preview_timeout_secs: None,
        preview_poll_secs: None,
        access,
        public: false,
        allow_open_egress: false,
        reviewed_previews: false,
        wake_on_reply: false,
        ci_accepted: true,
        if_rev: None,
    }
}

fn token_fields() -> RepoLinkFields {
    fields(RepoAccess::Token)
}

/// `f`, for a repository anyone may see.
fn public(f: RepoLinkFields) -> RepoLinkFields {
    RepoLinkFields { public: true, ..f }
}

/// Both of `q`'s tokens.
fn both_tokens(st: &Store, q: &str) {
    token(st, q, TokenKind::Write, WRITE).unwrap();
    token(st, q, TokenKind::Read, READ).unwrap();
}

fn app_fields() -> RepoLinkFields {
    fields(RepoAccess::GithubApp {
        installation_id: 42,
        account: "acme".into(),
    })
}

fn set_app(st: &Store) {
    st.set_github_app(
        DAEMON,
        &kek(),
        &GithubAppFields {
            app_id: 7,
            slug: "clax-bot".into(),
            api_url: None,
        },
        Some(PEM),
    )
    .unwrap();
}

fn code(e: CoreError) -> &'static str {
    match e {
        CoreError::Invalid { code, .. } => code,
        other => panic!("{other:?}"),
    }
}

/// The text of every audit event so far.
fn audit(st: &Store) -> String {
    st.events_after(0, 1000)
        .unwrap()
        .into_iter()
        .map(|e| e.body)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Sets `q`'s link as a panel that read it just now does.
fn link(st: &Store, q: &str, f: &RepoLinkFields, by_owner: bool) -> Result<RepoLink> {
    let mut f = f.clone();
    f.if_rev = st.queue_repo(q).unwrap().map(|l| l.rev);
    st.set_queue_repo(DAEMON, q, &f, by_owner)
}

/// Sets `q`'s token as a panel that read the link just now does.
fn token(st: &Store, q: &str, kind: TokenKind, value: &str) -> Result<RepoLink> {
    let rev = st.queue_repo(q)?.map_or(String::new(), |l| l.rev);
    st.set_queue_repo_token(DAEMON, &kek(), q, kind, value, &rev)
}

fn rev(st: &Store, q: &str) -> String {
    st.queue_repo(q).unwrap().unwrap().rev
}

fn tokens(link: &RepoLink) -> (Option<&SecretMeta>, Option<&SecretMeta>) {
    match &link.access {
        LinkAccess::Token {
            read_token,
            write_token,
            ..
        } => (read_token.as_deref(), write_token.as_deref()),
        LinkAccess::GithubApp { .. } => panic!("a token link"),
    }
}

#[test]
fn a_link_takes_the_workspace_targets_defaults() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    assert_eq!(st.queue_repo(&q).unwrap(), None);
    let l = st
        .set_queue_repo(DAEMON, &q, &token_fields(), true)
        .unwrap();
    assert_eq!(l.base, "main");
    assert_eq!(l.repo_url, "https://github.com/acme/web.git");
    assert_eq!(l.forge_api_url, "https://api.github.com");
    assert_eq!(l.preview.ready_path, "/healthz");
    assert!(l.preview.names_commit);
    assert_eq!((l.preview.timeout_secs, l.preview.poll_secs), (1200, 10));
    assert_eq!(l.status, LinkStatus::NeedsToken);
    assert_eq!(st.queue_repo(&q).unwrap(), Some(l));
}

#[test]
fn tokens_read_as_metadata_and_open_only_through_the_target() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    token(&st, &q, TokenKind::Read, READ).unwrap();
    let l = token(&st, &q, TokenKind::Write, WRITE).unwrap();
    assert_eq!(l.status, LinkStatus::Ready);
    let (r, w) = tokens(&l);
    assert_eq!(r.unwrap().last4, "read");
    assert_eq!(w.unwrap().last4, "rite");
    assert_eq!(w.unwrap().host.as_deref(), Some("https://github.com"));

    let t = st
        .open_queue_repo(&kek(), &q, Egress::Held)
        .unwrap()
        .unwrap();
    let RepoCredentials::Token { read, write } = t.credentials else {
        panic!("tokens");
    };
    assert_eq!(read.unwrap().expose(), READ);
    assert_eq!(write.expose(), WRITE);

    assert!(!format!("{l:?}").contains("0123456789"));
    let trail = audit(&st);
    assert!(!trail.contains("0123456789"), "{trail}");
    assert!(trail.contains("\"scope\":\"repo_write\""), "{trail}");
    assert!(trail.contains(&format!("\"queue_id\":\"{q}\"")), "{trail}");

    assert_eq!(
        code(
            st.open_queue_repo(&Kek::from_bytes([8; 32]), &q, Egress::Held)
                .unwrap_err()
        ),
        "kek_mismatch"
    );
}

#[test]
fn a_sealed_token_does_not_open_in_another_queues_slot() {
    let (_d, st) = store();
    let (a, b) = (queue(&st, "a"), queue(&st, "b"));
    for q in [&a, &b] {
        link(&st, q, &public(token_fields()), true).unwrap();
    }
    token(&st, &a, TokenKind::Write, WRITE).unwrap();
    st.with_write(|c| {
        c.execute(
            "INSERT INTO sealed_secrets (scope, person, nonce, sealed, last4, use_own, set_by, set_at, host, forge_host)
             SELECT scope, ?1, nonce, sealed, last4, use_own, set_by, set_at, host, forge_host
               FROM sealed_secrets WHERE person = ?2",
            params![b, a],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        code(st.open_queue_repo(&kek(), &b, Egress::Held).unwrap_err()),
        "kek_mismatch"
    );
    assert!(
        st.open_queue_repo(&kek(), &a, Egress::Held)
            .unwrap()
            .is_some()
    );
    // Nor a queue's write token in its own read slot.
    st.with_write(|c| {
        c.execute(
            "INSERT INTO sealed_secrets (scope, person, nonce, sealed, last4, use_own, set_by, set_at, host, forge_host)
             SELECT 'repo_read', person, nonce, sealed, last4, use_own, set_by, set_at, host, forge_host
               FROM sealed_secrets WHERE person = ?1",
            params![a],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        code(st.open_queue_repo(&kek(), &a, Egress::Held).unwrap_err()),
        "kek_mismatch"
    );
}

#[test]
fn the_model_keys_path_neither_sets_nor_opens_a_workspace_slot() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    set_app(&st);
    for slot in [
        SecretSlot::RepoRead(q.clone()),
        SecretSlot::RepoWrite(q.clone()),
        SecretSlot::GithubApp,
    ] {
        assert_eq!(
            code(st.set_secret(DAEMON, &kek(), &slot, WRITE).unwrap_err()),
            "wrong_slot"
        );
        assert_eq!(
            code(
                st.open_for_use(&kek(), &slot, "https://github.com")
                    .unwrap_err()
            ),
            "wrong_slot"
        );
    }
}

#[test]
fn the_read_and_write_tokens_may_not_be_the_same_value() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    let padded = format!("  {WRITE}\n");
    assert_eq!(
        code(token(&st, &q, TokenKind::Read, &padded).unwrap_err()),
        "same_token"
    );
    token(&st, &q, TokenKind::Read, READ).unwrap();
    assert_eq!(
        code(token(&st, &q, TokenKind::Write, READ).unwrap_err()),
        "same_token"
    );
    // The value in force stays.
    let t = st
        .open_queue_repo(&kek(), &q, Egress::Held)
        .unwrap()
        .unwrap();
    let RepoCredentials::Token { write, .. } = t.credentials else {
        panic!("tokens");
    };
    assert_eq!(write.expose(), WRITE);
}

#[test]
fn a_token_cannot_be_redirected_by_changing_where_the_link_points() {
    for change in [
        (|f: &mut RepoLinkFields| f.repo = "acme/other".into()) as fn(&mut RepoLinkFields),
        |f| {
            f.repo_url = Some("https://evil.example/acme/web.git".into());
            f.forge_api_url = Some("https://evil.example/api".into());
        },
        |f| f.forge_api_url = Some("https://evil.example".into()),
    ] {
        let (_d, st) = store();
        let q = queue(&st, "a");
        link(&st, &q, &token_fields(), false).unwrap();
        token(&st, &q, TokenKind::Read, READ).unwrap();
        token(&st, &q, TokenKind::Write, WRITE).unwrap();
        let mut f = token_fields();
        change(&mut f);
        let l = link(&st, &q, &f, false).unwrap();
        assert_eq!(l.status, LinkStatus::NeedsToken);
        assert_eq!(tokens(&l), (None, None));
        assert!(
            st.open_queue_repo(&kek(), &q, Egress::Held)
                .unwrap()
                .is_none()
        );
        let trail = audit(&st);
        assert!(trail.contains("\"tokens_cleared\":true"), "{trail}");
        assert_eq!(trail.matches("\"op\":\"clear\"").count(), 2, "{trail}");
    }
}

#[test]
fn a_change_that_sends_a_token_nowhere_new_keeps_it() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    both_tokens(&st, &q);
    let mut f = token_fields();
    f.base = Some("develop".into());
    f.preview_url = "https://{pr}.preview.acme.dev".into();
    let l = link(&st, &q, &f, true).unwrap();
    assert_eq!(l.status, LinkStatus::Ready);
    assert_eq!(l.base, "develop");
}

#[test]
fn a_change_to_nothing_records_nothing() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    let before = audit(&st);
    link(&st, &q, &token_fields(), true).unwrap();
    assert_eq!(audit(&st), before);
}

#[test]
fn app_access_needs_an_app_and_goes_to_its_forge_alone() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    assert_eq!(
        code(link(&st, &q, &app_fields(), true).unwrap_err()),
        "no_github_app"
    );
    set_app(&st);
    let mut f = app_fields();
    f.forge_api_url = Some("https://evil.example".into());
    assert_eq!(
        code(link(&st, &q, &f, true).unwrap_err()),
        "forge_not_instance"
    );
    let mut f = app_fields();
    f.repo_url = Some("https://evil.example/acme/web.git".into());
    assert_eq!(
        code(link(&st, &q, &f, true).unwrap_err()),
        "forge_not_instance"
    );
    // Naming the instance's own is fine.
    let mut f = app_fields();
    f.forge_api_url = Some("https://api.github.com/".into());
    f.repo_url = Some("https://github.com/acme/web.git".into());
    let l = link(&st, &q, &f, true).unwrap();
    assert_eq!(l.status, LinkStatus::Ready);
    let t = st
        .open_queue_repo(&kek(), &q, Egress::Held)
        .unwrap()
        .unwrap();
    assert_eq!(t.forge_api_url, GITHUB_API);
    let RepoCredentials::GithubApp {
        app_id,
        installation_id,
        private_key,
    } = t.credentials
    else {
        panic!("the App");
    };
    assert_eq!((app_id, installation_id), (7, 42));
    assert_eq!(private_key.expose(), PEM.trim());
    assert!(!audit(&st).contains("secretsecret"));
}

#[test]
fn a_links_addresses_follow_the_app_to_its_forge() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    link(&st, &q, &app_fields(), true).unwrap();
    st.set_github_app(
        DAEMON,
        &kek(),
        &GithubAppFields {
            app_id: 7,
            slug: "clax-bot".into(),
            api_url: Some("https://ghe.acme.dev/api/v3".into()),
        },
        Some(PEM),
    )
    .unwrap();
    let l = st.queue_repo(&q).unwrap().unwrap();
    assert_eq!(l.forge_api_url, "https://ghe.acme.dev/api/v3");
    assert_eq!(l.repo_url, "https://ghe.acme.dev/acme/web.git");
    let app = st.github_app().unwrap().unwrap();
    assert_eq!(
        app.install_url,
        "https://ghe.acme.dev/apps/clax-bot/installations/new"
    );
}

#[test]
fn an_installation_is_usable_only_once_the_owner_approves_that_repository_for_that_queue() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    let l = link(&st, &q, &public(app_fields()), false).unwrap();
    assert_eq!(l.status, LinkStatus::NeedsApproval);
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .is_none()
    );
    let links = st.repo_links().unwrap();
    assert_eq!(links.len(), 1);
    assert!(!links[0].approved);

    // Only what the owner read.
    assert_eq!(
        code(
            st.approve_queue_repo(DAEMON, &q, "an-older-rev", true)
                .unwrap_err()
        ),
        "link_changed"
    );
    st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), true)
        .unwrap();
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::Ready
    );

    // A change of anything else keeps it.
    let mut f = public(app_fields());
    f.base = Some("develop".into());
    let l = link(&st, &q, &f, false).unwrap();
    assert_eq!(l.status, LinkStatus::Ready);

    // Another repository or installation voids it.
    let mut f = public(app_fields());
    f.repo = "victim/secret".into();
    let l = link(&st, &q, &f, false).unwrap();
    assert_eq!(l.status, LinkStatus::NeedsApproval);
    st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), true)
        .unwrap();
    let f = public(fields(RepoAccess::GithubApp {
        installation_id: 99,
        account: "acme".into(),
    }));
    let l = link(&st, &q, &f, false).unwrap();
    assert_eq!(l.status, LinkStatus::NeedsApproval);

    // Withdrawn, it waits again.
    st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), true)
        .unwrap();
    st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), false)
        .unwrap();
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::NeedsApproval
    );
}

#[test]
fn moving_the_app_clears_its_key_and_voids_every_approval() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    link(&st, &q, &app_fields(), true).unwrap();
    let moved = GithubAppFields {
        app_id: 7,
        slug: "clax-bot".into(),
        api_url: Some("https://ghe.acme.dev/api/v3".into()),
    };
    let app = st.set_github_app(DAEMON, &kek(), &moved, None).unwrap();
    assert_eq!(app.key, None);
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::NeedsApp
    );
    let app = st
        .set_github_app(DAEMON, &kek(), &moved, Some(PEM))
        .unwrap();
    assert_eq!(
        app.key.unwrap().host.as_deref(),
        Some("https://ghe.acme.dev")
    );
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::NeedsApproval
    );
    assert!(st.clear_github_app(DAEMON).unwrap());
    assert_eq!(st.github_app().unwrap(), None);
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::NeedsApp
    );
    assert!(!st.clear_github_app(DAEMON).unwrap());
}

#[test]
fn the_apps_key_shows_a_digest_and_a_bad_one_is_refused() {
    let (_d, st) = store();
    let f = GithubAppFields {
        app_id: 7,
        slug: "clax-bot".into(),
        api_url: None,
    };
    for bad in [
        "not a key",
        "-----BEGIN PUBLIC KEY-----\nabc\n-----END PUBLIC KEY-----",
    ] {
        assert_eq!(
            code(
                st.set_github_app(DAEMON, &kek(), &f, Some(bad))
                    .unwrap_err()
            ),
            "invalid_app_key"
        );
    }
    let app = st.set_github_app(DAEMON, &kek(), &f, Some(PEM)).unwrap();
    let shown = app.key.unwrap().last4;
    assert_eq!(shown.len(), 4);
    assert!(shown.bytes().all(|b| b.is_ascii_hexdigit()), "{shown}");
    for (id, slug) in [(0, "ok"), (1, "-opt"), (1, ""), (1, "a/b")] {
        let f = GithubAppFields {
            app_id: id,
            slug: slug.into(),
            api_url: None,
        };
        assert_eq!(
            code(st.set_github_app(DAEMON, &kek(), &f, None).unwrap_err()),
            "invalid_app"
        );
    }
}

#[test]
fn what_the_host_would_contact_is_checked() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let bad_urls = [
        "http://git.acme.dev/a.git",
        "https://user:pass@git.acme.dev/a.git",
        "https://tok@git.acme.dev/a.git",
        "-upload-pack=evil",
        "--config=x",
        "ssh://git@github.com/a.git",
        "file:///etc/passwd",
        "ext::sh -c evil",
        "https://git.acme.dev/a.git?x=1",
        "https://git.acme.dev/a.git#x",
        "https://169.254.169.254/latest",
        "https://10.0.0.5/a.git",
        "https://127.1/a.git",
        "https://0x7f.0.0.1/a.git",
        "https://[::1]/a.git",
        "https://localhost/a.git",
        "https://git.internal/a.git",
        "https://git.acme.dev\\@evil.example/",
    ];
    for bad in bad_urls {
        let mut f = token_fields();
        f.repo_url = Some(bad.into());
        assert_eq!(
            code(link(&st, &q, &f, false).unwrap_err()),
            "invalid_repo_url",
            "{bad}"
        );
        let mut f = token_fields();
        f.forge_api_url = Some(bad.into());
        assert_eq!(
            code(link(&st, &q, &f, false).unwrap_err()),
            "invalid_forge_url",
            "{bad}"
        );
    }
    for bad in [
        "https://pr-.acme.dev",
        "http://pr-{pr}.acme.dev",
        "https://u:p@pr-{pr}.acme.dev",
        "https://{pr}.internal",
        "https://10.0.0.5/{pr}",
    ] {
        let mut f = token_fields();
        f.preview_url = bad.into();
        assert_eq!(
            code(link(&st, &q, &f, false).unwrap_err()),
            "invalid_preview_url",
            "{bad}"
        );
    }
    // A refusal repeats no part of what may be a credential.
    let mut f = token_fields();
    f.repo_url = Some("https://tok3n@git.acme.dev/a.git".into());
    let e = link(&st, &q, &f, false).unwrap_err();
    assert!(!format!("{e:?}").contains("tok3n"));
}

#[test]
fn every_other_field_refuses_what_a_command_would_read_as_an_option() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let refused = |change: fn(&mut RepoLinkFields, &'static str), bad: &'static str| {
        let mut f = token_fields();
        change(&mut f, bad);
        code(link(&st, &q, &f, true).unwrap_err())
    };
    for bad in [
        "-o/x", "acme", "a/b/c", "../x", "a/.git", "a/b.git", "a b/c", "",
    ] {
        assert_eq!(
            refused(|f, v| f.repo = v.into(), bad),
            "invalid_repo",
            "{bad}"
        );
    }
    for bad in [
        "-b",
        "--upload-pack=x",
        "a..b",
        "a b",
        "a/",
        "x.lock",
        "/a",
        "",
    ] {
        assert_eq!(
            refused(|f, v| f.base = Some(v.into()), bad),
            "invalid_base",
            "{bad}"
        );
    }
    for bad in [
        "healthz",
        "//evil.example/",
        "/a/../b",
        "/a b",
        "/a\\b",
        "/a#b",
        "@evil",
    ] {
        assert_eq!(
            refused(|f, v| f.preview_ready_path = Some(v.into()), bad),
            "invalid_ready_path",
            "{bad}"
        );
    }
    for bad in ["/etc", "-x", "a/../b", "", "a\\b"] {
        assert_eq!(
            refused(|f, v| f.protected_paths = vec![v.into()], bad),
            "invalid_protected_path",
            "{bad}"
        );
    }
    let mut f = token_fields();
    f.preview_poll_secs = Some(0);
    assert_eq!(
        code(link(&st, &q, &f, true).unwrap_err()),
        "invalid_preview_timing"
    );
    let mut f = token_fields();
    f.preview_timeout_secs = Some(5);
    assert_eq!(
        code(link(&st, &q, &f, true).unwrap_err()),
        "invalid_preview_timing"
    );
    let f = fields(RepoAccess::GithubApp {
        installation_id: 0,
        account: String::new(),
    });
    assert_eq!(
        code(link(&st, &q, &f, true).unwrap_err()),
        "invalid_installation"
    );
}

#[test]
fn only_the_owner_of_a_local_daemon_reaches_loopback() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let mut f = token_fields();
    f.repo_url = Some("http://127.0.0.1:8123/acme/web.git".into());
    f.forge_api_url = Some("http://127.0.0.1:8123/api".into());
    f.preview_url = "http://127.0.0.1:9000/pr/{pr}".into();
    assert_eq!(
        code(link(&st, &q, &f, false).unwrap_err()),
        "invalid_repo_url"
    );
    let l = link(&st, &q, &f, true).unwrap();
    assert_eq!(l.repo_url, "http://127.0.0.1:8123/acme/web.git");
    both_tokens(&st, &q);
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .is_some()
    );

    // On a hosted instance not even the owner does, and a link stored
    // before it became one is refused at use.
    st.set_base_url_policy(BaseUrlPolicy::Hosted);
    assert_eq!(
        code(link(&st, &q, &f, true).unwrap_err()),
        "invalid_repo_url"
    );
    assert_eq!(
        code(st.open_queue_repo(&kek(), &q, Egress::Held).unwrap_err()),
        "invalid_repo_url"
    );
}

#[test]
fn a_links_tokens_go_with_it_and_with_its_queue() {
    let (_d, st) = store();
    let sealed = |st: &Store| -> i64 {
        st.with_read(|c| Ok(c.query_row("SELECT COUNT(*) FROM sealed_secrets", [], |r| r.get(0))?))
            .unwrap()
    };
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    assert!(st.clear_queue_repo(DAEMON, &q).unwrap());
    assert!(!st.clear_queue_repo(DAEMON, &q).unwrap());
    assert_eq!(sealed(&st), 0);
    assert_eq!(st.queue_repo(&q).unwrap(), None);
    assert_eq!(
        code(token(&st, &q, TokenKind::Write, WRITE).unwrap_err()),
        "no_repo_link"
    );

    link(&st, &q, &token_fields(), true).unwrap();
    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    st.delete_queue(DAEMON, &q).unwrap();
    assert_eq!(sealed(&st), 0);
    assert_eq!(st.queue_repo(&q).unwrap(), None);
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        link(&st, &q, &token_fields(), true),
        Err(CoreError::NotAMember)
    ));
}

#[test]
fn clearing_a_token_leaves_the_other_and_switching_to_the_app_clears_both() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    link(&st, &q, &token_fields(), true).unwrap();
    token(&st, &q, TokenKind::Read, READ).unwrap();
    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    let l = st
        .clear_queue_repo_token(DAEMON, &q, TokenKind::Read)
        .unwrap();
    assert_eq!(l.status, LinkStatus::NeedsReadToken);
    assert!(tokens(&l).0.is_none() && tokens(&l).1.is_some());
    let l = link(&st, &q, &app_fields(), true).unwrap();
    assert_eq!(l.status, LinkStatus::NeedsCiAcceptance);
    assert_eq!(
        st.secret_meta(&SecretSlot::RepoWrite(q.clone())).unwrap(),
        None
    );
    assert_eq!(
        code(token(&st, &q, TokenKind::Write, WRITE).unwrap_err()),
        "wrong_access"
    );
}

#[test]
fn a_write_names_the_link_it_read_and_a_moved_link_refuses_it() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let seen = st
        .set_queue_repo(DAEMON, &q, &token_fields(), false)
        .unwrap()
        .rev;
    // A panel that saw no link does not overwrite one.
    assert_eq!(
        code(
            st.set_queue_repo(DAEMON, &q, &token_fields(), false)
                .unwrap_err()
        ),
        "link_changed"
    );
    let mut moved = token_fields();
    moved.repo_url = Some("https://git.acme.dev/acme/web.git".into());
    moved.forge_api_url = Some("https://git.acme.dev/api".into());
    moved.if_rev = Some(seen.clone());
    let now = st.set_queue_repo(DAEMON, &q, &moved, false).unwrap().rev;
    assert_ne!(now, seen);
    assert_eq!(
        code(
            st.set_queue_repo_token(DAEMON, &kek(), &q, TokenKind::Write, WRITE, &seen)
                .unwrap_err()
        ),
        "link_changed"
    );
    assert_eq!(
        st.secret_meta(&SecretSlot::RepoWrite(q.clone())).unwrap(),
        None
    );
    let mut stale = token_fields();
    stale.if_rev = Some(seen);
    assert_eq!(
        code(st.set_queue_repo(DAEMON, &q, &stale, false).unwrap_err()),
        "link_changed"
    );
    // Removed and made again, it is another link.
    st.clear_queue_repo(DAEMON, &q).unwrap();
    let again = st
        .set_queue_repo(DAEMON, &q, &token_fields(), false)
        .unwrap()
        .rev;
    assert_ne!(again, now);
    assert_eq!(
        code(
            st.set_queue_repo_token(DAEMON, &kek(), &q, TokenKind::Write, WRITE, &now)
                .unwrap_err()
        ),
        "link_changed"
    );
}

#[test]
fn a_repo_url_on_github_must_be_the_repositorys_own() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let mut f = token_fields();
    f.repo = "acme/renamed".into();
    f.repo_url = Some("https://github.com/acme/web.git".into());
    assert_eq!(
        code(link(&st, &q, &f, false).unwrap_err()),
        "repo_url_mismatch"
    );
    f.repo_url = Some("https://GitHub.com/ACME/Renamed/".into());
    link(&st, &q, &f, false).unwrap();
    f.repo_url = Some("https://git.acme.dev/mirrors/other.git".into());
    f.forge_api_url = Some("https://git.acme.dev/api".into());
    link(&st, &q, &f, false).unwrap();
}

#[test]
fn every_link_is_listed_for_the_owner_with_its_maker_and_admins() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), false).unwrap();
    set_app(&st);
    let b = queue(&st, "b");
    link(&st, &b, &app_fields(), false).unwrap();
    let lines = st.repo_links().unwrap();
    assert_eq!(lines.len(), 2);
    // What waits on the owner comes first.
    assert_eq!(lines[0].queue_id, b);
    assert_eq!(lines[0].status, LinkStatus::NeedsApproval);
    assert_eq!(lines[1].queue_id, q);
    assert_eq!(lines[1].repo_url, "https://github.com/acme/web.git");
    assert_eq!(lines[1].maker, crate::roles::OWNER);
    assert!(lines[1].admins.contains(&crate::roles::OWNER.to_string()));
}

#[test]
fn token_links_need_approval_when_the_owner_says_and_on_a_hosted_instance_by_default() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), false).unwrap();
    both_tokens(&st, &q);
    assert!(!st.repo_token_approval().unwrap());
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::Ready
    );

    st.set_base_url_policy(BaseUrlPolicy::Hosted);
    assert!(st.repo_token_approval().unwrap());
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::NeedsApproval
    );
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .is_none()
    );
    let rev = st.queue_repo(&q).unwrap().unwrap().rev;
    st.approve_queue_repo(DAEMON, &q, &rev, true).unwrap();
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::Ready
    );
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .is_some()
    );

    // The owner's word beats the default, and is recorded.
    st.set_repo_token_approval(DAEMON, false).unwrap();
    assert!(!st.repo_token_approval().unwrap());
    assert!(audit(&st).contains("\"token_approval\":false"));
    st.set_repo_token_approval(DAEMON, true).unwrap();

    // Sent somewhere new, the approval is gone with the tokens.
    let mut f = public(token_fields());
    f.repo_url = Some("https://git.acme.dev/acme/web.git".into());
    f.forge_api_url = Some("https://git.acme.dev/api".into());
    link(&st, &q, &f, false).unwrap();
    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::NeedsApproval
    );
}

#[test]
fn a_revoked_person_leaves_no_link_behind_on_a_queue_with_no_live_admin() {
    let (_d, st) = store();
    let person = |name: &str| {
        let cookie = clax_core::secret::viewer_cookie();
        let v = st.upsert_viewer(cookie.expose(), Some(name)).unwrap();
        st.with_write(|c| {
            c.execute("UPDATE viewers SET person = 1 WHERE id = ?1", params![v.id])?;
            Ok(())
        })
        .unwrap();
        v.public_id
    };
    let (sam, mia) = (person("Sam"), person("Mia"));
    let make = |who: &str, name: &str| {
        st.create_queue(
            DAEMON,
            Some(who),
            NewQueue {
                name: name.into(),
                origins: vec![],
                reporters_see: ReportersSee::Own,
            },
        )
        .unwrap()
        .id
    };
    let (alone, shared) = (make(&sam, "alone"), make(&sam, "shared"));
    st.set_member(DAEMON, &shared, &mia, Some(clax_core::queue::Role::Admin))
        .unwrap();
    for q in [&alone, &shared] {
        link(&st, q, &token_fields(), false).unwrap();
        token(&st, q, TokenKind::Write, WRITE).unwrap();
    }
    st.revoke_person(DAEMON, &sam).unwrap();
    assert_eq!(st.queue_repo(&alone).unwrap(), None);
    assert_eq!(st.secret_meta(&SecretSlot::RepoWrite(alone)).unwrap(), None);
    assert!(st.queue_repo(&shared).unwrap().is_some());
    assert!(audit(&st).contains("\"reason\":\"no_live_admin\""));
}

#[test]
fn the_owners_save_approves_only_what_they_changed_or_made() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    link(&st, &q, &public(app_fields()), false).unwrap();
    // A save of something else by the owner approves nothing unseen.
    let mut f = public(app_fields());
    f.base = Some("develop".into());
    let l = link(&st, &q, &f, true).unwrap();
    assert_eq!(l.status, LinkStatus::NeedsApproval);
    // Naming the repository anew does.
    f.repo = "acme/other".into();
    let l = link(&st, &q, &f, true).unwrap();
    assert_eq!(l.status, LinkStatus::Ready);
}

#[test]
fn an_approval_does_not_outlive_its_app() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    link(&st, &q, &app_fields(), true).unwrap();
    assert!(st.clear_github_app(DAEMON).unwrap());
    let trail = audit(&st);
    assert!(trail.contains("\"op\":\"repo_approval\""), "{trail}");
    assert!(trail.contains("\"approved\":false"), "{trail}");
    assert!(trail.contains("\"approvals_voided\":1"), "{trail}");
    assert_eq!(
        code(
            st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), true)
                .unwrap_err()
        ),
        "no_github_app"
    );
    // Another App starts with nothing approved, even with no row before it.
    st.with_write(|c| {
        c.execute(
            "UPDATE queue_repos SET approved_by = '*owner', approved_at = 't'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    set_app(&st);
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::NeedsApproval
    );
}

#[test]
fn a_token_links_approval_does_not_hang_on_the_app() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    st.set_repo_token_approval(DAEMON, true).unwrap();
    link(&st, &q, &token_fields(), true).unwrap();
    both_tokens(&st, &q);
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::Ready
    );
    st.clear_github_app(DAEMON).unwrap();
    assert_eq!(
        st.queue_repo(&q).unwrap().unwrap().status,
        LinkStatus::Ready
    );
}

#[test]
fn the_audit_names_what_changed_and_every_field_that_shapes_a_run() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    let mut f = token_fields();
    f.preview_url = "https://{pr}.preview.acme.dev".into();
    f.preview_poll_secs = Some(30);
    f.protected_paths = vec!["infra/".into()];
    link(&st, &q, &f, true).unwrap();
    let trail = audit(&st);
    assert!(
        trail.contains("\"fields\":[\"protected_paths\",\"preview_url\",\"preview_poll_secs\"]"),
        "{trail}"
    );
    assert!(trail.contains("\"preview_poll_secs\":30"), "{trail}");
    assert!(trail.contains("\"infra/\""), "{trail}");
}

#[test]
fn a_row_that_cannot_say_what_it_protects_is_an_error() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    st.with_write(|c| {
        c.execute("UPDATE queue_repos SET protected_paths_json = 'nope'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(st.queue_repo(&q).is_err());
    assert!(st.open_queue_repo(&kek(), &q, Egress::Held).is_err());
}

#[test]
fn a_new_kek_lets_each_token_be_set_again() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    token(&st, &q, TokenKind::Read, READ).unwrap();
    let new = Kek::from_bytes([9; 32]);
    let l = st
        .set_queue_repo_token(DAEMON, &new, &q, TokenKind::Write, WRITE, &rev(&st, &q))
        .unwrap();
    assert_eq!(l.status, LinkStatus::Ready);
    // The read token, sealed under the old key, must be set again to open.
    assert_eq!(
        code(st.open_queue_repo(&new, &q, Egress::Held).unwrap_err()),
        "kek_mismatch"
    );
    st.set_queue_repo_token(DAEMON, &new, &q, TokenKind::Read, READ, &rev(&st, &q))
        .unwrap();
    assert!(
        st.open_queue_repo(&new, &q, Egress::Held)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_repository_token_has_a_floor() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    assert_eq!(
        code(token(&st, &q, TokenKind::Write, "short-but-8").unwrap_err()),
        "invalid_key"
    );
}

#[test]
fn an_admin_may_save_around_the_owners_loopback_address_but_not_change_one() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let mut f = token_fields();
    f.repo_url = Some("http://127.0.0.1:8123/acme/web.git".into());
    f.forge_api_url = Some("http://127.0.0.1:8123/api".into());
    link(&st, &q, &f, true).unwrap();
    both_tokens(&st, &q);
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .unwrap()
            .local_forge
    );
    f.base = Some("develop".into());
    let l = link(&st, &q, &f, false).unwrap();
    assert_eq!(l.base, "develop");
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .unwrap()
            .local_forge
    );
    f.preview_url = "https://{pr}.preview.acme.dev".into();
    assert_eq!(
        code(link(&st, &q, &f, false).unwrap_err()),
        "invalid_repo_url"
    );
}

#[test]
fn a_link_made_by_anyone_else_never_counts_as_local() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), false).unwrap();
    both_tokens(&st, &q);
    assert!(
        !st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .unwrap()
            .local_forge
    );
}

#[test]
fn a_repository_off_github_must_name_its_forge() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let mut f = token_fields();
    f.repo_url = Some("https://ghe.acme.dev/acme/web.git".into());
    assert_eq!(
        code(link(&st, &q, &f, false).unwrap_err()),
        "forge_required"
    );
    f.forge_api_url = Some("https://ghe.acme.dev/api/v3".into());
    link(&st, &q, &f, false).unwrap();
    // GitHub's own needs none, however it is spelled.
    let mut f = token_fields();
    f.repo_url = Some("https://GitHub.com/acme/web.git".into());
    link(&st, &q, &f, false).unwrap();
}

#[test]
fn a_last_dot_does_not_take_a_url_off_github() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let mut f = token_fields();
    f.repo = "acme/renamed".into();
    f.repo_url = Some("https://github.com./acme/web.git".into());
    assert_eq!(
        code(link(&st, &q, &f, false).unwrap_err()),
        "repo_url_mismatch"
    );
}

#[test]
fn a_token_is_bound_to_both_of_its_origins() {
    for column in ["repo_url", "forge_api_url"] {
        let (_d, st) = store();
        let q = queue(&st, "a");
        link(&st, &q, &token_fields(), true).unwrap();
        both_tokens(&st, &q);
        let meta = st
            .secret_meta(&SecretSlot::RepoWrite(q.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(meta.host.as_deref(), Some("https://github.com"));
        assert_eq!(meta.forge_host.as_deref(), Some(GITHUB_API));
        // A row moved behind the store's back does not open.
        st.with_write(|c| {
            c.execute(
                &format!("UPDATE queue_repos SET {column} = 'https://evil.example/x'"),
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            code(st.open_queue_repo(&kek(), &q, Egress::Held).unwrap_err()),
            "key_host_mismatch",
            "{column}"
        );
    }
}

#[test]
fn an_app_on_the_owners_private_forge_opens_for_a_link_anyone_made() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    st.set_github_app(
        DAEMON,
        &kek(),
        &GithubAppFields {
            app_id: 7,
            slug: "clax-bot".into(),
            api_url: Some("http://127.0.0.1:8123/api/v3".into()),
        },
        Some(PEM),
    )
    .unwrap();
    link(&st, &q, &app_fields(), false).unwrap();
    st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), true)
        .unwrap();
    let t = st
        .open_queue_repo(&kek(), &q, Egress::Held)
        .unwrap()
        .unwrap();
    assert_eq!(t.repo_url, "http://127.0.0.1:8123/acme/web.git");
    assert!(t.local_forge, "the App's forge is the owner's");
    assert!(!t.local_preview, "the preview is the person's who set it");
}

#[test]
fn a_link_is_confidential_until_its_admin_says_public_and_needs_held_egress() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    both_tokens(&st, &q);
    let t = st
        .open_queue_repo(&kek(), &q, Egress::Held)
        .unwrap()
        .unwrap();
    assert!(!t.public && !t.reviewed_previews && !t.wake_on_reply);
    assert_eq!(
        code(st.open_queue_repo(&kek(), &q, Egress::Open).unwrap_err()),
        "egress_open"
    );
    let mut f = token_fields();
    f.allow_open_egress = true;
    link(&st, &q, &f, true).unwrap();
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Open)
            .unwrap()
            .is_some()
    );
    let mut f = token_fields();
    f.public = true;
    f.reviewed_previews = true;
    f.wake_on_reply = true;
    link(&st, &q, &f, true).unwrap();
    let t = st
        .open_queue_repo(&kek(), &q, Egress::Open)
        .unwrap()
        .unwrap();
    assert!(t.public && t.reviewed_previews && t.wake_on_reply);
    let trail = audit(&st);
    assert!(trail.contains("\"public\":true"), "{trail}");
    assert!(trail.contains("\"fields\":[\"public\""), "{trail}");
}

#[test]
fn only_the_owner_lifts_confidentiality_under_an_approval() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    link(&st, &q, &app_fields(), true).unwrap();
    let mut f = app_fields();
    f.public = true;
    assert_eq!(
        link(&st, &q, &f, false).unwrap().status,
        LinkStatus::NeedsApproval
    );
    st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), true)
        .unwrap();
    let mut f = app_fields();
    f.allow_open_egress = true;
    f.public = true;
    assert_eq!(link(&st, &q, &f, true).unwrap().status, LinkStatus::Ready);
}

#[test]
fn an_address_another_person_left_does_not_become_the_owners() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let mut f = public(token_fields());
    f.repo_url = Some("https://git.acme.dev/acme/web.git".into());
    f.forge_api_url = Some("https://git.acme.dev/api".into());
    link(&st, &q, &f, false).unwrap();
    both_tokens(&st, &q);
    let local = |st: &Store| {
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .unwrap()
            .local_forge
    };
    // The owner changes the preview alone: the forge stays another's.
    f.preview_url = "https://{pr}.preview.acme.dev".into();
    link(&st, &q, &f, true).unwrap();
    assert!(!local(&st));
    // The owner types every address anew.
    f.repo_url = Some("https://git2.acme.dev/acme/web.git".into());
    f.forge_api_url = Some("https://git2.acme.dev/api".into());
    f.preview_url = "https://{pr}.p2.acme.dev".into();
    link(&st, &q, &f, true).unwrap();
    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    assert!(local(&st));
}

#[test]
fn voiding_an_approval_gives_the_link_a_new_rev() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    link(&st, &q, &app_fields(), true).unwrap();
    let before = rev(&st, &q);
    st.clear_github_app(DAEMON).unwrap();
    assert_ne!(rev(&st, &q), before);
    set_app(&st);
    assert_eq!(
        code(
            st.approve_queue_repo(DAEMON, &q, &before, true)
                .unwrap_err()
        ),
        "link_changed"
    );
}

#[test]
fn turning_token_approval_off_counts_what_goes_live() {
    let (_d, st) = store();
    st.set_repo_token_approval(DAEMON, true).unwrap();
    let (a, b) = (queue(&st, "a"), queue(&st, "b"));
    for q in [&a, &b] {
        link(&st, q, &token_fields(), false).unwrap();
    }
    both_tokens(&st, &a);
    st.set_repo_token_approval(DAEMON, false).unwrap();
    let trail = audit(&st);
    assert!(trail.contains("\"links_released\":1"), "{trail}");
    assert!(
        trail.contains("\"values\":{\"token_approval\":false}"),
        "{trail}"
    );
}

#[test]
fn removing_a_link_names_what_was_read() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    assert_eq!(
        code(st.clear_queue_repo_at(DAEMON, &q, "older").unwrap_err()),
        "link_changed"
    );
    assert!(st.clear_queue_repo_at(DAEMON, &q, &rev(&st, &q)).unwrap());
    assert!(!st.clear_queue_repo_at(DAEMON, &q, "gone").unwrap());
}

#[test]
fn a_typed_installation_is_refused_as_an_unknown_field() {
    let body = |access: &str| {
        format!(
            r#"{{"repo": "a/b", "preview_url": "https://pr-{{pr}}.a.dev", "access": {access}}}"#
        )
    };
    let parse = |b: String| serde_json::from_str::<RepoLinkFields>(&b);
    let typed = body(r#"{"kind": "github_app", "installation_id": 7}"#);
    assert!(parse(typed).is_err());
    let f = parse(body(r#"{"kind": "github_app"}"#)).unwrap();
    assert_eq!(
        f.access,
        RepoAccess::GithubApp {
            installation_id: 0,
            account: String::new(),
        }
    );
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    assert_eq!(
        code(st.set_queue_repo(DAEMON, &q, &f, true).unwrap_err()),
        "invalid_installation",
        "a link the forge was not asked about is not made"
    );
}

#[test]
fn the_owner_reads_the_account_the_forge_named() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    let link = st.set_queue_repo(DAEMON, &q, &app_fields(), false).unwrap();
    let LinkAccess::GithubApp { account, .. } = link.access else {
        panic!("an App link");
    };
    assert_eq!(account, "acme");
    assert_eq!(st.repo_links().unwrap()[0].account.as_deref(), Some("acme"));
}

#[test]
fn a_links_outcome_is_noted_and_the_last_word_stands() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let link = st
        .set_queue_repo(DAEMON, &q, &token_fields(), true)
        .unwrap();
    let link = st
        .set_queue_repo_token(DAEMON, &kek(), &q, TokenKind::Write, WRITE, &link.rev)
        .unwrap();
    assert_eq!(
        (link.last_ok_at.is_none(), link.last_failure.is_none()),
        (true, true)
    );

    st.note_repo_failure(&q, None, "the host could not clone the repository")
        .unwrap();
    let link = st.queue_repo(&q).unwrap().unwrap();
    assert_eq!(
        link.last_failure.as_deref(),
        Some("the host could not clone the repository")
    );
    assert!(link.last_failure_at.is_some());

    published(&st, &q);
    let link = st.queue_repo(&q).unwrap().unwrap();
    assert!(link.last_ok_at.is_some());
    assert_eq!(link.last_failure, None);

    st.note_repo_failure(&q, None, "again").unwrap();
    let rev = st.queue_repo(&q).unwrap().unwrap().rev;
    let link = st
        .set_queue_repo_token(DAEMON, &kek(), &q, TokenKind::Write, WRITE, &rev)
        .unwrap();
    assert_eq!(link.last_failure, None, "a new token has failed at nothing");
}

/// A workspace published a change with `q`'s credentials as they stand.
fn published(st: &Store, q: &str) {
    let stamp = st.queue_repo_stamp(q).unwrap().unwrap_or_default();
    st.note_repo_ok(q, &stamp, RepoUse::Write).unwrap();
}

#[test]
fn a_run_on_old_credentials_says_nothing_of_a_links_new_ones() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    both_tokens(&st, &q);
    let old = st.queue_repo_stamp(&q).unwrap().unwrap();
    token(&st, &q, TokenKind::Write, "github_pat_WRITE_another-0123").unwrap();
    st.note_repo_failure(&q, Some(&old), "it failed").unwrap();
    st.note_repo_ok(&q, &old, RepoUse::Write).unwrap();
    let l = st.queue_repo(&q).unwrap().unwrap();
    assert_eq!((l.last_ok_at, l.last_failure), (None, None));

    let now = st.queue_repo_stamp(&q).unwrap().unwrap();
    assert_ne!(old, now);
    st.note_repo_failure(&q, Some(&now), "it failed").unwrap();
    let l = st.queue_repo(&q).unwrap().unwrap();
    assert_eq!(l.last_failure.as_deref(), Some("it failed"));
}

#[test]
fn a_clone_that_worked_clears_what_a_clone_failed_at_and_no_more() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    both_tokens(&st, &q);
    let stamp = st.queue_repo_stamp(&q).unwrap().unwrap();
    st.note_repo_failure(&q, Some(&stamp), "the clone failed")
        .unwrap();
    st.note_repo_ok(&q, &stamp, RepoUse::Read).unwrap();
    let l = st.queue_repo(&q).unwrap().unwrap();
    assert_eq!(l.last_failure, None);
    assert_eq!(
        l.last_ok_at, None,
        "a token that can only read gets this far"
    );

    st.note_repo_failure(&q, Some(&stamp), PUBLISH_FAILED)
        .unwrap();
    st.note_repo_ok(&q, &stamp, RepoUse::Read).unwrap();
    let l = st.queue_repo(&q).unwrap().unwrap();
    assert_eq!(l.last_failure.as_deref(), Some(PUBLISH_FAILED));

    st.note_repo_ok(&q, &stamp, RepoUse::Write).unwrap();
    let l = st.queue_repo(&q).unwrap().unwrap();
    assert_eq!(l.last_failure, None);
    assert!(l.last_ok_at.is_some());
}

#[test]
fn an_app_links_success_is_the_keys() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    st.set_queue_repo(DAEMON, &q, &app_fields(), true).unwrap();
    published(&st, &q);
    assert!(st.queue_repo(&q).unwrap().unwrap().last_ok_at.is_some());
}

#[test]
fn one_queues_success_on_the_app_says_nothing_of_anothers() {
    let (_d, st) = store();
    let (a, b) = (queue(&st, "a"), queue(&st, "b"));
    set_app(&st);
    st.set_queue_repo(DAEMON, &a, &app_fields(), true).unwrap();
    st.set_queue_repo(DAEMON, &b, &app_fields(), true).unwrap();
    st.mark_secret_ok(&SecretSlot::GithubApp).unwrap();
    assert_eq!(st.queue_repo(&a).unwrap().unwrap().last_ok_at, None);
    published(&st, &a);
    assert!(st.queue_repo(&a).unwrap().unwrap().last_ok_at.is_some());
    assert_eq!(st.queue_repo(&b).unwrap().unwrap().last_ok_at, None);
}

#[test]
fn a_new_app_key_has_worked_for_no_link_yet() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    st.set_queue_repo(DAEMON, &q, &app_fields(), true).unwrap();
    published(&st, &q);
    st.note_repo_failure(&q, None, "it failed").unwrap();
    set_app(&st);
    let l = st.queue_repo(&q).unwrap().unwrap();
    assert_eq!((l.last_ok_at, l.last_failure), (None, None));
}

#[test]
fn a_links_success_lasts_as_long_as_what_it_reached_with() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    both_tokens(&st, &q);
    let ok = |st: &Store| st.queue_repo(&q).unwrap().unwrap().last_ok_at.is_some();

    // Nothing that a clone or a push used has changed.
    published(&st, &q);
    let mut f = token_fields();
    f.base = Some("develop".into());
    link(&st, &q, &f, true).unwrap();
    assert!(ok(&st), "a new base branch reaches with the same token");
    token(&st, &q, TokenKind::Read, "github_pat_READ_another-0123").unwrap();
    assert!(ok(&st), "the write token is what it speaks of");

    token(&st, &q, TokenKind::Write, "github_pat_WRITE_another-0123").unwrap();
    assert!(!ok(&st), "a new write token has tried nothing");

    published(&st, &q);
    st.clear_queue_repo_token(DAEMON, &q, TokenKind::Write)
        .unwrap();
    assert!(!ok(&st));

    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    published(&st, &q);
    f.repo = "acme/other".into();
    link(&st, &q, &f, true).unwrap();
    assert!(!ok(&st), "another repository has tried nothing");
}

#[test]
fn clearing_a_token_forgets_what_failed_with_it() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    both_tokens(&st, &q);
    st.note_repo_failure(&q, None, "it failed").unwrap();
    let l = st
        .clear_queue_repo_token(DAEMON, &q, TokenKind::Read)
        .unwrap();
    assert_eq!(l.last_failure, None);
    assert_eq!(l.status, LinkStatus::NeedsReadToken);
}

#[test]
fn a_confidential_link_wants_a_read_token_and_its_ci_accepted_and_a_public_one_neither() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let mut f = token_fields();
    f.ci_accepted = false;
    link(&st, &q, &f, true).unwrap();
    let status = |st: &Store| st.queue_repo(&q).unwrap().unwrap().status;
    assert_eq!(status(&st), LinkStatus::NeedsToken);
    token(&st, &q, TokenKind::Write, WRITE).unwrap();
    assert_eq!(status(&st), LinkStatus::NeedsReadToken);
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .is_none()
    );
    token(&st, &q, TokenKind::Read, READ).unwrap();
    assert_eq!(status(&st), LinkStatus::NeedsCiAcceptance);
    assert!(
        st.open_queue_repo(&kek(), &q, Egress::Held)
            .unwrap()
            .is_none()
    );
    f.ci_accepted = true;
    link(&st, &q, &f, true).unwrap();
    assert_eq!(status(&st), LinkStatus::Ready);

    f.public = true;
    f.ci_accepted = false;
    link(&st, &q, &f, true).unwrap();
    st.clear_queue_repo_token(DAEMON, &q, TokenKind::Read)
        .unwrap();
    assert_eq!(status(&st), LinkStatus::Ready);
}

#[test]
fn an_app_link_waits_for_its_ci_to_be_accepted_before_the_owner_reads_it() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    let mut f = app_fields();
    f.ci_accepted = false;
    let l = link(&st, &q, &f, false).unwrap();
    assert_eq!(l.status, LinkStatus::NeedsCiAcceptance);
    f.ci_accepted = true;
    let l = link(&st, &q, &f, false).unwrap();
    assert_eq!(l.status, LinkStatus::NeedsApproval);
}

#[test]
fn an_acceptance_of_ci_is_of_one_repository_and_does_not_move_with_the_link() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    link(&st, &q, &token_fields(), true).unwrap();
    both_tokens(&st, &q);
    let mut f = token_fields();
    f.repo = "acme/other".into();
    let l = link(&st, &q, &f, true).unwrap();
    assert!(!l.ci_accepted, "that repository has a CI of its own");
    let trail = audit(&st);
    assert!(trail.contains("\"ci_accepted\":false"), "{trail}");

    // A new link says it of the repository it names.
    let other = queue(&st, "b");
    assert!(link(&st, &other, &f, true).unwrap().ci_accepted);
}

#[test]
fn the_owners_own_change_to_what_they_approved_keeps_their_approval() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    link(&st, &q, &app_fields(), true).unwrap();
    let mut f = app_fields();
    f.reviewed_previews = true;
    f.protected_paths = vec!["deploy/".into()];
    f.preview_ready_path = Some("/up".into());
    assert_eq!(link(&st, &q, &f, true).unwrap().status, LinkStatus::Ready);
}

#[test]
fn anyone_elses_change_voids_an_approval_in_either_direction() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    set_app(&st);
    let mut f = app_fields();
    f.reviewed_previews = true;
    f.protected_paths = vec!["deploy/".into()];
    link(&st, &q, &f, true).unwrap();
    // Dropping a review or a protected path loosens a link as surely as
    // `public` does.
    f.reviewed_previews = false;
    assert_eq!(
        link(&st, &q, &f, false).unwrap().status,
        LinkStatus::NeedsApproval
    );
    st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), true)
        .unwrap();
    f.protected_paths = vec![];
    assert_eq!(
        link(&st, &q, &f, false).unwrap().status,
        LinkStatus::NeedsApproval
    );
}

#[test]
fn a_token_swap_keeps_an_approval_and_can_reach_only_what_the_owner_read() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    st.set_repo_token_approval(DAEMON, true).unwrap();
    link(&st, &q, &token_fields(), false).unwrap();
    both_tokens(&st, &q);
    st.approve_queue_repo(DAEMON, &q, &rev(&st, &q), true)
        .unwrap();

    // An approval names where a token goes, which a swap cannot move.
    let swapped = "github_pat_ANOTHER_ACCOUNT_0123456";
    let l = token(&st, &q, TokenKind::Write, swapped).unwrap();
    assert_eq!(l.status, LinkStatus::Ready);
    let t = st
        .open_queue_repo(&kek(), &q, Egress::Held)
        .unwrap()
        .unwrap();
    assert_eq!(t.repo_url, "https://github.com/acme/web.git");
    assert_eq!(t.forge_api_url, GITHUB_API);
    let trail = audit(&st);
    assert!(trail.contains("\"last4\":\"3456\""), "{trail}");
}

#[test]
fn every_link_shows_the_owner_what_makes_it_dangerous() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    let mut f = token_fields();
    f.public = true;
    f.allow_open_egress = true;
    f.reviewed_previews = true;
    f.protected_paths = vec!["deploy/".into()];
    f.preview_ready_path = Some("/up".into());
    link(&st, &q, &f, false).unwrap();
    let line = st.repo_links().unwrap().remove(0);
    assert!(line.public && line.allow_open_egress && line.reviewed_previews);
    assert!(line.ci_accepted);
    assert_eq!(line.preview_url, "https://pr-{pr}.acme.dev");
    assert_eq!(line.preview_ready_path, "/up");
    assert_eq!(line.protected_paths, ["deploy/"]);
}

#[test]
fn turning_token_approval_off_counts_no_link_that_still_waits_for_something_else() {
    let (_d, st) = store();
    let (live, short) = (queue(&st, "a"), queue(&st, "b"));
    st.set_repo_token_approval(DAEMON, true).unwrap();
    link(&st, &live, &token_fields(), false).unwrap();
    both_tokens(&st, &live);
    link(&st, &short, &token_fields(), false).unwrap();
    token(&st, &short, TokenKind::Write, WRITE).unwrap();
    st.set_repo_token_approval(DAEMON, false).unwrap();
    let trail = audit(&st);
    assert!(trail.contains("\"links_released\":1"), "{trail}");
}

#[test]
fn noting_a_link_that_is_gone_is_nothing() {
    let (_d, st) = store();
    let q = queue(&st, "a");
    published(&st, &q);
    st.note_repo_failure(&q, None, "x").unwrap();
    assert_eq!(st.queue_repo(&q).unwrap(), None);
}

#[test]
fn the_app_opens_for_a_lookup_only_once_it_has_a_key() {
    let (_d, st) = store();
    assert!(st.open_github_app(&kek()).unwrap().is_none());
    let f = GithubAppFields {
        app_id: 7,
        slug: "clax-bot".into(),
        api_url: None,
    };
    st.set_github_app(DAEMON, &kek(), &f, None).unwrap();
    assert!(st.open_github_app(&kek()).unwrap().is_none());
    set_app(&st);
    let app = st.open_github_app(&kek()).unwrap().unwrap();
    assert_eq!((app.app_id, app.api_url.as_str()), (7, GITHUB_API));
    assert_eq!(app.private_key.expose(), PEM.trim());
    assert!(
        app.local_forge,
        "a local daemon's owner may use their own machine"
    );
    assert_eq!(
        code(st.open_github_app(&Kek::from_bytes([8; 32])).unwrap_err()),
        "kek_mismatch"
    );
}
