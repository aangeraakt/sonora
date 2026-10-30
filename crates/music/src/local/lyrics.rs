use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context as _, Result};
use async_trait::async_trait;

use crate::lyrics::{LOCAL, lrc, ttml};
use crate::{Lyrics, LyricsHit, LyricsProvider, LyricsQuery};

use super::{tags, wire};

/// Wins a tie against most services without outranking a better kind of sheet.
const TRUST: u32 = 100;

/// Extensions a lyrics file beside the track may carry, in the order they are tried. Linux
/// filesystems tell the cases apart.
const SIDECARS: [(&str, Format); 4] = [
    ("lrc", Format::Lrc),
    ("LRC", Format::Lrc),
    ("ttml", Format::Ttml),
    ("TTML", Format::Ttml),
];

/// How a lyrics file beside the track is written.
#[derive(Clone, Copy)]
enum Format {
    Lrc,
    Ttml,
}

/// Reads the lyrics of a local file: an `.lrc` or `.ttml` file beside it, else what its own tags
/// carry. Any other track gets no answer.
pub struct LocalLyrics;

#[async_trait]
impl LyricsProvider for LocalLyrics {
    fn name(&self) -> &'static str {
        LOCAL
    }

    async fn search(&self, query: &LyricsQuery) -> Result<Vec<LyricsHit>> {
        let Some(path) = query
            .track
            .as_ref()
            .and_then(|track| wire::path_from_track_id(&track.id))
        else {
            return Ok(Vec::new());
        };
        let path = path.to_path_buf();
        let read = tokio::task::spawn_blocking(move || read(&path));
        let Some(lyrics) = read.await.context("local lyrics task panicked")?? else {
            return Ok(Vec::new());
        };
        Ok(vec![LyricsHit {
            source: LOCAL,
            trust: TRUST,
            lyrics,
            instrumental: false,
            title: query.title.clone(),
            artist: query.artist.clone(),
            album: query.album.clone(),
            duration: Some(query.duration),
            writers: Vec::new(),
        }])
    }
}

/// A lyrics file of the same name wins, since someone put it there for this track; the tags are
/// the fallback when there is none, or when none holds any lyrics.
fn read(path: &Path) -> Result<Option<Lyrics>> {
    match sidecar(path) {
        Some(lyrics) => Ok(Some(lyrics)),
        None => tags::lyrics(path),
    }
}

/// Lyrics from `Song.lrc` or `Song.ttml` beside `Song.flac`, the first that decodes. A file that
/// cannot be read or decoded is logged and skipped rather than failing the lookup, so the next
/// file and then the tags still get their turn.
fn sidecar(path: &Path) -> Option<Lyrics> {
    SIDECARS.iter().find_map(|(extension, format)| {
        let sidecar = path.with_extension(extension);
        let text = match fs::read_to_string(&sidecar) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return None,
            Err(error) => {
                log::warn!("lyrics: cannot read {}: {error}", sidecar.display());
                return None;
            }
        };
        let lyrics = decode(&text, *format);
        if lyrics.is_none() {
            log::warn!("lyrics: {} holds no valid lyrics", sidecar.display());
        }
        lyrics
    })
}

/// Decodes a lyrics file, or `None` when it holds no lyrics at all.
fn decode(text: &str, format: Format) -> Option<Lyrics> {
    let text = text.trim_start_matches('\u{feff}');
    match format {
        Format::Lrc => {
            let lines = lrc::parse(text);
            (!lines.is_empty()).then(|| Lyrics::Synced {
                lines: lines.into(),
            })
        }
        Format::Ttml => ttml::parse(text)
            .inspect_err(|error| log::debug!("lyrics: {error:#}"))
            .ok()
            .map(|sheet| sheet.lyrics)
            .filter(|lyrics| !lyrics.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const TIMED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">
  <body>
    <div>
      <p begin="00:01.000" end="00:03.500"><span begin="00:01.000" end="00:02.000">First </span><span begin="00:02.000" end="00:03.500">line</span></p>
      <p begin="00:04.000" end="00:06.000">Second line</p>
    </div>
  </body>
</tt>"#;

    fn track(folder: &Path) -> std::path::PathBuf {
        folder.join("Song.flac")
    }

    #[test]
    fn a_timed_ttml_beside_the_track_is_read() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        fs::write(folder.path().join("Song.ttml"), TIMED).expect("the ttml is written");

        let Some(Lyrics::Synced { lines }) = sidecar(&track(folder.path())) else {
            panic!("the ttml should decode to timed lyrics");
        };

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "First line");
        assert_eq!(lines[0].start, Duration::from_secs(1));
        assert_eq!(lines[0].words.as_ref().map(Vec::len), Some(2));
        assert_eq!(lines[1].text, "Second line");
        assert_eq!(lines[1].start, Duration::from_secs(4));
    }

    #[test]
    fn a_broken_ttml_is_skipped() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        fs::write(folder.path().join("Song.ttml"), "<tt><body><p begin=").expect("written");

        assert!(sidecar(&track(folder.path())).is_none());
    }

    #[test]
    fn an_lrc_wins_over_a_ttml() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        fs::write(folder.path().join("Song.ttml"), TIMED).expect("the ttml is written");
        fs::write(folder.path().join("Song.lrc"), "[00:10.00]From the lrc").expect("written");

        let Some(Lyrics::Synced { lines }) = sidecar(&track(folder.path())) else {
            panic!("the lrc should decode to timed lyrics");
        };

        assert_eq!(lines[0].text, "From the lrc");
    }
}
