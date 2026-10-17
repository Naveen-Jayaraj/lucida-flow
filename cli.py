#!/usr/bin/env python3
"""
Lucida Flow CLI
Command-line interface for downloading music from Lucida.to
"""

import click
import os
import sys
from dotenv import load_dotenv

# Ensure UTF-8 output encoding on Windows consoles
if sys.platform == "win32":
    try:
        if sys.stdout.encoding and sys.stdout.encoding.lower() != "utf-8":
            sys.stdout.reconfigure(encoding="utf-8")
        if sys.stderr.encoding and sys.stderr.encoding.lower() != "utf-8":
            sys.stderr.reconfigure(encoding="utf-8")
    except Exception:
        pass

from rich.console import Console
from rich.table import Table
from rich import print as rprint
from lucida_client import LucidaClient

# Load environment variables
load_dotenv()

console = Console()


@click.group()
@click.version_option(version="1.0.0")
def cli():
    """Lucida Flow - Download music from various streaming services via Lucida.to"""
    pass


@cli.command()
@click.argument("query")
@click.option(
    "--service",
    "-s",
    default="amazon_music",
    type=click.Choice(
        [
            "amazon_music",
            "tidal",
            "qobuz",
            "spotify",
            "deezer",
            "soundcloud",
            "yandex_music",
        ],
        case_sensitive=False,
    ),
    help="Music service to search (default: amazon_music)",
)
@click.option("--limit", "-l", default=10, help="Maximum number of results")
def search(query, service, limit):
    """Search for music on a specific service"""
    console.print(f"\n[bold cyan]Searching {service} for:[/bold cyan] {query}\n")

    with console.status("[bold green]Searching..."):
        client = LucidaClient()
        results = client.search(query, service=service, limit=limit)

    if "error" in results:
        console.print(f"[bold red]Error:[/bold red] {results['error']}")
        if service.lower() != "amazon_music":
            console.print(
                f"[yellow]Tip: Searching on '{service}' is currently restricted upstream on Lucida.to.[/yellow]"
            )
            console.print(
                f"[green]Try searching with Amazon Music (the primary working service):[/green] [bold]python cli.py search \"{query}\" -s amazon_music[/bold]\n"
            )
        return

    tracks = results.get("tracks", [])

    if not tracks:
        console.print("[yellow]No results found[/yellow]")
        return

    # Create table
    table = Table(title=f"Search Results ({len(tracks)} tracks)")
    table.add_column("#", style="cyan", width=4)
    table.add_column("Title", style="green")
    table.add_column("Artist", style="yellow")
    table.add_column("Album", style="magenta")
    table.add_column("URL", style="dim cyan", overflow="fold")

    for idx, track in enumerate(tracks, 1):
        table.add_row(
            str(idx),
            track.get("name", "Unknown"),
            track.get("artist", "Unknown"),
            track.get("album", "Unknown"),
            track.get("url", ""),
        )

    console.print(table)
    console.print()


@cli.command()
@click.argument("query_or_url")
@click.option("--output", "-o", help="Output directory", default=None)
@click.option(
    "--service",
    "-s",
    default="amazon_music",
    help="Music service to search if a song title was provided instead of a URL",
)
def download(query_or_url, output, service):
    """Download a track from URL (or search and download by song name)"""
    client = LucidaClient()
    target_url = query_or_url

    # If the user passed a song title/search query instead of a URL
    if not (query_or_url.startswith("http://") or query_or_url.startswith("https://")):
        console.print(
            f"\n[cyan]Detected song query:[/cyan] \"{query_or_url}\""
        )
        with console.status(f"[bold green]Searching {service}..."):
            search_results = client.search(query_or_url, service=service, limit=1)

        tracks = search_results.get("tracks", [])
        if not tracks or not tracks[0].get("url"):
            console.print(f"[bold red]No matching tracks found for:[/bold red] {query_or_url}")
            console.print("[dim]Tip: Try searching with `python cli.py search \"<query>\"` to see available tracks or provide a full URL.[/dim]\n")
            return

        track = tracks[0]
        target_url = track["url"]
        console.print(
            f"[bold green]Selected track:[/bold green] {track.get('name')} by {track.get('artist')}"
        )
        console.print(f"[dim]URL: {target_url}[/dim]\n")

    console.print(f"[bold cyan]Downloading from:[/bold cyan] {target_url}\n")

    with console.status("[bold green]Preparing download..."):
        # Get track info first
        info = client.get_track_info(target_url)

        if "error" in info:
            console.print(
                f"[bold red]Error getting track info:[/bold red] {info['error']}"
            )
        elif info.get("name"):
            console.print(f"[bold]Track:[/bold] {info['name']}")
            if info.get("artist"):
                console.print(f"[bold]Artist:[/bold] {info['artist']}")
            if info.get("album"):
                console.print(f"[bold]Album:[/bold] {info['album']}")
            console.print()

    with console.status("[bold green]Downloading..."):
        result = client.download_track(target_url, output)

    if result.get("success"):
        console.print(f"[bold green]✓ Download successful![/bold green]")
        console.print(f"[bold]Saved to:[/bold] {result['filepath']}")
        console.print(
            f"[bold]Size:[/bold] {result['size']:,} bytes ({result['size'] / 1024 / 1024:.2f} MB)"
        )
    else:
        console.print(
            f"[bold red]✗ Download failed:[/bold red] {result.get('error', 'Unknown error')}"
        )


@cli.command()
@click.argument("url")
def info(url):
    """Get information about a track"""
    console.print(f"\n[bold cyan]Getting info for:[/bold cyan] {url}\n")

    with console.status("[bold green]Fetching information..."):
        client = LucidaClient()
        track_info = client.get_track_info(url)

    if "error" in track_info:
        console.print(f"[bold red]Error:[/bold red] {track_info['error']}")
        return

    # Display track information
    table = Table(title="Track Information")
    table.add_column("Field", style="cyan")
    table.add_column("Value", style="green")

    for key, value in track_info.items():
        if value and key != "error":
            table.add_row(key.capitalize(), str(value))

    console.print(table)
    console.print()


@cli.command()
def services():
    """List available streaming services"""
    console.print("\n[bold cyan]Available Services:[/bold cyan]\n")

    client = LucidaClient()
    services_list = client.get_available_services()

    for service in services_list:
        console.print(f"  [green]✓[/green] {service}")

    console.print(f"\n[dim]Total: {len(services_list)} services[/dim]\n")


@cli.command()
def stats():
    """Show rate limiter statistics"""
    console.print("\n[bold cyan]Rate Limiter Statistics:[/bold cyan]\n")

    client = LucidaClient()
    stats = client.get_rate_limit_stats()

    table = Table(title="Request Stats")
    table.add_column("Metric", style="cyan")
    table.add_column("Value", style="green")

    # Current usage
    table.add_row("Requests (last minute)", str(stats["requests_last_minute"]))
    table.add_row("Requests (last hour)", str(stats["requests_last_hour"]))
    table.add_row("Total tracked requests", str(stats["total_requests"]))
    table.add_row("Consecutive errors", str(stats["consecutive_errors"]))

    console.print(table)
    console.print()

    # Limits table
    limits_table = Table(title="Rate Limits")
    limits_table.add_column("Limit Type", style="cyan")
    limits_table.add_column("Value", style="yellow")

    limits = stats["limits"]
    limits_table.add_row("Per minute", str(limits["per_minute"]))
    limits_table.add_row("Per hour", str(limits["per_hour"]))
    limits_table.add_row("Min delay (seconds)", str(limits["min_delay_seconds"]))

    console.print(limits_table)
    console.print()


@cli.command()
def config():
    """Show current configuration"""
    console.print("\n[bold cyan]Current Configuration:[/bold cyan]\n")

    table = Table()
    table.add_column("Setting", style="cyan")
    table.add_column("Value", style="green")

    base_url = os.getenv("LUCIDA_BASE_URL", "https://lucida.to")
    download_dir = os.getenv("DOWNLOAD_DIR", "./downloads")
    timeout = os.getenv("REQUEST_TIMEOUT", "30")

    table.add_row("Base URL", base_url)
    table.add_row("Download Directory", download_dir)
    table.add_row("Request Timeout", f"{timeout}s")

    console.print(table)
    console.print()


if __name__ == "__main__":
    cli()
