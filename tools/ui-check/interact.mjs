import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const artifacts = path.join(root, "artifacts");
fs.mkdirSync(artifacts, { recursive: true });

const browser = await puppeteer.launch({
  executablePath: "D:\\APP\\Chrome\\Application\\chrome.exe",
  headless: "new",
  args: [
    "--no-first-run",
    "--no-default-browser-check",
    "--force-device-scale-factor=1",
    "--disable-features=Translate",
    "--hide-scrollbars",
  ],
});

const problems = [];
const checks = [];
function check(name, condition, detail = "") {
  checks.push({ name, condition, detail });
  console.log(`${condition ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
}

async function newPage(width, height, url) {
  const page = await browser.newPage();
  await page.setViewport({ width, height, deviceScaleFactor: 1 });
  page.on("console", (message) => {
    if (message.type() === "error" || message.type() === "warning") {
      problems.push(`[console:${message.type()}] ${message.text()}`);
    }
  });
  page.on("pageerror", (error) => problems.push(`[pageerror] ${error.message}`));
  await page.goto(url, { waitUntil: "networkidle0" });
  await page.waitForSelector("#bootCover.is-hidden", { timeout: 5000 });
  return page;
}

const page = await newPage(1240, 820, "http://127.0.0.1:7788/?preview=1&autoconnect=1");
await page.waitForFunction(() => document.querySelector("#streamState")?.textContent === "已连接", {
  timeout: 6000,
});
await page.waitForFunction(
  () => document.querySelectorAll("#terminalFlow .seg").length >= 3,
  { timeout: 5000 },
);

const initial = await page.evaluate(() => ({
  connected: document.querySelector("#sessionState").textContent,
  logText: document.querySelector("#logStatusText").textContent,
  logFooter: document.querySelector("#logFooter").textContent,
  sendDisabled: document.querySelector("#sendBtn").disabled,
  logOpenDisabled: document.querySelector("#openLogFile").disabled,
  encoding: document.querySelector("#encodingSelect").value,
  encodingDisabled: document.querySelector("#encodingSelect").disabled,
  terminalText: document.querySelector("#terminalFlow").textContent,
  docOverflow: document.documentElement.scrollWidth > window.innerWidth,
}));
check("connects in preview", initial.connected === "已连接", initial.connected);
check("log reports recording", initial.logText === "正在记录", initial.logText);
check("terminal receives mock frames", initial.terminalText.includes("Serial Desk Fixture"), initial.terminalText.slice(0, 80));
check("no horizontal page overflow", !initial.docOverflow);
check("send disabled while empty", initial.sendDisabled === true);
check("log open disabled while connected", initial.logOpenDisabled === true);
check("UTF-8 display encoding is the default", initial.encoding === "utf-8" && !initial.encodingDisabled, JSON.stringify(initial));

await page.type("#sendInput", "AT+TEST");
const typed = await page.evaluate(() => ({
  disabled: document.querySelector("#sendBtn").disabled,
  meta: document.querySelector("#sendMeta").textContent,
  preview: document.querySelector("#sendPreview").textContent,
}));
check("send enabled after typing", typed.disabled === false, JSON.stringify(typed));
check("send byte count is exact", typed.meta.startsWith("7 字节"), typed.meta);
check("send hex preview is correct", typed.preview === "41 54 2B 54 45 53 54", typed.preview);

await page.click("#sendBtn");
await page.waitForFunction(
  () => document.querySelector("#terminalFlow").textContent.includes("AT+TEST"),
  { timeout: 3000 },
);
await page.waitForFunction(
  () => document.querySelector("#terminalFlow").textContent.includes("OK"),
  { timeout: 3000 },
);
const txCount = await page.evaluate(() => document.querySelector("#txCounter").textContent);
check("TX counter updates", txCount !== "0 B", txCount);

await page.click("#viewHexBtn");
await new Promise((r) => setTimeout(r, 300));
const hex = await page.evaluate(() => ({
  lines: document.querySelectorAll("#terminalFlow .hex-line").length,
  text: document.querySelector("#terminalFlow").textContent,
  encodingDisabled: document.querySelector("#encodingSelect").disabled,
}));
check("HEX view renders lines", hex.lines > 5, String(hex.lines));
check("HEX view contains original bytes", /41 54 2B 54 45 53 54/.test(hex.text));
check("display encoding is locked in HEX view", hex.encodingDisabled === true);
await page.screenshot({ path: path.join(artifacts, "ui-hex.png") });
await page.click("#viewTextBtn");
await new Promise((r) => setTimeout(r, 120));
const textEncoding = await page.evaluate(() => ({
  disabled: document.querySelector("#encodingSelect").disabled,
  value: document.querySelector("#encodingSelect").value,
}));
check("display encoding re-enables in text view", !textEncoding.disabled && textEncoding.value === "utf-8", JSON.stringify(textEncoding));
await page.select("#encodingSelect", "gbk");
const gbkSelected = await page.$eval("#encodingSelect", (node) => node.value);
check("GBK display encoding can be selected", gbkSelected === "gbk", gbkSelected);
await page.select("#encodingSelect", "utf-8");

await page.click("#timestampToggle");
await new Promise((r) => setTimeout(r, 120));
const timeHidden = await page.evaluate(() => {
  const time = document.querySelector("#terminalFlow .seg-time");
  return time ? getComputedStyle(time).display : "missing";
});
check("timestamp toggle hides time", timeHidden === "none", timeHidden);
await page.click("#timestampToggle");

const cleared = await page.evaluate(() => {
  document.querySelector("#clearTerminal").click();
  return {
    frameCount: document.querySelectorAll("#terminalFlow .seg, #terminalFlow .sys-line").length,
    emptyHidden: document.querySelector("#terminalEmpty").hidden,
  };
});
check("clear empties terminal", cleared.frameCount === 0, String(cleared.frameCount));
check("clear restores empty state", cleared.emptyHidden === false);
check("log remains active after clear", await page.evaluate(() => document.querySelector("#logStatusText").textContent === "正在记录"));

await page.click("#saveLogNow");
await page.waitForFunction(
  () => document.querySelector("#statusMessage").textContent.includes("日志已写入磁盘"),
  { timeout: 3000 },
);
check("manual save works", true);

await page.click("#sendHexBtn");
await page.evaluate(() => {
  const input = document.querySelector("#sendInput");
  input.value = "4";
  input.dispatchEvent(new Event("input", { bubbles: true }));
});
await new Promise((r) => setTimeout(r, 100));
const invalidHex = await page.evaluate(() => ({
  error: document.querySelector("#sendError").textContent,
  disabled: document.querySelector("#sendBtn").disabled,
  lineEndingDisabled: document.querySelector("#lineEndingSelect").disabled,
}));
check("odd HEX is rejected clearly", invalidHex.error.includes("偶数位"), invalidHex.error);
check("send disabled for invalid HEX", invalidHex.disabled === true);
check("line ending disabled in HEX mode", invalidHex.lineEndingDisabled === true);

await page.evaluate(() => {
  const input = document.querySelector("#sendInput");
  input.value = "41 54 0D 0A";
  input.dispatchEvent(new Event("input", { bubbles: true }));
});
const validHex = await page.evaluate(() => ({
  meta: document.querySelector("#sendMeta").textContent,
  disabled: document.querySelector("#sendBtn").disabled,
}));
check("valid HEX reports 4 bytes", validHex.meta.startsWith("4 字节"), validHex.meta);
check("valid HEX enables send", validHex.disabled === false);

await page.screenshot({ path: path.join(artifacts, "ui-connected-final.png") });

const compact = await newPage(980, 660, "http://127.0.0.1:7788/?preview=1");
await compact.click("#baudMenuButton");
await compact.waitForSelector("#baudMenu:not([hidden])", { timeout: 2000 });
const baudMenu = await compact.evaluate(() => ({
  count: document.querySelectorAll("#baudMenu .combo-option").length,
  values: Array.from(document.querySelectorAll("#baudMenu .combo-option")).map((node) => node.dataset.value),
  current: document.querySelector("#baudInput").value,
  expanded: document.querySelector("#baudInput").getAttribute("aria-expanded"),
}));
check("baud menu is populated", baudMenu.count >= 15, String(baudMenu.count));
check("baud menu is not filtered by current value", baudMenu.count > 1 && baudMenu.values.includes("9600"), JSON.stringify(baudMenu.values));
check("baud menu reports expanded", baudMenu.expanded === "true", baudMenu.expanded);
await compact.screenshot({ path: path.join(artifacts, "ui-baud-menu.png") });
await compact.click('#baudMenu .combo-option[data-value="9600"]');
const baudChoice = await compact.evaluate(() => ({
  value: document.querySelector("#baudInput").value,
  hidden: document.querySelector("#baudMenu").hidden,
}));
check("baud menu selection updates value", baudChoice.value === "9600" && baudChoice.hidden === true, JSON.stringify(baudChoice));

// The log pane is a UI-only disclosure: hiding it must not affect logging.
if (await compact.evaluate(() => document.querySelector(".workspace").classList.contains("is-log-hidden"))) {
  await compact.click("#showLogPane");
}
const logBefore = await compact.evaluate(() => ({
  visible: !document.querySelector(".workspace").classList.contains("is-log-hidden"),
  display: getComputedStyle(document.querySelector("#logPanel")).display,
  heading: document.querySelector(".log-heading .panel-title").textContent.trim(),
  terminalWidth: document.querySelector(".terminal-pane").getBoundingClientRect().width,
  button: document.querySelector("#toggleLogPane").textContent.trim(),
  expanded: document.querySelector("#toggleLogPane").getAttribute("aria-expanded"),
  showHidden: document.querySelector("#showLogPane").hidden,
  toolbarFits: document.querySelector(".terminal-toolbar").scrollWidth <= document.querySelector(".terminal-toolbar").clientWidth + 1,
}));
check("log pane starts visible", logBefore.visible && logBefore.display !== "none", JSON.stringify(logBefore));
check("log pane is titled 保存日志", logBefore.heading === "保存日志", JSON.stringify(logBefore));
check("log header holds the collapse button", logBefore.button === "收起" && logBefore.showHidden, JSON.stringify(logBefore));
check("terminal toolbar fits without log toggle", logBefore.toolbarFits, JSON.stringify(logBefore));
await compact.click("#toggleLogPane");
await compact.waitForFunction(() => document.querySelector(".workspace").classList.contains("is-log-hidden"));
const logHidden = await compact.evaluate(() => ({
  visible: !document.querySelector(".workspace").classList.contains("is-log-hidden"),
  display: getComputedStyle(document.querySelector("#logPanel")).display,
  terminalWidth: document.querySelector(".terminal-pane").getBoundingClientRect().width,
  button: document.querySelector("#showLogPane").textContent.trim(),
  expanded: document.querySelector("#showLogPane").getAttribute("aria-expanded"),
  showHidden: document.querySelector("#showLogPane").hidden,
  persisted: localStorage.getItem("serialdesk.logPaneHidden"),
  pageOverflowX: document.documentElement.scrollWidth > window.innerWidth,
}));
check("log pane can be hidden", !logHidden.visible && logHidden.display === "none" && logHidden.terminalWidth > logBefore.terminalWidth + 100, JSON.stringify(logHidden));
check("hidden log leaves terminal full width", logHidden.terminalWidth > 900 && !logHidden.pageOverflowX, `${logHidden.terminalWidth}`);
check("log toggle exposes show action", logHidden.button === "展开日志" && logHidden.expanded === "false" && !logHidden.showHidden, JSON.stringify(logHidden));
check("log visibility preference persists", logHidden.persisted === "1", String(logHidden.persisted));
await compact.screenshot({ path: path.join(artifacts, "ui-log-hidden.png") });
await compact.click("#showLogPane");
await compact.waitForFunction(() => !document.querySelector(".workspace").classList.contains("is-log-hidden"));
const logRestored = await compact.evaluate(() => ({
  visible: !document.querySelector(".workspace").classList.contains("is-log-hidden"),
  display: getComputedStyle(document.querySelector("#logPanel")).display,
  button: document.querySelector("#toggleLogPane").textContent.trim(),
  showHidden: document.querySelector("#showLogPane").hidden,
  persisted: localStorage.getItem("serialdesk.logPaneHidden"),
}));
check("log pane can be restored", logRestored.visible && logRestored.display !== "none" && logRestored.button === "收起" && logRestored.showHidden && logRestored.persisted === "0", JSON.stringify(logRestored));

await compact.click("#openLogFile");
await compact.waitForSelector("#logViewBar:not([hidden])", { timeout: 3000 });
const replay = await compact.evaluate(() => ({
  stream: document.querySelector("#streamState").textContent,
  subtitle: document.querySelector("#terminalSubtitle").textContent,
  file: document.querySelector("#logViewName").textContent,
  meta: document.querySelector("#logViewMeta").textContent,
  frames: document.querySelectorAll("#terminalFlow .seg").length,
  connectDisabled: document.querySelector("#connectBtn").disabled,
  connectLabel: document.querySelector("#connectBtnLabel").textContent,
  clearDisabled: document.querySelector("#clearTerminal").disabled,
  openDisabled: document.querySelector("#openLogFile").disabled,
  rx: document.querySelector("#rxCounter").title,
  tx: document.querySelector("#txCounter").title,
  pageOverflowX: document.documentElement.scrollWidth > window.innerWidth,
}));
check("log replay enters read-only mode", replay.stream === "日志回放" && replay.connectDisabled && replay.clearDisabled, JSON.stringify(replay));
check("log replay shows file and metadata", replay.file.endsWith(".log") && replay.meta.includes("COM8") && replay.meta.includes("3 帧") && replay.meta.includes("已关闭"), replay.meta);
check("log replay loads text frames", replay.frames >= 3 && replay.subtitle.includes(".log"), String(replay.frames));
check("log replay restores historical counters", replay.rx.includes("28 字节") && replay.tx.includes("9 字节"), `${replay.rx} | ${replay.tx}`);
check("log replay keeps log opener disabled", replay.openDisabled === true, replay.connectLabel);
check("log replay does not overflow", !replay.pageOverflowX);
await compact.click("#viewHexBtn");
await new Promise((resolve) => setTimeout(resolve, 200));
const replayHex = await compact.evaluate(() => ({
  lines: document.querySelectorAll("#terminalFlow .hex-line").length,
  text: document.querySelector("#terminalFlow").textContent,
}));
check("log replay supports HEX view", replayHex.lines === 3 && replayHex.text.includes("00 01 7F 80 FF"), JSON.stringify(replayHex));
await compact.click("#viewTextBtn");
await compact.screenshot({ path: path.join(artifacts, "ui-log-replay.png") });
await compact.click("#exitLogView");
await compact.waitForSelector("#logViewBar[hidden]", { timeout: 2000 });
const replayExit = await compact.evaluate(() => ({
  stream: document.querySelector("#streamState").textContent,
  barHidden: document.querySelector("#logViewBar").hidden,
  connectDisabled: document.querySelector("#connectBtn").disabled,
  connectLabel: document.querySelector("#connectBtnLabel").textContent,
  clearDisabled: document.querySelector("#clearTerminal").disabled,
  openDisabled: document.querySelector("#openLogFile").disabled,
  emptyTitle: document.querySelector("#terminalEmptyTitle").textContent,
  frames: document.querySelectorAll("#terminalFlow .seg").length,
}));
check("log replay exits cleanly", replayExit.barHidden && !replayExit.connectDisabled && !replayExit.clearDisabled && !replayExit.openDisabled, JSON.stringify(replayExit));
check("log replay restores live empty state", replayExit.stream === "未连接" && replayExit.emptyTitle === "未连接" && replayExit.frames === 0, JSON.stringify(replayExit));

const compactLayout = await compact.evaluate(() => {
  const send = document.querySelector(".send-pane").getBoundingClientRect();
  const terminal = document.querySelector(".terminal-pane").getBoundingClientRect();
  const status = document.querySelector(".status-bar").getBoundingClientRect();
  const log = document.querySelector(".log-pane").getBoundingClientRect();
  return {
    sendBottom: send.bottom,
    terminalHeight: terminal.height,
    statusBottom: status.bottom,
    logWidth: log.width,
    viewportHeight: window.innerHeight,
    pageOverflowX: document.documentElement.scrollWidth > window.innerWidth,
  };
});
console.log(JSON.stringify(compactLayout, null, 2));
check("compact: send pane visible", compactLayout.sendBottom <= 660 + 0.5, String(compactLayout.sendBottom));
check("compact: status bar visible", compactLayout.statusBottom <= 660 + 0.5, String(compactLayout.statusBottom));
check("compact: terminal remains useful", compactLayout.terminalHeight > 220, String(compactLayout.terminalHeight));
check("compact: no horizontal overflow", !compactLayout.pageOverflowX);
await compact.screenshot({ path: path.join(artifacts, "ui-compact.png") });

const restorePage = await newPage(1160, 760, "http://127.0.0.1:7788/?preview=1&autoconnect=1");
await restorePage.waitForFunction(
  () => document.querySelector("#streamState")?.textContent === "已连接",
  { timeout: 6000 },
);
await restorePage.waitForFunction(
  () => document.querySelectorAll("#terminalFlow .seg").length >= 3,
  { timeout: 5000 },
);
await restorePage.click("#connectBtn");
await restorePage.waitForFunction(
  () => document.querySelector("#sessionState")?.textContent === "已断开",
  { timeout: 5000 },
);
await new Promise((resolve) => setTimeout(resolve, 150));
const liveSnapshot = await restorePage.evaluate(() => ({
  text: document.querySelector("#terminalFlow").textContent,
  segments: document.querySelectorAll("#terminalFlow .seg").length,
  rx: document.querySelector("#rxCounter").title,
  tx: document.querySelector("#txCounter").title,
  session: document.querySelector("#sessionState").textContent,
}));
await restorePage.click("#openLogFile");
await restorePage.waitForSelector("#logViewBar:not([hidden])", { timeout: 3000 });
await restorePage.click("#exitLogView");
await restorePage.waitForSelector("#logViewBar[hidden]", { timeout: 2000 });
await new Promise((resolve) => setTimeout(resolve, 150));
const restoredSnapshot = await restorePage.evaluate(() => ({
  text: document.querySelector("#terminalFlow").textContent,
  segments: document.querySelectorAll("#terminalFlow .seg").length,
  rx: document.querySelector("#rxCounter").title,
  tx: document.querySelector("#txCounter").title,
  session: document.querySelector("#sessionState").textContent,
}));
check("log replay restores prior live buffer", restoredSnapshot.text === liveSnapshot.text && restoredSnapshot.segments === liveSnapshot.segments, JSON.stringify({ before: liveSnapshot.segments, after: restoredSnapshot.segments }));
check("log replay restores prior counters", restoredSnapshot.rx === liveSnapshot.rx && restoredSnapshot.tx === liveSnapshot.tx, `${liveSnapshot.rx} | ${restoredSnapshot.rx}`);
check("log replay restores prior session state", restoredSnapshot.session === "已断开", restoredSnapshot.session);
await restorePage.close();

console.log("\nPROBLEMS", JSON.stringify(problems, null, 2));
const failures = checks.filter((item) => !item.condition);
console.log(`\nSUMMARY ${checks.length - failures.length}/${checks.length} passed`);
await browser.close();
if (failures.length > 0 || problems.some((p) => p.includes("pageerror"))) {
  process.exitCode = 1;
}
