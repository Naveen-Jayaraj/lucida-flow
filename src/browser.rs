use colored::*;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

pub struct DownloadResult {
    pub filepath: PathBuf,
    pub size: u64,
}

#[derive(Deserialize, Debug)]
struct PageItem {
    #[serde(rename = "type")]
    page_type: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct CdpMessage {
    id: Option<u64>,
    method: Option<String>,
    result: Option<serde_json::Value>,
    params: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

pub fn find_chrome_binary() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    // 1. Check custom environment variable
    if let Ok(path) = std::env::var("CHROME_BIN") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }

    // 2. Check PLAYWRIGHT_BROWSERS_PATH
    if let Ok(browser_dir) = std::env::var("PLAYWRIGHT_BROWSERS_PATH") {
        let p = PathBuf::from(&browser_dir).join("chromium-1248/chrome-win64/chrome.exe");
        if p.exists() {
            return Ok(p);
        }
    }

    // 3. Check local directory candidates
    let candidates = [
        ".playwright-browsers/chromium-1248/chrome-win64/chrome.exe",
        ".playwright-browsers/chromium_headless_shell-1248/chrome-headless-shell-win64/chrome-headless-shell.exe",
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
        "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
    ];

    for c in &candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Ok(p);
        }
    }

    // Recursive search in .playwright-browsers if present
    if Path::new(".playwright-browsers").exists() {
        if let Ok(entries) = fs::read_dir(".playwright-browsers") {
            for entry in entries.flatten() {
                let candidate = entry.path().join("chrome-win64/chrome.exe");
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }

    Err("Could not find Chrome or Edge binary. Please install Chrome or set CHROME_BIN.".into())
}

struct ChromeProcess {
    child: Child,
    temp_dir: PathBuf,
}

impl Drop for ChromeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

pub async fn download_track(
    url: &str,
    output_path: Option<&str>,
    download_dir: &str,
) -> Result<DownloadResult, Box<dyn Error + Send + Sync>> {
    let chrome_bin = find_chrome_binary()?;
    fs::create_dir_all(download_dir)?;
    let canonical = fs::canonicalize(download_dir)?;
    let canonical_str = canonical.to_string_lossy().to_string();
    let clean_download_dir = if canonical_str.starts_with(r"\\?\") {
        canonical_str[4..].to_string()
    } else {
        canonical_str
    };
    let abs_download_dir = PathBuf::from(&clean_download_dir);

    let unique_id = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let temp_profile = std::env::temp_dir().join(format!("lucida_rust_{}", unique_id));
    fs::create_dir_all(&temp_profile)?;

    // Snapshot existing files in download directory
    let before_files: HashSet<PathBuf> = fs::read_dir(&abs_download_dir)?
        .flatten()
        .map(|e| e.path())
        .collect();

    let lucida_url = format!("https://lucida.to/?url={}", url);

    // Launch Chromium headless with anti-bot stealth parameters
    let child = Command::new(chrome_bin)
        .arg("--headless=new")
        .arg("--remote-debugging-port=0")
        .arg("--no-sandbox")
        .arg("--disable-gpu")
        .arg("--disable-dev-shm-usage")
        .arg("--disable-background-networking")
        .arg("--disable-background-timer-throttling")
        .arg("--disable-client-side-phishing-detection")
        .arg("--disable-default-apps")
        .arg("--disable-sync")
        .arg("--disable-translate")
        .arg("--metrics-recording-only")
        .arg("--no-first-run")
        .arg("--mute-audio")
        .arg("--disable-extensions")
        .arg("--disable-blink-features=AutomationControlled")
        .arg("--user-agent=Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .arg("--window-size=1920,1080")
        .arg(format!("--user-data-dir={}", temp_profile.display()))
        .arg(&lucida_url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let _chrome = ChromeProcess {
        child,
        temp_dir: temp_profile.clone(),
    };

    // Wait for Chrome to write its assigned port into DevToolsActivePort
    let active_port_file = temp_profile.join("DevToolsActivePort");
    let start_wait = Instant::now();
    let mut assigned_port = None;
    let mut browser_path = None;

    while start_wait.elapsed() < Duration::from_secs(10) {
        if active_port_file.exists() {
            if let Ok(content) = fs::read_to_string(&active_port_file) {
                let lines: Vec<&str> = content.lines().collect();
                if lines.len() >= 2 {
                    if let Ok(p) = lines[0].trim().parse::<u16>() {
                        assigned_port = Some(p);
                        browser_path = Some(lines[1].trim().to_string());
                        break;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let port = assigned_port.ok_or("Failed to read Chromium DevTools port")?;
    let b_path = browser_path.ok_or("Failed to read Chromium DevTools browser path")?;

    // 1. Connect Browser WebSocket and set download behavior (KEEP b_ws alive!)
    let browser_ws_url = format!("ws://127.0.0.1:{}{}", port, b_path);

    let (mut b_ws, _) = connect_async(&browser_ws_url).await?;
    let set_dl = serde_json::json!({
        "id": 1,
        "method": "Browser.setDownloadBehavior",
        "params": {
            "behavior": "allow",
            "downloadPath": clean_download_dir,
            "eventsEnabled": true
        }
    });
    b_ws.send(Message::Text(set_dl.to_string())).await?;

    // Auto attach to any popups or new tabs/windows
    let _ = b_ws.send(Message::Text(serde_json::json!({
        "id": 80,
        "method": "Target.setAutoAttach",
        "params": {
            "autoAttach": true,
            "waitForDebuggerOnStart": false,
            "flatten": true
        }
    }).to_string())).await;

    if let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(300), b_ws.next()).await {
        if let Ok(Message::Text(txt)) = msg {
            if txt.contains("error") {
                eprintln!("  {} Browser.setDownloadBehavior warning: {}", "⚠".yellow(), txt);
            }
        }
    }

    // 2. Find the page target from /json/list (must match the track URL)
    let http_client = reqwest::Client::new();
    let list_url = format!("http://127.0.0.1:{}/json/list", port);
    let mut page_ws_url = None;
    let list_start = Instant::now();

    while list_start.elapsed() < Duration::from_secs(15) {
        if let Ok(res) = http_client.get(&list_url).send().await {
            if let Ok(items) = res.json::<Vec<PageItem>>().await {
                // First priority: target containing ?url= or the song URL
                for it in &items {
                    if it.page_type == "page" && (it.url.contains("?url=") || it.url.contains("amazon.com")) {
                        if let Some(ws) = &it.web_socket_debugger_url {
                            println!("  {} Connected to track page: {}", "→".cyan(), it.url);
                            page_ws_url = Some(ws.clone());
                            break;
                        }
                    }
                }
                // Second priority: any non-chrome page if exact not ready yet
                if page_ws_url.is_none() && list_start.elapsed() > Duration::from_secs(3) {
                    for it in &items {
                        if it.page_type == "page"
                            && !it.url.starts_with("chrome://")
                            && !it.url.starts_with("chrome-extension://")
                        {
                            if let Some(ws) = &it.web_socket_debugger_url {
                                page_ws_url = Some(ws.clone());
                                break;
                            }
                        }
                    }
                }
                if page_ws_url.is_some() {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let page_ws_url = page_ws_url.ok_or("Failed to find Lucida page in Chromium")?;

    // Connect to Page WebSocket
    let (mut p_ws, _) = connect_async(&page_ws_url).await?;

    // Enable Page & Network domains and allow downloads
    let _ = p_ws.send(Message::Text(serde_json::json!({"id": 2, "method": "Page.enable"}).to_string())).await;
    let _ = p_ws.send(Message::Text(serde_json::json!({"id": 10, "method": "Network.enable"}).to_string())).await;
    let _ = p_ws.send(Message::Text(serde_json::json!({
        "id": 3,
        "method": "Page.addScriptToEvaluateOnNewDocument",
        "params": {
            "source": "Object.defineProperty(navigator, 'webdriver', {get: () => undefined});"
        }
    }).to_string())).await;
    let _ = p_ws.send(Message::Text(serde_json::json!({
        "id": 4,
        "method": "Page.setDownloadBehavior",
        "params": {
            "behavior": "allow",
            "downloadPath": clean_download_dir
        }
    }).to_string())).await;

    // 3. Wait for the download button to appear and click it
    let mut clicked = false;
    let button_check_start = Instant::now();
    let mut eval_seq = 100u64;

    while button_check_start.elapsed() < Duration::from_secs(45) {
        eval_seq += 1;
        let eval_cmd = serde_json::json!({
            "id": eval_seq,
            "method": "Runtime.evaluate",
            "params": {
                "expression": r#"
                    (() => {
                        // 1. Look for download button
                        const btn = document.querySelector('button.download-button, a.download-button') ||
                            Array.from(document.querySelectorAll('button, a')).find(b => {
                                const t = (b.innerText || b.textContent || '').trim().toLowerCase();
                                const cls = (b.className && typeof b.className === 'string') ? b.className.toLowerCase() : '';
                                const isVisible = (b.offsetWidth > 0 || b.offsetHeight > 0 || (b.getClientRects && b.getClientRects().length > 0));
                                return isVisible && (t.includes('download') || cls.includes('download-button') || b.hasAttribute('download'));
                            });

                        if (btn && (btn.offsetWidth > 0 || btn.offsetHeight > 0 || (btn.getClientRects && btn.getClientRects().length > 0))) {
                            try {
                                btn.scrollIntoView({ behavior: 'instant', block: 'center' });
                            } catch (e) {}
                            const rect = btn.getBoundingClientRect();
                            if (rect.width > 0 && rect.height > 0) {
                                return JSON.stringify({
                                    status: "found",
                                    x: Math.round(rect.x + rect.width / 2),
                                    y: Math.round(rect.y + rect.height / 2),
                                    text: (btn.innerText || btn.textContent || '').trim(),
                                    tag: btn.tagName.toLowerCase(),
                                    cls: btn.className || ''
                                });
                            }
                        }

                        // 2. Only return error if an explicit, VISIBLE error dialog/alert is shown
                        const errorSelectors = ['.alert-error', '.toast-error', '.notification-error', '.error-message'];
                        for (const sel of errorSelectors) {
                            const el = document.querySelector(sel);
                            if (el && (el.offsetWidth > 0 || el.offsetHeight > 0)) {
                                const txt = (el.innerText || el.textContent || '').trim();
                                if (txt.length > 0) {
                                    return JSON.stringify({ status: "error", message: txt });
                                }
                            }
                        }

                        return JSON.stringify({ status: "waiting" });
                    })()
                "#,
                "returnByValue": true
            }
        });

        let _ = p_ws.send(Message::Text(eval_cmd.to_string())).await;

        let mut check_json: Option<serde_json::Value> = None;
        while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(350), p_ws.next()).await {
            if let Ok(Message::Text(txt)) = msg {
                if let Ok(resp) = serde_json::from_str::<CdpMessage>(&txt) {
                    if resp.id == Some(eval_seq) {
                        if let Some(res) = resp.result {
                            if let Some(val) = res["result"]["value"].as_str() {
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(val) {
                                    check_json = Some(parsed);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(info) = check_json {
            let status = info["status"].as_str().unwrap_or("waiting");
            if status == "found" {
                let x = info["x"].as_f64().unwrap_or(100.0);
                let y = info["y"].as_f64().unwrap_or(100.0);
                let text = info["text"].as_str().unwrap_or("download");
                let tag = info["tag"].as_str().unwrap_or("button");

                println!("  {} Found {} \"{}\" at ({:.0}, {:.0}). Dispatching trusted click...", "✓".green(), tag, text, x, y);

                // Mouse move
                let _ = p_ws.send(Message::Text(serde_json::json!({
                    "id": 501,
                    "method": "Input.dispatchMouseEvent",
                    "params": {
                        "type": "mouseMoved",
                        "x": x,
                        "y": y
                    }
                }).to_string())).await;

                tokio::time::sleep(Duration::from_millis(60)).await;

                // Mouse press
                let _ = p_ws.send(Message::Text(serde_json::json!({
                    "id": 502,
                    "method": "Input.dispatchMouseEvent",
                    "params": {
                        "type": "mousePressed",
                        "x": x,
                        "y": y,
                        "button": "left",
                        "clickCount": 1
                    }
                }).to_string())).await;

                tokio::time::sleep(Duration::from_millis(80)).await;

                // Mouse release
                let _ = p_ws.send(Message::Text(serde_json::json!({
                    "id": 503,
                    "method": "Input.dispatchMouseEvent",
                    "params": {
                        "type": "mouseReleased",
                        "x": x,
                        "y": y,
                        "button": "left",
                        "clickCount": 1
                    }
                }).to_string())).await;

                println!("  {} Download initiated! Streaming lossless audio...", "✓".green());
                clicked = true;
                break;
            } else if status == "error" {
                let msg = info["message"].as_str().unwrap_or("Unknown Lucida error");
                return Err(format!("Lucida error: {}", msg).into());
            } else if eval_seq % 6 == 0 {
                println!("  {} Waiting for track button to render...", "•".cyan());
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    if !clicked {
        return Err("Download button did not appear within 45s. Verify the track URL.".into());
    }

    let click_time = SystemTime::now();

    // 4. Monitor both WebSocket events and download directories for completion
    let dl_start = Instant::now();
    let mut downloaded_file: Option<PathBuf> = None;
    let mut suggested_filename: Option<String> = None;
    let temp_downloads = temp_profile.join("Downloads");
    let _ = fs::create_dir_all(&temp_downloads);
    let mut loop_count = 0u64;

    while dl_start.elapsed() < Duration::from_secs(180) {
        loop_count += 1;
        if loop_count % 6 == 0 {
            let eval_id = 9000 + loop_count;
            let check_cmd = serde_json::json!({
                "id": eval_id,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": r#"
                        (() => {
                            const err = document.querySelector('.alert-error, .toast-error');
                            if (err && (err.offsetWidth > 0 || err.offsetHeight > 0)) {
                                return "Page error: " + err.innerText.trim();
                            }
                            return "Page text: " + (document.body ? document.body.innerText.replace(/\s+/g, ' ').slice(0, 400) : '');
                        })()
                    "#,
                    "returnByValue": true
                }
            });
            let _ = p_ws.send(Message::Text(check_cmd.to_string())).await;
        }

        // Drain Browser WebSocket download events
        while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(50), b_ws.next()).await {
            if let Ok(Message::Text(txt)) = msg {
                if let Ok(cdp) = serde_json::from_str::<CdpMessage>(&txt) {
                    if let Some(method) = cdp.method.as_deref() {
                        if method == "Target.attachedToTarget" {
                            if let Some(params) = &cdp.params {
                                let session_id = params["sessionId"].as_str().unwrap_or("");
                                let target_url = params["targetInfo"]["url"].as_str().unwrap_or("");
                                println!("  {} Attached to new target session: {} (url: {})", "→".magenta(), session_id, target_url);
                                let dl_cmd = serde_json::json!({
                                    "id": 888,
                                    "sessionId": session_id,
                                    "method": "Page.setDownloadBehavior",
                                    "params": {
                                        "behavior": "allow",
                                        "downloadPath": clean_download_dir
                                    }
                                });
                                let _ = b_ws.send(Message::Text(dl_cmd.to_string())).await;
                            }
                        } else if method.ends_with("downloadWillBegin") {
                            if let Some(params) = cdp.params {
                                if let Some(sug) = params["suggestedFilename"].as_str() {
                                    println!("  {} Audio stream filename: {}", "↓".cyan(), sug);
                                    suggested_filename = Some(sug.to_string());
                                }
                            }
                        } else if method.ends_with("downloadProgress") {
                            if let Some(params) = cdp.params {
                                if params["state"].as_str() == Some("completed") {
                                    if let Some(sug) = &suggested_filename {
                                        let p = abs_download_dir.join(sug);
                                        if p.exists() {
                                            downloaded_file = Some(p);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Drain Page WebSocket download events
        while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(50), p_ws.next()).await {
            if let Ok(Message::Text(txt)) = msg {
                if let Ok(resp) = serde_json::from_str::<CdpMessage>(&txt) {
                    if let Some(id) = resp.id {
                        if id >= 9000 {
                            if let Some(res) = resp.result {
                                if let Some(val) = res["result"]["value"].as_str() {
                                    println!("  {} {}", "•".dimmed(), val);
                                }
                            }
                        }
                    }
                    if let Some(method) = resp.method.as_deref() {
                        if method == "Network.requestWillBeSent" {
                            if let Some(params) = &resp.params {
                                if let Some(req) = params.get("request") {
                                    if let Some(url_str) = req["url"].as_str() {
                                        if url_str.contains("/api/") || url_str.contains("download") || url_str.contains("stream") {
                                            println!("  {} Network: {}", "→".cyan(), url_str);
                                        }
                                    }
                                }
                            }
                        }
                        if method == "Network.responseReceived" {
                            if let Some(params) = &resp.params {
                                if let Some(res) = params.get("response") {
                                    let status = res["status"].as_u64().unwrap_or(0);
                                    let url_str = res["url"].as_str().unwrap_or("");
                                    if url_str.contains("/api/") || url_str.contains("download") || url_str.contains("stream") {
                                        println!("  {} Network Response {}: {}", "←".cyan(), status, url_str);
                                    }
                                }
                            }
                        }
                        if method.ends_with("downloadWillBegin") {
                            if let Some(params) = resp.params {
                                if let Some(sug) = params["suggestedFilename"].as_str() {
                                    println!("  {} Audio stream filename: {}", "↓".cyan(), sug);
                                    suggested_filename = Some(sug.to_string());
                                }
                            }
                        } else if method.ends_with("downloadProgress") {
                            if let Some(params) = resp.params {
                                if params["state"].as_str() == Some("completed") {
                                    if let Some(sug) = &suggested_filename {
                                        let p = abs_download_dir.join(sug);
                                        if p.exists() {
                                            downloaded_file = Some(p);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if downloaded_file.is_some() {
            break;
        }

        // Check temp profile downloads directory fallback
        if temp_downloads.exists() {
            if let Ok(entries) = fs::read_dir(&temp_downloads) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                        let ext_low = ext.to_lowercase();
                        if ["flac", "mp3", "m4a", "aac", "wav", "zip"].contains(&ext_low.as_str()) {
                            if let Ok(meta) = fs::metadata(&p) {
                                if meta.len() > 0 {
                                    let dest = abs_download_dir.join(p.file_name().unwrap());
                                    let _ = fs::copy(&p, &dest);
                                    downloaded_file = Some(dest);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        if downloaded_file.is_some() {
            break;
        }

        // Target download directory check
        let current_files: Vec<PathBuf> = fs::read_dir(&abs_download_dir)?
            .flatten()
            .map(|e| e.path())
            .collect();

        // Check for crdownload in progress
        let in_progress = current_files.iter().any(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("crdownload")
        });

        // Find completed audio file (either new or updated after click_time)
        let completed: Vec<&PathBuf> = current_files
            .iter()
            .filter(|p| {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    let ext_low = ext.to_lowercase();
                    ["flac", "mp3", "m4a", "aac", "wav", "zip"].contains(&ext_low.as_str())
                } else {
                    false
                }
            })
            .filter(|p| {
                if !before_files.contains(*p) {
                    true
                } else if let Ok(meta) = fs::metadata(p) {
                    if let Ok(mod_time) = meta.modified() {
                        mod_time >= click_time - Duration::from_secs(2)
                    } else {
                        false
                    }
                } else {
                    false
                }
            })
            .collect();

        if !in_progress && !completed.is_empty() {
            let target = completed[0].clone();
            let size1 = fs::metadata(&target)?.len();
            tokio::time::sleep(Duration::from_millis(800)).await;
            let size2 = fs::metadata(&target)?.len();

            if size1 > 0 && size1 == size2 {
                downloaded_file = Some(target);
                break;
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let final_file = downloaded_file.ok_or("Download timed out or failed to complete")?;

    // Rename to custom output path if requested
    let result_path = if let Some(custom_out) = output_path {
        let custom_buf = PathBuf::from(custom_out);
        if let Some(parent) = custom_buf.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&final_file, &custom_buf)?;
        custom_buf
    } else {
        final_file
    };

    let size = fs::metadata(&result_path)?.len();

    Ok(DownloadResult {
        filepath: result_path,
        size,
    })
}
