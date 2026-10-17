import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..");
export const stateFile = join(here, "..", ".e2e-home", "state.json");

function chromeInstalled(): boolean {
  if (process.env.WG_E2E_CHROME === "skip") return false;
  const candidates =
    process.platform === "win32"
      ? [
          join(process.env["PROGRAMFILES"] ?? "C:\\Program Files", "Google", "Chrome", "Application", "chrome.exe"),
          join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Google", "Chrome", "Application", "chrome.exe"),
          join(process.env["LOCALAPPDATA"] ?? "", "Google", "Chrome", "Application", "chrome.exe"),
        ]
      : process.platform === "darwin"
        ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
        : ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/opt/google/chrome/chrome"];
  return candidates.some((p) => existsSync(p));
}

function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : 0;
      server.close(() => resolvePort(port));
    });
    server.on("error", reject);
  });
}

function workgraph(home: string, args: string[]) {
  return spawnSync("uv", ["run", "--no-sync", "workgraph", "--home", home, ...args], {
    cwd: repoRoot,
    encoding: "utf-8",
    shell: false,
    timeout: 600_000,
    env: { ...process.env, PYTHONUTF8: "1" },
  });
}

export default async function globalSetup(): Promise<void> {
  if (!chromeInstalled()) {
    process.env.WG_E2E_SKIP = "Google Chrome is not installed; browser tests skipped";
    return;
  }
  const home = join(here, "..", ".e2e-home", `home-${Date.now()}`);
  rmSync(join(here, "..", ".e2e-home"), { recursive: true, force: true });
  mkdirSync(home, { recursive: true });
  // Seed demo data. `demo seed` is the documented path; the end-to-end demonstration is
  // used as a fallback data source when seeding is unavailable.
  let seeded = workgraph(home, ["demo", "seed"]);
  let source = "demo seed";
  if (seeded.status !== 0) {
    rmSync(home, { recursive: true, force: true });
    mkdirSync(home, { recursive: true });
    seeded = workgraph(home, ["demo", "e2e", "--home", home, "--keep"]);
    source = "demo e2e";
  }
  if (seeded.status !== 0) {
    throw new Error(`failed to seed demo data:\n${seeded.stdout}\n${seeded.stderr}`);
  }
  // `demo e2e --home` keeps its own home layout under the given directory.
  const port = await freePort();
  const server = spawn("uv", ["run", "--no-sync", "workgraph", "--home", home, "serve", "--port", String(port), "--no-worker"], {
    cwd: repoRoot,
    stdio: "ignore",
    shell: false,
    detached: process.platform !== "win32",
  });
  const baseURL = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + 90_000;
  for (;;) {
    try {
      const response = await fetch(`${baseURL}/api/v1/health`);
      if (response.ok) break;
    } catch {
      // not up yet
    }
    if (Date.now() > deadline) throw new Error("backend did not start");
    await new Promise((r) => setTimeout(r, 300));
  }
  process.env.WG_E2E_BASE_URL = baseURL;
  process.env.WG_E2E_SOURCE = source;
  writeFileSync(stateFile, JSON.stringify({ pid: server.pid, baseURL, home, source }));
}
