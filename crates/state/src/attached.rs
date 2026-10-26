//! The lyrics files listeners attach to tracks themselves. `state.sqlite` keeps only where each
//! file is, and the file is read again on every play, so an edit to it shows the next time the
//! track plays.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use music::Lyrics as Sheet;
use rusqlite::{Connection, params};
use storage::Database;

/// The attached files by track. Every method blocks on sqlite, so a caller runs it off the main
/// thread.
#[derive(Clone)]
pub(crate) struct Attached {
    database: Database,
}

impl Attached {
    pub(crate) fn new(database: Database) -> Self {
        Self { database }
    }

    fn open(&self) -> Result<Connection> {
        self.database
            .open()
            .context("cannot open the attached lyrics files")
    }

    /// Every attached file, by the track key it belongs to.
    pub(crate) fn all(&self) -> Result<HashMap<String, PathBuf>> {
        let connection = self.open()?;
        let mut query = connection
            .prepare("SELECT track_key, path FROM lyrics_files")
            .context("cannot read the attached lyrics files")?;
        let rows = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .context("cannot read the attached lyrics files")?;
        rows.map(|row| row.map(|(key, path)| (key, PathBuf::from(path))))
            .collect::<rusqlite::Result<_>>()
            .context("cannot read the attached lyrics files")
    }

    /// Attaches `path` to the track, in place of any file it had.
    pub(crate) fn set(&self, key: &str, path: &Path) -> Result<()> {
        self.open()?
            .execute(
                "INSERT OR REPLACE INTO lyrics_files (track_key, path) VALUES (?, ?)",
                params![key, path.to_string_lossy()],
            )
            .context("cannot attach a lyrics file")?;
        Ok(())
    }

    pub(crate) fn remove(&self, key: &str) -> Result<()> {
        self.open()?
            .execute("DELETE FROM lyrics_files WHERE track_key = ?", params![key])
            .context("cannot remove an attached lyrics file")?;
        Ok(())
    }
}

/// The lyrics in an attached file, timed or plain. A file that is not valid UTF-8 has its
/// broken bytes replaced rather than refused.
pub(crate) fn read(path: &Path) -> Result<Sheet> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    music::lyrics::lrc::sheet(&String::from_utf8_lossy(&bytes))
        .with_context(|| format!("cannot find lyrics in {}", path.display()))
}
