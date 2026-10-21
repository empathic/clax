//! `clax tag` (add and remove an artifact's tags) and `clax tags` (list the
//! tags; the owner makes, edits and deletes them). Tags are one list the
//! owner keeps: applying a tag that is not on it is refused, naming the
//! tags there are.

use crate::client::Client;
use clax_core::Home;
use serde_json::{Value, json};

#[derive(clap::Args)]
pub struct TagArgs {
    /// The artifact: its ID or URL.
    pub target: String,
    /// A tag to add, by name or ID. Repeatable.
    #[arg(long)]
    pub add: Vec<String>,
    /// A tag to remove, by name or ID. Repeatable.
    #[arg(long)]
    pub remove: Vec<String>,
}

#[derive(clap::Args)]
pub struct TagsArgs {
    #[command(subcommand)]
    pub cmd: Option<TagsCmd>,
}

#[derive(clap::Subcommand)]
pub enum TagsCmd {
    /// List the tags (the default): name, colour, how many artifacts carry
    /// each, and description.
    List,
    /// Make a tag (the owner's).
    Create {
        /// Its name: 1 to 40 characters, unique whatever its case.
        name: String,
        /// Its colour: gray, red, orange, yellow, green, teal, blue,
        /// purple, pink or brown (default gray).
        #[arg(long)]
        colour: Option<String>,
        /// A one-line description.
        #[arg(long)]
        description: Option<String>,
    },
    /// Rename, recolour or redescribe a tag (the owner's). A rename keeps
    /// the artifacts that carry it.
    Edit {
        /// The tag: its name or ID.
        tag: String,
        /// Its new name.
        #[arg(long)]
        name: Option<String>,
        /// Its new colour.
        #[arg(long)]
        colour: Option<String>,
        /// Its new description; empty clears it.
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a tag (the owner's): every artifact carrying it loses it.
    Delete {
        /// The tag: its name or ID.
        tag: String,
    },
}

/// The tag `name_or_id` names in `GET /api/tags`'s answer `list`: its ID.
fn tag_id(list: &Value, name_or_id: &str) -> anyhow::Result<String> {
    let want = name_or_id.trim();
    let tags = list["tags"].as_array().cloned().unwrap_or_default();
    let found = tags
        .iter()
        .find(|t| t["id"].as_str() == Some(want))
        .or_else(|| {
            tags.iter().find(|t| {
                t["name"]
                    .as_str()
                    .is_some_and(|n| clax_core::tags::fold(n) == clax_core::tags::fold(want))
            })
        });
    match found {
        Some(t) => Ok(t["id"].as_str().unwrap_or_default().to_string()),
        None => {
            let names: Vec<&str> = tags.iter().filter_map(|t| t["name"].as_str()).collect();
            anyhow::bail!(
                "no tag is named '{want}'; {}",
                if names.is_empty() {
                    "there are no tags yet".to_string()
                } else {
                    format!("the tags are: {}", names.join(", "))
                }
            )
        }
    }
}

/// The names of `tags` (`[{name, …}]`), comma-separated.
pub(crate) fn names(tags: &Value) -> String {
    tags.as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn run_tag(cli: &crate::Cli, home: &Home, a: &TagArgs) -> anyhow::Result<()> {
    if a.add.is_empty() && a.remove.is_empty() {
        anyhow::bail!("pass --add or --remove (tag names)");
    }
    let id = super::open::id_from(&a.target)?;
    let c = Client::connect(home, cli.port_for(home)?)?;
    let res = c.patch(
        &format!("/api/artifacts/{id}/tags"),
        &json!({"add": a.add, "remove": a.remove}),
    )?;
    super::print(cli, res, |j| {
        let tags = names(&j["tags"]);
        if tags.is_empty() {
            format!("{id} carries no tags")
        } else {
            format!("{id} carries {tags}")
        }
    });
    Ok(())
}

pub fn run_tags(cli: &crate::Cli, home: &Home, a: &TagsArgs) -> anyhow::Result<()> {
    let c = Client::connect(home, cli.port_for(home)?)?;
    match a.cmd.as_ref().unwrap_or(&TagsCmd::List) {
        TagsCmd::List => {
            let res = c.get("/api/tags")?;
            super::print(cli, res, |j| {
                let mut lines = vec![format!(
                    "{:<24}  {:<7}  {:>5}  {}",
                    "TAG", "COLOUR", "USED", "DESCRIPTION"
                )];
                for t in j["tags"].as_array().map(Vec::as_slice).unwrap_or_default() {
                    lines.push(format!(
                        "{:<24}  {:<7}  {:>5}  {}",
                        t["name"].as_str().unwrap_or(""),
                        t["colour"].as_str().unwrap_or(""),
                        t["count"],
                        t["description"].as_str().unwrap_or("")
                    ));
                }
                lines.join("\n")
            });
        }
        TagsCmd::Create {
            name,
            colour,
            description,
        } => {
            let mut body = json!({"name": name});
            if let Some(c) = colour {
                body["colour"] = json!(c);
            }
            if let Some(d) = description {
                body["description"] = json!(d);
            }
            let res = c.post("/api/tags", &body)?;
            super::print(cli, res, |j| {
                format!(
                    "made tag {} ({})",
                    j["tag"]["name"].as_str().unwrap_or(""),
                    j["tag"]["colour"].as_str().unwrap_or("")
                )
            });
        }
        TagsCmd::Edit {
            tag,
            name,
            colour,
            description,
        } => {
            if name.is_none() && colour.is_none() && description.is_none() {
                anyhow::bail!("pass --name, --colour or --description");
            }
            let id = tag_id(&c.get("/api/tags")?, tag)?;
            let mut body = json!({});
            for (k, v) in [
                ("name", name),
                ("colour", colour),
                ("description", description),
            ] {
                if let Some(v) = v {
                    body[k] = json!(v);
                }
            }
            let res = c.patch(&format!("/api/tags/{id}"), &body)?;
            super::print(cli, res, |j| {
                format!(
                    "tag {} ({})",
                    j["tag"]["name"].as_str().unwrap_or(""),
                    j["tag"]["colour"].as_str().unwrap_or("")
                )
            });
        }
        TagsCmd::Delete { tag } => {
            let id = tag_id(&c.get("/api/tags")?, tag)?;
            let res = c.send(reqwest::Method::DELETE, &format!("/api/tags/{id}"), None)?;
            super::print(cli, res, |j| {
                format!(
                    "deleted tag {tag}, removed from {} artifacts",
                    j["artifacts"]
                )
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_named_by_id_or_by_name_whatever_its_case() {
        let list = json!({"tags": [
            {"id": "t_0000000001", "name": "Design"},
            {"id": "t_0000000002", "name": "draft"},
        ]});
        assert_eq!(tag_id(&list, "design").unwrap(), "t_0000000001");
        assert_eq!(tag_id(&list, "t_0000000002").unwrap(), "t_0000000002");
        let e = tag_id(&list, "nope").unwrap_err().to_string();
        assert!(e.contains("Design, draft"), "{e}");
        let e = tag_id(&json!({"tags": []}), "x").unwrap_err().to_string();
        assert!(e.contains("no tags yet"), "{e}");
        assert_eq!(names(&json!([{"name": "a"}, {"name": "b"}])), "a, b");
    }
}
