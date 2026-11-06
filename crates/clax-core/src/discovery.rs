//! What a Clax says it is: the answer of `GET /.well-known/clax`
//! (docs/contract.md, spec 2026-10-27-clax-sdk-design §4.2), which a
//! daemon serves and which a local daemon reads from other origins to
//! learn whether they are Clax instances.
//!
//! Another release may answer: reading one, a field it leaves out takes
//! its default, and a field it adds is ignored.

use serde::{Deserialize, Serialize};

/// The answer of `GET /.well-known/clax`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    /// `hosted` or `local`.
    #[serde(default)]
    pub kind: String,
    /// The version of the contract it serves.
    #[serde(default)]
    pub contract_version: String,
    /// Its name.
    #[serde(default)]
    pub name: String,
    /// Where it says it is reached.
    #[serde(default)]
    pub origin: String,
    /// Its API root: `/api`.
    #[serde(default)]
    pub api: String,
    /// The domain its artifacts' sites are served under, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_domain: Option<String>,
    /// How a client obtains a credential there.
    #[serde(default)]
    pub sign_in: Vec<String>,
    /// The paths of its device sign-in, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DevicePaths>,
    /// What it offers beyond the core.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// The paths of a device sign-in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePaths {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub approve: String,
    #[serde(default)]
    pub token: String,
}

impl Discovery {
    /// Whether this answer describes a Clax this release can talk to: kind
    /// `hosted` or `local`, a contract version, and the API at `/api`.
    pub fn is_clax(&self) -> bool {
        matches!(self.kind.as_str(), "hosted" | "local")
            && !self.contract_version.is_empty()
            && self.api == "/api"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_answer_reads_with_what_it_leaves_out_defaulted() {
        let d: Discovery = serde_json::from_value(json!({
            "kind": "hosted", "contract_version": "1", "api": "/api", "later": true
        }))
        .unwrap();
        assert!(d.is_clax());
        assert_eq!((d.name.as_str(), d.site_domain), ("", None));
        let not: Discovery =
            serde_json::from_value(json!({"kind": "worker", "api": "/api"})).unwrap();
        assert!(!not.is_clax());
    }
}
