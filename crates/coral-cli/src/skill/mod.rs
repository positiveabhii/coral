//! Install the Coral skill into Claude Code and/or Codex.

#![allow(
    clippy::print_stdout,
    reason = "CLI subcommand intentionally prints a completion message."
)]

use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use directories::BaseDirs;

const SKILL_NAME: &str = "coral";
const SKILL_BODY: &str = include_str!("SKILL.md");

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum SkillTarget {
    /// Install only for Claude Code (~/.claude/skills).
    Claude,
    /// Install only for Codex (~/.codex/skills).
    Codex,
    /// Install for both Claude Code and Codex.
    Both,
}

#[derive(Debug, Args)]
pub(crate) struct SkillArgs {
    #[command(subcommand)]
    pub(crate) command: SkillCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SkillCommand {
    /// Install the Coral skill into Claude Code and/or Codex skills directories.
    Install {
        /// Which agent's skills directory to install into.
        #[arg(long, value_enum, default_value_t = SkillTarget::Both)]
        target: SkillTarget,
        /// Overwrite an existing skill file without failing.
        #[arg(long)]
        force: bool,
    },
}

pub(crate) fn run(args: &SkillArgs) -> Result<(), anyhow::Error> {
    match &args.command {
        SkillCommand::Install { target, force } => {
            let home = resolve_home()?;
            install_targets(&home, *target, *force)
        }
    }
}

fn resolve_home() -> Result<PathBuf, anyhow::Error> {
    Ok(BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("could not resolve user home directory"))?
        .home_dir()
        .to_path_buf())
}

fn install_targets(home: &Path, target: SkillTarget, force: bool) -> Result<(), anyhow::Error> {
    let mut roots = Vec::new();
    if matches!(target, SkillTarget::Claude | SkillTarget::Both) {
        roots.push(home.join(".claude").join("skills"));
    }
    if matches!(target, SkillTarget::Codex | SkillTarget::Both) {
        roots.push(home.join(".codex").join("skills"));
    }
    for root in &roots {
        install(root, force)?;
    }
    Ok(())
}

fn install(skills_root: &Path, force: bool) -> Result<(), anyhow::Error> {
    let target_dir = skills_root.join(SKILL_NAME);
    let skill_file = target_dir.join("SKILL.md");
    if skill_file.exists() && !force {
        anyhow::bail!(
            "skill already exists at {}; pass --force to overwrite",
            skill_file.display()
        );
    }
    fs::create_dir_all(&target_dir)?;
    fs::write(&skill_file, SKILL_BODY)?;
    println!("Installed Coral skill at {}", skill_file.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{SKILL_BODY, SkillTarget, install, install_targets};

    #[test]
    fn install_writes_skill_file_to_expected_location() {
        let temp = TempDir::new().expect("temp dir");
        install(temp.path(), false).expect("install");
        let skill_path = temp.path().join("coral").join("SKILL.md");
        let contents = fs::read_to_string(&skill_path).expect("read skill");
        assert_eq!(contents, SKILL_BODY);
    }

    #[test]
    fn install_fails_when_skill_exists_and_force_not_set() {
        let temp = TempDir::new().expect("temp dir");
        install(temp.path(), false).expect("install");
        let err = install(temp.path(), false).expect_err("second install should fail");
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn install_overwrites_when_force_is_set() {
        let temp = TempDir::new().expect("temp dir");
        let skill_dir = temp.path().join("coral");
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(skill_dir.join("SKILL.md"), "stale content").expect("seed stale skill");
        install(temp.path(), true).expect("install with force");
        let contents = fs::read_to_string(skill_dir.join("SKILL.md")).expect("read skill");
        assert_eq!(contents, SKILL_BODY);
    }

    #[test]
    fn install_targets_both_writes_to_claude_and_codex_directories() {
        let temp = TempDir::new().expect("temp dir");
        install_targets(temp.path(), SkillTarget::Both, false).expect("install both");
        let claude_skill = temp
            .path()
            .join(".claude")
            .join("skills")
            .join("coral")
            .join("SKILL.md");
        let codex_skill = temp
            .path()
            .join(".codex")
            .join("skills")
            .join("coral")
            .join("SKILL.md");
        assert_eq!(fs::read_to_string(&claude_skill).expect("claude"), SKILL_BODY);
        assert_eq!(fs::read_to_string(&codex_skill).expect("codex"), SKILL_BODY);
    }

    #[test]
    fn install_targets_claude_only_skips_codex() {
        let temp = TempDir::new().expect("temp dir");
        install_targets(temp.path(), SkillTarget::Claude, false).expect("install claude");
        assert!(
            temp.path()
                .join(".claude")
                .join("skills")
                .join("coral")
                .join("SKILL.md")
                .exists()
        );
        assert!(!temp.path().join(".codex").exists());
    }

    #[test]
    fn install_targets_codex_only_skips_claude() {
        let temp = TempDir::new().expect("temp dir");
        install_targets(temp.path(), SkillTarget::Codex, false).expect("install codex");
        assert!(
            temp.path()
                .join(".codex")
                .join("skills")
                .join("coral")
                .join("SKILL.md")
                .exists()
        );
        assert!(!temp.path().join(".claude").exists());
    }
}
