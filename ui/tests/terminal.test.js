import assert from "node:assert/strict";
import test from "node:test";

import { shouldStartNewTextSegment } from "../js/terminal.js";

test("a long pause starts a new timestamped segment even mid-line", () => {
  assert.equal(
    shouldStartNewTextSegment({
      atLineStart: false,
      gap: 60_000,
      closed: false,
      directionChanged: false,
    }),
    true,
  );
});

test("ordinary fragments on the same line keep their timestamp", () => {
  assert.equal(
    shouldStartNewTextSegment({
      atLineStart: false,
      gap: 120,
      closed: false,
      directionChanged: false,
    }),
    false,
  );
});

test("a line start uses the normal merge threshold", () => {
  assert.equal(
    shouldStartNewTextSegment({ atLineStart: true, gap: 300 }),
    true,
  );
  assert.equal(
    shouldStartNewTextSegment({ atLineStart: true, gap: 100 }),
    false,
  );
});

test("a direction switch always starts a new segment", () => {
  assert.equal(
    shouldStartNewTextSegment({
      atLineStart: false,
      gap: 10,
      directionChanged: true,
    }),
    true,
  );
});

test("GBK bytes decode to the expected Chinese text", () => {
  const bytes = Uint8Array.from([
    0xb1, 0xe0, 0xc2, 0xeb, 0xa3, 0xba, 0x32, 0x32, 0x30, 0x34, 0x33, 0x36,
    0x39,
  ]);
  assert.equal(new TextDecoder("gbk", { fatal: false }).decode(bytes), "编码：2204369");
});
