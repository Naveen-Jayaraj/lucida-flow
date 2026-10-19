# Lucida Flow + YouTube Music Downloader

High-performance music search and downloader engine written in pure Rust with Python integration. Searches both **Lucida (Amazon Music / Lossless FLAC)** and **YouTube (Highest Quality 320kbps MP3 / Best Opus)** simultaneously in parallel, displays all details side-by-side, and lets you choose your preferred source to download.

---

## ⚡ Features

- 🏎️ **Parallel Search Engine**: Queries Lucida and YouTube concurrently with zero added latency.
- 🟢 **Spotify Playlist Downloader**: Pass any Spotify playlist link or ID (`https://open.spotify.com/playlist/...`), parse all tracks via Spotify API proxy, put each through the dual Lucida/YouTube pipeline, pick the best match, and download all tracks into a dedicated folder named after the playlist with batch concurrency.
- 🎯 **Fuzzy Logic Matching**: Advanced multi-tier scoring (token set coverage, character bigrams, and typo-resilience) ranks close matches with match percentages.
- ⚖️ **Balanced 50/50 Results**: Lists ~10 total results (top 5 for Lucida + top 5 for YouTube) so you can choose the exact version you want.
- 🎧 **Dual Source Quality Selection**:
  - **Lucida**: Lossless FLAC / High-Res Audio from Amazon Music, Tidal, Deezer, etc.
  - **YouTube (yt-audio-api / yt-dlp)**: Highest quality audio (320kbps MP3 / Best Opus) extracted with FFmpeg.
- 📋 **Full Track Details**: View Title, Artist/Channel, Album, Duration, Views, and Audio Format before downloading.
- 🔄 **Smart Interactive Prompt**: Prompt to choose `[1] Lucida`, `[2] YouTube`, `[3] Both`, or `[q] Cancel`.
- 🔗 **Direct URL Auto-Routing**: Pass any direct YouTube link, Lucida streaming link, or Spotify playlist link and it downloads immediately.
- 🐍 **Python Wrapper & REST API**: Run via `download.py` or start the REST API service via `yt_audio_api.py --serve`.

---

## 🚀 Usage

### 1. Spotify Playlist Downloader (Fast Concurrent Batching)
Download an entire Spotify playlist directly by URL or ID:
```bash
# Direct command with concurrency control (default: 3 concurrent downloads)
lucida playlist "https://open.spotify.com/playlist/37i9dQZF1DX3bcfiyW6qms?si=AlR_u-HMT3q3hoknYT9N-Q" --concurrency 4

# Or pass Spotify link directly (auto-detected)
lucida "https://open.spotify.com/playlist/37i9dQZF1DX3bcfiyW6qms"

# Or use the playlist ID
lucida playlist 37i9dQZF1DX3bcfiyW6qms
```
- Fetches all tracks via `https://spotify.xwolf.space/api/playlist/{PLAYLIST_ID}`.
- Puts tracks one-by-one into the dual Lucida & YouTube pipeline.
- Automatically selects the best matching candidate (Lossless FLAC from Lucida or 320kbps MP3 from YouTube).
- Saves all tracks inside `./downloads/<Playlist_Name>/`.
- Embeds metadata, front cover art, and synchronized `.lrc` lyrics.
- Automatically skips tracks that have already been downloaded for instant resumes.

### 2. Quick Download (Interactive Dual Search)
Search both sources in parallel and choose:
```bash
lucida "Orikkal Nee Paranju"
```
or via Python:
```bash
python download.py "Orikkal Nee Paranju"
```

**Interactive Prompt Display:**
```text
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  PARALLEL SEARCH RESULTS   Choose which source to download
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  [1] LUCIDA (amazon_music / Lossless Audio)
      Track Name:    Orikkal Nee Paranju
      Artist:        Umbayee
      Album:         Orikkal Nee Paranju
      Audio Format:  Lossless FLAC / High-Res Audio
      Source URL:    https://music.amazon.com/tracks/B0H4VXZRGP?musicTerritory=US

  [2] YOUTUBE (yt-audio-api / yt-dlp highest quality)
      Video Title:   Orikkal Nee Paranju | Positive | Jayasurya | Alex Paul
      Channel:       Speed Music Videos
      Duration:      4:03
      Views:         88,338 views
      Audio Format:  Highest Audio (320kbps MP3 / Best Opus)
      Source URL:    https://www.youtube.com/watch?v=gPTEPtZ8tIM

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Select source to download [1: Lucida, 2: YouTube, 3: Both, q: Cancel] (default: 1):
```

### 2. Force Source via CLI Flags
To bypass interactive prompts and download directly:
```bash
# Force YouTube download
lucida download "hotel california" --source youtube

# Force Lucida download
lucida download "hotel california" --source lucida

# Download both
lucida download "hotel california" --source both
```

### 3. Parallel Search Only
Search both sources without downloading:
```bash
lucida search "hotel california" --limit 5
```

### 4. Direct URL Downloads
```bash
# Direct YouTube URL
lucida "https://www.youtube.com/watch?v=gPTEPtZ8tIM"

# Direct Amazon Music / Lucida URL
lucida "https://music.amazon.com/tracks/B0H4VXZRGP"
```

### 5. Standalone YouTube Audio API
```bash
# Download audio from YouTube directly
python yt_audio_api.py "https://www.youtube.com/watch?v=gPTEPtZ8tIM"

# Run FastAPI REST server on port 8000
python yt_audio_api.py --serve
```
