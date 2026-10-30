//! Self-update from the git checkout the binary was built in: notice when its upstream branch
//! has new commits, then pull and rerun the install script, which quits and relaunches the app.
use std::{
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

/// Checkout this binary was built from.
const REPO: &str = env!("CARGO_MANIFEST_DIR");
/// How often the upstream branch is fetched.
pub(super) const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Fetch the upstream branch and report whether it has commits the checkout lacks. Any git
/// failure (no checkout, no upstream, offline) reports no update.
pub(super) async fn check() -> bool {
    tokio::task::spawn_blocking(|| {
        if !Path::new(REPO).join(".git").exists() || !git(&["fetch", "--quiet"]) {
            return false;
        }
        Command::new("git")
            .args(["rev-list", "--count", "HEAD..@{u}"])
            .current_dir(REPO)
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
            .is_some_and(|behind| behind > 0)
    })
    .await
    .unwrap_or(false)
}

/// Pull and reinstall in a separate process group, so the script survives this app quitting.
/// On success the script relaunches the new build; an `Err` means the update stopped early and
/// this instance keeps running. Output goes to `scytify-update.log` in the temp directory.
pub(super) async fn install() -> Result<(), String> {
    tokio::task::spawn_blocking(|| {
        let log_path = std::env::temp_dir().join("scytify-update.log");
        let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
        let status = Command::new("sh")
            // Apps opened from a launcher get a minimal PATH without rustup's cargo.
            .args([
                "-c",
                r#"PATH="$HOME/.cargo/bin:$PATH"; git pull --ff-only && exec tools/install.sh"#,
            ])
            .current_dir(REPO)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .process_group(0)
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Update failed; see {}", log_path.display()))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

fn git(args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(REPO)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
