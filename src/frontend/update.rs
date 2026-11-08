//! Self-update from the git checkout the binary was built in: notice when the installed build
//! lags the checkout's `HEAD` or the checkout lags its upstream branch, then pull and rerun the
//! install script. The app then quits itself and the script's watcher opens the new build.
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

/// Commit this binary was built from; empty when it was built outside a git checkout.
const BUILT_COMMIT: &str = env!("SCYTIFY_BUILD_COMMIT");

/// Report whether the installed app is out of date: the checkout's `HEAD` is not the commit this
/// binary was built from (something pulled without reinstalling), or the upstream branch has
/// commits the checkout lacks. The upstream is fetched first; if that fails (offline) only the
/// local comparison runs. No checkout or an unreadable `HEAD` reports no update.
pub(super) async fn check() -> bool {
    tokio::task::spawn_blocking(|| {
        if !Path::new(REPO).join(".git").exists() {
            return false;
        }
        let fetched = git(&["fetch", "--quiet"]);
        let head = git_output(&["rev-parse", "HEAD"]);
        if head
            .as_deref()
            .is_some_and(|head| installed_is_stale(BUILT_COMMIT, head))
        {
            return true;
        }
        fetched
            && git_output(&["rev-list", "--count", "HEAD..@{u}"])
                .and_then(|count| count.parse::<u32>().ok())
                .is_some_and(|behind| behind > 0)
    })
    .await
    .unwrap_or(false)
}

/// Whether a binary built from `built` lags the checkout at `head`. An unknown build commit
/// can't be compared, so it is never stale.
fn installed_is_stale(built: &str, head: &str) -> bool {
    !built.is_empty() && built != head
}

/// Pull and reinstall in a separate process group, so the script survives this app quitting.
/// On success the new build is installed and a watcher opens it once this process has exited,
/// so the caller must quit; an `Err` means the update stopped early and
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
            .env("SCYTIFY_UPDATE_PID", std::process::id().to_string())
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

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(REPO)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
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

#[cfg(test)]
mod tests {
    use super::installed_is_stale;

    #[test]
    fn stale_only_when_built_commit_differs_from_head() {
        assert!(installed_is_stale("aaa", "bbb"));
        assert!(!installed_is_stale("aaa", "aaa"));
        assert!(!installed_is_stale("", "bbb"));
    }
}
