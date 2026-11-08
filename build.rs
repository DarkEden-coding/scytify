//! Records the git commit being built as `SCYTIFY_BUILD_COMMIT`, so the running app can tell
//! whether the checkout has moved on since it was installed (see `frontend/update.rs`).
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A pull moves the branch ref (or `packed-refs`), not `HEAD`, so watch all three or the
    // recorded commit goes stale and the app keeps offering an update it already has.
    for path in ["HEAD", "packed-refs"] {
        watch_git_path(path);
    }
    if let Some(reference) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        watch_git_path(&reference);
    }
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=SCYTIFY_BUILD_COMMIT={commit}");
}

fn watch_git_path(path: &str) {
    if let Some(resolved) = git(&["rev-parse", "--git-path", path]) {
        println!("cargo:rerun-if-changed={resolved}");
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_owned())
}
