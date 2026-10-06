//! `clax version`: the version, and with `--verbose` the commit this binary
//! was built from. `clax --version` stays exactly `clax <version>`, which
//! the plugin wrappers and installers compare.

#[derive(clap::Args)]
pub struct Args {
    /// Also name the commit this binary was built from.
    #[arg(long, short)]
    pub verbose: bool,
}

pub fn run(cli: &crate::Cli, a: &Args) -> anyhow::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let commit = clax_core::build_commit();
    super::print(
        cli,
        serde_json::json!({"version": version, "commit": commit}),
        |_| {
            if a.verbose {
                format!("clax {version}\ncommit {commit}")
            } else {
                format!("clax {version}")
            }
        },
    );
    Ok(())
}
