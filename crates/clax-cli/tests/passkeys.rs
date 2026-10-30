//! `clax passkeys reset`: the offline recovery of browser confirmations'
//! passkeys refuses while the home's daemon runs, and with it stopped
//! removes every passkey and stops requiring one, saying what it removed.

use assert_cmd::Command;
use clax_core::Home;
use clax_core::daemon::{DaemonInfo, write_daemon_info};
use clax_store::passkeys::ConfirmPrefs;

fn reset(dir: &std::path::Path) -> std::process::Output {
    Command::new(crate::common::clax())
        .args(["passkeys", "reset"])
        .env("CLAX_HOME", dir.join("ax"))
        .env("HOME", dir)
        .env_remove("CLAX_CONFIG_DIR")
        .output()
        .unwrap()
}

#[test]
fn a_reset_refuses_while_the_daemon_runs_and_resets_once_it_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("ax"));
    home.ensure_dirs().unwrap();
    {
        let st = clax_store::Store::open(&home).unwrap();
        let key = clax_core::webauthn::Registered {
            credential_id: "c1".into(),
            public_key: vec![1],
            alg: clax_core::webauthn::ES256,
            sign_count: 0,
        };
        st.add_passkey(
            &key,
            &serde_json::json!({"via": "a browser session", "agents_live": ["ada (claude)"]}),
        )
        .unwrap();
        st.set_confirm_prefs(ConfirmPrefs {
            require_passkey: true,
            offer_dismissed: false,
        })
        .unwrap();
    }
    // A daemon runs on the home: this test's own live process stands for it.
    let info = DaemonInfo {
        port: 1,
        pid: std::process::id(),
        token: "t".into(),
        started_at: "2026-10-30T10:00:00Z".into(),
        bind: "127.0.0.1".into(),
        version: "0.0.0".into(),
        exe: None,
        commit: None,
    };
    write_daemon_info(&home, &info).unwrap();
    let out = reset(dir.path());
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("is running"), "{err}");
    assert!(
        clax_store::Store::open(&home)
            .unwrap()
            .passkeys_required()
            .unwrap(),
        "nothing changed"
    );
    // Stopped, it resets and names what it removed.
    std::fs::remove_file(home.daemon_json()).unwrap();
    let out = reset(dir.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("removed 1 passkey:"), "{text}");
    assert!(text.contains("c1 added"), "{text}");
    assert!(text.contains("agents running then: ada (claude)"), "{text}");
    let st = clax_store::Store::open(&home).unwrap();
    assert!(st.passkeys().unwrap().is_empty());
    assert!(!st.confirm_prefs().unwrap().require_passkey);
}
