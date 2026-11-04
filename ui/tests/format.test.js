import assert from "node:assert/strict";
import test from "node:test";

import {
  SEND_BYTE_LIMIT,
  basename,
  buildSendPayload,
  displaySafeParts,
  formatBytes,
  formatDuration,
  hasUnpairedSurrogate,
  lineEndingBytes,
  parseHexInput,
  toHexString,
} from "../js/format.js";

test("strict HEX parser accepts whitespace and rejects everything else", () => {
  const parsed = parseHexInput("  41 54\n0d\t0A  ");
  assert.equal(parsed.error, null);
  assert.deepEqual(Array.from(parsed.bytes), [0x41, 0x54, 0x0d, 0x0a]);

  assert.match(parseHexInput("41,54").error, /只能包含/);
  assert.match(parseHexInput("41 5").error, /偶数位/);
  assert.match(parseHexInput("GG").error, /只能包含/);
  assert.match(parseHexInput("0x41").error, /只能包含/);

  const empty = parseHexInput("  \n ");
  assert.equal(empty.error, null);
  assert.equal(empty.bytes.length, 0);
});

test("text payloads preserve UTF-8 and append exactly one selected ending", () => {
  const base = buildSendPayload("text", "串口", "none");
  assert.equal(base.error, null);
  assert.deepEqual(Array.from(base.bytes), Array.from(new TextEncoder().encode("串口")));

  const crlf = buildSendPayload("text", "AT", "crlf");
  assert.deepEqual(Array.from(crlf.bytes), [0x41, 0x54, 0x0d, 0x0a]);

  const onlyEnding = buildSendPayload("text", "", "cr");
  assert.deepEqual(Array.from(onlyEnding.bytes), [0x0d]);
  assert.equal(onlyEnding.empty, false);

  const nothing = buildSendPayload("text", "", "none");
  assert.equal(nothing.error, null);
  assert.equal(nothing.empty, true);
});

test("HEX sends exactly the entered bytes and ignores the line ending", () => {
  const payload = buildSendPayload("hex", "41 54 0D 0A", "crlf");
  assert.equal(payload.error, null);
  assert.deepEqual(Array.from(payload.bytes), [0x41, 0x54, 0x0d, 0x0a]);
  assert.equal(payload.bytes.length, 4);
});

test("unpaired UTF-16 surrogates are rejected instead of being replaced", () => {
  assert.equal(hasUnpairedSurrogate("正常文本"), false);
  assert.equal(hasUnpairedSurrogate("emoji 😀"), false);
  assert.equal(hasUnpairedSurrogate("bad \ud800 text"), true);
  assert.equal(hasUnpairedSurrogate("\udc00"), true);

  const payload = buildSendPayload("text", "bad \ud800", "none");
  assert.match(payload.error, /无法编码/);
});

test("send payload enforces the byte limit exactly", () => {
  const exact = buildSendPayload("hex", "00 ".repeat(SEND_BYTE_LIMIT).trim(), "none");
  assert.equal(exact.error, null);
  assert.equal(exact.bytes.length, SEND_BYTE_LIMIT);

  const over = buildSendPayload("hex", `${"00 ".repeat(SEND_BYTE_LIMIT)}00`, "none");
  assert.match(over.error, /不能超过/);
});

test("line ending lookup returns fresh arrays", () => {
  const first = lineEndingBytes("crlf");
  first[0] = 0xff;
  assert.deepEqual(Array.from(lineEndingBytes("crlf")), [0x0d, 0x0a]);
  assert.deepEqual(Array.from(lineEndingBytes("invalid")), []);
});

test("HEX preview formatting is uppercase and bounded", () => {
  assert.equal(toHexString(Uint8Array.from([0x00, 0xaf, 0x2b])), "00 AF 2B");
  assert.equal(toHexString(Uint8Array.from([1, 2, 3, 4]), 2), "01 02 …");
});

test("display sanitizer preserves actual text and reveals unsafe characters", () => {
  const normal = displaySafeParts("AT+OK\r\n中文");
  assert.deepEqual(normal, [{ text: "AT+OK\r\n中文", unsafe: false }]);

  const unsafe = displaySafeParts("A\u0000B\u001bC\u202eD");
  assert.equal(unsafe[0].text, "A");
  assert.equal(unsafe[1].text, "\\x00");
  assert.equal(unsafe[1].unsafe, true);
  assert.equal(unsafe[3].text, "\\x1B");
  assert.equal(unsafe[5].text, "\\u{202E}");
});

test("byte and duration formatting remain readable", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(1024), "1.0 KB");
  assert.equal(formatBytes(1024 * 1024), "1.0 MB");
  assert.equal(formatDuration(0), "00:00");
  assert.equal(formatDuration(65_000), "01:05");
  assert.equal(formatDuration(3_665_000), "01:01:05");
});

test("basename handles Windows and POSIX separators", () => {
  assert.equal(basename("C:\\Logs\\COM8.log"), "COM8.log");
  assert.equal(basename("/tmp/serial.log"), "serial.log");
  assert.equal(basename("C:\\Logs\\"), "Logs");
});
