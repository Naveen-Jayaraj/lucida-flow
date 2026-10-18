#!/usr/bin/env python3
"""
yt-audio-api - YouTube High Quality Audio Downloader & API Service
Extracts highest quality audio (320kbps MP3 / Best Opus) using yt-dlp & FFmpeg.
Can be used as a CLI tool, a Python library, or a REST API server.
"""

import os
import sys
import uuid
import shutil
import argparse
import subprocess
from pathlib import Path
from typing import Optional, Dict, Any, List

DOWNLOAD_DIR = os.getenv("DOWNLOAD_DIR", "./downloads")
os.makedirs(DOWNLOAD_DIR, exist_ok=True)

def find_ffmpeg_path() -> Optional[str]:
    """Find local FFmpeg binary or directory."""
    user_profile = os.environ.get("USERPROFILE", "C:\\Users\\USER")
    candidates = [
        os.path.join(user_profile, r"AppData\Local\Programs\Python\Python312\Lib\site-packages\imageio_ffmpeg\binaries"),
        r"C:\ffmpeg\bin",
    ]
    for c in candidates:
        if os.path.exists(c):
            return c
    # Check if in PATH
    ffmpeg_exe = shutil.which("ffmpeg")
    if ffmpeg_exe:
        return os.path.dirname(ffmpeg_exe)
    return None

def download_youtube_audio(url_or_id: str, output_path: Optional[str] = None, download_dir: str = DOWNLOAD_DIR) -> Dict[str, Any]:
    """
    Download the highest quality audio from a YouTube video and convert to MP3.
    """
    os.makedirs(download_dir, exist_ok=True)
    
    if not url_or_id.startswith("http://") and not url_or_id.startswith("https://"):
        target_url = f"https://www.youtube.com/watch?v={url_or_id}"
    else:
        target_url = url_or_id

    out_template = output_path if output_path else os.path.join(download_dir, "%(title)s.%(ext)s")
    ffmpeg_loc = find_ffmpeg_path()

    cmd = [
        sys.executable, "-m", "yt_dlp",
        "-f", "bestaudio/best",
        "-x",
        "--audio-format", "mp3",
        "--audio-quality", "0",
        "--no-playlist",
        "--no-warnings",
        "-o", out_template,
        "--print", "after_move:filepath",
        target_url
    ]

    if ffmpeg_loc:
        cmd.extend(["--ffmpeg-location", ffmpeg_loc])

    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"yt-dlp failed: {proc.stderr}")

    lines = [line.strip() for line in proc.stdout.splitlines() if line.strip()]
    downloaded_file = lines[-1] if lines else None

    if downloaded_file and os.path.exists(downloaded_file):
        size = os.path.getsize(downloaded_file)
        return {
            "status": "success",
            "filepath": downloaded_file,
            "filename": os.path.basename(downloaded_file),
            "size_bytes": size,
            "size_mb": round(size / (1024 * 1024), 2),
            "quality": "320kbps MP3 (Highest)"
        }

    raise FileNotFoundError("Could not locate downloaded audio file.")

# FastAPI REST Service Mode
def start_api_server(host: str = "0.0.0.0", port: int = 8000):
    try:
        from fastapi import FastAPI, HTTPException, Query
        from fastapi.responses import FileResponse
        import uvicorn
    except ImportError:
        print("Error: fastapi and uvicorn are required to run the REST API server.")
        sys.exit(1)

    app = FastAPI(title="yt-audio-api", description="YouTube Audio Extraction API")
    tokens: Dict[str, str] = {}

    @app.get("/")
    def request_audio(url: str = Query(..., description="YouTube video URL")):
        try:
            res = download_youtube_audio(url)
            token = str(uuid.uuid4())
            tokens[token] = res["filepath"]
            return {
                "token": token,
                "filename": res["filename"],
                "size_mb": res["size_mb"],
                "quality": res["quality"],
                "download_url": f"/download?token={token}"
            }
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e))

    @app.get("/download")
    def download_file(token: str = Query(..., description="Download token")):
        if token not in tokens or not os.path.exists(tokens[token]):
            raise HTTPException(status_code=404, detail="Invalid or expired token")
        path = tokens[token]
        return FileResponse(path, media_type="audio/mpeg", filename=os.path.basename(path))

    print(f"Starting yt-audio-api server on http://{host}:{port}")
    uvicorn.run(app, host=host, port=port)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="yt-audio-api YouTube Downloader")
    parser.add_argument("query_or_url", nargs="?", help="YouTube URL or video ID to download")
    parser.add_argument("-o", "--output", help="Custom output path")
    parser.add_argument("--serve", action="store_true", help="Start the FastAPI REST server")
    parser.add_argument("--port", type=int, default=8000, help="Port for the API server")

    args = parser.parse_args()

    if args.serve:
        start_api_server(port=args.port)
    elif args.query_or_url:
        print(f"Downloading YouTube audio: {args.query_or_url}...")
        res = download_youtube_audio(args.query_or_url, args.output)
        print(f"✓ Downloaded: {res['filename']} ({res['size_mb']} MB)")
        print(f"  Saved to: {res['filepath']}")
    else:
        parser.print_help()
