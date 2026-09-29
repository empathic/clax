//! Serialisable records returned by the store and the HTTP API.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub current_version: u32,
    pub pinned: bool,
    pub capabilities: serde_json::Value,
    pub contract_version: String,
    pub owner_session_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    pub content_type: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub artifact_id: String,
    pub n: u32,
    pub label: Option<String>,
    pub created_at: String,
    pub session_id: Option<String>,
    pub files: BTreeMap<String, FileMeta>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub artifact_id: String,
    pub content_type: String,
    pub size: u64,
    pub ext: String,
    pub created_at: String,
}

/// One harness conversation (Claude Code, Codex, Pi) that publishes artifacts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub harness: String,
    pub harness_session_id: Option<String>,
    pub cwd: String,
    pub pid: Option<u32>,
    pub parent_pid: Option<u32>,
    pub started_at: String,
    pub last_seen_at: String,
    pub ended_at: Option<String>,
}

pub const CONTRACT_VERSION: &str = "0.2.61";
