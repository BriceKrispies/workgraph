"""serve and mcp commands."""

from __future__ import annotations

import socket
import threading
from ipaddress import ip_address

import typer

from workgraph.cli.common import STATE, err_console, fail, handle_errors


def _is_loopback(host: str) -> bool:
    if host == "localhost":
        return True
    try:
        return ip_address(host).is_loopback
    except ValueError:
        return False


def _bind(host: str, port: int) -> socket.socket:
    family = socket.AF_INET6 if ":" in host else socket.AF_INET
    sock = socket.socket(family, socket.SOCK_STREAM)
    try:
        sock.bind((host.strip("[]"), port))
    except OSError as exc:
        sock.close()
        fail(f"cannot listen on {host}:{port}: {exc}", 2)
    sock.set_inheritable(True)
    return sock


@handle_errors
def serve_command(
    host: str | None = typer.Option(None, help="Bind address (default from config: 127.0.0.1)"),
    port: int | None = typer.Option(None, help="Port (default from config: 8765)"),
    worker: bool = typer.Option(True, "--worker/--no-worker", help="Run a background job worker in-process"),
    mcp: bool = typer.Option(True, "--mcp/--no-mcp", help="Expose the Streamable HTTP MCP endpoint at /mcp/"),
    allow_remote: bool = typer.Option(False, help="Permit binding to a non-loopback address"),
    lan: bool | None = typer.Option(
        None,
        "--lan/--local",
        help="Also serve on this machine's local-network address for paired devices "
        "(default from config: on), or loopback only",
    ),
) -> None:
    """Run the local HTTP API, web UI and (optionally) a background worker."""
    import uvicorn

    from workgraph.api.app import create_app
    from workgraph.api.security import LanAccess, detect_lan_address
    from workgraph.core.logging import configure_logging
    from workgraph.jobs.worker import Worker

    app = STATE.app()
    config = app.config.server
    bind_host = host or config.host
    bind_port = port or config.port
    if not _is_loopback(bind_host) and not (allow_remote or config.allow_non_loopback):
        fail(f"refusing to bind to non-loopback address {bind_host}; pass --allow-remote to opt in", 2)
    lan_access: LanAccess | None = None
    if (config.lan if lan is None else lan) and _is_loopback(bind_host):
        address = config.lan_address or detect_lan_address()
        if address is None:
            err_console.print("[yellow]No private local-network address found; serving on loopback only.[/]")
        else:
            lan_access = LanAccess.build(address, config.lan_networks)
    configure_logging("INFO", app.paths.log_dir)
    asgi = create_app(app, host=bind_host, port=bind_port, with_mcp=mcp, lan=lan_access)
    sockets = [_bind(bind_host, bind_port)]
    if lan_access is not None:
        sockets.append(_bind(lan_access.address, bind_port))
    background: Worker | None = None
    thread: threading.Thread | None = None
    if worker:
        background = Worker(app)
        thread = threading.Thread(target=background.run, name="workgraph-worker", daemon=True)
        thread.start()
    err_console.print(
        f"Workgraph listening on http://{bind_host}:{bind_port}/  (API token: {app.paths.api_token_file})"
    )
    if lan_access is not None:
        err_console.print(
            f"Local network: http://{lan_access.address}:{bind_port}/  "
            f"pairing code [bold]{lan_access.pairing_code}[/] (enter it once per device)"
        )
    server = uvicorn.Server(uvicorn.Config(asgi, log_level="warning"))
    try:
        server.run(sockets=sockets)
    finally:
        if background is not None:
            background.stop()
        if thread is not None:
            thread.join(timeout=30)


@handle_errors
def mcp_command(
    transport: str = typer.Option("stdio", help="stdio or http"),
    agent_name: str = typer.Option("mcp-client", "--agent-name", help="Agent identity recorded in history"),
    grant: list[str] = typer.Option(
        None, "--grant", help="Extra permission to grant (repeatable), e.g. run:start"
    ),
    host: str = typer.Option("127.0.0.1"),
    port: int = typer.Option(8766),
) -> None:
    """Run the MCP server (stdio for local clients such as Claude Code; http for Streamable HTTP)."""
    from workgraph.mcp.server import build_server
    from workgraph.security.permissions import DEFAULT_AGENT_PERMISSIONS, parse_permissions

    app = STATE.app()
    permissions = DEFAULT_AGENT_PERMISSIONS | parse_permissions(grant or [])
    if transport == "stdio":
        server = build_server(app, agent_name=agent_name, permissions=permissions)
        server.run("stdio")
        return
    if transport != "http":
        fail("transport must be stdio or http", 2)
    if not _is_loopback(host):
        fail("the MCP HTTP transport only binds to loopback addresses", 2)
    import uvicorn

    from workgraph.api.app import create_app

    asgi = create_app(app, host=host, port=port, with_mcp=True, mcp_agent=agent_name)
    err_console.print(
        f"MCP Streamable HTTP on http://{host}:{port}/mcp/ (bearer token in {app.paths.api_token_file})"
    )
    uvicorn.run(asgi, host=host, port=port, log_level="warning")
