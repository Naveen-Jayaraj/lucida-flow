use colored::*;
use futures_util::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::fuzzy;
use crate::lucida::LucidaClient;
use crate::metadata;
use crate::youtube::{YouTubeClient, YouTubeTrack};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyApiResponse {
    pub success: Option<bool>,
    pub provider: Option<String>,
    pub playlist: Option<SpotifyPlaylist>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyPlaylist {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub total_tracks: Option<usize>,
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub tracks: Vec<SpotifyTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: Option<String>,
    pub duration_ms: Option<u64>,
    pub explicit: Option<bool>,
    pub preview_url: Option<String>,
    pub url: Option<String>,
}

pub struct SpotifyClient {
    client: reqwest::Client,
}

impl SpotifyClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            .timeout(Duration::from_secs(30))
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .pool_max_idle_per_host(10)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }

    /// Fetch playlist details and full tracklist from Spotify API proxy
    pub async fn fetch_playlist(&self, playlist_id: &str) -> Result<SpotifyPlaylist, Box<dyn Error + Send + Sync>> {
        let url = format!("https://spotify.xwolf.space/api/playlist/{}", playlist_id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(format!("Spotify API returned HTTP status {}", resp.status()).into());
        }

        let body = resp.text().await?;
        let data: SpotifyApiResponse = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse Spotify API JSON: {} (response: {})", e, &body[..body.len().min(200)]))?;

        if data.success == Some(false) {
            let err = data.error.unwrap_or_else(|| "Could not fetch playlist info.".to_string());
            return Err(format!("Spotify API error: {}", err).into());
        }

        let mut pl = data.playlist.ok_or_else(|| "Missing playlist object in Spotify API response".to_string())?;

        // Normalize non-breaking spaces and whitespace
        for t in &mut pl.tracks {
            t.title = t.title.replace('\u{00a0}', " ").trim().to_string();
            t.artist = t.artist.replace('\u{00a0}', " ").trim().to_string();
        }

        Ok(pl)
    }
}

/// Parse Spotify playlist ID from direct ID, web URL, or URI
pub fn extract_playlist_id(input: &str) -> Option<String> {
    let trimmed = input.trim();

    // 1. spotify:playlist:ID format
    if let Some(rest) = trimmed.strip_prefix("spotify:playlist:") {
        let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        if !id.is_empty() {
            return Some(id);
        }
    }

    // 2. https://open.spotify.com/playlist/<id>?si=...
    if let Ok(url) = url::Url::parse(trimmed) {
        if let Some(segments) = url.path_segments() {
            let seg_vec: Vec<&str> = segments.collect();
            for (i, &seg) in seg_vec.iter().enumerate() {
                if seg == "playlist" && i + 1 < seg_vec.len() {
                    let id = seg_vec[i + 1].split('?').next().unwrap_or("").trim();
                    let clean_id: String = id.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                    if !clean_id.is_empty() {
                        return Some(clean_id);
                    }
                }
            }
        }
    }

    // 3. Fallback regex for "playlist/<id>" in case URL has no http scheme (e.g. open.spotify.com/playlist/...)
    if let Ok(re) = Regex::new(r"playlist/([a-zA-Z0-9]+)") {
        if let Some(caps) = re.captures(trimmed) {
            if let Some(m) = caps.get(1) {
                return Some(m.as_str().to_string());
            }
        }
    }

    // 4. Raw base62 Spotify ID (typically 22 alphanumeric characters, e.g. 37i9dQZF1DX3bcfiyW6qms)
    if trimmed.len() >= 15 && trimmed.len() <= 32 && trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(trimmed.to_string());
    }

    None
}

/// Check if a string is a Spotify playlist link or URI
pub fn is_spotify_playlist(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.contains("spotify.com/playlist/")
        || trimmed.starts_with("spotify:playlist:")
        || (trimmed.contains("spotify") && trimmed.contains("playlist"))
}

/// Sanitize folder name for cross-platform filesystem (Windows, macOS, Linux)
pub fn sanitize_folder_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            other => other,
        })
        .collect();

    let trimmed = safe.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "Spotify_Playlist".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Sanitize audio filename (removes filesystem forbidden chars)
pub fn sanitize_filename(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            other => other,
        })
        .collect();

    let trimmed = safe.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "track".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Parse time string (e.g. "4:04", "1:23:45") into total seconds
pub fn parse_duration_to_secs(d: &str) -> Option<u64> {
    let parts: Vec<&str> = d.split(':').collect();
    if parts.len() == 2 {
        let m: u64 = parts[0].trim().parse().ok()?;
        let s: u64 = parts[1].trim().parse().ok()?;
        Some(m * 60 + s)
    } else if parts.len() == 3 {
        let h: u64 = parts[0].trim().parse().ok()?;
        let m: u64 = parts[1].trim().parse().ok()?;
        let s: u64 = parts[2].trim().parse().ok()?;
        Some(h * 3600 + m * 60 + s)
    } else {
        None
    }
}

#[derive(Clone)]
enum CandidateTrack {
    Lucida {
        track: crate::lucida::Track,
        score: f64,
    },
    YouTube {
        track: YouTubeTrack,
        score: f64,
    },
}

impl CandidateTrack {
    fn score(&self) -> f64 {
        match self {
            CandidateTrack::Lucida { score, .. } => *score,
            CandidateTrack::YouTube { score, .. } => *score,
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum TrackDownloadStatus {
    Downloaded {
        title: String,
        artist: String,
        source: &'static str,
        size_bytes: u64,
        filepath: PathBuf,
    },
    SkippedAlreadyExists {
        title: String,
        filepath: PathBuf,
    },
    Failed {
        title: String,
        error: String,
    },
}

/// Tag audio file with authoritative Spotify metadata, high-res cover art, and LRCLIB lyrics
async fn tag_spotify_track(
    filepath: &Path,
    track: &SpotifyTrack,
    playlist_name: &str,
    playlist_thumbnail: Option<&str>,
    yt_video_id: Option<&str>,
) {
    let fetcher = metadata::MetadataFetcher::new();

    // 1. Fetch cover art (YouTube maxres thumbnail or Spotify playlist thumbnail)
    let mut cover_bytes = None;
    if let Some(vid) = yt_video_id {
        if let Ok(Some(bytes)) = fetcher.fetch_youtube_thumbnail(vid).await {
            cover_bytes = Some(bytes);
        }
    }
    if cover_bytes.is_none() {
        if let Some(thumb_url) = playlist_thumbnail {
            let client = reqwest::Client::new();
            if let Ok(resp) = client.get(thumb_url).send().await {
                if resp.status().is_success() {
                    if let Ok(b) = resp.bytes().await {
                        cover_bytes = Some(b.to_vec());
                    }
                }
            }
        }
    }

    // 2. Fetch synchronized lyrics from LRCLIB
    let clean_title = metadata::clean_track_title(&track.title);
    let primary_artist = track.artist.split([',', '&', ';']).next().unwrap_or(&track.artist).trim();
    let lyrics = fetcher.fetch_lrclib(&clean_title, Some(primary_artist), None, None).await;

    let meta = metadata::TrackMetadata {
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: playlist_name.to_string(),
        year: None,
        cover_bytes,
        lyrics,
    };

    let _ = metadata::tag_audio_file(filepath, &meta);
}

/// Process a single track through the pipeline:
/// 1. Check if already exists in folder (instant resume)
/// 2. Parallel search Lucida & YouTube
/// 3. Rank candidates using fuzzy matching & duration validation
/// 4. Download best candidate (Lossless FLAC or 320kbps MP3) with automatic fallback
/// 5. Tag metadata, cover art and lyrics
async fn process_track(
    lucida_client: &LucidaClient,
    youtube_client: &YouTubeClient,
    track: &SpotifyTrack,
    _track_index: usize,
    _total_tracks: usize,
    playlist_name: &str,
    playlist_folder: &Path,
    playlist_thumbnail: Option<&str>,
    service: &str,
    source_pref: &str,
) -> TrackDownloadStatus {
    let primary_artist = track
        .artist
        .split([',', '&', ';'])
        .next()
        .unwrap_or(&track.artist)
        .trim();

    // Clean track title (strip extraneous file extensions or junk)
    let clean_title = metadata::clean_track_title(&track.title);

    // Target filename stem: "Artist - Title"
    let file_stem = if !primary_artist.is_empty() {
        format!("{} - {}", primary_artist, clean_title)
    } else {
        clean_title.clone()
    };
    let safe_stem = sanitize_filename(&file_stem);

    // Step 1: Deduplication check - if already downloaded with non-trivial size, skip
    for ext in &["mp3", "flac", "m4a", "opus"] {
        let existing = playlist_folder.join(format!("{}.{}", safe_stem, ext));
        if existing.exists() {
            if let Ok(meta) = fs::metadata(&existing) {
                if meta.len() > 100_000 {
                    return TrackDownloadStatus::SkippedAlreadyExists {
                        title: track.title.clone(),
                        filepath: existing,
                    };
                }
            }
        }
    }

    // Step 2: Build search query and run parallel Lucida + YouTube search
    let search_query = if !primary_artist.is_empty() {
        format!("{} {}", track.title, primary_artist)
    } else {
        track.title.clone()
    };

    let target_duration_secs = track
        .duration_ms
        .map(|ms| ms / 1000)
        .or_else(|| track.duration.as_deref().and_then(parse_duration_to_secs));

    let search_lucida = source_pref == "auto" || source_pref == "lucida";
    let search_youtube = source_pref == "auto" || source_pref == "youtube";

    let (lucida_res, youtube_res) = tokio::join!(
        async {
            if search_lucida {
                lucida_client.search(&search_query, service, 5).await.ok()
            } else {
                None
            }
        },
        async {
            if search_youtube {
                youtube_client.search(&search_query, 5).await.ok()
            } else {
                None
            }
        }
    );

    // Step 3: Score candidates with fuzzy matching + duration verification
    let mut lucida_scored = Vec::new();
    if let Some(tracks) = lucida_res {
        for t in tracks {
            let s1 = fuzzy::calculate_fuzzy_score(&search_query, &t.name, &t.artist, Some(&t.album));
            let s2 = fuzzy::calculate_fuzzy_score(&track.title, &t.name, &t.artist, Some(&t.album));
            let score = s1.max(s2);
            lucida_scored.push(CandidateTrack::Lucida { track: t, score });
        }
    }
    lucida_scored.sort_by(|a, b| b.score().partial_cmp(&a.score()).unwrap_or(std::cmp::Ordering::Equal));

    let mut youtube_scored = Vec::new();
    if let Some(videos) = youtube_res {
        for v in videos {
            let s1 = fuzzy::calculate_fuzzy_score(&search_query, &v.title, &v.channel, None);
            let s2 = fuzzy::calculate_fuzzy_score(&track.title, &v.title, &v.channel, None);
            let mut score = s1.max(s2);

            // Duration verification: reward close match, penalize large discrepancy
            if let Some(target_sec) = target_duration_secs {
                if let Some(cand_sec) = parse_duration_to_secs(&v.duration) {
                    let diff = (target_sec as i64 - cand_sec as i64).abs();
                    if diff <= 6 {
                        score = (score + 0.10).min(1.0);
                    } else if diff <= 15 {
                        score = (score + 0.05).min(1.0);
                    } else if diff > 60 {
                        score = (score - 0.25).max(0.0);
                    }
                }
            }

            youtube_scored.push(CandidateTrack::YouTube { track: v, score });
        }
    }
    youtube_scored.sort_by(|a, b| b.score().partial_cmp(&a.score()).unwrap_or(std::cmp::Ordering::Equal));

    // Fallback: If both searches returned 0 results, retry once with clean title only
    if lucida_scored.is_empty() && youtube_scored.is_empty() && clean_title != search_query {
        if let Ok(videos) = youtube_client.search(&clean_title, 5).await {
            for v in videos {
                let score = fuzzy::calculate_fuzzy_score(&clean_title, &v.title, &v.channel, None);
                youtube_scored.push(CandidateTrack::YouTube { track: v, score });
            }
            youtube_scored.sort_by(|a, b| b.score().partial_cmp(&a.score()).unwrap_or(std::cmp::Ordering::Equal));
        }
    }

    if lucida_scored.is_empty() && youtube_scored.is_empty() {
        return TrackDownloadStatus::Failed {
            title: track.title.clone(),
            error: "No matching track found on Lucida or YouTube".to_string(),
        };
    }

    // Step 4: Pick the best candidate and download
    let best_lucida = lucida_scored.first();
    let best_yt = youtube_scored.first();

    let playlist_folder_str = playlist_folder.to_string_lossy().to_string();

    // Determine preference order:
    // If auto: Prefer Lucida Lossless FLAC if score >= 0.70 and within 0.10 of YouTube score; otherwise prefer YouTube.
    let prefer_lucida = match source_pref {
        "lucida" => best_lucida.is_some(),
        "youtube" => false,
        _ => match (best_lucida, best_yt) {
            (Some(l), Some(y)) => l.score() >= 0.70 && l.score() >= (y.score() - 0.10),
            (Some(_), None) => true,
            _ => false,
        },
    };

    // Try primary candidate
    if prefer_lucida {
        if let Some(CandidateTrack::Lucida { track: l_track, .. }) = best_lucida {
            let custom_out = playlist_folder.join(format!("{}.flac", safe_stem));
            match lucida_client.download_track(&l_track.url, Some(&custom_out.to_string_lossy()), &playlist_folder_str).await {
                Ok(res) => {
                    tag_spotify_track(&res.filepath, track, playlist_name, playlist_thumbnail, None).await;
                    return TrackDownloadStatus::Downloaded {
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        source: "Lucida (Lossless FLAC)",
                        size_bytes: res.size,
                        filepath: res.filepath,
                    };
                }
                Err(e) => {
                    // Seamless automatic fallback to YouTube
                    if let Some(CandidateTrack::YouTube { track: yt_track, .. }) = best_yt {
                        let out_tmpl = playlist_folder.join(format!("{}.%(ext)s", safe_stem));
                        if let Ok(res) = youtube_client
                            .download_track(&yt_track.url, Some(&out_tmpl.to_string_lossy()), &playlist_folder_str)
                            .await
                        {
                            tag_spotify_track(
                                &res.filepath,
                                track,
                                playlist_name,
                                playlist_thumbnail,
                                Some(&yt_track.video_id),
                            )
                            .await;
                            return TrackDownloadStatus::Downloaded {
                                title: track.title.clone(),
                                artist: track.artist.clone(),
                                source: "YouTube (320kbps MP3 fallback)",
                                size_bytes: res.size,
                                filepath: res.filepath,
                            };
                        }
                    }
                    return TrackDownloadStatus::Failed {
                        title: track.title.clone(),
                        error: format!("Lucida stream failed ({}) and YouTube fallback failed", e),
                    };
                }
            }
        }
    }

    // Try YouTube
    if let Some(CandidateTrack::YouTube { track: yt_track, .. }) = best_yt {
        let out_tmpl = playlist_folder.join(format!("{}.%(ext)s", safe_stem));
        match youtube_client
            .download_track(&yt_track.url, Some(&out_tmpl.to_string_lossy()), &playlist_folder_str)
            .await
        {
            Ok(res) => {
                tag_spotify_track(
                    &res.filepath,
                    track,
                    playlist_name,
                    playlist_thumbnail,
                    Some(&yt_track.video_id),
                )
                .await;
                return TrackDownloadStatus::Downloaded {
                    title: track.title.clone(),
                    artist: track.artist.clone(),
                    source: "YouTube (Highest Audio)",
                    size_bytes: res.size,
                    filepath: res.filepath,
                };
            }
            Err(e) => {
                // Secondary fallback to Lucida if available
                if let Some(CandidateTrack::Lucida { track: l_track, .. }) = best_lucida {
                    let custom_out = playlist_folder.join(format!("{}.flac", safe_stem));
                    if let Ok(res) = lucida_client
                        .download_track(&l_track.url, Some(&custom_out.to_string_lossy()), &playlist_folder_str)
                        .await
                    {
                        tag_spotify_track(&res.filepath, track, playlist_name, playlist_thumbnail, None).await;
                        return TrackDownloadStatus::Downloaded {
                            title: track.title.clone(),
                            artist: track.artist.clone(),
                            source: "Lucida (Lossless FLAC fallback)",
                            size_bytes: res.size,
                            filepath: res.filepath,
                        };
                    }
                }
                return TrackDownloadStatus::Failed {
                    title: track.title.clone(),
                    error: format!("YouTube download failed: {}", e),
                };
            }
        }
    }

    TrackDownloadStatus::Failed {
        title: track.title.clone(),
        error: "All download candidates failed".to_string(),
    }
}

/// Download an entire Spotify playlist with concurrent batching and live progress
pub async fn download_spotify_playlist(
    lucida_client: &LucidaClient,
    youtube_client: &YouTubeClient,
    url_or_id: &str,
    output_dir: Option<&str>,
    service: &str,
    source_pref: &str,
    concurrency: usize,
    base_download_dir: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let playlist_id = extract_playlist_id(url_or_id)
        .ok_or_else(|| format!("Invalid Spotify playlist URL or ID: '{}'", url_or_id))?;

    println!("\n{}", "━".repeat(88).dimmed());
    println!(
        "  {}  {}",
        "SPOTIFY PLAYLIST DOWNLOADER".bold().on_green().black(),
        format!("(Playlist ID: {})", playlist_id.cyan()).dimmed()
    );
    println!("{}\n", "━".repeat(88).dimmed());

    let pb_fetch = ProgressBar::new_spinner();
    pb_fetch.set_style(ProgressStyle::default_spinner().template("{spinner:.green} Fetching playlist metadata & tracks from Spotify API...")?);
    pb_fetch.enable_steady_tick(Duration::from_millis(80));

    let spotify_client = SpotifyClient::new();
    let playlist = spotify_client.fetch_playlist(&playlist_id).await?;
    pb_fetch.finish_and_clear();

    let safe_folder_name = sanitize_folder_name(&playlist.name);
    let playlist_folder = if let Some(custom) = output_dir {
        PathBuf::from(custom)
    } else {
        Path::new(base_download_dir).join(&safe_folder_name)
    };

    fs::create_dir_all(&playlist_folder)?;

    let total_tracks = playlist.tracks.len();

    println!("  {:<18} {}", "Playlist Name:".bold(), playlist.name.bold().green());
    if let Some(ref owner) = playlist.owner {
        println!("  {:<18} {}", "Curated By:".dimmed(), owner.cyan());
    }
    println!("  {:<18} {}", "Total Tracks:".bold(), total_tracks.to_string().yellow().bold());
    println!("  {:<18} {}", "Target Folder:".bold(), playlist_folder.display().to_string().cyan());
    println!("  {:<18} {}", "Batch Concurrency:".dimmed(), format!("{} workers in parallel", concurrency).magenta());
    println!("  {:<18} {}", "Audio Matching:".dimmed(), "Best match from Lucida (FLAC) & YouTube (320k)".green());
    println!("\n{}", "─".repeat(88).dimmed());

    if total_tracks == 0 {
        println!("{}", "Playlist has no tracks to download.".yellow());
        return Ok(());
    }

    let start_time = Instant::now();
    let pb = Arc::new(ProgressBar::new(total_tracks as u64));
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.green/white}] {pos}/{len} tracks ({eta}) - {msg}")?
            .progress_chars("█▓▒░")
    );
    pb.set_message(format!("Processing playlist: {}", playlist.name));

    let downloaded_count = Arc::new(AtomicUsize::new(0));
    let skipped_count = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));

    let lucida_ref = lucida_client;
    let youtube_ref = youtube_client;
    let service_owned = service.to_string();
    let source_owned = source_pref.to_string();
    let playlist_name_owned = playlist.name.clone();
    let playlist_folder_owned = playlist_folder.clone();
    let playlist_thumb_owned = playlist.thumbnail.clone();

    let concurrency_limit = concurrency.max(1).min(10);

    // Stream with bounded concurrency (workers)
    let _results: Vec<TrackDownloadStatus> = stream::iter(playlist.tracks.into_iter().enumerate())
        .map(|(idx, track)| {
            let pb_clone = Arc::clone(&pb);
            let downloaded_cnt = Arc::clone(&downloaded_count);
            let skipped_cnt = Arc::clone(&skipped_count);
            let failed_cnt = Arc::clone(&failed_count);
            let pl_folder = playlist_folder_owned.clone();
            let pl_name = playlist_name_owned.clone();
            let pl_thumb = playlist_thumb_owned.clone();
            let srv = service_owned.clone();
            let src = source_owned.clone();

            async move {
                let status = process_track(
                    lucida_ref,
                    youtube_ref,
                    &track,
                    idx + 1,
                    total_tracks,
                    &pl_name,
                    &pl_folder,
                    pl_thumb.as_deref(),
                    &srv,
                    &src,
                )
                .await;

                pb_clone.inc(1);

                match &status {
                    TrackDownloadStatus::Downloaded { title, source, size_bytes, .. } => {
                        let cur = downloaded_cnt.fetch_add(1, Ordering::SeqCst) + 1;
                        let mb = *size_bytes as f64 / (1024.0 * 1024.0);
                        pb_clone.println(format!(
                            "  {} [{}/{}] \"{}\" [{}] ({:.1} MB)",
                            "✓".bold().green(),
                            cur,
                            total_tracks,
                            title.bold(),
                            source.cyan(),
                            mb
                        ));
                    }
                    TrackDownloadStatus::SkippedAlreadyExists { title, .. } => {
                        skipped_cnt.fetch_add(1, Ordering::SeqCst);
                        pb_clone.println(format!(
                            "  {} \"{}\" (already downloaded in folder)",
                            "↷".yellow(),
                            title.dimmed()
                        ));
                    }
                    TrackDownloadStatus::Failed { title, error } => {
                        failed_cnt.fetch_add(1, Ordering::SeqCst);
                        pb_clone.println(format!(
                            "  {} Failed \"{}\": {}",
                            "✗".bold().red(),
                            title.bold(),
                            error.red()
                        ));
                    }
                }

                status
            }
        })
        .buffer_unordered(concurrency_limit)
        .collect()
        .await;

    pb.finish_and_clear();

    let elapsed = start_time.elapsed();
    let downloaded = downloaded_count.load(Ordering::SeqCst);
    let skipped = skipped_count.load(Ordering::SeqCst);
    let failed = failed_count.load(Ordering::SeqCst);

    println!("\n{}", "━".repeat(88).dimmed());
    println!("  {}", "PLAYLIST DOWNLOAD COMPLETE".bold().green());
    println!("{}", "━".repeat(88).dimmed());
    println!("  {:<22} {}", "Playlist:".bold(), playlist_name_owned.green());
    println!("  {:<22} {}", "Total Tracks:".bold(), total_tracks);
    println!("  {:<22} {}", "Downloaded:".bold(), downloaded.to_string().bold().green());
    if skipped > 0 {
        println!("  {:<22} {}", "Skipped (Existing):".bold(), skipped.to_string().yellow());
    }
    if failed > 0 {
        println!("  {:<22} {}", "Failed:".bold(), failed.to_string().red());
    }
    println!("  {:<22} {:.1} seconds", "Time Elapsed:".bold(), elapsed.as_secs_f64());
    println!("  {:<22} {}\n", "Saved Folder:".bold(), playlist_folder_owned.display().to_string().cyan().bold());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_playlist_id_variants() {
        let sample_id = "37i9dQZF1DX3bcfiyW6qms";

        // Full URL with query params
        let url1 = "https://open.spotify.com/playlist/37i9dQZF1DX3bcfiyW6qms?si=AlR_u-HMT3q3hoknYT9N-Q";
        assert_eq!(extract_playlist_id(url1), Some(sample_id.to_string()));

        // Clean HTTP URL
        let url2 = "http://open.spotify.com/playlist/37i9dQZF1DX3bcfiyW6qms";
        assert_eq!(extract_playlist_id(url2), Some(sample_id.to_string()));

        // URL without scheme
        let url3 = "open.spotify.com/playlist/37i9dQZF1DX3bcfiyW6qms";
        assert_eq!(extract_playlist_id(url3), Some(sample_id.to_string()));

        // Spotify URI
        let uri = "spotify:playlist:37i9dQZF1DX3bcfiyW6qms";
        assert_eq!(extract_playlist_id(uri), Some(sample_id.to_string()));

        // Direct raw ID
        assert_eq!(extract_playlist_id(sample_id), Some(sample_id.to_string()));
    }

    #[test]
    fn test_is_spotify_playlist() {
        assert!(is_spotify_playlist("https://open.spotify.com/playlist/37i9dQZF1DX3bcfiyW6qms?si=abc"));
        assert!(is_spotify_playlist("spotify:playlist:37i9dQZF1DX3bcfiyW6qms"));
        assert!(!is_spotify_playlist("https://open.spotify.com/track/3LP4QpKptfe2yEC7oG5iNa"));
    }

    #[test]
    fn test_duration_parsing() {
        assert_eq!(parse_duration_to_secs("4:04"), Some(244));
        assert_eq!(parse_duration_to_secs("3:52"), Some(232));
        assert_eq!(parse_duration_to_secs("1:02:15"), Some(3735));
    }

    #[test]
    fn test_sanitize_folder_name() {
        assert_eq!(sanitize_folder_name("Sad Melodies: Tamil? / [2024]"), "Sad Melodies_ Tamil_ _ [2024]");
        assert_eq!(sanitize_folder_name("..."), "Spotify_Playlist");
    }
}
