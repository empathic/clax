//! The instance's Cloudflare credentials, which the daemon's web source
//! ([`crate::web_source::DaemonWeb`]) and its browser source (Browser
//! Run) both use: the account ID setting `web.cloudflare.account_id` and
//! the token sealed in [`SecretSlot::Cloudflare`], opened only for
//! [`CLOUDFLARE_ORIGIN`].
//! Each source reads them at each call, so an admin's change takes effect
//! at once.

use clax_core::secret::Secret;
use clax_store::Store;
use clax_store::bot_settings::Setting;
use clax_store::secrets::{CLOUDFLARE_ORIGIN, Kek, SecretSlot};

/// A Cloudflare account ID and its opened token.
pub type Credentials = (String, Secret);

/// The account ID and token `st` holds now, the token opened with `kek`
/// for `base` (the URL it is sent to); `None` without both (or without
/// `kek`).
///
/// # Errors
/// `kek_mismatch` or `key_host_mismatch` from opening the token; the
/// store's.
pub fn credentials(
    st: &Store,
    kek: Option<&Kek>,
    base: &str,
) -> clax_core::Result<Option<Credentials>> {
    let Some(kek) = kek else {
        return Ok(None);
    };
    let account = st.bot_setting(Setting::WebCloudflareAccountId)?;
    let account = account.as_str().unwrap_or_default().trim().to_string();
    if account.is_empty() {
        return Ok(None);
    }
    let token = st.open_for_use(kek, &SecretSlot::Cloudflare, base)?;
    Ok(token.map(|t| (account, t)))
}

/// `creds` with a token that cannot be opened (sealed under another
/// key-encryption key, or bound to another origin) read as none: the admin
/// sets it again, and until then the sources that need it are not
/// configured.
///
/// # Errors
/// Any other error of `creds`.
pub fn usable(
    creds: clax_core::Result<Option<Credentials>>,
) -> clax_core::Result<Option<Credentials>> {
    match creds {
        Err(clax_core::CoreError::Invalid {
            code: "kek_mismatch" | "key_host_mismatch",
            ..
        }) => Ok(None),
        other => other,
    }
}

/// Whether the instance holds a key-encryption key, a Cloudflare account
/// ID and a sealed token that opens with that key for
/// [`CLOUDFLARE_ORIGIN`]. A token sealed under another key reads as none,
/// as the sources read it.
///
/// # Errors
/// The store's.
pub fn has_credentials(st: &Store, kek: Option<&Kek>) -> clax_core::Result<bool> {
    Ok(usable(credentials(st, kek, CLOUDFLARE_ORIGIN))?.is_some())
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use clax_core::Home;
    use clax_core::audit::AuditCtx;
    use serde_json::json;

    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
    const TOKEN: &str = "cf-test-token-1234";

    #[test]
    fn credentials_need_a_kek_an_account_and_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&Home::at(dir.path().join("ax"))).unwrap();
        let kek = Kek::generate();
        let read = |kek: Option<&Kek>| {
            usable(credentials(&st, kek, CLOUDFLARE_ORIGIN))
                .unwrap()
                .map(|(a, t)| (a, t.expose().to_string()))
        };
        assert_eq!(read(Some(&kek)), None, "nothing set");
        st.set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::WebCloudflareAccountId, Some(json!(ACCOUNT)))],
        )
        .unwrap();
        assert_eq!(read(Some(&kek)), None, "no token");
        st.set_secret(&AuditCtx::DAEMON, &kek, &SecretSlot::Cloudflare, TOKEN)
            .unwrap();
        assert_eq!(
            read(Some(&kek)),
            Some((ACCOUNT.to_string(), TOKEN.to_string()))
        );
        assert!(has_credentials(&st, Some(&kek)).unwrap());
        assert_eq!(read(None), None, "no key-encryption key");
        assert_eq!(read(Some(&Kek::generate())), None, "another key");
        // The token opens only for Cloudflare's origin.
        assert!(
            credentials(&st, Some(&kek), "https://elsewhere.example").is_err(),
            "another origin is refused"
        );
        assert!(
            usable(credentials(&st, Some(&kek), "https://elsewhere.example"))
                .unwrap()
                .is_none(),
            "and reads as none"
        );
        st.set_bot_settings(
            &AuditCtx::DAEMON,
            &[(Setting::WebCloudflareAccountId, None)],
        )
        .unwrap();
        assert_eq!(read(Some(&kek)), None, "no account ID");
        assert!(!has_credentials(&st, Some(&kek)).unwrap());
    }
}
