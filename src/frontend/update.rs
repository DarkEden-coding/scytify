//! Self-update from the git checkout the binary was built in: notice when the installed build
//! lags the checkout's `HEAD` or the checkout lags its upstream branch, then pull and rerun the
//! install script, reporting its progress from the script's output. The app then quits itself
//! and the script's watcher opens the new build.
use iced::futures::{SinkExt, Stream, channel::mpsc::Sender};
use std::{
    fs::File,
    io::Read,
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

/// Where a running update is, as read from the install script's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Downloading,
    /// Cargo's finished and total build units; `total` is zero until cargo reports it.
    Building { done: u32, total: u32 },
    Installing,
    /// Installed; the app quits and the new build opens.
    Restarting,
}

impl Stage {
    pub(super) fn label(self) -> String {
        match self {
            Stage::Downloading => "Downloading update".into(),
            Stage::Building { total: 0, .. } => "Building".into(),
            Stage::Building { done, total } => format!("Building {done}/{total}"),
            Stage::Installing => "Installing".into(),
            Stage::Restarting => "Restarting".into(),
        }
    }

    /// Share of the whole update that is done. Building takes nearly all of the time.
    pub(super) fn fraction(self) -> f32 {
        match self {
            Stage::Downloading => 0.02,
            Stage::Building { done, total } => 0.05 + 0.85 * done as f32 / total.max(1) as f32,
            Stage::Installing => 0.93,
            Stage::Restarting => 1.0,
        }
    }
}

pub(super) enum Event {
    Progress(Stage),
    /// `Ok`: the new build is installed and a watcher opens it once this process has exited,
    /// so the app must quit. `Err`: the update stopped early and this instance keeps running.
    Finished(Result<(), String>),
}

/// Pull and reinstall in a separate process group, so the script survives this app quitting.
/// Output goes to `scytify-update.log` in the temp directory; the stream reads it back to
/// report progress.
pub(super) fn install() -> impl Stream<Item = Event> {
    iced::stream::channel(8, |mut output: Sender<Event>| async move {
        let result = run_install(&mut output).await;
        let _ = output.send(Event::Finished(result)).await;
    })
}

async fn run_install(output: &mut Sender<Event>) -> Result<(), String> {
    let log_path = std::env::temp_dir().join("scytify-update.log");
    let log = File::create(&log_path).map_err(|e| e.to_string())?;
    let mut reader = File::open(&log_path).map_err(|e| e.to_string())?;
    let mut child = Command::new("sh")
        // Apps opened from a launcher get a minimal PATH without rustup's cargo.
        .args([
            "-c",
            r#"PATH="$HOME/.cargo/bin:$PATH"; echo "scytify-step: download"; git pull --ff-only && exec tools/install.sh"#,
        ])
        .current_dir(REPO)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("SCYTIFY_UPDATE_PID", std::process::id().to_string())
        // Cargo draws its `done/total` progress line only on a terminal unless asked.
        .env("CARGO_TERM_PROGRESS_WHEN", "always")
        .env("CARGO_TERM_PROGRESS_WIDTH", "100")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut parser = Parser::default();
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        // Checked before reading, so the output of a finished script is read in full.
        let status = child.try_wait().map_err(|e| e.to_string())?;
        bytes.clear();
        reader.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if let Some(stage) = parser.feed(&String::from_utf8_lossy(&bytes)) {
            let _ = output.send(Event::Progress(stage)).await;
        }
        if let Some(status) = status {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("Update failed; see {}", log_path.display()))
            };
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Follows the install script's output as it arrives in chunks.
#[derive(Default)]
struct Parser {
    /// Text after the last line break, still being written.
    partial: String,
    stage: Option<Stage>,
}

impl Parser {
    /// Read more output; returns the stage when it changed.
    fn feed(&mut self, text: &str) -> Option<Stage> {
        self.partial.push_str(text);
        let before = self.stage;
        // Cargo redraws its progress line after `\r` and leaves it unterminated until the next
        // redraw, so the unfinished tail is read too; it is only dropped once complete.
        for line in self.partial.split(['\n', '\r']) {
            if let Some(stage) = parse_line(line) {
                self.stage = Some(stage);
            }
        }
        if let Some(end) = self.partial.rfind(['\n', '\r']) {
            self.partial.drain(..=end);
        }
        (self.stage != before).then_some(self.stage).flatten()
    }
}

/// A `scytify-step:` marker from the scripts, or cargo's progress line,
/// `Building [=====>     ] 120/673: serde, tokio`.
fn parse_line(line: &str) -> Option<Stage> {
    let line = line.trim();
    if let Some(step) = line.strip_prefix("scytify-step: ") {
        return match step {
            "download" => Some(Stage::Downloading),
            "build" => Some(Stage::Building { done: 0, total: 0 }),
            "install" => Some(Stage::Installing),
            "restart" => Some(Stage::Restarting),
            _ => None,
        };
    }
    let counts = line.strip_prefix("Building [")?.split_once("] ")?.1;
    // Requiring the colon skips a redraw cut off mid-number.
    let (done, total) = counts.split_once(':')?.0.split_once('/')?;
    Some(Stage::Building {
        done: done.trim().parse().ok()?,
        total: total.trim().parse().ok()?,
    })
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
    use super::{Parser, Stage, installed_is_stale};

    #[test]
    fn stale_only_when_built_commit_differs_from_head() {
        assert!(installed_is_stale("aaa", "bbb"));
        assert!(!installed_is_stale("aaa", "aaa"));
        assert!(!installed_is_stale("", "bbb"));
    }

    #[test]
    fn progress_follows_markers_and_cargo_counts_across_chunks() {
        let mut parser = Parser::default();
        assert_eq!(
            parser.feed("scytify-step: download\nUpdating 85a7b66..84e963f\n"),
            Some(Stage::Downloading)
        );
        assert_eq!(
            parser.feed("scytify-step: build\n   Compiling serde v1.0\n"),
            Some(Stage::Building { done: 0, total: 0 })
        );
        // A redraw split across reads, then the unterminated latest one.
        assert_eq!(parser.feed("\r    Building [=>   ] 12/6"), None);
        assert_eq!(
            parser.feed("73: serde, tokio\r    Building [===> ] 672/673: scytify(bin)"),
            Some(Stage::Building { done: 672, total: 673 })
        );
        assert_eq!(parser.feed("\r      \r"), None);
        assert_eq!(
            parser.feed("    Finished `release`\nscytify-step: install\nInstalled\n"),
            Some(Stage::Installing)
        );
        assert_eq!(parser.feed("scytify-step: restart\n"), Some(Stage::Restarting));
    }
}
