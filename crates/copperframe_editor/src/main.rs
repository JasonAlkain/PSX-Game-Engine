//! `copperframe_editor [PROJECT_DIR] [--scene PATH] [--steal-lock] [--frames N]`
//!
//! Opens a project in the scene editor. Without `PROJECT_DIR` the editor
//! starts at the project hub, where projects are created and opened.

use std::path::PathBuf;

use anyhow::{Context, bail};
use copperframe_editor::EditorConfig;

const USAGE: &str =
    "usage: copperframe_editor [PROJECT_DIR] [--scene PATH] [--steal-lock] [--frames N]";

fn parse(args: impl Iterator<Item = String>) -> anyhow::Result<EditorConfig> {
    let mut config = EditorConfig {
        project: None,
        scene: None,
        steal_lock: false,
        exit_after_frames: None,
    };
    let mut args = args.skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scene" => config.scene = Some(args.next().context("--scene needs a path")?.into()),
            "--steal-lock" => config.steal_lock = true,
            "--frames" => {
                let v = args.next().context("--frames needs a number")?;
                let n: u64 = v
                    .parse()
                    .with_context(|| format!("--frames expects a positive integer, got `{v}`"))?;
                config.exit_after_frames = Some(n.max(1));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other if other.starts_with('-') => bail!("unknown argument `{other}` ({USAGE})"),
            other if config.project.is_none() => config.project = Some(PathBuf::from(other)),
            other => bail!("unexpected argument `{other}` ({USAGE})"),
        }
    }
    Ok(config)
}

fn main() -> anyhow::Result<()> {
    use copperframe_core::report;
    report::install(
        "copperframe_editor",
        report::engine_data_dir(),
        Some(report::REPORT_EMAIL),
    );
    copperframe_runtime::init_logging();
    copperframe_editor::run(parse(std::env::args())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> anyhow::Result<EditorConfig> {
        parse(
            std::iter::once("copperframe_editor")
                .chain(args.iter().copied())
                .map(String::from),
        )
    }

    #[test]
    fn parses_arguments() {
        let c = p(&["my game", "--frames", "3", "--steal-lock"]).unwrap();
        assert_eq!(c.project, Some(PathBuf::from("my game")));
        assert_eq!(c.exit_after_frames, Some(3));
        assert!(c.steal_lock);
        assert!(p(&["a", "b"]).is_err());
        assert!(p(&["--bogus"]).is_err());
        assert_eq!(p(&[]).unwrap().project, None, "no project: the hub");
    }
}
