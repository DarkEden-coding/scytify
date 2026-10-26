//! One-time move of per-user files from the app's former name ("Custom Spotify") to Scytify.
//! Saved credentials are moved by the vault in `backend::auth`.
use directories::ProjectDirs;
use std::path::Path;

/// Moves the old data and config directories into their Scytify locations. Safe to run on
/// every launch: entries already present in the new directory are kept and never overwritten.
pub fn run() {
    let moves = [
        (
            ProjectDirs::from("dev", "CustomSpotify", "CustomSpotify")
                .map(|d| d.data_local_dir().to_owned()),
            ProjectDirs::from("dev", "Scytify", "Scytify").map(|d| d.data_local_dir().to_owned()),
        ),
        (
            ProjectDirs::from("", "CustomSpotify", "CustomSpotify")
                .map(|d| d.config_dir().to_owned()),
            ProjectDirs::from("", "Scytify", "Scytify").map(|d| d.config_dir().to_owned()),
        ),
    ];
    for (old, new) in moves {
        if let (Some(old), Some(new)) = (old, new)
            && old != new
            && let Err(error) = move_dir(&old, &new)
        {
            eprintln!("Could not migrate {}: {error}", old.display());
        }
    }
}

/// Moves each entry of `old` into `new` unless it already exists there, then removes `old`
/// if nothing is left in it.
fn move_dir(old: &Path, new: &Path) -> std::io::Result<()> {
    if !old.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(new)?;
    for entry in std::fs::read_dir(old)? {
        let entry = entry?;
        let target = new.join(entry.file_name());
        if !target.exists() {
            std::fs::rename(entry.path(), target)?;
        }
    }
    // Fails, leaving the directory, if skipped entries remain.
    let _ = std::fs::remove_dir(old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("scytify-migrate-{name}-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moves_entries_and_keeps_existing_ones() {
        let root = scratch("move");
        let (old, new) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("metadata.sqlite3"), "old").unwrap();
        std::fs::write(old.join("shared"), "old").unwrap();
        std::fs::write(new.join("shared"), "new").unwrap();

        move_dir(&old, &new).unwrap();

        assert_eq!(
            std::fs::read_to_string(new.join("metadata.sqlite3")).unwrap(),
            "old"
        );
        assert_eq!(std::fs::read_to_string(new.join("shared")).unwrap(), "new");
        assert!(old.join("shared").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_old_directory_is_a_no_op() {
        let root = scratch("missing");
        move_dir(&root.join("old"), &root.join("new")).unwrap();
        assert!(!root.join("new").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
