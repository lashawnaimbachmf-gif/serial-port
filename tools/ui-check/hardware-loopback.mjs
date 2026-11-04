import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const artifacts = path.join(root, "artifacts");
const logDir = path.join(process.env.USERPROFILE, "Documents", "Serial Desk", "Logs");
fs.mkdirSync(artifacts, { recursive: true });

const textMessage = "DSH-闭环-20261019-B8E4";
const textBytes = new TextEncoder().encode(`${textMessage}\r\n`);
const binaryBytes = Uint8Array.from([0x00, 0x01, 0x7f, 0x80, 0xff]);
const expected = Uint8Array.from([...textBytes, ...binaryBytes]);
const expectedHex = Array.from(expected, (byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(" ");
const expectedPreview = Array.from(expected.slice(0, 16), (byte) =>
  byte.toString(16).toUpperCase().padStart(2, "0"),
).join(" ");

const beforeLogs = new Set(fs.existsSync(logDir) ? fs.readdirSync(logDir) : []);

const browser = await puppeteer.connect({
  browserURL: "http://127.0.0.1:9222",
  defaultViewport: null,
});
const pages = await browser.pages();
const page = pages.find((candidate) => candidate.url().includes("tauri.localhost")) || pages[0];
page.setDefaultTimeout(8000);

const problems = [];
page.on("console", (message) => {
  if (message.type() === "error" || message.type() === "warning") {
    problems.push(`[console:${message.type()}] ${message.text()}`);
  }
});
page.on("pageerror", (error) => problems.push(`[pageerror] ${error.message}`));

function report(step, value) {
  console.log(`\n[${step}]`);
  console.log(typeof value === "string" ? value : JSON.stringify(value, null, 2));
}

await page.waitForSelector("#bootCover.is-hidden");
const initialSessionState = await page.$eval("#sessionState", (node) => node.textContent);
if (["正在连接", "已连接", "正在关闭", "连接中断"].includes(initialSessionState)) {
  throw new Error(`A serial session is active (${initialSessionState}); refusing to interfere.`);
}

// Make the logging path explicit; this test validates timed file persistence.
if (!(await page.$eval("#autoLogToggle", (node) => node.checked))) {
  await page.click("#autoLogToggle");
  await page.waitForFunction(
    () => document.querySelector("#autoLogToggle").checked === true,
  );
}
await page.click("#clearTerminal");
await page.click("#viewTextBtn");
if (!(await page.$eval("#timestampToggle", (node) => node.checked))) {
  await page.click("#timestampToggle");
}

await page.select("#portSelect", "COM10");
await page.evaluate(() => {
  const interval = document.querySelector("#logIntervalInput");
  interval.value = "5";
  interval.dispatchEvent(new Event("input", { bubbles: true }));
  interval.dispatchEvent(new Event("change", { bubbles: true }));
});
const config = await page.evaluate(() => ({
  port: document.querySelector("#portSelect").value,
  baud: document.querySelector("#baudInput").value,
  framing: `${document.querySelector("#dataBitsSelect").value}-${document.querySelector("#paritySelect").value}-${document.querySelector("#stopBitsSelect").value}`,
  interval: document.querySelector("#logIntervalInput").value,
  autoLog: document.querySelector("#autoLogToggle").checked,
  logDir: document.querySelector("#logDirLabel").title,
}));
report("configuration", config);

await page.click("#connectBtn");
await page.waitForFunction(
  () => document.querySelector("#streamState").textContent === "已连接",
  { timeout: 10000 },
);
await new Promise((resolve) => setTimeout(resolve, 800));
const opened = await page.evaluate(() => ({
  session: document.querySelector("#sessionState").textContent,
  port: document.querySelector("#sessionPort").textContent,
  subtitle: document.querySelector("#terminalSubtitle").textContent,
  logStatus: document.querySelector("#logStatusText").textContent,
  logSave: document.querySelector("#logSaveText").textContent,
  autoLogDisabled: document.querySelector("#autoLogToggle").disabled,
  saveEnabled: !document.querySelector("#saveLogNow").disabled,
}));
report("opened", opened);
await page.screenshot({ path: path.join(artifacts, "ui-tauri-connected.png") });

const initialSaveText = opened.logSave;

// Text send with a visible newline, exercising UTF-8 decoding and TX echo.
await page.click("#sendTextBtn");
await page.select("#lineEndingSelect", "crlf");
await page.evaluate((value) => {
  const input = document.querySelector("#sendInput");
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}, textMessage);
const previewBeforeTextSend = await page.evaluate(() => ({
  meta: document.querySelector("#sendMeta").textContent,
  preview: document.querySelector("#sendPreview").textContent,
}));
await page.click("#sendBtn");
await page.waitForFunction(
  (message) => {
    const received = Array.from(document.querySelectorAll(".seg.rx .seg-text"))
      .map((node) => node.textContent)
      .join("");
    return received.includes(message);
  },
  { timeout: 5000 },
  textMessage,
);
await page.screenshot({ path: path.join(artifacts, "ui-tauri-connected.png") });
await page.screenshot({ path: path.join(artifacts, "ui-tauri-connected-text.png") });

// Binary bytes, exercising HEX validation and raw payload fidelity.
await page.click("#sendHexBtn");
await page.evaluate((hex) => {
  const input = document.querySelector("#sendInput");
  input.value = hex;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}, Array.from(binaryBytes, (byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(" "));
const binaryMeta = await page.$eval("#sendMeta", (node) => node.textContent);
await page.click("#sendBtn");

await page.waitForFunction(
  (expectedCount) => {
    const title = document.querySelector("#rxCounter").title;
    const count = Number((title.match(/^([\d,]+)/) || [])[1]?.replace(/,/g, "") || 0);
    return count >= expectedCount;
  },
  { timeout: 6000 },
  expected.length,
);
await new Promise((resolve) => setTimeout(resolve, 900));

// Inspect the exact received bytes through the HEX view.
await page.click("#viewHexBtn");
await new Promise((resolve) => setTimeout(resolve, 300));
const hexData = await page.evaluate(() => ({
  rxHex: Array.from(document.querySelectorAll(".hex-line.rx .hex-bytes"))
    .map((node) => node.textContent)
    .join(" "),
  txHex: Array.from(document.querySelectorAll(".hex-line.tx .hex-bytes"))
    .map((node) => node.textContent)
    .join(" "),
  counts: {
    rx: document.querySelector("#rxCounter").title,
    tx: document.querySelector("#txCounter").title,
  },
  logStatus: document.querySelector("#logStatusText").textContent,
  logSave: document.querySelector("#logSaveText").textContent,
  logSize: document.querySelector("#logSizeText").textContent,
}));
const compact = (value) => value.replace(/\s+/g, " ").trim();
const actualHex = compact(hexData.rxHex);
report("send_preview", { text: previewBeforeTextSend, binaryMeta });
report("hex_view", { ...hexData, actualHex, expectedHex });

const timedSaveDeadline = Date.now() + 7000;
let latestSave = hexData.logSave;
while (Date.now() < timedSaveDeadline) {
  await new Promise((resolve) => setTimeout(resolve, 500));
  latestSave = await page.$eval("#logSaveText", (node) => node.textContent);
  if (latestSave !== initialSaveText) {
    break;
  }
}
const timedSave = await page.evaluate(() => ({
  save: document.querySelector("#logSaveText").textContent,
  size: document.querySelector("#logSizeText").textContent,
  status: document.querySelector("#logStatusText").textContent,
}));
report("timed_save", { initial: initialSaveText, after: timedSave });

await page.click("#connectBtn");
await page.waitForFunction(
  () => document.querySelector("#sessionState").textContent === "已断开",
  { timeout: 10000 },
);
await new Promise((resolve) => setTimeout(resolve, 700));
const closed = await page.evaluate(() => ({
  session: document.querySelector("#sessionState").textContent,
  logStatus: document.querySelector("#logStatusText").textContent,
  logSave: document.querySelector("#logSaveText").textContent,
  logSize: document.querySelector("#logSizeText").textContent,
  logError: document.querySelector("#logError").hidden ? null : document.querySelector("#logError").textContent,
  terminalTail: document.querySelector("#terminalFlow").textContent.slice(-800),
  connectLabel: document.querySelector("#connectBtnLabel").textContent,
}));
report("closed", closed);
await page.screenshot({ path: path.join(artifacts, "ui-tauri-loopback-closed.png") });

const afterLogs = fs.existsSync(logDir)
  ? fs.readdirSync(logDir).filter((name) => !beforeLogs.has(name) && name.endsWith(".log"))
  : [];
const newLogs = afterLogs
  .map((name) => {
    const fullPath = path.join(logDir, name);
    const stat = fs.statSync(fullPath);
    return { name, fullPath, mtime: stat.mtimeMs, size: stat.size };
  })
  .sort((a, b) => b.mtime - a.mtime);
const logPath = newLogs[0]?.fullPath || null;
const logText = logPath ? fs.readFileSync(logPath, "utf8") : "";

const checks = {
  configuredCom10: config.port === "COM10",
  autoLogEnabled: config.autoLog === true,
  logStarted: opened.logStatus === "正在记录",
  logControlsLocked: opened.autoLogDisabled === true && opened.saveEnabled === true,
  textPreviewCount: previewBeforeTextSend.meta.startsWith(`${textBytes.length} 字节`),
  textPreviewExact: previewBeforeTextSend.preview.startsWith(expectedPreview),
  binaryPreviewCount: binaryMeta.startsWith(`${binaryBytes.length} 字节`),
  rxHexExact: actualHex === expectedHex,
  rxCountExact: hexData.counts.rx.startsWith(`${expected.length} 字节`),
  txCountExact: hexData.counts.tx.startsWith(`${expected.length} 字节`),
  timedSaveAdvanced: timedSave.save !== initialSaveText,
  logFileCreated: Boolean(logPath),
  logContainsTxText: new RegExp(`TX\\s+${textBytes.length} B \\| ${textMessage}\\\\r\\\\n`).test(logText),
  logContainsRxText: new RegExp(`RX\\s+${textBytes.length} B \\| ${textMessage}\\\\r\\\\n`).test(logText),
  logContainsChinese: logText.includes("闭环"),
  logContainsBinaryEscapes: logText.includes("\\x00\\x01\\x7F\\x80\\xFF"),
  logHasFooter: logText.includes("# closed:"),
  noLogError: closed.logError === null,
};
report("log_file", {
  path: logPath,
  size: logPath ? fs.statSync(logPath).size : 0,
  excerpt: logText.slice(0, 3000),
});
report("checks", checks);
console.log("\nPROBLEMS", JSON.stringify(problems, null, 2));

const allPassed = Object.values(checks).every(Boolean) && problems.length === 0;
fs.writeFileSync(
  path.join(artifacts, "hardware-report.json"),
  JSON.stringify({ config, opened, hexData, expectedHex, actualHex, timedSave, closed, logPath, logText, checks, problems }, null, 2),
);
browser.disconnect();
if (!allPassed) {
  process.exitCode = 1;
}
