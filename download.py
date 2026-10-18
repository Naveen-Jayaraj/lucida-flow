#!/usr/bin/env python3
"""
Lucida Flow - High Performance Rust Engine Wrapper
"""
import sys
import os
import subprocess

if __name__ == "__main__":
    exe_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "lucida.exe")
    if not os.path.exists(exe_path):
        exe_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "target", "release", "lucida.exe")
    
    args = [exe_path] + sys.argv[1:]
    sys.exit(subprocess.call(args))
