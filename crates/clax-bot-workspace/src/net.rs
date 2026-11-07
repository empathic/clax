//! Where a request for a repository may go. A person gives the addresses of a
//! store link, and a name that is public today can answer with a private
//! address tomorrow, so a link's addresses are held to [`PublicOnly`] unless
//! its owner may reach their own machine.
//!
//! Each of the host's three ways out keeps the rule at the connection:
//!
//! - The forge's and a preview's clients ([`PublicOnly::client`]) resolve
//!   here, connect only to the public answers and follow no redirect.
//! - Host-side `git` resolves for itself, so [`PublicOnly::pin`] resolves
//!   first, checks, and hands git the answers with `http.curloptResolve`: the
//!   command never asks DNS for a name it was given. Each command is resolved
//!   and checked anew, and follows no redirect.
//! - A container's clone goes out on the runtime's network and none of this
//!   reaches it: what holds it is the runtime's own egress
//!   ([`crate::runtime::ContainerSpec::domain_allow_list`]). It has no way to
//!   the daemon's own machine or network on a third party's runtime, and on
//!   Docker it has whatever the engine's bridge gives it. It carries one read
//!   token, which is ended when the clone returns.
//!
//! No way out of the host uses a proxy in the daemon's environment: a proxy
//! resolves names itself, past this rule. Those held to it connect directly,
//! and a daemon that can leave its network only through a proxy reaches no
//! repository this way.

use crate::runtime::control_builder;
use anyhow::{Context, Result, bail};
use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// Answers a name, as the host's resolver does.
#[async_trait::async_trait]
pub trait Names: Send + Sync {
    async fn lookup(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>>;
}

/// The host's resolver.
pub struct SystemNames;

#[async_trait::async_trait]
impl Names for SystemNames {
    async fn lookup(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
        let found = tokio::net::lookup_host((host, port)).await?;
        Ok(found.map(|a| a.ip()).collect())
    }
}

/// Reaches public addresses alone
/// ([`clax_core::config::public_address`]).
#[derive(Clone)]
pub struct PublicOnly {
    names: Arc<dyn Names>,
    public: fn(IpAddr) -> bool,
}

impl Default for PublicOnly {
    fn default() -> PublicOnly {
        PublicOnly::with_names(Arc::new(SystemNames))
    }
}

impl PublicOnly {
    /// The rule, asking `names`.
    pub fn with_names(names: Arc<dyn Names>) -> PublicOnly {
        PublicOnly {
            names,
            public: clax_core::config::public_address,
        }
    }

    /// The rule, asking `names` and taking what `public` says is public: a
    /// test's way to reach its own loopback through a name only this rule
    /// knows.
    #[must_use]
    pub fn calling_public(self, public: fn(IpAddr) -> bool) -> PublicOnly {
        PublicOnly { public, ..self }
    }

    /// The public answers for `host`.
    async fn answers(&self, host: &str, port: u16) -> Result<Vec<IpAddr>> {
        let found = self.names.lookup(host, port).await;
        let found = found.with_context(|| format!("resolving {host}"))?;
        let public: Vec<IpAddr> = found.into_iter().filter(|ip| (self.public)(*ip)).collect();
        if public.is_empty() {
            bail!("{host} resolves to no public address");
        }
        Ok(public)
    }

    /// A client for the APIs a link names: [`crate::runtime::control_client`]
    /// connecting to public addresses alone and through no proxy. A name with
    /// none fails as unresolved.
    pub fn client(&self) -> reqwest::Client {
        control_builder()
            .dns_resolver(Arc::new(self.clone()))
            .no_proxy()
            .build()
            .expect("a client with no custom TLS builds")
    }

    /// The `-c` arguments that hold one `git` command on `url` to this rule,
    /// or why it may not start. A URL that names an address needs no pin, and
    /// that address must be public. One that is not http or https cannot be
    /// held here, so it is refused.
    pub async fn pin(&self, url: &str) -> Result<Vec<String>> {
        let parsed = Url::parse(url).context("the repository's address is not a URL")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            bail!("the repository's address is not http or https");
        }
        let port = parsed
            .port_or_known_default()
            .context("the repository's address has no port")?;
        let mut args = ["http.followRedirects=false", "http.proxy="]
            .into_iter()
            .flat_map(|set| ["-c".to_string(), set.to_string()])
            .collect::<Vec<_>>();
        let host = parsed
            .host_str()
            .context("the repository's address has no host")?;
        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
            if !(self.public)(ip) {
                bail!("the repository's address is not public");
            }
            return Ok(args);
        }
        let answers = self.answers(host, port).await?;
        let list: Vec<String> = answers.iter().map(|ip| shown(*ip)).collect();
        args.push("-c".to_string());
        args.push(format!(
            "http.curloptResolve={host}:{port}:{}",
            list.join(",")
        ));
        Ok(args)
    }
}

/// `ip` as curl takes it in a resolve entry.
fn shown(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let me = self.clone();
        Box::pin(async move {
            let found = me.answers(name.as_str(), 0).await;
            let found = found.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("{e:#}").into()
            })?;
            let addrs = found.into_iter().map(|ip| SocketAddr::new(ip, 0));
            Ok(Box::new(addrs.collect::<Vec<_>>().into_iter()) as Addrs)
        })
    }
}

/// How a target's addresses are reached.
#[derive(Clone)]
pub struct Reach {
    /// Held to this rule, host-side git pins each command; `None` for a
    /// target whose repository may be on the daemon's machine or network.
    pub git: Option<PublicOnly>,
    /// What looks at its previews.
    pub preview: reqwest::Client,
}

impl Reach {
    /// Anywhere: a target an operator wrote, or a link whose owner may reach
    /// their own machine.
    pub fn any() -> Reach {
        Reach {
            git: None,
            preview: crate::runtime::control_client(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Answers from a list, one call at a time, the last for ever.
    struct Scripted(Mutex<Vec<&'static [&'static str]>>);

    impl Scripted {
        fn saying(answers: Vec<&'static [&'static str]>) -> PublicOnly {
            PublicOnly::with_names(Arc::new(Scripted(Mutex::new(answers))))
        }
    }

    #[async_trait::async_trait]
    impl Names for Scripted {
        async fn lookup(&self, _host: &str, _port: u16) -> io::Result<Vec<IpAddr>> {
            let mut held = self.0.lock().unwrap();
            let next = if held.len() > 1 {
                held.remove(0)
            } else {
                held[0]
            };
            Ok(next.iter().map(|a| a.parse().unwrap()).collect())
        }
    }

    #[tokio::test]
    async fn a_name_resolving_to_a_private_address_starts_no_git() {
        for address in ["127.0.0.1", "10.0.0.8", "169.254.169.254", "::1", "fd00::1"] {
            let rule = PublicOnly::with_names(Arc::new(Scripted(Mutex::new(vec![Box::leak(
                vec![address].into_boxed_slice(),
            )
                as &[&str]]))));
            let e = rule.pin("https://forge.example/o/r.git").await.unwrap_err();
            assert!(
                e.to_string().contains("no public address"),
                "{address}: {e}"
            );
        }
    }

    #[tokio::test]
    async fn git_is_pinned_to_the_public_answers_alone_and_follows_no_redirect() {
        let rule = Scripted::saying(vec![&["10.0.0.8", "1.1.1.1", "2606:4700::1111"]]);
        let args = rule.pin("https://forge.example/o/r.git").await.unwrap();
        assert_eq!(
            args,
            [
                "-c",
                "http.followRedirects=false",
                "-c",
                "http.proxy=",
                "-c",
                "http.curloptResolve=forge.example:443:1.1.1.1,[2606:4700::1111]",
            ]
        );
    }

    #[tokio::test]
    async fn each_command_is_checked_anew_so_an_answer_that_changes_is_caught() {
        let rule = Scripted::saying(vec![&["1.1.1.1"], &["127.0.0.1"]]);
        let first = rule
            .pin("https://forge.example:8443/o/r.git")
            .await
            .unwrap();
        assert!(
            first.contains(&"http.curloptResolve=forge.example:8443:1.1.1.1".to_string()),
            "{first:?}"
        );
        assert!(rule.pin("https://forge.example/o/r.git").await.is_err());
    }

    #[tokio::test]
    async fn a_url_naming_an_address_is_held_to_the_same_rule() {
        let rule = Scripted::saying(vec![&[]]);
        let args = rule.pin("https://1.1.1.1/o/r.git").await.unwrap();
        assert_eq!(
            args,
            ["-c", "http.followRedirects=false", "-c", "http.proxy="]
        );
        for url in [
            "http://127.0.0.1:9/o/r.git",
            "https://[::1]/o/r.git",
            "http://169.254.169.254/o/r.git",
        ] {
            assert!(rule.pin(url).await.is_err(), "{url}");
        }
    }

    #[tokio::test]
    async fn only_http_can_be_held() {
        let rule = Scripted::saying(vec![&["1.1.1.1"]]);
        for url in [
            "file:///srv/r.git",
            "ssh://git@forge.example/r.git",
            "/srv/r.git",
        ] {
            assert!(rule.pin(url).await.is_err(), "{url}");
        }
    }

    #[tokio::test]
    async fn a_client_connects_to_no_private_answer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let rule = Scripted::saying(vec![&["127.0.0.1"]]);
        let url = format!("http://forge.example:{port}/");
        let e = rule.client().get(url).send().await.unwrap_err();
        assert!(format!("{e:?}").contains("no public address"), "{e:?}");
    }

    /// A client of the rule, in a process whose environment names a proxy. Run
    /// by [`a_proxy_in_the_environment_is_not_used`], since a test cannot
    /// change the environment of its own process.
    #[tokio::test]
    #[ignore = "run in a process of its own"]
    async fn with_a_proxy_in_the_environment() {
        let rule = Scripted::saying(vec![&["127.0.0.1"]]);
        let sent = rule.client().get("http://forge.example/").send().await;
        assert!(sent.is_err());
    }

    #[tokio::test]
    async fn a_proxy_in_the_environment_is_not_used() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        // Counted before it is closed, and a client that came waits for that.
        let reached = Arc::new(AtomicUsize::new(0));
        let count = reached.clone();
        tokio::spawn(async move {
            while listener.accept().await.is_ok() {
                count.fetch_add(1, Ordering::SeqCst);
            }
        });
        let out = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "net::tests::with_a_proxy_in_the_environment"])
            .arg("--ignored")
            .env("HTTP_PROXY", &proxy)
            .env("http_proxy", &proxy)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .await
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(said.contains("1 passed"), "{said}");
        assert_eq!(reached.load(Ordering::SeqCst), 0, "it went to the proxy");
    }

    #[tokio::test]
    async fn a_client_filters_at_the_connection_whatever_was_said_before() {
        // The lookup is the connection's own: a name that answered a public
        // address when a person saved it has no say here.
        let rule = Scripted::saying(vec![&["1.1.1.1"], &["127.0.0.1"]]);
        rule.answers("forge.example", 443).await.unwrap();
        let e = rule
            .client()
            .get("http://forge.example:9/")
            .send()
            .await
            .unwrap_err();
        assert!(format!("{e:?}").contains("no public address"), "{e:?}");
    }
}
