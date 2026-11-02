//! Milestone 1 sandbox: a window with a fly camera and a textured, lit cube.
//!
//! Usage: `psx_sandbox [--frames N]`
//!   --frames N   exit after presenting N frames (for automated smoke runs)

use anyhow::{Context, bail};
use psx_runtime::AppConfig;

fn parse_args(args: impl Iterator<Item = String>) -> anyhow::Result<AppConfig> {
    let mut config = AppConfig {
        title: "PSX Engine — Milestone 1 sandbox".to_owned(),
        ..AppConfig::default()
    };
    let mut args = args.skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => {
                let value = args.next().context("--frames needs a number")?;
                let n: u64 = value.parse().with_context(|| {
                    format!("--frames expects a positive integer, got `{value}`")
                })?;
                config.exit_after_frames = Some(n.max(1));
            }
            "-h" | "--help" => {
                println!("usage: psx_sandbox [--frames N]");
                std::process::exit(0);
            }
            other => bail!("unknown argument `{other}` (try --help)"),
        }
    }
    Ok(config)
}

fn main() -> anyhow::Result<()> {
    psx_runtime::init_logging();
    let config = parse_args(std::env::args())?;
    psx_runtime::run(config)
}
