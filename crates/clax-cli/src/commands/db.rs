//! `clax db`: an artifact's page database, through the `db_*` MCP tools (so
//! the arguments, checks, results and the caller level, `owner`, are theirs).

use super::tools;
use crate::term;
use clax_core::Home;
use clax_mcp::tools::{
    DbBatchArgs, DbDeleteArgs, DbDirection, DbGetArgs, DbLevel, DbOrderBy, DbQueryArgs,
    DbQueryOpts, DbStrReplaceArgs, DbWriteArgs,
};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Map, Value};
use std::io::Read;
use std::path::Path;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Read one document.
    Get(GetArgs),
    /// List a collection in document ID order, a page at a time.
    List(ListArgs),
    /// Query a collection with filters and an order.
    Query(QueryArgs),
    /// Replace a document, creating it when absent.
    Set(WriteArgs),
    /// Merge fields into an existing document.
    Update(WriteArgs),
    /// Delete a document.
    Delete(DeleteArgs),
    /// Replace text inside one top-level string field.
    StrReplace(StrReplaceArgs),
    /// Apply 1 to 50 writes atomically.
    Batch(BatchArgs),
}

/// A lower access level to act at.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Level {
    View,
    Interact,
    Admin,
}

impl From<Level> for DbLevel {
    fn from(l: Level) -> DbLevel {
        match l {
            Level::View => DbLevel::View,
            Level::Interact => DbLevel::Interact,
            Level::Admin => DbLevel::Admin,
        }
    }
}

#[derive(clap::Args)]
pub struct Doc {
    /// Artifact ID or URL.
    pub target: String,
    /// Collection path, such as `tasks` or `boards/b1/columns`.
    pub collection: String,
    /// Document ID (one path segment).
    pub doc_id: String,
}

#[derive(clap::Args)]
pub struct AsLevel {
    /// Act at this lower level to check what the page's rules allow; it
    /// narrows your access (owner), never raises it.
    #[arg(long, value_enum)]
    pub as_level: Option<Level>,
}

#[derive(clap::Args)]
pub struct GetArgs {
    #[command(flatten)]
    pub doc: Doc,
    #[command(flatten)]
    pub level: AsLevel,
}

#[derive(clap::Args)]
pub struct Page {
    /// Most documents to return, 1 to 1000 (default 100).
    #[arg(long)]
    pub limit: Option<u32>,
    /// `next_cursor` from the previous result.
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// Artifact ID or URL.
    pub target: String,
    /// Collection path.
    pub collection: String,
    #[command(flatten)]
    pub page: Page,
    #[command(flatten)]
    pub level: AsLevel,
}

#[derive(clap::Args)]
pub struct QueryArgs {
    /// Artifact ID or URL.
    pub target: String,
    /// Collection path.
    pub collection: String,
    /// A filter as a JSON triple, such as '["done", "==", false]'; repeat
    /// for more (at most 10). Operators: == != < <= > >= in not-in array-contains.
    #[arg(long = "where", value_name = "JSON")]
    pub where_: Vec<String>,
    /// Order by this top-level field; the result is then one page.
    #[arg(long)]
    pub order_by: Option<String>,
    /// With --order-by, descending.
    #[arg(long, requires = "order_by")]
    pub desc: bool,
    #[command(flatten)]
    pub page: Page,
    #[command(flatten)]
    pub level: AsLevel,
}

#[derive(clap::Args)]
pub struct WriteArgs {
    #[command(flatten)]
    pub doc: Doc,
    /// The document fields as a JSON object; `-` reads it from stdin.
    #[arg(required_unless_present = "file", conflicts_with = "file")]
    pub data: Option<String>,
    /// A local JSON file whose top-level object is the document.
    #[arg(long)]
    pub file: Option<String>,
    /// The version you last read; required when the document exists.
    #[arg(long)]
    pub if_version: Option<u64>,
    #[command(flatten)]
    pub level: AsLevel,
}

#[derive(clap::Args)]
pub struct DeleteArgs {
    #[command(flatten)]
    pub doc: Doc,
    /// The version you last read; required when the document exists.
    #[arg(long)]
    pub if_version: Option<u64>,
    #[command(flatten)]
    pub level: AsLevel,
}

#[derive(clap::Args)]
pub struct StrReplaceArgs {
    #[command(flatten)]
    pub doc: Doc,
    /// The top-level string field to edit.
    pub field: String,
    /// The exact text to replace; it must occur once unless --replace-all.
    pub old_str: String,
    /// The replacement text (may be empty).
    pub new_str: String,
    /// Replace every occurrence.
    #[arg(long)]
    pub replace_all: bool,
    /// The version you last read.
    #[arg(long)]
    pub if_version: Option<u64>,
    #[command(flatten)]
    pub level: AsLevel,
}

#[derive(clap::Args)]
pub struct BatchArgs {
    /// Artifact ID or URL.
    pub target: String,
    /// The writes as a JSON array of `{op, collection, doc_id, data |
    /// file_path, if_version}` (the `db_batch` tool's `writes`); `-` reads
    /// it from stdin. Relative `file_path`s resolve against the working
    /// directory.
    pub writes: String,
    #[command(flatten)]
    pub level: AsLevel,
}

/// `raw`, or stdin when it is `-`.
fn text_or_stdin(raw: &str) -> anyhow::Result<String> {
    if raw == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(raw.to_string())
    }
}

/// `p` joined to the working directory when relative.
fn absolute(p: &str) -> anyhow::Result<String> {
    Ok(std::env::current_dir()?
        .join(Path::new(p))
        .to_string_lossy()
        .into_owned())
}

fn object(raw: &str) -> anyhow::Result<Map<String, Value>> {
    match serde_json::from_str::<Value>(&text_or_stdin(raw)?) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => anyhow::bail!("the document must be a JSON object"),
        Err(e) => anyhow::bail!("the document is not JSON: {e}"),
    }
}

/// Prints the tool's result object (`--json`), or a readable form of it.
pub fn run(cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let result = match cmd {
        Cmd::Get(a) => {
            let args = DbGetArgs {
                url_or_id: a.doc.target.clone(),
                collection: a.doc.collection.clone(),
                doc_id: a.doc.doc_id.clone(),
                as_level: a.level.as_level.map(Into::into),
            };
            tools::call(
                cli,
                home,
                |t| async move { t.db_get(Parameters(args)).await },
            )?
        }
        Cmd::List(a) => {
            let args = DbQueryArgs {
                url_or_id: a.target.clone(),
                collection: a.collection.clone(),
                query: Some(DbQueryOpts {
                    where_: None,
                    order_by: None,
                    limit: a.page.limit,
                    cursor: a.page.cursor.clone(),
                }),
                as_level: a.level.as_level.map(Into::into),
            };
            tools::call(
                cli,
                home,
                |t| async move { t.db_list(Parameters(args)).await },
            )?
        }
        Cmd::Query(a) => {
            let where_ = if a.where_.is_empty() {
                None
            } else {
                Some(
                    a.where_
                        .iter()
                        .map(|w| {
                            serde_json::from_str::<Value>(w).map_err(|e| {
                                anyhow::anyhow!("--where {w}: not JSON ({e}); pass a triple such as '[\"done\", \"==\", false]'")
                            })
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?,
                )
            };
            let args = DbQueryArgs {
                url_or_id: a.target.clone(),
                collection: a.collection.clone(),
                query: Some(DbQueryOpts {
                    where_,
                    order_by: a.order_by.as_ref().map(|f| DbOrderBy {
                        field: f.clone(),
                        direction: Some(if a.desc {
                            DbDirection::Desc
                        } else {
                            DbDirection::Asc
                        }),
                    }),
                    limit: a.page.limit,
                    cursor: a.page.cursor.clone(),
                }),
                as_level: a.level.as_level.map(Into::into),
            };
            tools::call(
                cli,
                home,
                |t| async move { t.db_query(Parameters(args)).await },
            )?
        }
        Cmd::Set(a) | Cmd::Update(a) => {
            let update = matches!(cmd, Cmd::Update(_));
            let args = DbWriteArgs {
                url_or_id: a.doc.target.clone(),
                collection: a.doc.collection.clone(),
                doc_id: a.doc.doc_id.clone(),
                data: a.data.as_deref().map(object).transpose()?,
                file_path: a.file.as_deref().map(absolute).transpose()?,
                if_version: a.if_version,
                as_level: a.level.as_level.map(Into::into),
            };
            tools::call(cli, home, |t| async move {
                if update {
                    t.db_update(Parameters(args)).await
                } else {
                    t.db_set(Parameters(args)).await
                }
            })?
        }
        Cmd::Delete(a) => {
            let args = DbDeleteArgs {
                url_or_id: a.doc.target.clone(),
                collection: a.doc.collection.clone(),
                doc_id: a.doc.doc_id.clone(),
                if_version: a.if_version,
                as_level: a.level.as_level.map(Into::into),
            };
            tools::call(
                cli,
                home,
                |t| async move { t.db_delete(Parameters(args)).await },
            )?
        }
        Cmd::StrReplace(a) => {
            let args = DbStrReplaceArgs {
                url_or_id: a.doc.target.clone(),
                collection: a.doc.collection.clone(),
                doc_id: a.doc.doc_id.clone(),
                field: a.field.clone(),
                old_str: a.old_str.clone(),
                new_str: a.new_str.clone(),
                replace_all: Some(a.replace_all),
                if_version: a.if_version,
                as_level: a.level.as_level.map(Into::into),
            };
            tools::call(cli, home, |t| async move {
                t.db_str_replace(Parameters(args)).await
            })?
        }
        Cmd::Batch(a) => {
            let mut writes: Value = serde_json::from_str(&text_or_stdin(&a.writes)?)
                .map_err(|e| anyhow::anyhow!("the writes are not JSON: {e}"))?;
            for w in writes.as_array_mut().into_iter().flatten() {
                if let Some(p) = w.get("file_path").and_then(Value::as_str) {
                    w["file_path"] = Value::String(absolute(p)?);
                }
            }
            let mut args = serde_json::json!({"url_or_id": a.target, "writes": writes});
            if let Some(l) = a.level.as_level {
                args["as_level"] = serde_json::to_value(DbLevel::from(l))?;
            }
            let args: DbBatchArgs =
                serde_json::from_value(args).map_err(|e| anyhow::anyhow!("invalid_args: {e}"))?;
            tools::call(
                cli,
                home,
                |t| async move { t.db_batch(Parameters(args)).await },
            )?
        }
    };
    super::print(cli, result, |r| readable(cmd, r));
    Ok(())
}

fn doc_line(d: &Value) -> String {
    format!(
        "{}  v{}  {}",
        term::clean_line(d["id"].as_str().unwrap_or("?")),
        d["version"],
        term::snippet(&d["data"].to_string(), 120)
    )
}

fn readable(cmd: &Cmd, r: &Value) -> String {
    let path = term::clean_line(r["path"].as_str().unwrap_or(""));
    match cmd {
        Cmd::Get(_) => match r["doc"].as_object() {
            None => format!("{path}: no such document"),
            Some(d) => format!(
                "{path}  v{}\n{}",
                d["version"],
                term::clean_text(&serde_json::to_string_pretty(&d["data"]).unwrap_or_default())
            ),
        },
        Cmd::List(_) | Cmd::Query(_) => {
            let mut lines: Vec<String> = r["docs"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(doc_line)
                .collect();
            if lines.is_empty() {
                lines.push("no documents".into());
            }
            if let Some(c) = r["next_cursor"].as_str() {
                lines.push(format!("more: --cursor {}", term::clean_line(c)));
            }
            lines.join("\n")
        }
        Cmd::Set(_) => format!(
            "{} {path}: v{}",
            if r["created"] == true {
                "created"
            } else {
                "set"
            },
            r["version"]
        ),
        Cmd::Update(_) => format!("updated {path}: v{}", r["version"]),
        Cmd::StrReplace(_) => format!("edited {path}: v{}", r["version"]),
        Cmd::Delete(_) => {
            if r["deleted"] == true {
                format!("deleted {path}")
            } else {
                format!("{path}: no such document; nothing deleted")
            }
        }
        Cmd::Batch(_) => r["results"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|x| {
                let p = term::clean_line(x["path"].as_str().unwrap_or("?"));
                match x["op"].as_str() {
                    Some("delete") if x["deleted"] != true => {
                        format!("delete {p}: no such document")
                    }
                    Some("delete") => format!("delete {p}"),
                    Some(op) => format!("{op} {p}: v{}", x["version"]),
                    None => term::snippet(&x.to_string(), 160),
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
