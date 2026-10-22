"""Local HTTP security: bearer token, Host validation (DNS rebinding) and Origin checks.

* The server binds to loopback by default; non-loopback binding needs explicit opt-in.
* Every ``/api`` and ``/mcp`` request needs the local API token (``Authorization:
  Bearer``). The bundled UI receives it embedded in ``index.html``, which other
  origins cannot read because no permissive CORS headers are ever sent. The SSE
  stream may pass it as ``?token=`` because ``EventSource`` cannot set headers.
* The ``Host`` header must name an allowed host, defeating DNS-rebinding attacks.
* State-changing requests carrying an ``Origin`` header must come from the server's
  own origin (or an explicitly configured development origin).
* On the local network (``LanAccess``) only clients from private networks are served,
  ``/mcp`` stays loopback-only, and a browser on another device receives the token only
  after it has been paired once with the code printed by ``workgraph serve``.
"""

from __future__ import annotations

import hashlib
import hmac
import os
import secrets
import socket
import threading
from collections.abc import Awaitable, Callable, Iterable
from dataclasses import dataclass, field
from ipaddress import IPv4Network, IPv6Network, ip_address, ip_network

from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.types import ASGIApp, Receive, Scope, Send

SAFE_METHODS = {"GET", "HEAD", "OPTIONS"}
PUBLIC_PATHS = {"/api/v1/health"}


DEVICE_COOKIE = "workgraph_device"
PAIRING_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789"
PAIRING_MAX_FAILURES = 10

Network = IPv4Network | IPv6Network

# RFC 1918 and IPv6 unique-local ranges: addresses a home or office LAN hands out.
PRIVATE_NETWORKS: tuple[Network, ...] = tuple(
    ip_network(n) for n in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "fc00::/7")
)


def detect_lan_address() -> str | None:
    """The private IPv4 address of the interface holding the default route, if there is one.

    Connecting a UDP socket sends no packets; it only asks the OS which source address it
    would use, which is the machine's address on the network it is attached to.
    """
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
            probe.connect(("192.0.2.1", 9))  # TEST-NET-1, never routed anywhere real
            address = probe.getsockname()[0]
    except OSError:
        return None
    if any(ip_address(address) in net for net in PRIVATE_NETWORKS):
        return address
    return None


def _new_pairing_code() -> str:
    raw = "".join(secrets.choice(PAIRING_ALPHABET) for _ in range(8))
    return f"{raw[:4]}-{raw[4:]}"


def _normalise_code(code: str) -> str:
    return "".join(ch for ch in code.upper() if ch.isalnum())


@dataclass
class LanAccess:
    """Serving the UI to other devices on the local network, e.g. a phone on the same Wi-Fi."""

    address: str
    networks: tuple[Network, ...] = PRIVATE_NETWORKS
    pairing_code: str = field(default_factory=_new_pairing_code)
    _failures: int = 0
    _lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    @classmethod
    def build(cls, address: str, networks: Iterable[str] = ()) -> LanAccess:
        parsed = tuple(ip_network(n, strict=False) for n in networks)
        return cls(address=address, networks=parsed or PRIVATE_NETWORKS)

    @property
    def hosts(self) -> set[str]:
        name = socket.gethostname().lower()
        return {self.address, name, f"{name}.local"}

    def client_allowed(self, client: str) -> bool:
        try:
            addr = ip_address(client)
        except ValueError:
            return False
        if addr.version == 6 and addr.ipv4_mapped is not None:
            addr = addr.ipv4_mapped
        return addr.is_loopback or any(addr in net for net in self.networks)

    @property
    def locked(self) -> bool:
        return self._failures >= PAIRING_MAX_FAILURES

    def try_pair(self, code: str) -> bool:
        """Check a pairing code; after too many wrong guesses pairing stays off until restart."""
        with self._lock:
            if self.locked:
                return False
            ok = hmac.compare_digest(
                _normalise_code(code).encode(), _normalise_code(self.pairing_code).encode()
            )
            if not ok:
                self._failures += 1
            return ok


def device_cookie_value(token: str) -> str:
    """Derived from the API token, so rotating the token unpairs every device."""
    return hmac.new(token.encode(), b"workgraph-paired-device", hashlib.sha256).hexdigest()


def is_loopback_client(request: Request) -> bool:
    client = request.client.host if request.client else ""
    try:
        return ip_address(client).is_loopback
    except ValueError:
        return False


def device_paired(request: Request, token: str) -> bool:
    supplied = request.cookies.get(DEVICE_COOKIE, "")
    return bool(supplied) and hmac.compare_digest(supplied.encode(), device_cookie_value(token).encode())


def allowed_hosts(host: str, port: int, lan: LanAccess | None = None) -> set[str]:
    hosts = {"localhost", "127.0.0.1", "[::1]", host}
    if lan is not None:
        hosts |= lan.hosts
    extra = os.environ.get("WORKGRAPH_ALLOWED_HOSTS", "")
    hosts |= {h.strip() for h in extra.split(",") if h.strip()}
    with_ports = {f"{h}:{port}" for h in hosts}
    return hosts | with_ports


def allowed_origins(host: str, port: int, lan: LanAccess | None = None) -> set[str]:
    names = {"localhost", "127.0.0.1", "[::1]", host} | (lan.hosts if lan is not None else set())
    origins = {f"http://{h}:{port}" for h in names}
    extra = os.environ.get("WORKGRAPH_ALLOWED_ORIGINS", "")
    return origins | {o.strip().rstrip("/") for o in extra.split(",") if o.strip()}


class LocalSecurityMiddleware:
    def __init__(
        self, app: ASGIApp, *, token: str, host: str, port: int, lan: LanAccess | None = None
    ) -> None:
        self.app = app
        self.token = token
        self.lan = lan
        self.hosts = allowed_hosts(host, port, lan)
        self.origins = allowed_origins(host, port, lan)

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        request = Request(scope)
        if self.lan is not None:
            client = request.client.host if request.client else ""
            if not self.lan.client_allowed(client):
                await JSONResponse(
                    {"error": {"code": "forbidden_client", "message": "client network not allowed"}},
                    status_code=403,
                )(scope, receive, send)
                return
            if scope["path"].startswith("/mcp") and not is_loopback_client(request):
                await JSONResponse(
                    {"error": {"code": "forbidden_client", "message": "MCP is only served on loopback"}},
                    status_code=403,
                )(scope, receive, send)
                return
        host = (request.headers.get("host") or "").lower()
        if host not in self.hosts:
            await JSONResponse(
                {"error": {"code": "forbidden_host", "message": f"host {host!r} not allowed"}},
                status_code=421,
            )(scope, receive, send)
            return
        path = scope["path"]
        protected = (path.startswith(("/api/", "/mcp"))) and path not in PUBLIC_PATHS
        if protected:
            origin = request.headers.get("origin")
            if origin and request.method not in SAFE_METHODS and origin.rstrip("/") not in self.origins:
                await JSONResponse(
                    {"error": {"code": "forbidden_origin", "message": f"origin {origin} not allowed"}},
                    status_code=403,
                )(scope, receive, send)
                return
            if not self._authorized(request):
                await JSONResponse(
                    {"error": {"code": "unauthorized", "message": "missing or invalid API token"}},
                    status_code=401,
                    headers={"WWW-Authenticate": "Bearer"},
                )(scope, receive, send)
                return
        await self.app(scope, receive, send)

    def _authorized(self, request: Request) -> bool:
        header = request.headers.get("authorization", "")
        supplied = (
            header[7:].strip()
            if header.lower().startswith("bearer ")
            else request.headers.get("x-workgraph-token")
        )
        if not supplied and request.url.path.endswith("/events/stream"):
            supplied = request.query_params.get("token")
        return bool(supplied) and hmac.compare_digest(supplied.encode(), self.token.encode())


Handler = Callable[[Request], Awaitable[Response]]


async def security_headers(request: Request, call_next: Handler) -> Response:
    response = await call_next(request)
    response.headers.setdefault("X-Content-Type-Options", "nosniff")
    response.headers.setdefault("X-Frame-Options", "DENY")
    response.headers.setdefault("Referrer-Policy", "no-referrer")
    if request.url.path.startswith("/api/"):
        response.headers.setdefault("Cache-Control", "no-store")
    return response
