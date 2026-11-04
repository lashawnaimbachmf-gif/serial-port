import { displaySafeParts, formatBytes, formatTime, toHexString } from "./format.js";

const MAX_BUFFER_BYTES = 8 * 1024 * 1024;
const TRIM_TARGET_BYTES = 6 * 1024 * 1024;
const MERGE_GAP_MS = 220;
const HARD_GAP_MS = 1000;

/**
 * Decides whether a frame starts a new timestamped visual segment.
 *
 * A frame may arrive without a trailing newline while the device is still
 * printing a line. We normally keep appending it to the same visual line, but
 * a gap of more than one second is long enough that sharing the old timestamp
 * would be misleading. Such a frame starts a new segment even mid-line.
 */
export function shouldStartNewTextSegment({
  directionChanged = false,
  atLineStart = false,
  gap = 0,
  closed = false,
} = {}) {
  if (directionChanged) {
    return true;
  }
  if (Number.isFinite(gap) && gap > HARD_GAP_MS) {
    return true;
  }
  return Boolean(atLineStart) && (gap > MERGE_GAP_MS || Boolean(closed));
}

export class TerminalView {
  constructor({
    scrollEl,
    flowEl,
    emptyEl,
    jumpBtn,
    noticeEl,
    noticeTextEl,
    cardEl,
    onFollowChange,
    onTrim,
  }) {
    this.scrollEl = scrollEl;
    this.flowEl = flowEl;
    this.emptyEl = emptyEl;
    this.jumpBtn = jumpBtn;
    this.noticeEl = noticeEl;
    this.noticeTextEl = noticeTextEl;
    this.cardEl = cardEl;
    this.onFollowChange = onFollowChange;
    this.onTrim = onTrim;

    this.mode = "text";
    this.cardEl?.setAttribute("data-view", this.mode);
    this.timestamps = true;
    this.follow = true;
    this.encoding = "utf-8";
    this.frames = [];
    this.pending = [];
    this.totalBytes = 0;
    this.rafId = null;
    this.scrollRafId = null;
    this.currentSegment = null;
    this.lastRenderedTs = 0;
    this.atLineStart = true;
    this.decoders = this.createDecoders();
    this.rebuilding = false;
    this.trimmed = false;

    this.scrollEl.addEventListener("scroll", () => this.handleScroll(), { passive: true });
    this.jumpBtn.addEventListener("click", () => {
      this.follow = true;
      this.updateFollowUi();
      this.scrollToBottom();
      this.onFollowChange?.(true, { fromUser: true });
    });

    this.applyTimestampAttribute();
    this.updateEmpty();
    this.updateJumpVisibility();
  }

  createDecoders() {
    const label = this.encoding === "gbk" ? "gbk" : "utf-8";
    try {
      return {
        rx: new TextDecoder(label, { fatal: false }),
        tx: new TextDecoder(label, { fatal: false }),
      };
    } catch {
      return {
        rx: new TextDecoder("utf-8", { fatal: false }),
        tx: new TextDecoder("utf-8", { fatal: false }),
      };
    }
  }

  setEncoding(encoding) {
    const next = encoding === "gbk" ? "gbk" : "utf-8";
    if (this.encoding === next) {
      return;
    }
    this.encoding = next;
    this.decoders = this.createDecoders();
    if (this.mode === "text") {
      this.rebuild();
    }
  }

  setMode(mode) {
    const next = mode === "hex" ? "hex" : "text";
    if (this.mode === next) {
      return;
    }
    this.mode = next;
    this.cardEl?.setAttribute("data-view", next);
    this.rebuild();
  }

  setTimestamps(enabled) {
    this.timestamps = Boolean(enabled);
    this.applyTimestampAttribute();
  }

  applyTimestampAttribute() {
    this.cardEl?.setAttribute("data-timestamps", this.timestamps ? "on" : "off");
  }

  setFollow(enabled, { scroll = true } = {}) {
    this.follow = Boolean(enabled);
    this.updateFollowUi();
    if (this.follow && scroll) {
      this.scrollToBottom();
    } else {
      this.updateJumpVisibility();
    }
  }

  updateFollowUi() {
    this.updateJumpVisibility();
  }

  isNearBottom(slop = 30) {
    const max = this.scrollEl.scrollHeight - this.scrollEl.clientHeight;
    return max <= 0 || max - this.scrollEl.scrollTop <= slop;
  }

  handleScroll() {
    if (this.rebuilding) {
      return;
    }
    const nearBottom = this.isNearBottom();
    if (!nearBottom && this.follow) {
      this.follow = false;
      this.updateFollowUi();
      this.onFollowChange?.(false, { fromUser: true });
    } else if (nearBottom && !this.follow) {
      this.follow = true;
      this.updateFollowUi();
      this.onFollowChange?.(true, { fromUser: true });
    } else {
      this.updateJumpVisibility();
    }
  }

  updateJumpVisibility() {
    if (!this.jumpBtn) {
      return;
    }
    this.jumpBtn.hidden = this.follow || this.isNearBottom(8);
  }

  scrollToBottom() {
    if (this.scrollRafId !== null) {
      cancelAnimationFrame(this.scrollRafId);
    }
    this.scrollRafId = requestAnimationFrame(() => {
      this.scrollRafId = null;
      this.scrollEl.scrollTop = this.scrollEl.scrollHeight;
      this.updateJumpVisibility();
    });
  }

  appendFrame(direction, timestampMs, bytes) {
    if (!bytes || bytes.length === 0) {
      return;
    }
    const copy = bytes instanceof Uint8Array ? bytes.slice() : new Uint8Array(bytes);
    const frame = {
      kind: "data",
      dir: direction === "tx" ? "tx" : "rx",
      ts: Number(timestampMs),
      bytes: copy,
    };
    this.frames.push(frame);
    this.totalBytes += copy.length;
    this.pending.push(frame);
    this.scheduleFlush();
  }

  appendSystem(text) {
    const frame = {
      kind: "system",
      text: String(text ?? ""),
    };
    this.frames.push(frame);
    this.pending.push(frame);
    this.scheduleFlush();
  }

  scheduleFlush() {
    if (this.rafId !== null) {
      return;
    }
    this.rafId = requestAnimationFrame(() => {
      this.rafId = null;
      this.flush();
    });
  }

  flush() {
    if (this.pending.length === 0) {
      return;
    }

    if (this.totalBytes > MAX_BUFFER_BYTES) {
      this.trimBuffer();
      return;
    }

    const shouldStick = this.follow;
    for (const frame of this.pending) {
      this.renderFrameIncremental(frame);
    }
    this.pending = [];
    this.updateEmpty();
    if (shouldStick) {
      this.scrollToBottom();
    } else {
      this.updateJumpVisibility();
    }
  }

  trimBuffer() {
    const previousBytes = this.totalBytes;
    while (this.frames.length > 1 && this.totalBytes > TRIM_TARGET_BYTES) {
      const frame = this.frames.shift();
      if (frame.kind === "data") {
        this.totalBytes -= frame.bytes.length;
      }
    }
    const droppedBytes = previousBytes - this.totalBytes;
    this.pending = [];
    this.trimmed = true;
    this.setNotice(
      `终端仅保留最近 ${formatBytes(this.totalBytes)} 数据，较早内容已移除。`,
    );
    this.onTrim?.({
      droppedBytes,
      retainedBytes: this.totalBytes,
    });
    this.rebuild();
    this.updateEmpty();
  }

  clear() {
    if (this.rafId !== null) {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    this.frames = [];
    this.pending = [];
    this.totalBytes = 0;
    this.trimmed = false;
    this.rebuilding = true;
    this.flowEl.replaceChildren();
    this.resetRenderState();
    this.updateEmpty();
    this.setNotice("");
    this.scrollEl.scrollTop = 0;
    requestAnimationFrame(() => {
      this.rebuilding = false;
      this.updateJumpVisibility();
    });
  }

  snapshot() {
    return {
      frames: this.frames.slice(),
      totalBytes: this.totalBytes,
      trimmed: this.trimmed,
      notice: this.noticeTextEl?.textContent ?? "",
      follow: this.follow,
      scrollTop: this.scrollEl.scrollTop,
    };
  }

  restore(snapshot) {
    if (!snapshot) {
      this.clear();
      return;
    }
    if (this.rafId !== null) {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    this.frames = snapshot.frames.slice();
    this.totalBytes = snapshot.totalBytes;
    this.trimmed = Boolean(snapshot.trimmed);
    this.follow = Boolean(snapshot.follow);
    this.pending = [];
    this.rebuilding = true;
    this.flowEl.replaceChildren();
    this.resetRenderState();
    for (const frame of this.frames) {
      this.renderFrameIncremental(frame);
    }
    this.updateEmpty();
    this.setNotice(snapshot.notice || "");
    requestAnimationFrame(() => {
      this.rebuilding = false;
      if (this.follow) {
        this.scrollEl.scrollTop = this.scrollEl.scrollHeight;
      } else {
        this.scrollEl.scrollTop = Math.min(
          Number(snapshot.scrollTop) || 0,
          Math.max(0, this.scrollEl.scrollHeight - this.scrollEl.clientHeight),
        );
      }
      this.updateJumpVisibility();
    });
  }

  replaceFrames(dataFrames) {
    if (this.rafId !== null) {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    this.frames = dataFrames.map((frame) => ({
      kind: "data",
      dir: frame.dir === "tx" ? "tx" : "rx",
      ts: Number(frame.ts),
      bytes: frame.bytes instanceof Uint8Array ? frame.bytes.slice() : new Uint8Array(frame.bytes),
    }));
    this.totalBytes = this.frames.reduce((sum, frame) => sum + frame.bytes.length, 0);
    this.trimmed = false;
    this.pending = [];
    this.rebuilding = true;
    this.flowEl.replaceChildren();
    this.resetRenderState();
    for (const frame of this.frames) {
      this.renderFrameIncremental(frame);
    }
    this.updateEmpty();
    this.setNotice("");
    requestAnimationFrame(() => {
      this.rebuilding = false;
      this.scrollEl.scrollTop = this.follow ? this.scrollEl.scrollHeight : 0;
      this.updateJumpVisibility();
    });
  }

  setNotice(text) {
    if (!this.noticeEl || !this.noticeTextEl) {
      return;
    }
    const value = String(text ?? "");
    this.noticeTextEl.textContent = value;
    this.noticeEl.hidden = value.length === 0;
  }

  updateEmpty() {
    if (this.emptyEl) {
      this.emptyEl.hidden = this.frames.length > 0;
    }
  }

  resetRenderState() {
    this.currentSegment = null;
    this.lastRenderedTs = 0;
    this.atLineStart = true;
    this.decoders = this.createDecoders();
  }

  rebuild() {
    if (this.rafId !== null) {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    this.pending = [];

    const shouldStick = this.follow;
    const distanceFromBottom = Math.max(
      0,
      this.scrollEl.scrollHeight - this.scrollEl.scrollTop - this.scrollEl.clientHeight,
    );

    this.rebuilding = true;
    this.flowEl.replaceChildren();
    this.resetRenderState();
    for (const frame of this.frames) {
      this.renderFrameIncremental(frame);
    }
    this.updateEmpty();

    requestAnimationFrame(() => {
      if (shouldStick) {
        this.scrollEl.scrollTop = this.scrollEl.scrollHeight;
      } else {
        this.scrollEl.scrollTop = Math.max(
          0,
          this.scrollEl.scrollHeight - this.scrollEl.clientHeight - distanceFromBottom,
        );
      }
      this.rebuilding = false;
      this.updateJumpVisibility();
    });
  }

  renderFrameIncremental(frame) {
    if (frame.kind === "system") {
      this.renderSystemLine(frame.text);
      return;
    }
    if (this.mode === "hex") {
      this.renderHexLine(frame);
    } else {
      this.renderTextFrame(frame);
    }
  }

  renderSystemLine(text) {
    const line = document.createElement("div");
    line.className = "sys-line";
    const label = document.createElement("span");
    label.textContent = text;
    line.appendChild(label);
    this.flowEl.appendChild(line);
    this.currentSegment = null;
  }

  renderTextFrame(frame) {
    const directionChanged =
      !this.currentSegment || this.currentSegment.dir !== frame.dir;
    const gap = frame.ts - this.lastRenderedTs;
    const startsNew = shouldStartNewTextSegment({
      directionChanged,
      atLineStart: this.atLineStart,
      gap,
      closed: Boolean(this.currentSegment?.closed),
    });
    const longGap = Number.isFinite(gap) && gap > HARD_GAP_MS;

    if ((directionChanged || longGap) && !this.atLineStart && this.currentSegment) {
      // A TX/RX switch or a very long pause mid-line would otherwise attach a
      // new timestamped frame to a stale device line. Break before the new
      // segment so each timestamp keeps its own data.
      this.flowEl.appendChild(document.createElement("br"));
      this.atLineStart = true;
    }

    if (startsNew) {
      const segment = document.createElement("span");
      segment.className = `seg ${frame.dir}`;

      const meta = document.createElement("span");
      meta.className = "seg-meta";

      const time = document.createElement("span");
      time.className = "seg-time";
      time.textContent = formatTime(frame.ts);
      meta.appendChild(time);

      const direction = document.createElement("span");
      direction.className = "seg-dir";
      direction.textContent = frame.dir.toUpperCase();
      meta.appendChild(direction);

      const textNode = document.createElement("span");
      textNode.className = "seg-text";

      segment.append(meta, textNode);
      this.flowEl.appendChild(segment);
      this.currentSegment = {
        dir: frame.dir,
        node: segment,
        textNode,
        closed: false,
      };
    }

    const decoded = this.decoders[frame.dir].decode(frame.bytes, { stream: true });
    this.appendDisplayText(this.currentSegment.textNode, decoded);
    if (decoded.length > 0) {
      // Only LF ends a logical line. Treating CR as a line start would show a
      // timestamp on an empty line when a CRLF pair is split across frames.
      this.atLineStart = decoded.endsWith("\n");
    }
    this.currentSegment.closed =
      frame.bytes.includes(0x0a) || frame.bytes.includes(0x0d);
    this.lastRenderedTs = frame.ts;
  }

  renderHexLine(frame) {
    const line = document.createElement("div");
    line.className = `hex-line ${frame.dir}`;

    const meta = document.createElement("span");
    meta.className = "seg-meta";

    const time = document.createElement("span");
    time.className = "seg-time";
    time.textContent = formatTime(frame.ts);
    meta.appendChild(time);

    const direction = document.createElement("span");
    direction.className = "hex-dir";
    direction.textContent = frame.dir.toUpperCase();
    meta.appendChild(direction);

    const count = document.createElement("span");
    count.className = "hex-count";
    count.textContent = `${frame.bytes.length}B`;
    meta.appendChild(count);

    const hex = document.createElement("span");
    hex.className = "hex-bytes";
    hex.textContent = toHexString(frame.bytes);

    line.append(meta, hex);
    this.flowEl.appendChild(line);
    this.currentSegment = null;
    this.lastRenderedTs = frame.ts;
  }

  appendDisplayText(parent, text) {
    if (!text) {
      return;
    }
    const parts = displaySafeParts(text);
    if (parts.length === 0) {
      return;
    }
    const fragment = document.createDocumentFragment();
    for (const part of parts) {
      if (part.unsafe) {
        const span = document.createElement("span");
        span.className = "ctrl-byte";
        span.textContent = part.text;
        fragment.appendChild(span);
      } else {
        fragment.appendChild(document.createTextNode(part.text));
      }
    }
    parent.appendChild(fragment);
  }
}

export { MAX_BUFFER_BYTES, TRIM_TARGET_BYTES, MERGE_GAP_MS, HARD_GAP_MS };
