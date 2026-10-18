use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YouTubeTrack {
    pub title: String,
    pub channel: String,
    pub duration: String,
    pub views: String,
    pub url: String,
    pub video_id: String,
    pub quality: String,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub filepath: PathBuf,
    pub size: u64,
}

pub struct YouTubeClient {
    client: reqwest::Client,
}

impl YouTubeClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            .timeout(Duration::from_secs(15))
            .tcp_nodelay(true)
            .tcp_keepalive(Some(Duration::from_secs(60)))
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .pool_max_idle_per_host(10)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }

    /// Locate yt-dlp binary on the system
    pub fn find_ytdlp_binary() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
        // 1. Explicit env var
        if let Ok(path) = std::env::var("YTDLP_BIN") {
            let p = PathBuf::from(path);
            if p.exists() {
                return Ok(p);
            }
        }

        // 2. Common Windows paths
        let user_profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\USER".to_string());
        let candidates = [
            PathBuf::from(format!(r"{}\AppData\Local\Programs\Python\Python312\Scripts\yt-dlp.exe", user_profile)),
            PathBuf::from(format!(r"{}\AppData\Local\Programs\Python\Python311\Scripts\yt-dlp.exe", user_profile)),
            PathBuf::from(r".\venv\Scripts\yt-dlp.exe"),
            PathBuf::from("yt-dlp.exe"),
            PathBuf::from("yt-dlp"),
        ];

        for c in &candidates {
            if c.exists() {
                return Ok(c.clone());
            }
        }

        // 3. Check system PATH
        if let Ok(out) = std::process::Command::new("where.exe").arg("yt-dlp").output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(first_line) = text.lines().next() {
                    let p = PathBuf::from(first_line.trim());
                    if p.exists() {
                        return Ok(p);
                    }
                }
            }
        }

        Err("Could not find yt-dlp executable. Please ensure yt-dlp is installed.".into())
    }

    /// Locate ffmpeg binary on the system
    pub fn find_ffmpeg_location() -> Option<PathBuf> {
        if let Ok(path) = std::env::var("FFMPEG_BIN") {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }

        let user_profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\USER".to_string());
        let candidates = [
            PathBuf::from(format!(
                r"{}\AppData\Local\Programs\Python\Python312\Lib\site-packages\imageio_ffmpeg\binaries",
                user_profile
            )),
            PathBuf::from(format!(
                r"{}\AppData\Local\Programs\Python\Python312\Lib\site-packages\imageio_ffmpeg\binaries\ffmpeg.exe",
                user_profile
            )),
            PathBuf::from(r"C:\ffmpeg\bin"),
            PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"),
        ];

        for c in &candidates {
            if c.exists() {
                return Some(c.clone());
            }
        }

        // Check where.exe ffmpeg
        if let Ok(out) = std::process::Command::new("where.exe").arg("ffmpeg").output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(first_line) = text.lines().next() {
                    let p = PathBuf::from(first_line.trim());
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }

        None
    }

    /// Fast search using YouTube innerTube API with fallback to yt-dlp
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<YouTubeTrack>, Box<dyn Error + Send + Sync>> {
        // Try innerTube search first (ultra fast HTTP JSON)
        match self.search_innertube(query, limit).await {
            Ok(results) if !results.is_empty() => return Ok(results),
            _ => {}
        }

        // Fallback to yt-dlp search
        self.search_ytdlp(query, limit).await
    }

    async fn search_innertube(&self, query: &str, limit: usize) -> Result<Vec<YouTubeTrack>, Box<dyn Error + Send + Sync>> {
        let url = "https://www.youtube.com/youtubei/v1/search";
        let body = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "WEB",
                    "clientVersion": "2.20240101.00.00"
                }
            },
            "query": query
        });

        let resp = self.client.post(url)
            .json(&body)
            .header("Content-Type", "application/json")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(format!("YouTube API returned status: {}", resp.status()).into());
        }

        let json: serde_json::Value = resp.json().await?;
        let mut results = Vec::new();

        let sections = json
            .pointer("/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents")
            .and_then(|v| v.as_array());

        if let Some(sections) = sections {
            for sec in sections {
                let items = sec.pointer("/itemSectionRenderer/contents").and_then(|v| v.as_array());
                if let Some(items) = items {
                    for item in items {
                        if let Some(vr) = item.get("videoRenderer") {
                            let video_id = match vr.get("videoId").and_then(|v| v.as_str()) {
                                Some(id) if !id.is_empty() => id.to_string(),
                                _ => continue,
                            };

                            let title = vr.pointer("/title/runs/0/text")
                                .or_else(|| vr.pointer("/title/simpleText"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown Title")
                                .to_string();

                            let channel = vr.pointer("/ownerText/runs/0/text")
                                .or_else(|| vr.pointer("/longBylineText/runs/0/text"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown Channel")
                                .to_string();

                            let duration = vr.pointer("/lengthText/simpleText")
                                .and_then(|v| v.as_str())
                                .unwrap_or("N/A")
                                .to_string();

                            let views = vr.pointer("/viewCountText/simpleText")
                                .or_else(|| vr.pointer("/shortViewCountText/simpleText"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("N/A")
                                .to_string();

                            let url = format!("https://www.youtube.com/watch?v={}", video_id);

                            results.push(YouTubeTrack {
                                title,
                                channel,
                                duration,
                                views,
                                url,
                                video_id,
                                quality: "Highest Audio (320kbps MP3 / Best Opus)".to_string(),
                            });

                            if results.len() >= limit {
                                return Ok(results);
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    async fn search_ytdlp(&self, query: &str, limit: usize) -> Result<Vec<YouTubeTrack>, Box<dyn Error + Send + Sync>> {
        let ytdlp_path = Self::find_ytdlp_binary()?;
        let search_arg = format!("ytsearch{}:{}", limit, query);

        let output = tokio::process::Command::new(ytdlp_path)
            .arg(&search_arg)
            .arg("--print")
            .arg("%(id)s\t%(title)s\t%(uploader)s\t%(duration_string)s\t%(view_count)s\t%(webpage_url)s")
            .arg("--no-playlist")
            .arg("--no-warnings")
            .output()
            .await?;

        if !output.status.success() {
            return Err("yt-dlp search failed".into());
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut results = Vec::new();

        for line in text.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 6 {
                let video_id = parts[0].trim().to_string();
                let title = parts[1].trim().to_string();
                let channel = parts[2].trim().to_string();
                let duration = parts[3].trim().to_string();
                let views = parts[4].trim().to_string();
                let url = parts[5].trim().to_string();

                results.push(YouTubeTrack {
                    title,
                    channel,
                    duration,
                    views: if views.is_empty() || views == "NA" { "N/A".to_string() } else { format!("{} views", views) },
                    url,
                    video_id,
                    quality: "Highest Audio (320kbps MP3 / Best Opus)".to_string(),
                });
            }
        }

        Ok(results)
    }

    /// Download the highest quality audio from YouTube using yt-dlp & FFmpeg
    pub async fn download_track(
        &self,
        url_or_id: &str,
        output_path: Option<&str>,
        download_dir: &str,
    ) -> Result<DownloadResult, Box<dyn Error + Send + Sync>> {
        let ytdlp_bin = Self::find_ytdlp_binary()?;
        fs::create_dir_all(download_dir)?;

        let target_url = if url_or_id.starts_with("http://") || url_or_id.starts_with("https://") {
            url_or_id.to_string()
        } else {
            format!("https://www.youtube.com/watch?v={}", url_or_id)
        };

        let output_template = if let Some(out) = output_path {
            out.to_string()
        } else {
            let mut base = PathBuf::from(download_dir);
            base.push("%(title)s.%(ext)s");
            base.to_string_lossy().to_string()
        };

        let pb = ProgressBar::new_spinner();
        pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg}")?);
        pb.set_message("Downloading highest quality audio from YouTube (bestaudio / 320kbps MP3)...".to_string());
        pb.enable_steady_tick(Duration::from_millis(80));

        let mut cmd = tokio::process::Command::new(ytdlp_bin);
        cmd.arg("-f").arg("bestaudio/best")
            .arg("-x")
            .arg("--audio-format").arg("mp3")
            .arg("--audio-quality").arg("0")
            .arg("-N").arg("8")
            .arg("--concurrent-fragments").arg("8")
            .arg("--buffer-size").arg("16M")
            .arg("--no-mtime")
            .arg("--extractor-retries").arg("2")
            .arg("--fragment-retries").arg("2")
            .arg("--no-playlist")
            .arg("--no-warnings")
            .arg("-o").arg(&output_template)
            .arg("--print").arg("after_move:filepath");

        if let Some(ffmpeg_loc) = Self::find_ffmpeg_location() {
            cmd.arg("--ffmpeg-location").arg(ffmpeg_loc);
        }

        cmd.arg(&target_url);

        let output = cmd.output().await?;
        pb.finish_and_clear();

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(format!("YouTube download failed: {}", err).into());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let downloaded_file = stdout
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && (l.ends_with(".mp3") || l.ends_with(".m4a") || l.ends_with(".opus") || Path::new(l).exists()))
            .last();

        if let Some(filepath_str) = downloaded_file {
            let p = PathBuf::from(filepath_str);
            if p.exists() {
                let size = fs::metadata(&p)?.len();
                return Ok(DownloadResult { filepath: p, size });
            }
        }

        // Fallback: check download_dir for the most recently modified audio file
        let mut newest_file = None;
        let mut newest_time = std::time::SystemTime::UNIX_EPOCH;

        if let Ok(entries) = fs::read_dir(download_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Some(ext) = p.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if ["mp3", "flac", "m4a", "opus", "wav"].contains(&ext_str.as_str()) {
                        if let Ok(meta) = entry.metadata() {
                            if let Ok(modified) = meta.modified() {
                                if modified > newest_time {
                                    newest_time = modified;
                                    newest_file = Some((p, meta.len()));
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some((filepath, size)) = newest_file {
            return Ok(DownloadResult { filepath, size });
        }

        Err("YouTube download succeeded but downloaded file path could not be determined.".into())
    }
}
