import { chromium } from "@playwright/test";
import { createServer } from "node:net";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { spawn, spawnSync } from "node:child_process";

const webRoot = resolve(import.meta.dirname, "..");
const repoRoot = resolve(webRoot, "../..");
const authFile = resolve(webRoot, ".playwright-auth.json");
const runtimeFile = resolve(webRoot, ".playwright-runtime.json");

function binary(name) {
  return resolve(repoRoot, "target", "debug", process.platform === "win32" ? `${name}.exe` : name);
}

function checked(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", timeout: 240_000, ...options });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed\n${result.stdout}\n${result.stderr}`);
  return result.stdout;
}

function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => resolvePort(address.port));
    });
  });
}

function waitForBootstrap(child) {
  return new Promise((resolveUrl, reject) => {
    let output = "";
    const timeout = setTimeout(() => reject(new Error(`Console did not emit a bootstrap URL\n${output}`)), 90_000);
    const receive = (chunk) => {
      output += chunk.toString();
      const match = output.match(/Open this URL in your browser: (http:\/\/127\.0\.0\.1:\d+\/#bootstrap=[0-9a-f]+)/);
      if (match) { clearTimeout(timeout); resolveUrl(match[1]); }
    };
    child.stdout.on("data", receive);
    child.stderr.on("data", receive);
    child.once("exit", (code) => { clearTimeout(timeout); reject(new Error(`Console exited before bootstrap (code ${code})\n${output}`)); });
  });
}

export default async function globalSetup() {
  checked("cargo", ["build", "-p", "draft-cli", "-p", "draftd", "--offline"], { cwd: repoRoot, stdio: ["ignore", "pipe", "pipe"] });
  const testHome = mkdtempSync(resolve(tmpdir(), "draft-console-playwright-"));
  const workspace = resolve(testHome, "workspace");
  const globalStore = resolve(testHome, "global");
  mkdirSync(workspace, { recursive: true });
  const env = { ...process.env, HOME: testHome, DRAFT_GLOBAL_HOME: globalStore };
  const initialized = JSON.parse(checked(binary("draft"), ["project", "init", workspace, "--json"], { cwd: testHome, env }));
  const port = await freePort();
  const consoleProcess = spawn(binary("draft"), ["console", "--no-open", "--port", String(port), "--project", initialized.workspace_id], {
    cwd: testHome,
    env,
    stdio: ["ignore", "pipe", "pipe"]
  });
  const bootstrapUrl = await waitForBootstrap(consoleProcess);
  const origin = new URL(bootstrapUrl).origin;
  const browser = await chromium.launch({ headless: true });
  try {
    const diagnostics = [];
    const context = await browser.newContext();
    const page = await context.newPage();
    page.on("console", (message) => diagnostics.push(`console ${message.type()}: ${message.text()}`));
    page.on("pageerror", (error) => diagnostics.push(`pageerror: ${error.message}`));
    const response = await page.goto(bootstrapUrl);
    try {
      await page.getByRole("heading", { name: basename(workspace) }).waitFor();
    } catch (error) {
      throw new Error(`Console bootstrap page did not become ready\nURL: ${page.url()}\nHTTP: ${response?.status()}\nText: ${await page.locator("body").innerText()}\n${diagnostics.join("\n")}\n${error.message}`);
    }
    await context.storageState({ path: authFile });
  } catch (error) {
    consoleProcess.kill("SIGINT");
    spawnSync(binary("draft"), ["service", "stop"], { cwd: testHome, env, timeout: 30_000 });
    rmSync(testHome, { recursive: true, force: true });
    throw error;
  } finally {
    await browser.close();
  }
  writeFileSync(runtimeFile, JSON.stringify({ origin, workspace, workspaceId: initialized.workspace_id, testHome, globalStore, projectName: basename(workspace) }));

  return async () => {
    consoleProcess.kill("SIGINT");
    spawnSync(binary("draft"), ["service", "stop"], { cwd: testHome, env, timeout: 30_000 });
    rmSync(authFile, { force: true });
    rmSync(runtimeFile, { force: true });
    rmSync(testHome, { recursive: true, force: true });
  };
}
