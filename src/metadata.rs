use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag};
use serde_json::Value;
use std::error::Error;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct LyricsData {
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<u32>,
    pub cover_bytes: Option<Vec<u8>>,
    pub lyrics: Option<LyricsData>,
}

pub struct MetadataFetcher {
    client: reqwest::Client,
}

impl MetadataFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("LucidaFlow/1.0.0 ( contact@lucidaflow.dev )")
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }

    /// Query MusicBrainz API for canonical recording metadata
    pub async fn fetch_musicbrainz(
        &self,
        title: &str,
        artist: Option<&str>,
    ) -> Result<Option<TrackMetadata>, Box<dyn Error + Send + Sync>> {
        let (clean_title, inferred_artist) = parse_artist_and_title(title, artist);
        let artist_to_use = inferred_artist.as_deref().or(artist);

        // Build search queries in order of specificity
        let mut queries = Vec::new();
        if let Some(art) = artist_to_use {
            let clean_art = clean_artist_name(art);
            if !clean_art.is_empty() {
                queries.push(format!("recording:\"{}\" AND artist:\"{}\"", clean_title, clean_art));
            }
        }
        queries.push(format!("recording:\"{}\"", clean_title));
        queries.push(format!("\"{}\"", clean_title));

        let mut recordings = Vec::new();

        for q in queries {
            let mb_url = format!("https://musicbrainz.org/ws/2/recording");
            let req = self.client.get(&mb_url)
                .query(&[("query", &q), ("fmt", &"json".to_string()), ("limit", &"10".to_string())])
                .header("Accept", "application/json");

            if let Ok(resp) = req.send().await {
                if resp.status().is_success() {
                    if let Ok(json) = resp.json::<Value>().await {
                        if let Some(arr) = json["recordings"].as_array() {
                            if !arr.is_empty() {
                                recordings = arr.clone();
                                break;
                            }
                        }
                    }
                }
            }
        }

        if recordings.is_empty() {
            return Ok(None);
        }

        // Pick the recording that best matches our expected artist (preferring non-video audio tracks)
        let rec = if let Some(art) = artist_to_use {
            let art_lower = art.to_lowercase();
            recordings
                .iter()
                .find(|r| {
                    let not_video = r["video"].as_bool() != Some(true);
                    let matches_art = r["artist-credit"].as_array().map_or(false, |artists| {
                        artists.iter().any(|a| {
                            let nl = a["name"].as_str().unwrap_or("").to_lowercase();
                            nl.contains(&art_lower) || art_lower.contains(&nl)
                        })
                    });
                    not_video && matches_art
                })
                .or_else(|| {
                    recordings.iter().find(|r| {
                        r["artist-credit"].as_array().map_or(false, |artists| {
                            artists.iter().any(|a| {
                                let nl = a["name"].as_str().unwrap_or("").to_lowercase();
                                nl.contains(&art_lower) || art_lower.contains(&nl)
                            })
                        })
                    })
                })
                .unwrap_or(&recordings[0])
        } else {
            &recordings[0]
        };

        let mb_title = rec["title"].as_str().unwrap_or(&clean_title).to_string();

        let mb_artist = rec["artist-credit"]
            .as_array()
            .and_then(|artists| {
                let names: Vec<_> = artists
                    .iter()
                    .filter_map(|a| a["name"].as_str())
                    .collect();
                if !names.is_empty() {
                    Some(names.join(", "))
                } else {
                    None
                }
            })
            .or_else(|| artist_to_use.map(|a| a.to_string()))
            .unwrap_or_else(|| "Unknown Artist".to_string());

        let mut mb_album = "Single".to_string();
        let mut mb_year: Option<u32> = None;
        let mut cover_bytes = None;
        let mut release_group_id = None;

        if let Some(releases) = rec["releases"].as_array() {
            for rel in releases {
                if mb_album == "Single" {
                    if let Some(t) = rel["title"].as_str() {
                        mb_album = t.to_string();
                    }
                }
                if mb_year.is_none() {
                    if let Some(date_str) = rel["date"].as_str() {
                        if date_str.len() >= 4 {
                            mb_year = date_str[..4].parse::<u32>().ok();
                        }
                    }
                }
                if release_group_id.is_none() {
                    release_group_id = rel["release-group"]["id"].as_str().map(|s| s.to_string());
                }

                // Check Cover Art Archive for this release
                if cover_bytes.is_none() {
                    if let Some(rid) = rel["id"].as_str() {
                        if let Ok(Some(bytes)) = self.fetch_cover_art_archive(rid, false).await {
                            cover_bytes = Some(bytes);
                            if let Some(t) = rel["title"].as_str() {
                                mb_album = t.to_string();
                            }
                            if let Some(date_str) = rel["date"].as_str() {
                                if date_str.len() >= 4 {
                                    mb_year = date_str[..4].parse::<u32>().ok();
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }

        // Check release-group on Cover Art Archive
        if cover_bytes.is_none() {
            if let Some(ref rgid) = release_group_id {
                if let Ok(Some(bytes)) = self.fetch_cover_art_archive(rgid, true).await {
                    cover_bytes = Some(bytes);
                }
            }
        }

        // Scan other matching recordings in the search results for Cover Art Archive releases
        if cover_bytes.is_none() {
            for other in &recordings {
                if let Some(releases) = other["releases"].as_array() {
                    for rel in releases {
                        if let Some(rid) = rel["id"].as_str() {
                            if let Ok(Some(bytes)) = self.fetch_cover_art_archive(rid, false).await {
                                cover_bytes = Some(bytes);
                                if mb_album == "Single" {
                                    if let Some(t) = rel["title"].as_str() {
                                        mb_album = t.to_string();
                                    }
                                }
                                break;
                            }
                        }
                        if let Some(rgid) = rel["release-group"]["id"].as_str() {
                            if let Ok(Some(bytes)) = self.fetch_cover_art_archive(rgid, true).await {
                                cover_bytes = Some(bytes);
                                break;
                            }
                        }
                    }
                }
                if cover_bytes.is_some() {
                    break;
                }
            }
        }

        Ok(Some(TrackMetadata {
            title: mb_title,
            artist: mb_artist,
            album: mb_album,
            year: mb_year,
            cover_bytes,
            lyrics: None,
        }))
    }

    /// Fetch cover image bytes from Cover Art Archive
    pub async fn fetch_cover_art_archive(
        &self,
        mbid: &str,
        is_release_group: bool,
    ) -> Result<Option<Vec<u8>>, Box<dyn Error + Send + Sync>> {
        let entity = if is_release_group {
            "release-group"
        } else {
            "release"
        };
        let info_url = format!("https://coverartarchive.org/{}/{}", entity, mbid);

        let resp = match self.client.get(&info_url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(None),
        };

        let json: Value = match resp.json().await {
            Ok(j) => j,
            Err(_) => return Ok(None),
        };

        let image_url = json["images"]
            .as_array()
            .and_then(|images| {
                // Find front cover or fallback to first image
                images
                    .iter()
                    .find(|img| img["front"].as_bool() == Some(true))
                    .or_else(|| images.first())
            })
            .and_then(|img| {
                img["thumbnails"]["500"]
                    .as_str()
                    .or_else(|| img["thumbnails"]["large"].as_str())
                    .or_else(|| img["image"].as_str())
            });

        if let Some(url) = image_url {
            if let Ok(img_resp) = self.client.get(url).send().await {
                if img_resp.status().is_success() {
                    let bytes = img_resp.bytes().await?.to_vec();
                    return Ok(Some(bytes));
                }
            }
        }

        Ok(None)
    }

    /// Fetch YouTube high-resolution thumbnail as fallback cover art
    pub async fn fetch_youtube_thumbnail(
        &self,
        video_id: &str,
    ) -> Result<Option<Vec<u8>>, Box<dyn Error + Send + Sync>> {
        let candidates = [
            format!("https://i.ytimg.com/vi/{}/maxresdefault.jpg", video_id),
            format!("https://i.ytimg.com/vi/{}/sddefault.jpg", video_id),
            format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id),
        ];

        for url in &candidates {
            if let Ok(resp) = self.client.get(url).send().await {
                if resp.status().is_success() {
                    let bytes = resp.bytes().await?.to_vec();
                    // YouTube returns small placeholder if maxres doesn't exist (<1000 bytes)
                    if bytes.len() > 2048 {
                        return Ok(Some(bytes));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Search YouTube for top video thumbnail if Cover Art Archive has no art
    pub async fn fetch_youtube_search_thumbnail(&self, query: &str) -> Option<Vec<u8>> {
        let yt = crate::youtube::YouTubeClient::new();
        if let Ok(tracks) = yt.search(query, 1).await {
            if let Some(first) = tracks.first() {
                if let Ok(Some(bytes)) = self.fetch_youtube_thumbnail(&first.video_id).await {
                    return Some(bytes);
                }
            }
        }
        None
    }

    /// Fetch cover image from an arbitrary direct image URL
    #[allow(dead_code)]
    /// Query LRCLIB API for plain and synchronized (.lrc) lyrics
    pub async fn fetch_lrclib(
        &self,
        title: &str,
        artist: Option<&str>,
        album: Option<&str>,
        duration: Option<f64>,
    ) -> Option<LyricsData> {
        let clean_t = clean_track_title(title);
        let clean_a = artist.map(clean_artist_name).unwrap_or_default();

        // 1. Try exact /api/get if artist is known
        if !clean_a.is_empty() {
            let mut req = self.client.get("https://lrclib.net/api/get")
                .query(&[("track_name", &clean_t), ("artist_name", &clean_a)]);

            if let Some(alb) = album {
                if !alb.is_empty() && alb != "Single" {
                    req = req.query(&[("album_name", alb)]);
                }
            }
            if let Some(d) = duration {
                let d_str = (d as u64).to_string();
                req = req.query(&[("duration", &d_str)]);
            }

            if let Ok(resp) = req.send().await {
                if resp.status().is_success() {
                    if let Ok(json) = resp.json::<Value>().await {
                        let plain = json["plainLyrics"].as_str().map(|s| s.to_string());
                        let synced = json["syncedLyrics"].as_str().map(|s| s.to_string());
                        if plain.is_some() || synced.is_some() {
                            return Some(LyricsData {
                                plain_lyrics: plain,
                                synced_lyrics: synced,
                                track_name: json["trackName"].as_str().map(|s| s.to_string()),
                                artist_name: json["artistName"].as_str().map(|s| s.to_string()),
                                album_name: json["albumName"].as_str().map(|s| s.to_string()),
                            });
                        }
                    }
                }
            }
        }

        // 2. Fallback to /api/search with track_name and artist_name or full query
        let mut search_req = self.client.get("https://lrclib.net/api/search");
        if !clean_a.is_empty() {
            search_req = search_req.query(&[("track_name", &clean_t), ("artist_name", &clean_a)]);
        } else {
            search_req = search_req.query(&[("q", &clean_t)]);
        }

        if let Ok(resp) = search_req.send().await {
            if resp.status().is_success() {
                if let Ok(json) = resp.json::<Value>().await {
                    if let Some(arr) = json.as_array() {
                        if let Some(first) = arr.first() {
                            let plain = first["plainLyrics"].as_str().map(|s| s.to_string());
                            let synced = first["syncedLyrics"].as_str().map(|s| s.to_string());
                            if plain.is_some() || synced.is_some() {
                                return Some(LyricsData {
                                    plain_lyrics: plain,
                                    synced_lyrics: synced,
                                    track_name: first["trackName"].as_str().map(|s| s.to_string()),
                                    artist_name: first["artistName"].as_str().map(|s| s.to_string()),
                                    album_name: first["albumName"].as_str().map(|s| s.to_string()),
                                });
                            }
                        }
                    }
                }
            }
        }

        None
    }
}

/// Parse artist and title from song title strings like "Artist - Title" or "Title | Artist"
pub fn parse_artist_and_title(title: &str, explicit_artist: Option<&str>) -> (String, Option<String>) {
    let cleaned = clean_track_title(title);

    // If explicit artist is already provided and not generic
    let explicit_clean = explicit_artist
        .map(clean_artist_name)
        .filter(|a| !a.is_empty() && a != "Unknown Artist");

    // Check for "Artist - Title" separator (e.g. "Rick Astley - Never Gonna Give You Up")
    if let Some(pos) = cleaned.find(" - ") {
        let artist_part = clean_artist_name(&cleaned[..pos]);
        let title_part = cleaned[pos + 3..].trim().to_string();
        if !artist_part.is_empty() && !title_part.is_empty() {
            return (title_part, Some(artist_part));
        }
    }

    // Check for "Title | Artist" separator
    if let Some(pos) = cleaned.find(" | ") {
        let first = cleaned[..pos].trim().to_string();
        let second = clean_artist_name(&cleaned[pos + 3..]);
        if !first.is_empty() {
            let artist = if !second.is_empty() {
                Some(second)
            } else {
                explicit_clean
            };
            return (first, artist);
        }
    }

    let final_title = cleaned.trim().to_string();
    (final_title, explicit_clean)
}

/// Strip common YouTube junk, audio extensions, and video tags for accurate music metadata matching
pub fn clean_track_title(title: &str) -> String {
    // Normalize Unicode dashes (en-dash, em-dash, minus) to standard hyphen
    let mut s = title.replace(['\u{2013}', '\u{2014}', '\u{2212}'], "-");

    // Strip audio file extensions (.flac, .mp3, etc.)
    for ext in &[".flac", ".mp3", ".m4a", ".opus", ".wav", ".aac", ".ogg"] {
        if s.to_lowercase().ends_with(ext) {
            s.truncate(s.len() - ext.len());
        }
    }

    // Remove text inside brackets/parentheses like (Official Video), [Lyrics], etc.
    let patterns = [
        r"(?i)\(official\s+(music\s+)?video\)",
        r"(?i)\[official\s+(music\s+)?video\]",
        r"(?i)\(official\s+audio\)",
        r"(?i)\[official\s+audio\]",
        r"(?i)\(official\s+lyric\s+video\)",
        r"(?i)\[official\s+lyric\s+video\]",
        r"(?i)\(lyrics?\)",
        r"(?i)\[lyrics?\]",
        r"(?i)\(lyric\s+video\)",
        r"(?i)\[lyric\s+video\]",
        r"(?i)\(visualizer\)",
        r"(?i)\[visualizer\]",
        r"(?i)\(hd\)",
        r"(?i)\[hd\]",
        r"(?i)\(4k\)",
        r"(?i)\[4k\]",
        r"(?i)\(sped\s+up\)",
        r"(?i)\(slowed\s*\+?\s*reverb\)",
        r"(?i)\(audio\)",
        r"(?i)\[audio\]",
    ];

    for pat in patterns {
        if let Ok(re) = regex::Regex::new(pat) {
            s = re.replace_all(&s, "").to_string();
        }
    }

    s.trim().to_string()
}

pub fn clean_artist_name(artist: &str) -> String {
    let mut s = artist.to_string();
    if let Some(pos) = s.find(" - Topic") {
        s = s[..pos].to_string();
    }
    if let Some(pos) = s.find("VEVO") {
        s = s[..pos].to_string();
    }
    s.trim().to_string()
}

/// Tag an audio file (.mp3, .flac, .m4a, etc.) with metadata, cover art, and lyrics
pub fn tag_audio_file(
    filepath: &Path,
    metadata: &TrackMetadata,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut tagged_file = Probe::open(filepath)?.read()?;

    let tag = match tagged_file.primary_tag_mut() {
        Some(primary) => primary,
        None => {
            if let Some(first) = tagged_file.first_tag_mut() {
                first
            } else {
                let tag_type = tagged_file.primary_tag_type();
                tagged_file.insert_tag(Tag::new(tag_type));
                tagged_file.primary_tag_mut().ok_or("Failed to create tag")?
            }
        }
    };

    tag.insert_text(ItemKey::TrackTitle, metadata.title.clone());
    tag.insert_text(ItemKey::TrackArtist, metadata.artist.clone());
    tag.insert_text(ItemKey::AlbumTitle, metadata.album.clone());

    if let Some(year) = metadata.year {
        tag.insert_text(ItemKey::Year, year.to_string());
    }

    if let Some(ref cover) = metadata.cover_bytes {
        let mime = if cover.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
            MimeType::Png
        } else {
            MimeType::Jpeg
        };
        let pic = Picture::unchecked(cover.clone())
            .pic_type(PictureType::CoverFront)
            .mime_type(mime)
            .build();
        tag.push_picture(pic);
    }

    // Embed lyrics into audio tag (USLT for ID3v2, LYRICS for Vorbis/FLAC)
    if let Some(ref lyr) = metadata.lyrics {
        let text_to_embed = lyr.synced_lyrics.as_deref().or(lyr.plain_lyrics.as_deref());
        if let Some(text) = text_to_embed {
            tag.insert_text(ItemKey::Lyrics, text.to_string());
        }
    }

    tag.save_to_path(filepath, WriteOptions::default())?;

    // Also write synchronized .lrc file alongside the audio file for media players
    if let Some(ref lyr) = metadata.lyrics {
        let content = lyr.synced_lyrics.as_deref().or(lyr.plain_lyrics.as_deref());
        if let Some(c) = content {
            let lrc_path = filepath.with_extension("lrc");
            let _ = std::fs::write(&lrc_path, c);
        }
    }

    Ok(())
}

/// Complete pipeline step: fetch MusicBrainz, Cover Art Archive & LRCLIB and tag the file
pub async fn enrich_and_tag_audio(
    filepath: &Path,
    initial_title: &str,
    initial_artist: Option<&str>,
    initial_album: Option<&str>,
    yt_video_id: Option<&str>,
) -> Result<TrackMetadata, Box<dyn Error + Send + Sync>> {
    let fetcher = MetadataFetcher::new();

    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} Fetching metadata, cover art & lyrics ({msg})...")?);
    pb.set_message("MusicBrainz, Cover Art Archive & LRCLIB");
    pb.enable_steady_tick(Duration::from_millis(80));

    let (clean_title, inferred_artist) = parse_artist_and_title(initial_title, initial_artist);
    let artist_to_use = inferred_artist.as_deref().or(initial_artist);

    // Concurrently fetch MusicBrainz metadata + LRCLIB lyrics in parallel
    let (mb_res, lyrics_res) = tokio::join!(
        fetcher.fetch_musicbrainz(initial_title, initial_artist),
        fetcher.fetch_lrclib(&clean_title, artist_to_use, initial_album, None)
    );

    let mut final_metadata = if let Ok(Some(mut mb)) = mb_res {
        mb.lyrics = lyrics_res;
        mb
    } else {
        TrackMetadata {
            title: clean_title,
            artist: artist_to_use.unwrap_or("Unknown Artist").to_string(),
            album: initial_album.unwrap_or("Single").to_string(),
            year: None,
            cover_bytes: None,
            lyrics: lyrics_res,
        }
    };

    // If title still contains remnants like extensions, clean it up
    final_metadata.title = clean_track_title(&final_metadata.title);

    // If LRCLIB gave cleaner canonical title or artist, use it if missing
    if let Some(ref lyr) = final_metadata.lyrics {
        if let Some(ref t) = lyr.track_name {
            if final_metadata.title.contains(" - ") || final_metadata.title.eq_ignore_ascii_case("track") {
                final_metadata.title = t.clone();
            }
        }
        if let Some(ref a) = lyr.artist_name {
            if final_metadata.artist == "Unknown Artist" {
                final_metadata.artist = a.clone();
            }
        }
        if let Some(ref alb) = lyr.album_name {
            if final_metadata.album == "Single" && !alb.is_empty() {
                final_metadata.album = alb.clone();
            }
        }
    }

    // 2. If cover art was missing from Cover Art Archive, try YouTube thumbnail as fallback
    if final_metadata.cover_bytes.is_none() {
        if let Some(vid) = yt_video_id {
            if let Ok(Some(bytes)) = fetcher.fetch_youtube_thumbnail(vid).await {
                final_metadata.cover_bytes = Some(bytes);
            }
        } else {
            let search_term = format!("{} {}", final_metadata.title, final_metadata.artist);
            if let Some(bytes) = fetcher.fetch_youtube_search_thumbnail(&search_term).await {
                final_metadata.cover_bytes = Some(bytes);
            }
        }
    }

    pb.finish_and_clear();

    // 3. Tag the audio file using lofty and save .lrc
    match tag_audio_file(filepath, &final_metadata) {
        Ok(_) => {
            println!("  {} Embedded metadata, cover art & lyrics successfully!", "✓".green());
            println!("      {:<14} {}", "Title:".bold(), final_metadata.title.bold());
            println!("      {:<14} {}", "Artist:".bold(), final_metadata.artist.cyan());
            println!("      {:<14} {}", "Album:".bold(), final_metadata.album.yellow());
            if let Some(y) = final_metadata.year {
                println!("      {:<14} {}", "Year:".bold(), y.to_string().magenta());
            }
            if final_metadata.cover_bytes.is_some() {
                println!("      {:<14} {}", "Cover Art:".bold(), "Embedded Front Cover Art (JPEG)".green());
            }
            if let Some(ref lyr) = final_metadata.lyrics {
                if lyr.synced_lyrics.is_some() {
                    println!("      {:<14} {}", "Lyrics:".bold(), "Synchronized .lrc saved & embedded (LRCLIB)".cyan());
                } else if lyr.plain_lyrics.is_some() {
                    println!("      {:<14} {}", "Lyrics:".bold(), "Plain lyrics saved & embedded (LRCLIB)".cyan());
                }
            }
        }
        Err(e) => {
            eprintln!("  {} Could not tag audio file: {}", "⚠".yellow(), e);
        }
    }

    Ok(final_metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_artist_and_title() {
        let (title, artist) = parse_artist_and_title("Rick Astley - Never Gonna Give You Up (Lyrics).flac", Some("Amazing Lyrics"));
        assert_eq!(title, "Never Gonna Give You Up");
        assert_eq!(artist, Some("Rick Astley".to_string()));

        let (title2, artist2) = parse_artist_and_title("TWISTED, Oliver Tree - WORTH NOTHING (Official Music Video)", None);
        assert_eq!(title2, "WORTH NOTHING");
        assert_eq!(artist2, Some("TWISTED, Oliver Tree".to_string()));
    }

    #[tokio::test]
    async fn test_lrclib_query() {
        let fetcher = MetadataFetcher::new();
        let lyrics = fetcher.fetch_lrclib("Never Gonna Give You Up", Some("Rick Astley"), None, None).await;
        assert!(lyrics.is_some());
        let l = lyrics.unwrap();
        assert!(l.plain_lyrics.is_some() || l.synced_lyrics.is_some());
        println!("LRCLIB Track: {:?}", l.track_name);
        println!("LRCLIB Has Synced: {}", l.synced_lyrics.is_some());
    }

    #[tokio::test]
    async fn test_musicbrainz_query() {
        let fetcher = MetadataFetcher::new();
        let meta = fetcher.fetch_musicbrainz("Never Gonna Give You Up", Some("Rick Astley")).await.unwrap();
        assert!(meta.is_some());
        let m = meta.unwrap();
        println!("Resolved Title: {}", m.title);
        println!("Resolved Artist: {}", m.artist);
        println!("Resolved Album: {}", m.album);
        println!("Resolved Year: {:?}", m.year);
        println!("Cover bytes len: {:?}", m.cover_bytes.as_ref().map(|b| b.len()));
        assert_eq!(m.title, "Never Gonna Give You Up");
        assert!(m.artist.contains("Rick Astley"));
    }
}
