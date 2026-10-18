use base64::Engine;
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use url::form_urlencoded;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub artist: String,
    pub album: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub filepath: PathBuf,
    pub size: u64,
}

pub struct LucidaClient {
    client: reqwest::Client,
    pub base_url: String,
}

impl LucidaClient {
    pub fn new() -> Self {
        let base_url = std::env::var("LUCIDA_BASE_URL").unwrap_or_else(|_| "https://lucida.to".to_string());
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            .timeout(Duration::from_secs(45))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client, base_url }
    }

    pub fn get_available_services(&self) -> Vec<&'static str> {
        vec![
            "amazon_music",
            "tidal",
            "qobuz",
            "deezer",
            "soundcloud",
            "yandex_music",
            "spotify",
        ]
    }

    pub async fn search(&self, query: &str, service: &str, limit: usize) -> Result<Vec<Track>, Box<dyn Error + Send + Sync>> {
        let service_lower = service.to_lowercase();
        let api_service = match service_lower.as_str() {
            "amazon_music" => "amazon",
            "yandex_music" => "yandex",
            other => other,
        };

        let country = match api_service {
            "qobuz" => "GB",
            "deezer" => "FR",
            _ => "US",
        };

        let encoded_query: String = form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let search_url = format!(
            "{}/search?service={}&country={}&query={}",
            self.base_url, api_service, country, encoded_query
        );

        let response = self
            .client
            .get(&search_url)
            .header("Referer", "https://lucida.to/")
            .send()
            .await?;

        let html = response.text().await?;

        // Extract SvelteKit data JSON array: "const data = ["
        let start_marker = "const data = [";
        if let Some(start_idx) = html.find(start_marker) {
            let json_start = start_idx + "const data = ".len();
            let chars: Vec<(usize, char)> = html[json_start..].char_indices().collect();
            let mut bracket_count = 0i32;
            let mut in_string = false;
            let mut escape_next = false;
            let mut end_idx = None;

            for (i, c) in chars {
                if escape_next {
                    escape_next = false;
                    continue;
                }
                if c == '\\' {
                    escape_next = true;
                    continue;
                }
                if c == '"' {
                    in_string = !in_string;
                    continue;
                }
                if !in_string {
                    if c == '[' || c == '{' {
                        bracket_count += 1;
                    } else if c == ']' || c == '}' {
                        bracket_count -= 1;
                        if bracket_count == 0 {
                            end_idx = Some(json_start + i + 1);
                            break;
                        }
                    }
                }
            }

            if let Some(end) = end_idx {
                let json_str = &html[json_start..end];
                let parsed_val: Result<serde_json::Value, _> = json5::from_str(json_str)
                    .or_else(|_| serde_json::from_str(json_str));

                if let Ok(val) = parsed_val {
                    if let Some(arr) = val.as_array() {
                        if arr.len() > 1 {
                            let results = &arr[1]["data"]["results"];
                            if results.get("success").and_then(|s| s.as_bool()) == Some(false) {
                                if let Some(err) = results.get("error").and_then(|e| e.as_str()) {
                                    return Err(format!("Lucida search error: {}", err).into());
                                }
                            }

                            if let Some(tracks_arr) = results["results"]["tracks"].as_array() {
                                let mut tracks = Vec::new();
                                for t in tracks_arr.iter().take(limit) {
                                    let name = t["title"].as_str().unwrap_or("Unknown").to_string();
                                    let artists = t["artists"]
                                        .as_array()
                                        .map(|a| {
                                            a.iter()
                                                .filter_map(|art| art["name"].as_str())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        })
                                        .unwrap_or_else(|| "Unknown".to_string());
                                    let album = t["album"]["title"].as_str().unwrap_or("").to_string();
                                    let url = t["url"].as_str().unwrap_or("").to_string();

                                    tracks.push(Track {
                                        name,
                                        artist: artists,
                                        album,
                                        url,
                                    });
                                }
                                return Ok(tracks);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: regex extraction of track links and titles
        let re = Regex::new(r#"href="([^"]*music\.amazon\.com/tracks/[^"]*)""#)?;
        let mut tracks = Vec::new();
        for cap in re.captures_iter(&html).take(limit) {
            tracks.push(Track {
                name: query.to_string(),
                artist: "Unknown".to_string(),
                album: "Unknown".to_string(),
                url: cap[1].to_string(),
            });
        }

        Ok(tracks)
    }

    /// Pure Rust blazing fast direct audio stream downloader
    pub async fn download_track(
        &self,
        url: &str,
        output_path: Option<&str>,
        download_dir: &str,
    ) -> Result<DownloadResult, Box<dyn Error + Send + Sync>> {
        let page_url = format!("{}/?url={}", self.base_url, url);
        
        let page_res = self.client.get(&page_url)
            .header("Referer", "https://lucida.to/")
            .header("Origin", "https://lucida.to")
            .send()
            .await?;

        if !page_res.status().is_success() {
            return Err(format!("Failed to load Lucida track page, status code: {}", page_res.status()).into());
        }

        let html = page_res.text().await?;

        // Extract cryptographic SSR token and expiry
        let re_token = Regex::new(r#"token:\s*"([^"]+)""#)?;
        let re_expiry = Regex::new(r#"tokenExpiry:\s*(\d+)"#)?;

        let token_caps = re_token.captures(&html).ok_or("Could not find authorization token on Lucida page")?;
        let expiry_caps = re_expiry.captures(&html).ok_or("Could not find token expiration timestamp")?;

        let raw_token = token_caps[1].to_string();
        let expiry: i64 = expiry_caps[1].parse().unwrap_or(0);

        // Double Base64 decode to reveal the primary token
        let step1 = base64::engine::general_purpose::STANDARD.decode(&raw_token)?;
        let step2 = base64::engine::general_purpose::STANDARD.decode(&step1)?;
        let primary_token = String::from_utf8(step2)?;

        // Initialize server stream rip
        let payload = serde_json::json!({
            "url": url,
            "metadata": true,
            "compat": false,
            "private": false,
            "handoff": true,
            "account": {
                "type": "country",
                "id": "auto"
            },
            "upload": {
                "enabled": false,
                "service": "pixeldrain"
            },
            "downscale": "original",
            "token": {
                "primary": primary_token,
                "expiry": expiry
            }
        });

        let init_url = format!("{}/api/load?url=%2Fapi%2Ffetch%2Fstream%2Fv2", self.base_url);
        let init_res: serde_json::Value = self.client.post(&init_url)
            .header("Referer", "https://lucida.to/")
            .header("Origin", "https://lucida.to")
            .json(&payload)
            .send()
            .await?
            .json()
            .await?;

        if init_res.get("success").and_then(|s| s.as_bool()) != Some(true) {
            let err_msg = init_res.get("error").and_then(|e| e.as_str()).unwrap_or("Failed to initialize server stream");
            return Err(format!("Lucida error: {}", err_msg).into());
        }

        let handoff = init_res["handoff"].as_str().ok_or("Missing server handoff ID")?;
        let server_name = init_res["name"].as_str().unwrap_or("katze");

        // Poll rip status with live status indicator
        let status_url = format!(
            "{}/api/load?url=%2Fapi%2Ffetch%2Frequest%2F{}&force={}",
            self.base_url, handoff, server_name
        );

        let pb = ProgressBar::new_spinner();
        pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg}")?);
        pb.enable_steady_tick(Duration::from_millis(80));
        pb.set_message("Requesting lossless stream from upstream...".to_string());

        let poll_start = std::time::Instant::now();
        let timeout = Duration::from_secs(180);
        let mut last_msg = String::new();

        loop {
            if poll_start.elapsed() > timeout {
                pb.finish_and_clear();
                return Err("Server processing timed out after 3 minutes".into());
            }

            tokio::time::sleep(Duration::from_millis(800)).await;

            let st_res: serde_json::Value = self.client.get(&status_url)
                .header("Referer", "https://lucida.to/")
                .send()
                .await?
                .json()
                .await?;

            if let Some(status) = st_res["status"].as_str() {
                if status == "completed" {
                    pb.finish_and_clear();
                    println!("  {} Lossless audio rip ready! Streaming file...", "✓".green());
                    break;
                } else if status == "error" {
                    pb.finish_and_clear();
                    let msg = st_res["message"].as_str().unwrap_or("Unknown error during rip");
                    return Err(format!("Server processing failed: {}", msg).into());
                }
            }

            if let Some(msg) = st_res["message"].as_str() {
                let clean = msg.replace("{item}", "track");
                if clean != last_msg {
                    pb.set_message(clean.clone());
                    last_msg = clean;
                }
            }
        }

        // Direct stream download
        let download_url = format!(
            "{}/api/load?url=%2Fapi%2Ffetch%2Frequest%2F{}%2Fdownload&force={}&redirect=true",
            self.base_url, handoff, server_name
        );

        let resp = self.client.get(&download_url)
            .header("Referer", "https://lucida.to/")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(format!("Download request failed with status: {}", resp.status()).into());
        }

        // Extract filename from Content-Disposition header
        let cd = resp.headers().get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let mut filename = "track.flac".to_string();
        if let Some(idx) = cd.find("filename*=") {
            let part = &cd[idx + "filename*=".len()..];
            let raw = part.trim_matches('"').trim_start_matches("UTF-8''");
            let decoded = percent_encoding::percent_decode_str(raw).decode_utf8_lossy().to_string();
            if !decoded.is_empty() {
                filename = decoded;
            }
        } else if let Some(idx) = cd.find("filename=") {
            let part = &cd[idx + "filename=".len()..];
            let clean = part.trim_matches('"').split(';').next().unwrap_or("track.flac");
            filename = clean.trim().to_string();
        }

        // Sanitize filename for filesystem
        let safe_filename: String = filename
            .chars()
            .map(|c| match c {
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
                other => other,
            })
            .collect();

        // Determine destination file path
        fs::create_dir_all(download_dir)?;
        let target_path = if let Some(custom_out) = output_path {
            let p = PathBuf::from(custom_out);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent)?;
            }
            p
        } else {
            Path::new(download_dir).join(&safe_filename)
        };

        let total_size = resp.content_length().unwrap_or(0);
        let pb_dl = ProgressBar::new(total_size);
        pb_dl.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta}) {msg}")?
                .progress_chars("#>-")
        );
        pb_dl.set_message(safe_filename);

        let mut file = tokio::fs::File::create(&target_path).await?;
        let mut downloaded: u64 = 0;
        let mut resp = resp;

        while let Some(chunk) = resp.chunk().await? {
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            pb_dl.set_position(downloaded);
        }

        file.flush().await?;
        pb_dl.finish_and_clear();

        Ok(DownloadResult {
            filepath: target_path,
            size: downloaded,
        })
    }
}
