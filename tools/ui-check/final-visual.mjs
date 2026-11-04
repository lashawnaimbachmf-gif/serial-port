import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const artifacts = path.join(root, "artifacts");
fs.mkdirSync(artifacts, { recursive: true });

const browser = await puppeteer.connect({
  browserURL: "http://127.0.0.1:9222",
  defaultViewport: null,
});
const pages = await browser.pages();
const page = pages.find((candidate) => candidate.url().includes("tauri.localhost")) || pages[0];
if (!page) throw new Error("Tauri page not found");
page.setDefaultTimeout(8000);

const problems = [];
page.on("console", (message) => {
  if (message.type() === "error" || message.type() === "warning") {
    problems.push(`[console:${message.type()}] ${message.text()}`);
  }
});
page.on("pageerror", (error) => problems.push(`[pageerror] ${error.message}`));

await page.waitForSelector("#bootCover.is-hidden");
const state = await page.$eval("#sessionState", (node) => node.textContent);
if (["正在连接", "已连接", "正在关闭", "连接中断"].includes(state)) {
  throw new Error(`Session active (${state}); refusing to interfere.`);
}

if (!(await page.$eval("#autoLogToggle", (node) => node.checked))) {
  await page.click("#autoLogToggle");
}
if (!(await page.$eval("#timestampToggle", (node) => node.checked))) {
  await page.click("#timestampToggle");
}
await page.select("#portSelect", "COM10");
await page.click("#sendTextBtn");
await page.click("#viewTextBtn");
await page.select("#lineEndingSelect", "crlf");
await page.evaluate(() => {
  const interval = document.querySelector("#logIntervalInput");
  interval.value = "30";
  interval.dispatchEvent(new Event("input", { bubbles: true }));
  interval.dispatchEvent(new Event("change", { bubbles: true }));
});
await page.click("#clearTerminal");

await page.click("#connectBtn");
await page.waitForFunction(() => document.querySelector("#streamState").textContent === "已连接", {
  timeout: 10000,
});
await page.waitForFunction(() => document.querySelector("#logStatusText").textContent === "正在记录", {
  timeout: 5000,
});

const messages = ["AT+VER?", "Serial Desk", "LOOPBACK · 字节一致"];
for (const message of messages) {
  await page.evaluate((value) => {
    const input = document.querySelector("#sendInput");
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }, message);
  await page.click("#sendBtn");
  await page.waitForFunction(
    (value) => {
      const received = Array.from(document.querySelectorAll(".seg.rx .seg-text"))
        .map((node) => node.textContent)
        .join("");
      return received.includes(value);
    },
    { timeout: 4000 },
    message,
  );
  await new Promise((resolve) => setTimeout(resolve, 180));
}

// Leave a representative draft in the composer for the screenshot.
await page.evaluate(() => {
  const input = document.querySelector("#sendInput");
  input.value = "AT+VER?";
  input.dispatchEvent(new Event("input", { bubbles: true }));
});
await new Promise((resolve) => setTimeout(resolve, 1000));

const finalState = await page.evaluate(() => ({
  session: document.querySelector("#sessionState").textContent,
  stream: document.querySelector("#streamState").textContent,
  logStatus: document.querySelector("#logStatusText").textContent,
  counters: {
    rx: document.querySelector("#rxCounter").title,
    tx: document.querySelector("#txCounter").title,
  },
  toastCount: document.querySelector("#statusMessage").textContent ? 1 : 0,
  notice: document.querySelector("#notice").hidden ? null : document.querySelector("#noticeText").textContent,
}));
console.log("FINAL_STATE", JSON.stringify(finalState, null, 2));
console.log("PROBLEMS", JSON.stringify(problems));

await page.screenshot({ path: path.join(artifacts, "ui-tauri-connected.png") });
await page.screenshot({ path: path.join(artifacts, "ui-final-connected.png") });

await page.click("#viewHexBtn");
await new Promise((resolve) => setTimeout(resolve, 250));
await page.screenshot({ path: path.join(artifacts, "ui-final-hex.png") });
await page.click("#viewTextBtn");

await page.click("#connectBtn");
await page.waitForFunction(() => document.querySelector("#sessionState").textContent === "已断开", {
  timeout: 10000,
});
await new Promise((resolve) => setTimeout(resolve, 700));
await page.screenshot({ path: path.join(artifacts, "ui-final-closed.png") });

const closed = await page.evaluate(() => ({
  session: document.querySelector("#sessionState").textContent,
  logStatus: document.querySelector("#logStatusText").textContent,
  logSave: document.querySelector("#logSaveText").textContent,
  logSize: document.querySelector("#logSizeText").textContent,
  logError: document.querySelector("#logError").hidden ? null : document.querySelector("#logError").textContent,
  statusMessage: document.querySelector("#statusMessage").textContent,
}));
console.log("CLOSED", JSON.stringify(closed, null, 2));
fs.writeFileSync(
  path.join(artifacts, "final-visual-report.json"),
  JSON.stringify({ finalState, closed, problems }, null, 2),
);
browser.disconnect();
if (problems.length > 0) {
  process.exitCode = 1;
}
