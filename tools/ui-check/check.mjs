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

async function inspect(page, label) {
  const layout = await page.evaluate(() => {
    const bad = [];
    for (const element of document.querySelectorAll("body *")) {
      const style = getComputedStyle(element);
      if (style.display === "none" || style.visibility === "hidden") continue;
      if (element.scrollWidth > element.clientWidth + 2 && element.clientWidth > 0) {
        const overflowX = style.overflowX;
        if (overflowX === "visible") {
          bad.push({
            tag: element.tagName,
            id: element.id,
            cls: element.className?.baseVal ?? element.className,
            scrollWidth: element.scrollWidth,
            clientWidth: element.clientWidth,
          });
        }
      }
    }
    return {
      bad,
      body: document.body.getBoundingClientRect().toJSON(),
      bootHidden: document.querySelector("#bootCover")?.classList.contains("is-hidden"),
      notice: document.querySelector("#notice")?.hidden === false
        ? document.querySelector("#noticeText")?.textContent
        : null,
      stream: document.querySelector("#streamState")?.textContent,
      title: document.title,
    };
  });
  console.log(`\n[${label}]`, JSON.stringify(layout, null, 2));
  await page.screenshot({ path: path.join(artifacts, `${label}.png`) });
}

const page = await browser.newPage();
await page.setViewport({ width: 1240, height: 820, deviceScaleFactor: 1 });
const problems = [];
page.on("console", (message) => {
  if (message.type() === "error" || message.type() === "warning") {
    problems.push(`[console:${message.type()}] ${message.text()}`);
  }
});
page.on("pageerror", (error) => problems.push(`[pageerror] ${error.message}`));

await page.goto("http://127.0.0.1:7788/?preview=1", { waitUntil: "networkidle0" });
await page.waitForSelector("#bootCover.is-hidden", { timeout: 5000 });
await new Promise((r) => setTimeout(r, 700));
await inspect(page, "ui-offline");

await page.goto("http://127.0.0.1:7788/?preview=1&autoconnect=1", {
  waitUntil: "networkidle0",
});
await page.waitForSelector("#bootCover.is-hidden", { timeout: 5000 });
await page.waitForFunction(
  () => document.querySelectorAll("#terminalFlow .seg, #terminalFlow .sys-line").length >= 4,
  { timeout: 7000 },
);
await new Promise((r) => setTimeout(r, 1500));
await inspect(page, "ui-connected");

console.log("\nPROBLEMS", JSON.stringify(problems, null, 2));
await browser.close();
