import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { stateFile } from "./global-setup";

export default async function globalTeardown(): Promise<void> {
  if (!existsSync(stateFile)) return;
  const { pid } = JSON.parse(readFileSync(stateFile, "utf-8")) as { pid?: number };
  if (!pid) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/F", "/T", "/PID", String(pid)], { stdio: "ignore" });
  } else {
    try {
      process.kill(-pid, "SIGTERM");
    } catch {
      // already gone
    }
  }
}
