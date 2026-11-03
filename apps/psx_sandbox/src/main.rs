//! Sandbox: the PS1 comparison scene with a fly camera and live style controls.
//!
//! Usage: `psx_sandbox [--frames N] [--style PATH] [--split]`
//!   --frames N    exit after presenting N frames (for automated smoke runs)
//!   --style PATH  style settings file (default: psx_style.json in the
//!                 current directory); loaded if present, F5 saves to it
//!   --split       start in split view (left: current style, right: effects off)

use std::path::PathBuf;

use anyhow::{Context, bail};
use psx_runtime::AppConfig;

const USAGE: &str = "usage: psx_sandbox [--frames N] [--style PATH] [--split]";

fn parse_args(args: impl Iterator<Item = String>) -> anyhow::Result<AppConfig> {
    let mut config = AppConfig {
        title: "PSX Engine sandbox".to_owned(),
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
            "--style" => {
                let value = args.next().context("--style needs a file path")?;
                config.style_path = PathBuf::from(value);
            }
            "--split" => config.split = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => bail!("unknown argument `{other}` ({USAGE})"),
        }
    }
    Ok(config)
}

fn main() -> anyhow::Result<()> {
    psx_runtime::init_logging();
    let config = parse_args(std::env::args())?;
    psx_runtime::run(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<AppConfig> {
        parse_args(
            std::iter::once("psx_sandbox")
                .chain(args.iter().copied())
                .map(String::from),
        )
    }

    #[test]
    fn parses_all_flags() {
        let c = parse(&["--frames", "5", "--style", "my style.json", "--split"]).unwrap();
        assert_eq!(c.exit_after_frames, Some(5));
        assert_eq!(c.style_path, PathBuf::from("my style.json"));
        assert!(c.split);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&["--frames", "x"]).is_err());
        assert!(parse(&["--style"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
