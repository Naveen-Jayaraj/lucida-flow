mod browser;
mod lucida;

use clap::{Parser, Subcommand};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use lucida::LucidaClient;
use std::env;
use std::error::Error;

#[derive(Parser)]
#[command(name = "lucida")]
#[command(about = "Blazing fast Lucida Flow music search & download CLI in pure Rust", long_about = None)]
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
    },

    /// Search for tracks on a streaming service
    Search {
        /// Search query (e.g., song title, artist)
        query: String,

        /// Streaming service (default: amazon_music)
        #[arg(short, long, default_value = "amazon_music")]
        service: String,

        /// Maximum number of results
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
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
    let client = LucidaClient::new();
    let download_dir = env::var("DOWNLOAD_DIR").unwrap_or_else(|_| "./downloads".to_string());

    match cli.command {
        Some(Commands::Download {
            query_or_url,
            output,
            service,
        }) => {
            run_download(&client, &query_or_url, output.as_deref(), &service, &download_dir).await?;
        }

        Some(Commands::Search {
            query,
            service,
            limit,
        }) => {
            run_search(&client, &query, &service, limit).await?;
        }

        Some(Commands::Services) => {
            println!("\n{}", "Available Services:".bold().cyan());
            for s in client.get_available_services() {
                println!("  {} {}", "✓".green(), s);
            }
            println!("\n{}\n", format!("Total: {} services", client.get_available_services().len()).dimmed());
        }

        Some(Commands::Config) => {
            println!("\n{}", "Current Configuration:".bold().cyan());
            println!("  {:<20} {}", "Base URL:".dimmed(), client.base_url.green());
            println!("  {:<20} {}", "Download Directory:".dimmed(), download_dir.green());
            if let Ok(p) = browser::find_chrome_binary() {
                println!("  {:<20} {}", "Chromium Binary:".dimmed(), p.display().to_string().cyan());
            }
            println!();
        }

        None => {
            // Quick download mode: `lucida "song title"` or `lucida "https://..."`
            if !cli.quick_args.is_empty() {
                let query_or_url = cli.quick_args.join(" ");
                run_download(&client, &query_or_url, None, "amazon_music", &download_dir).await?;
            } else {
                println!("{}", "Lucida Flow (Rust Edition)".bold().cyan());
                println!("Usage:");
                println!("  lucida \"<song title or url>\"");
                println!("  lucida download \"<song title or url>\"");
                println!("  lucida search \"<query>\"");
                println!("  lucida services");
            }
        }
    }

    Ok(())
}

async fn run_download(
    client: &LucidaClient,
    query_or_url: &str,
    output: Option<&str>,
    service: &str,
    download_dir: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let target_url = if !query_or_url.starts_with("http://") && !query_or_url.starts_with("https://") {
        println!("\n{} \"{}\"", "Detected song query:".cyan(), query_or_url);

        let pb = ProgressBar::new_spinner();
        pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} Searching {msg}")?);
        pb.set_message(format!("{}...", service));
        pb.enable_steady_tick(std::time::Duration::from_millis(80));

        let results = client.search(query_or_url, service, 1).await?;
        pb.finish_and_clear();

        if results.is_empty() || results[0].url.is_empty() {
            eprintln!("{} {}", "✗ No matching tracks found for:".bold().red(), query_or_url);
            return Ok(());
        }

        let track = &results[0];
        println!(
            "{} {} by {}",
            "Selected track:".bold().green(),
            track.name.bold(),
            track.artist.cyan()
        );
        println!("{}\n", format!("URL: {}", track.url).dimmed());
        track.url.clone()
    } else {
        query_or_url.to_string()
    };

    println!("{} {}\n", "Downloading from:".bold().cyan(), target_url);

    // 1. Try fast pure Rust direct stream download first
    match client.download_track(&target_url, output, download_dir).await {
        Ok(res) => {
            println!("{}", "✓ Download successful!".bold().green());
            println!("{}: {}", "Saved to".bold(), res.filepath.display().to_string().cyan());
            let mb = res.size as f64 / (1024.0 * 1024.0);
            println!("{}: {} bytes ({:.2} MB)\n", "Size".bold(), res.size, mb);
            return Ok(());
        }
        Err(e) => {
            eprintln!("  {} Direct stream failed ({}), falling back to browser automation...", "⚠".yellow(), e);
        }
    }

    // 2. Fallback to Chromium automation
    match browser::download_track(&target_url, output, download_dir).await {
        Ok(res) => {
            println!("\n{}", "✓ Download successful (via browser)!".bold().green());
            println!("{}: {}", "Saved to".bold(), res.filepath.display().to_string().cyan());
            let mb = res.size as f64 / (1024.0 * 1024.0);
            println!("{}: {} bytes ({:.2} MB)\n", "Size".bold(), res.size, mb);
        }
        Err(e) => {
            eprintln!("\n{} {}\n", "✗ Download failed:".bold().red(), e);
        }
    }

    Ok(())
}

async fn run_search(
    client: &LucidaClient,
    query: &str,
    service: &str,
    limit: usize,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    println!("\n{} {} for: {}\n", "Searching".bold().cyan(), service.cyan(), query.bold());

    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} Searching...")?);
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    match client.search(query, service, limit).await {
        Ok(tracks) => {
            pb.finish_and_clear();
            if tracks.is_empty() {
                println!("{}", "No results found".yellow());
                return Ok(());
            }

            println!(
                "{}",
                format!("Search Results ({} tracks)", tracks.len()).bold().underline()
            );
            println!("{:<4} {:<32} {:<25} {:<30} {}", "#".cyan(), "Title".green(), "Artist".yellow(), "Album".magenta(), "URL".dimmed());
            println!("{}", "─".repeat(120).dimmed());

            for (i, t) in tracks.iter().enumerate() {
                let title = if t.name.len() > 30 { format!("{}...", &t.name[..27]) } else { t.name.clone() };
                let artist = if t.artist.len() > 23 { format!("{}...", &t.artist[..20]) } else { t.artist.clone() };
                let album = if t.album.len() > 28 { format!("{}...", &t.album[..25]) } else { t.album.clone() };
                println!("{:<4} {:<32} {:<25} {:<30} {}", i + 1, title.green(), artist.yellow(), album.magenta(), t.url.dimmed());
            }
            println!();
        }
        Err(e) => {
            pb.finish_and_clear();
            eprintln!("{} {}", "Error:".bold().red(), e);
            if service.to_lowercase() != "amazon_music" {
                println!("{}", "Tip: Searching on other services is currently restricted upstream on Lucida.to.".yellow());
                println!("Try using Amazon Music: {} {}\n", "lucida search".green(), format!("\"{}\" -s amazon_music", query).bold());
            }
        }
    }

    Ok(())
}
