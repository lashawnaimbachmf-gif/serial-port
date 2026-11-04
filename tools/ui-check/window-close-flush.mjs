import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const artifacts = path.join(root, "artifacts");
const logDir = path.join(process.env.USERPROFILE, "Documents", "Serial Desk", "Logs");
const marker = "DSH-WINDOW-CLOSE-20261019";
fs.mkdirSync(artifacts, { recursive: true });
const before = new Set(fs.existsSync(logDir) ? fs.readdirSync(logDir) : []);

const browser = await puppeteer.connect({
  browserURL: "http://127.0.0.1:9222",
  defaultViewport: null,
});
const pages = await browser.pages();
const page = pages.find((candidate) => candidate.url().includes("tauri.localhost")) || pages[0];
if (!page) {
  throw new Error("Tauri page not found");
}
page.setDefaultTimeout(8000);
await page.waitForSelector("#bootCover.is-hidden");

const initial = await page.$eval("#sessionState", (node) => node.textContent);
if (["正在连接", "已连接", "正在关闭", "连接中断"].includes(initial)) {
  throw new Error(`Session active (${initial}); refusing to interfere.`);
}

if (!(await page.$eval("#autoLogToggle", (node) => node.checked))) {
  await page.click("#autoLogToggle");
}
await page.select("#portSelect", "COM10");
await page.click("#sendTextBtn");
await page.click("#viewTextBtn");
await page.select("#lineEndingSelect", "crlf");
await page.evaluate(() => {
  const interval = document.querySelector("#logIntervalInput");
  interval.value = "3600";
  interval.dispatchEvent(new Event("input", { bubbles: true }));
  interval.dispatchEvent(new Event("change", { bubbles: true }));
});
await page.click("#connectBtn");
await page.waitForFunction(() => document.querySelector("#streamState").textContent === "已连接", {
  timeout: 10000,
});
await page.waitForFunction(() => document.querySelector("#logStatusText").textContent === "正在记录", {
  timeout: 5000,
});

await page.evaluate((value) => {
  const input = document.querySelector("#sendInput");
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}, marker);
await page.click("#sendBtn");
await page.waitForFunction(
  (value) =>
    Array.from(document.querySelectorAll(".seg.rx .seg-text"))
      .map((node) => node.textContent)
      .join("")
      .includes(value),
  { timeout: 5000 },
  marker,
);
const beforeClose = await page.evaluate(() => ({
  session: document.querySelector("#sessionState").textContent,
  logStatus: document.querySelector("#logStatusText").textContent,
  saveText: document.querySelector("#logSaveText").textContent,
  counters: {
    rx: document.querySelector("#rxCounter").title,
    tx: document.querySelector("#txCounter").title,
  },
}));

// Close the native window from Windows itself (the custom titlebar was
// removed in favor of native window chrome), well before the 3600 s timed
// flush. Only the Rust window-close shutdown path can add the footer.
let pageClosed = false;
try {
  const pythonClose = [
    "import win32con, win32gui",
    "targets = []",
    "def visit(hwnd, _):",
    "    if win32gui.IsWindowVisible(hwnd) and win32gui.GetWindowText(hwnd) == 'Serial Desk':",
    "        targets.append(hwnd)",
    "win32gui.EnumWindows(visit, None)",
    "if not targets:",
    "    raise SystemExit(2)",
    "win32gui.PostMessage(targets[0], win32con.WM_CLOSE, 0, 0)",
  ].join("\n");
  execFileSync("python.exe", ["-c", pythonClose], { encoding: "utf8" });
  pageClosed = true;
  await new Promise((resolve) => setTimeout(resolve, 1500));
} catch (error) {
  console.error("Failed to close native window:", error.message);
}
await new Promise((resolve) => setTimeout(resolve, 300));
let processAlive = true;
try {
  const output = execFileSync(
    "powershell.exe",
    ["-NoProfile", "-Command", "Get-Process SerialDesk -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id"],
    { encoding: "utf8" },
  ).trim();
  processAlive = output.length > 0;
} catch {
  processAlive = false;
}
try {
  browser.disconnect();
} catch {
  // The webview is expected to be gone.
}

const newLogs = fs.existsSync(logDir)
  ? fs
      .readdirSync(logDir)
      .filter((name) => !before.has(name) && name.endsWith(".log"))
      .map((name) => {
        const full = path.join(logDir, name);
        return { name, full, mtime: fs.statSync(full).mtimeMs };
      })
      .sort((a, b) => b.mtime - a.mtime)
  : [];
const logPath = newLogs[0]?.full || null;
const logText = logPath ? fs.readFileSync(logPath, "utf8") : "";

const markerPattern = marker.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const checks = {
  pageClosing: pageClosed,
  processExited: !processAlive,
  logCreated: Boolean(logPath),
  logHasTxMarker: new RegExp(`TX\\s+\\d+ B \\| ${markerPattern}`).test(logText),
  logHasRxMarker: new RegExp(`RX\\s+\\d+ B \\| ${markerPattern}`).test(logText),
  logHasCloseFooter: /# closed: .* RX \d+ B \/ \d+ frames \| TX \d+ B \/ \d+ frames/.test(logText),
};
console.log(JSON.stringify({ beforeClose, logPath, logText, checks }, null, 2));
console.log("PROBLEMS", JSON.stringify(checks));
fs.writeFileSync(
  path.join(artifacts, "window-close-report.json"),
  JSON.stringify({ beforeClose, logPath, logText, checks }, null, 2),
);
if (!Object.values(checks).every(Boolean)) {
  process.exitCode = 1;
}
