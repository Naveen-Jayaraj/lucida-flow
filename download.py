#!/usr/bin/env python3
"""
Direct song search & download script.
Usage:
    python download.py "song name or URL"
"""

import sys
from cli import download

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python download.py \"song name or URL\" [--output ./downloads/song.flac]")
        sys.exit(1)
    
    # Delegate to click command
    download.main(args=sys.argv[1:], standalone_mode=True)
