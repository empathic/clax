//! `clax perf-calibrate <db>` (hidden): the daemon latency gate's
//! calibration read ([`clax_core::perf`]) through this binary's bundled
//! SQLite. Creates the database at `<db>`, prints `{"ready": true}`, then
//! for each line read on stdin runs the read once and prints
//! `{"ms": <milliseconds>}`, until stdin closes.

use std::io::{BufRead, Write};
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// Where to create the calibration database; must not exist.
    pub db: PathBuf,
}

pub fn run(a: &Args) -> anyhow::Result<()> {
    anyhow::ensure!(!a.db.exists(), "{} already exists", a.db.display());
    let cal = clax_core::perf::Calibration::create(&a.db)?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", serde_json::json!({"ready": true}))?;
    out.flush()?;
    for line in std::io::stdin().lock().lines() {
        line?;
        let ms = cal.run()?.as_secs_f64() * 1000.0;
        writeln!(out, "{}", serde_json::json!({ "ms": ms }))?;
        out.flush()?;
    }
    Ok(())
}
