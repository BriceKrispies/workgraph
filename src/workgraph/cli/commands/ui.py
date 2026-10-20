"""ui commands: work with the web UI served by ``workgraph serve``."""

from __future__ import annotations

from pathlib import Path
from urllib.parse import urljoin

import typer

from workgraph.cli.common import STATE, emit, fail, handle_errors, kv

ui_app = typer.Typer(help="Work with the web UI served by `workgraph serve`.", no_args_is_help=True)

# Pages whose content renders after their own data arrives, and the element that marks it.
_READY_SELECTORS = {
    "graph": '[data-testid="graph-counts"]',
}


def _base_url(url: str | None) -> str:
    if url:
        return url.rstrip("/") + "/"
    server = STATE.app().config.server
    return f"http://{server.host}:{server.port}/"


@ui_app.command("screenshot")
@handle_errors
def screenshot_command(
    page: str = typer.Argument(
        "/", help="UI path with its query, e.g. 'graph?repo=rho&linked=1' or 'tasks/WG-12'"
    ),
    out: Path = typer.Option(Path("workgraph-screenshot.png"), "--out", "-o", help="PNG to write"),
    width: int = typer.Option(1600, help="Viewport width in CSS pixels"),
    height: int = typer.Option(1000, help="Viewport height in CSS pixels"),
    scale: float = typer.Option(1.0, help="Device scale factor (2 for a sharp, zoomable image)"),
    wait_for: str | None = typer.Option(
        None, "--wait-for", help="CSS selector to wait for (default: chosen per page)"
    ),
    settle: float = typer.Option(1.0, help="Seconds to wait after the page is ready"),
    timeout: float = typer.Option(60.0, help="Seconds to wait for the page before giving up"),
    url: str | None = typer.Option(None, help="Base URL of the server (default: from config)"),
    browser: str | None = typer.Option(
        None, help="Chrome/Edge/Chromium executable (default: $WORKGRAPH_BROWSER, then auto-detect)"
    ),
    theme: str | None = typer.Option(
        None, help="Emulate the system colour scheme: light or dark (default: the browser's)"
    ),
    mobile: bool = typer.Option(
        False, "--mobile", help="Emulate a touch phone; pair with a phone size, e.g. --width 390 --height 844"
    ),
) -> None:
    """Capture a page of the running web UI to a PNG with a headless browser."""
    import httpx

    from workgraph.integrations.browser import screenshot

    base = _base_url(url)
    try:
        httpx.get(urljoin(base, "api/v1/health"), timeout=5).raise_for_status()
    except httpx.HTTPError as exc:
        fail(f"no Workgraph server at {base} ({exc}); start one with `workgraph serve`")
    target = urljoin(base, page.lstrip("/"))
    route = page.lstrip("/").split("?", 1)[0].split("/", 1)[0]
    # the app shell has rendered, plus anything the page marks as its own ready state
    selector = wait_for or _READY_SELECTORS.get(route, "#root > *")
    capture = screenshot(
        target,
        out,
        width=width,
        height=height,
        scale=scale,
        wait_for=selector,
        settle=settle,
        timeout=timeout,
        browser=browser,
        theme=theme,
        mobile=mobile,
    )
    result = {
        "url": capture.url,
        "file": str(capture.path),
        "width": capture.width,
        "height": capture.height,
        "scale": capture.scale,
        "browser": str(capture.browser),
        "theme": theme,
        "mobile": mobile,
    }
    emit(result, lambda r: kv([(k, r[k]) for k in ("url", "file", "width", "height", "scale", "browser")]))
