use super::tools;
use base64::Engine;
use clax_core::Home;
use clax_mcp::tools::ReadArgs;
use rmcp::handler::server::wrapper::Parameters;
use std::io::Write;

#[derive(clap::Args)]
pub struct Args {
    /// Artifact ID or URL; a URL naming a version selects it unless --version is given.
    pub target: String,
    /// Version to read; defaults to the URL's version, else the current version.
    #[arg(long)]
    pub version: Option<u32>,
    /// Published path to read.
    #[arg(long, default_value = clax_core::publish::INDEX)]
    pub path: String,
    /// Most bytes of content to return; longer files are cut and flagged truncated.
    #[arg(long, default_value_t = clax_mcp::tools::DEFAULT_READ_MAX_BYTES)]
    pub max_bytes: u64,
}

/// Prints the `read` tool's result object (`--json`), or the file's content.
pub fn run(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    let args = ReadArgs {
        url_or_id: a.target.clone(),
        path: Some(a.path.clone()),
        version: a.version,
        max_bytes: Some(a.max_bytes),
    };
    let result = tools::call(cli, home, |t| async move { t.read(Parameters(args)).await })?;
    if cli.json {
        println!("{result}");
        return Ok(());
    }
    let mut out = std::io::stdout().lock();
    if let Some(text) = result["content"].as_str() {
        out.write_all(text.as_bytes())?;
    } else if let Some(b64) = result["content_base64"].as_str() {
        out.write_all(&base64::engine::general_purpose::STANDARD.decode(b64)?)?;
    } else {
        anyhow::bail!(
            "{} is {} bytes, over --max-bytes {}; nothing printed",
            a.path,
            result["size"],
            a.max_bytes
        );
    }
    out.flush()?;
    Ok(())
}
