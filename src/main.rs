mod browser;
mod fuzzy;
mod lucida;
mod metadata;
mod spotify;
mod youtube;

use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use lucida::LucidaClient;
use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use youtube::YouTubeClient;

#[derive(Parser)]
#[command(name = "lucida")]
#[command(about = "Blazing fast Lucida Flow & YouTube music search & download CLI with fuzzy logic", long_about = None)]
#[command(version = "1.0.0")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Quick download: pass a song title or track URL directly
    #[arg(trailing_var_arg = true)]
    quick_args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Download a track from URL or search query
    Download {
        /// Song title or direct track URL
        query_or_url: String,

        /// Custom output file path
        #[arg(short, long)]
        output: Option<String>,

        /// Streaming service to search if a song title is provided
        #[arg(short, long, default_value = "amazon_music")]
        service: String,

        /// Download source preference: "auto", "lucida", "youtube", "both"
        #[arg(long, default_value = "auto")]
        source: String,
    },

    /// Download all tracks from a Spotify playlist URL or ID
    #[command(alias = "spotify")]
    Playlist {
        /// Spotify playlist URL or ID (e.g. https://open.spotify.com/playlist/...)
        url_or_id: String,

        /// Custom output folder (default: downloads/<playlist_name>)
        #[arg(short, long)]
        output: Option<String>,

        /// Streaming service to search on Lucida (default: amazon_music)
        #[arg(short, long, default_value = "amazon_music")]
        service: String,

        /// Download source preference: "auto", "lucida", "youtube"
        #[arg(long, default_value = "auto")]
        source: String,

        /// Number of concurrent downloads (batching) to maximize speed (default: 3)
        #[arg(short, long, default_value_t = 3)]
        concurrency: usize,
    },

    /// Search for tracks on Lucida and YouTube in parallel with fuzzy logic
    Search {
        /// Search query (e.g., song title, artist)
        query: String,

        /// Streaming service for Lucida (default: amazon_music)
        #[arg(short, long, default_value = "amazon_music")]
        service: String,

        /// Maximum number of results per source (default: 5, total ~10)
        #[arg(short, long, default_value_t = 5)]
        limit: usize,

        /// Source to search: "all", "lucida", "youtube"
        #[arg(long, default_value = "all")]
        source: String,
    },

    /// List available streaming services
    Services,

    /// Show configuration
    Config,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // Load .env if present
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();
    let lucida_client = LucidaClient::new();
    let youtube_client = YouTubeClient::new();
    let download_dir = env::var("DOWNLOAD_DIR").unwrap_or_else(|_| "./downloads".to_string());

    match cli.command {
        Some(Commands::Download {
            query_or_url,
            output,
            service,
            source,
        }) => {
            if spotify::is_spotify_playlist(&query_or_url) {
                spotify::download_spotify_playlist(
                    &lucida_client,
                    &youtube_client,
                    &query_or_url,
                    output.as_deref(),
                    &service,
                    &source,
                    3,
                    &download_dir,
                )
                .await?;
            } else {
                run_download(
                    &lucida_client,
                    &youtube_client,
                    &query_or_url,
                    output.as_deref(),
                    &service,
                    &source,
                    &download_dir,
                )
                .await?;
            }
        }

        Some(Commands::Playlist {
            url_or_id,
            output,
            service,
            source,
            concurrency,
        }) => {
            spotify::download_spotify_playlist(
                &lucida_client,
                &youtube_client,
                &url_or_id,
                output.as_deref(),
                &service,
                &source,
                concurrency,
                &download_dir,
            )
            .await?;
        }

        Some(Commands::Search {
            query,
            service,
            limit,
            source,
        }) => {
            run_search(
                &lucida_client,
                &youtube_client,
                &query,
                &service,
                limit,
                &source,
            )
            .await?;
        }

        Some(Commands::Services) => {
            println!("\n{}", "Available Services on Lucida:".bold().cyan());
            for s in lucida_client.get_available_services() {
                println!("  {} {}", "✓".green(), s);
            }
            println!("  {} {}", "✓".red(), "youtube (via yt-audio-api / yt-dlp highest quality)");
            println!("  {} {}", "✓".green(), "spotify (playlists via xwolf API + dual Lucida/YouTube pipeline)");
            println!(
                "\n{}\n",
                format!("Total: {} services + YouTube + Spotify", lucida_client.get_available_services().len()).dimmed()
            );
        }

        Some(Commands::Config) => {
            println!("\n{}", "Current Configuration:".bold().cyan());
            println!("  {:<20} {}", "Base URL:".dimmed(), lucida_client.base_url.green());
            println!("  {:<20} {}", "Download Directory:".dimmed(), download_dir.green());
            if let Ok(p) = browser::find_chrome_binary() {
                println!("  {:<20} {}", "Chromium Binary:".dimmed(), p.display().to_string().cyan());
            }
            if let Ok(p) = YouTubeClient::find_ytdlp_binary() {
                println!("  {:<20} {}", "yt-dlp Binary:".dimmed(), p.display().to_string().red());
            }
            if let Some(p) = YouTubeClient::find_ffmpeg_location() {
                println!("  {:<20} {}", "FFmpeg Binary/Dir:".dimmed(), p.display().to_string().yellow());
            }
            println!();
        }

        None => {
            // Quick download mode: `lucida "song title"` or `lucida "https://..."`
            if !cli.quick_args.is_empty() {
                let query_or_url = cli.quick_args.join(" ");
                if spotify::is_spotify_playlist(&query_or_url) {
                    spotify::download_spotify_playlist(
                        &lucida_client,
                        &youtube_client,
                        &query_or_url,
                        None,
                        "amazon_music",
                        "auto",
                        3,
                        &download_dir,
                    )
                    .await?;
                } else {
                    run_download(
                        &lucida_client,
                        &youtube_client,
                        &query_or_url,
                        None,
                        "amazon_music",
                        "auto",
                        &download_dir,
                    )
                    .await?;
                }
            } else {
                println!("{}", "Lucida Flow + YouTube + Spotify (Rust Edition)".bold().cyan());
                println!("Usage:");
                println!("  lucida \"<song title or url>\"");
                println!("  lucida playlist \"<spotify playlist url or id>\" [--concurrency 3]");
                println!("  lucida download \"<song title or url>\" [--source auto|lucida|youtube|both]");
                println!("  lucida search \"<query>\" [--limit 5] [--source all|lucida|youtube]");
                println!("  lucida services");
                println!("  lucida config");
            }
        }
    }

    Ok(())
}

fn is_youtube_url(url: &str) -> bool {
    url.contains("youtube.com/") || url.contains("youtu.be/")
}

#[derive(Clone)]
enum CandidateTrack {
    Lucida {
        track: lucida::Track,
        score: f64,
    },
    YouTube {
        track: youtube::YouTubeTrack,
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

async fn run_download(
    lucida_client: &LucidaClient,
    youtube_client: &YouTubeClient,
    query_or_url: &str,
    output: Option<&str>,
    service: &str,
    source_pref: &str,
    download_dir: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let is_url = query_or_url.starts_with("http://") || query_or_url.starts_with("https://");

    // Case 0: Spotify Playlist URL or ID provided
    if spotify::is_spotify_playlist(query_or_url) {
        return spotify::download_spotify_playlist(
            lucida_client,
            youtube_client,
            query_or_url,
            output,
            service,
            source_pref,
            3,
            download_dir,
        )
        .await;
    }

    // Case 1: Direct YouTube URL provided
    if is_url && is_youtube_url(query_or_url) {
        println!("\n{} {}\n", "Detected direct YouTube link:".bold().red(), query_or_url);
        let path = download_from_youtube(youtube_client, query_or_url, output, download_dir).await?;
        let vid = query_or_url.split("v=").nth(1).and_then(|v| v.split('&').next());
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "track".to_string());
        let _ = metadata::enrich_and_tag_audio(&path, &stem, None, None, vid).await;
        return Ok(());
    }

    // Case 2: Direct streaming service URL provided (Lucida)
    if is_url {
        println!("\n{} {}\n", "Detected streaming track link:".bold().cyan(), query_or_url);
        let path = download_from_lucida(lucida_client, query_or_url, output, download_dir).await?;
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "track".to_string());
        let _ = metadata::enrich_and_tag_audio(&path, &stem, None, None, None).await;
        return Ok(());
    }

    // Case 3: Song query string - Fuzzy parallel search (up to 10 candidates each)
    println!("\n{} \"{}\"", "Searching with fuzzy logic for:".bold().cyan(), query_or_url);

    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} Searching both Lucida & YouTube in parallel...")?);
    pb.enable_steady_tick(Duration::from_millis(80));

    let (lucida_res, youtube_res) = tokio::join!(
        lucida_client.search(query_or_url, service, 10),
        youtube_client.search(query_or_url, 10)
    );

    pb.finish_and_clear();

    // 1. Process and score Lucida candidates
    let mut lucida_scored: Vec<CandidateTrack> = Vec::new();
    if let Ok(tracks) = lucida_res {
        for t in tracks {
            let score = fuzzy::calculate_fuzzy_score(query_or_url, &t.name, &t.artist, Some(&t.album));
            lucida_scored.push(CandidateTrack::Lucida { track: t, score });
        }
    }
    lucida_scored.sort_by(|a, b| b.score().partial_cmp(&a.score()).unwrap_or(std::cmp::Ordering::Equal));
    lucida_scored.truncate(5); // Top 5 (half)

    // 2. Process and score YouTube candidates
    let mut youtube_scored: Vec<CandidateTrack> = Vec::new();
    if let Ok(videos) = youtube_res {
        for v in videos {
            let score = fuzzy::calculate_fuzzy_score(query_or_url, &v.title, &v.channel, None);
            youtube_scored.push(CandidateTrack::YouTube { track: v, score });
        }
    }
    youtube_scored.sort_by(|a, b| b.score().partial_cmp(&a.score()).unwrap_or(std::cmp::Ordering::Equal));
    youtube_scored.truncate(5); // Top 5 (half)

    if lucida_scored.is_empty() && youtube_scored.is_empty() {
        eprintln!("\n{} No matching tracks found on either Lucida or YouTube for: \"{}\"\n", "✗".bold().red(), query_or_url);
        return Ok(());
    }

    // Combine into indexed candidate list: [1..L] Lucida, [L+1..L+Y] YouTube
    let mut all_candidates: Vec<CandidateTrack> = Vec::new();
    all_candidates.extend(lucida_scored);
    all_candidates.extend(youtube_scored);

    println!("\n{}", "━".repeat(90).dimmed());
    println!(
        "  {}  {}",
        "FUZZY SEARCH RESULTS".bold().on_cyan().black(),
        format!("(Top {} matches across Lucida & YouTube)", all_candidates.len()).dimmed()
    );
    println!("{}", "━".repeat(90).dimmed());

    // Display Lucida Section
    let lucida_count = all_candidates.iter().filter(|c| matches!(c, CandidateTrack::Lucida { .. })).count();
    if lucida_count > 0 {
        println!("\n  {} {}", "LUCIDA CANDIDATES (Amazon Music / Lossless FLAC)".bold().green(), format!("[{} results]", lucida_count).dimmed());
        println!("  {}", "─".repeat(86).dimmed());
        for (i, c) in all_candidates.iter().enumerate() {
            if let CandidateTrack::Lucida { track, score } = c {
                let match_pct = (score * 100.0).round() as u32;
                let match_badge = if match_pct >= 85 {
                    format!("Match: {}%", match_pct).bold().green()
                } else if match_pct >= 60 {
                    format!("Match: {}%", match_pct).bold().yellow()
                } else {
                    format!("Match: {}%", match_pct).dimmed()
                };

                println!(
                    "  [{}] {:<34} [{}] [{}]",
                    (i + 1).to_string().bold().cyan(),
                    track.name.bold(),
                    match_badge,
                    "Lossless FLAC".green()
                );
                println!(
                    "      Artist: {:<22} | Album: {}",
                    track.artist.cyan(),
                    if track.album.is_empty() { "Single" } else { &track.album }.yellow()
                );
                println!("      URL: {}", track.url.dimmed());
                println!();
            }
        }
    }

    // Display YouTube Section
    let yt_count = all_candidates.iter().filter(|c| matches!(c, CandidateTrack::YouTube { .. })).count();
    if yt_count > 0 {
        println!("  {} {}", "YOUTUBE CANDIDATES (yt-audio-api / 320kbps MP3)".bold().red(), format!("[{} results]", yt_count).dimmed());
        println!("  {}", "─".repeat(86).dimmed());
        for (i, c) in all_candidates.iter().enumerate() {
            if let CandidateTrack::YouTube { track, score } = c {
                let match_pct = (score * 100.0).round() as u32;
                let match_badge = if match_pct >= 85 {
                    format!("Match: {}%", match_pct).bold().green()
                } else if match_pct >= 60 {
                    format!("Match: {}%", match_pct).bold().yellow()
                } else {
                    format!("Match: {}%", match_pct).dimmed()
                };

                let title = if track.title.len() > 34 { format!("{}...", &track.title[..31]) } else { track.title.clone() };
                println!(
                    "  [{}] {:<34} [{}] [{}]",
                    (i + 1).to_string().bold().cyan(),
                    title.bold(),
                    match_badge,
                    "320kbps MP3 / Best Opus".red()
                );
                println!(
                    "      Channel: {:<20} | Duration: {:<6} | Views: {}",
                    track.channel.cyan(),
                    track.duration.yellow(),
                    track.views.magenta()
                );
                println!("      URL: {}", track.url.dimmed());
                println!();
            }
        }
    }

    println!("{}", "━".repeat(90).dimmed());

    // User selection handling
    let total = all_candidates.len();
    let selected_indices: Vec<usize> = match source_pref.to_lowercase().as_str() {
        "lucida" => {
            if lucida_count > 0 { vec![0] } else { vec![] }
        }
        "youtube" => {
            if let Some(pos) = all_candidates.iter().position(|c| matches!(c, CandidateTrack::YouTube { .. })) {
                vec![pos]
            } else {
                vec![]
            }
        }
        "both" => {
            let mut list = Vec::new();
            if lucida_count > 0 { list.push(0); }
            if let Some(pos) = all_candidates.iter().position(|c| matches!(c, CandidateTrack::YouTube { .. })) {
                list.push(pos);
            }
            list
        }
        _ => {
            // Interactive prompt
            print!(
                "\n{} ",
                format!("Select track number to download (1-{}, or multiple e.g. '1, 6', [q] to cancel) [default: 1]:", total)
                    .bold()
                    .yellow()
            );
            io::stdout().flush()?;
            let mut buffer = String::new();
            io::stdin().read_line(&mut buffer)?;
            let trimmed = buffer.trim();

            if trimmed.is_empty() {
                vec![0]
            } else if trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit") || trimmed.eq_ignore_ascii_case("cancel") {
                println!("{}", "Download cancelled by user.".yellow());
                return Ok(());
            } else {
                let mut indices = Vec::new();
                for part in trimmed.split([',', ' ', ';']) {
                    let p = part.trim();
                    if let Ok(num) = p.parse::<usize>() {
                        if num >= 1 && num <= total {
                            indices.push(num - 1);
                        }
                    }
                }
                if indices.is_empty() {
                    println!("Invalid selection. Defaulting to track #1...");
                    vec![0]
                } else {
                    indices
                }
            }
        }
    };

    for idx in selected_indices {
        if idx >= all_candidates.len() {
            continue;
        }
        match &all_candidates[idx] {
            CandidateTrack::Lucida { track, .. } => {
                println!("\n{} {} (Lossless FLAC)...", "Downloading from Lucida:".bold().green(), track.name.bold());
                match download_from_lucida(lucida_client, &track.url, output, download_dir).await {
                    Ok(path) => {
                        let _ = metadata::enrich_and_tag_audio(
                            &path,
                            &track.name,
                            Some(&track.artist),
                            Some(&track.album),
                            None,
                        ).await;
                    }
                    Err(e) => {
                        eprintln!("  {} Lucida download failed: {}", "⚠".yellow(), e);

                        // Automatic fallback to matching YouTube candidate so user gets high-quality audio
                        if let Some(yt_cand) = all_candidates.iter().find_map(|c| match c {
                            CandidateTrack::YouTube { track: yt, .. } => Some(yt),
                            _ => None,
                        }) {
                            println!(
                                "\n{} {} (Highest Quality Audio via yt-audio-api)...",
                                "🔄 Automatically falling back to YouTube:".bold().yellow(),
                                yt_cand.title.bold()
                            );
                            if let Ok(path) = download_from_youtube(youtube_client, &yt_cand.url, output, download_dir).await {
                                let _ = metadata::enrich_and_tag_audio(
                                    &path,
                                    &yt_cand.title,
                                    Some(&yt_cand.channel),
                                    None,
                                    Some(&yt_cand.video_id),
                                ).await;
                            }
                        }
                    }
                }
            }
            CandidateTrack::YouTube { track, .. } => {
                println!("\n{} {} (Highest Quality Audio)...", "Downloading from YouTube:".bold().red(), track.title.bold());
                match download_from_youtube(youtube_client, &track.url, output, download_dir).await {
                    Ok(path) => {
                        let _ = metadata::enrich_and_tag_audio(
                            &path,
                            &track.title,
                            Some(&track.channel),
                            None,
                            Some(&track.video_id),
                        ).await;
                    }
                    Err(e) => {
                        eprintln!("  {} YouTube download failed: {}", "✗".red(), e);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn download_from_lucida(
    client: &LucidaClient,
    target_url: &str,
    output: Option<&str>,
    download_dir: &str,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    println!("{} {}\n", "Downloading from Lucida:".bold().cyan(), target_url);

    // 1. Try fast pure Rust direct stream download first
    match client.download_track(target_url, output, download_dir).await {
        Ok(res) => {
            println!("{}", "✓ Download successful via Lucida direct stream!".bold().green());
            println!("{}: {}", "Saved to".bold(), res.filepath.display().to_string().cyan());
            let mb = res.size as f64 / (1024.0 * 1024.0);
            println!("{}: {} bytes ({:.2} MB)\n", "Size".bold(), res.size, mb);
            return Ok(res.filepath);
        }
        Err(e) => {
            eprintln!("  {} Direct stream failed ({}), falling back to browser automation...", "⚠".yellow(), e);
        }
    }

    // 2. Fallback to Chromium automation
    let res = browser::download_track(target_url, output, download_dir).await?;
    println!("\n{}", "✓ Download successful (via browser automation)!".bold().green());
    println!("{}: {}", "Saved to".bold(), res.filepath.display().to_string().cyan());
    let mb = res.size as f64 / (1024.0 * 1024.0);
    println!("{}: {} bytes ({:.2} MB)\n", "Size".bold(), res.size, mb);

    Ok(res.filepath)
}

async fn download_from_youtube(
    youtube_client: &YouTubeClient,
    url: &str,
    output: Option<&str>,
    download_dir: &str,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    println!("{} {}\n", "Downloading from YouTube:".bold().red(), url);

    match youtube_client.download_track(url, output, download_dir).await {
        Ok(res) => {
            println!("\n{}", "✓ YouTube audio download successful!".bold().green());
            println!("{}: {}", "Saved to".bold(), res.filepath.display().to_string().cyan());
            let mb = res.size as f64 / (1024.0 * 1024.0);
            println!("{}: {} bytes ({:.2} MB)", "Size".bold(), res.size, mb);
            println!("{}: {}\n", "Quality".bold(), "Highest (320kbps MP3 / Best Opus)".green());
            Ok(res.filepath)
        }
        Err(e) => {
            eprintln!("\n{} {}\n", "✗ YouTube audio download failed:".bold().red(), e);
            Err(e)
        }
    }
}

async fn run_search(
    lucida_client: &LucidaClient,
    youtube_client: &YouTubeClient,
    query: &str,
    service: &str,
    limit: usize,
    source: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let search_lucida = source == "all" || source == "lucida";
    let search_youtube = source == "all" || source == "youtube";

    println!("\n{} for: {}\n", "Fuzzy Searching".bold().cyan(), query.bold());

    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} Searching both sources in parallel...")?);
    pb.enable_steady_tick(Duration::from_millis(80));

    let fetch_count = (limit * 2).max(10);

    let (lucida_res, youtube_res) = tokio::join!(
        async {
            if search_lucida {
                Some(lucida_client.search(query, service, fetch_count).await)
            } else {
                None
            }
        },
        async {
            if search_youtube {
                Some(youtube_client.search(query, fetch_count).await)
            } else {
                None
            }
        }
    );

    pb.finish_and_clear();

    // 1. Display Lucida Results ranked by fuzzy logic
    if let Some(res) = lucida_res {
        println!("{}", format!("Lucida Results ({}) - Ranked by Fuzzy Match", service).bold().green().underline());
        match res {
            Ok(tracks) if !tracks.is_empty() => {
                let mut scored: Vec<(lucida::Track, f64)> = tracks
                    .into_iter()
                    .map(|t| {
                        let score = fuzzy::calculate_fuzzy_score(query, &t.name, &t.artist, Some(&t.album));
                        (t, score)
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scored.truncate(limit);

                println!("{:<4} {:<8} {:<30} {:<22} {:<24} {}", "#".cyan(), "Match".bold(), "Title".green(), "Artist".yellow(), "Album".magenta(), "URL".dimmed());
                println!("{}", "─".repeat(128).dimmed());
                for (i, (t, score)) in scored.iter().enumerate() {
                    let match_pct = format!("{}%", (score * 100.0).round() as u32);
                    let title = if t.name.len() > 28 { format!("{}...", &t.name[..25]) } else { t.name.clone() };
                    let artist = if t.artist.len() > 20 { format!("{}...", &t.artist[..17]) } else { t.artist.clone() };
                    let album = if t.album.len() > 22 { format!("{}...", &t.album[..19]) } else { t.album.clone() };
                    println!("{:<4} {:<8} {:<30} {:<22} {:<24} {}", i + 1, match_pct.green(), title.green(), artist.yellow(), album.magenta(), t.url.dimmed());
                }
            }
            Ok(_) => println!("{}", "No Lucida tracks found.".yellow()),
            Err(e) => eprintln!("{} {}", "Lucida search error:".red(), e),
        }
        println!();
    }

    // 2. Display YouTube Results ranked by fuzzy logic
    if let Some(res) = youtube_res {
        println!("{}", "YouTube Results - Ranked by Fuzzy Match".bold().red().underline());
        match res {
            Ok(videos) if !videos.is_empty() => {
                let mut scored: Vec<(youtube::YouTubeTrack, f64)> = videos
                    .into_iter()
                    .map(|v| {
                        let score = fuzzy::calculate_fuzzy_score(query, &v.title, &v.channel, None);
                        (v, score)
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scored.truncate(limit);

                println!("{:<4} {:<8} {:<36} {:<22} {:<10} {:<15} {}", "#".cyan(), "Match".bold(), "Title".red(), "Channel".yellow(), "Duration".magenta(), "Views".blue(), "URL".dimmed());
                println!("{}", "─".repeat(128).dimmed());
                for (i, (v, score)) in scored.iter().enumerate() {
                    let match_pct = format!("{}%", (score * 100.0).round() as u32);
                    let title = if v.title.len() > 34 { format!("{}...", &v.title[..31]) } else { v.title.clone() };
                    let channel = if v.channel.len() > 20 { format!("{}...", &v.channel[..17]) } else { v.channel.clone() };
                    println!("{:<4} {:<8} {:<36} {:<22} {:<10} {:<15} {}", i + 1, match_pct.green(), title.red(), channel.yellow(), v.duration.magenta(), v.views.blue(), v.url.dimmed());
                }
            }
            Ok(_) => println!("{}", "No YouTube tracks found.".yellow()),
            Err(e) => eprintln!("{} {}", "YouTube search error:".red(), e),
        }
        println!();
    }

    Ok(())
}
