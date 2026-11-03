//! `copperframe_editor [PROJECT_DIR] [--scene PATH] [--steal-lock] [--frames N]`
//!
//! Opens a project (default: the current directory) in the scene editor.
//! Create a project first with `copperframe new DIR`.

use std::path::PathBuf;

use anyhow::{Context, bail};
use copperframe_editor::EditorConfig;

const USAGE: &str =
    "usage: copperframe_editor [PROJECT_DIR] [--scene PATH] [--steal-lock] [--frames N]";

fn parse(args: impl Iterator<Item = String>) -> anyhow::Result<EditorConfig> {
    let mut config = EditorConfig {
        project: PathBuf::from("."),
        scene: None,
        steal_lock: false,
        exit_after_frames: None,
    };
    let mut project_set = false;
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
            other if !project_set => {
                config.project = PathBuf::from(other);
                project_set = true;
            }
            other => bail!("unexpected argument `{other}` ({USAGE})"),
        }
    }
    Ok(config)
}

fn main() -> anyhow::Result<()> {
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
        assert_eq!(c.project, PathBuf::from("my game"));
        assert_eq!(c.exit_after_frames, Some(3));
        assert!(c.steal_lock);
        assert!(p(&["a", "b"]).is_err());
        assert!(p(&["--bogus"]).is_err());
        assert_eq!(p(&[]).unwrap().project, PathBuf::from("."));
    }
}
