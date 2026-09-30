use super::tools;
use clax_core::Home;
use clax_mcp::tools::AssetUploadArgs;
use rmcp::handler::server::wrapper::Parameters;
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Upload local files as assets of an artifact.
    Upload(UploadArgs),
}

#[derive(clap::Args)]
pub struct UploadArgs {
    /// Artifact ID or URL the assets belong to.
    pub target: String,
    /// Files to upload; relative paths resolve against the working directory.
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
}

/// Prints the `asset_upload` tool's result object (`--json`), or one asset URL
/// per line.
pub fn run(cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let Cmd::Upload(a) = cmd;
    let cwd = std::env::current_dir()?;
    let args = AssetUploadArgs {
        url_or_id: a.target.clone(),
        file_path: None,
        file_paths: Some(
            a.files
                .iter()
                .map(|f| cwd.join(f).to_string_lossy().into_owned())
                .collect(),
        ),
    };
    let result = tools::call(cli, home, |t| async move {
        t.asset_upload(Parameters(args)).await
    })?;
    super::print(cli, result, |r| {
        r["assets"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|a| a["url"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    });
    Ok(())
}
