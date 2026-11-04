export const SEND_BYTE_LIMIT = 256 * 1024;

export const LINE_ENDINGS = Object.freeze({
  none: Object.freeze([]),
  cr: Object.freeze([0x0d]),
  lf: Object.freeze([0x0a]),
  crlf: Object.freeze([0x0d, 0x0a]),
});

export function lineEndingBytes(name) {
  const bytes = LINE_ENDINGS[name] ?? LINE_ENDINGS.none;
  return Uint8Array.from(bytes);
}

/** Parse HEX input without silently ignoring anything. */
export function parseHexInput(raw) {
  const text = String(raw ?? "");
  const compact = text.replace(/\s+/g, "");
  if (compact.length === 0) {
    return { bytes: new Uint8Array(0), error: null };
  }
  if (!/^[0-9a-fA-F]+$/.test(compact)) {
    return {
      bytes: new Uint8Array(0),
      error: "HEX 只能包含 0–9、A–F 和空格，不能包含其他字符。",
    };
  }
  if (compact.length % 2 !== 0) {
    return {
      bytes: new Uint8Array(0),
      error: `HEX 必须是偶数位字符（当前 ${compact.length} 位）。`,
    };
  }
  const bytes = new Uint8Array(compact.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(compact.slice(index * 2, index * 2 + 2), 16);
  }
  return { bytes, error: null };
}

export function hasUnpairedSurrogate(text) {
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = text.charCodeAt(index + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        index += 1;
      } else {
        return true;
      }
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return true;
    }
  }
  return false;
}

export function buildSendPayload(mode, text, lineEnding, limit = SEND_BYTE_LIMIT) {
  const value = String(text ?? "");
  let bytes;

  if (mode === "hex") {
    const parsed = parseHexInput(value);
    if (parsed.error) {
      return { bytes: new Uint8Array(0), error: parsed.error };
    }
    bytes = parsed.bytes;
  } else {
    if (hasUnpairedSurrogate(value)) {
      return {
        bytes: new Uint8Array(0),
        error: "输入包含无法编码的 Unicode 字符，请检查粘贴内容。",
      };
    }
    const ending = lineEndingBytes(lineEnding);
    const encoded = new TextEncoder().encode(value);
    bytes = new Uint8Array(encoded.length + ending.length);
    bytes.set(encoded, 0);
    bytes.set(ending, encoded.length);
  }

  if (bytes.length === 0) {
    return { bytes, error: null, empty: true };
  }
  if (bytes.length > limit) {
    return {
      bytes,
      error: `单次发送不能超过 ${Math.floor(limit / 1024)} KB（当前 ${bytes.length} 字节）。`,
      empty: false,
    };
  }
  return { bytes, error: null, empty: false };
}

export function toHexString(bytes, maxBytes = Infinity, separator = " ") {
  const count = Math.min(bytes.length, maxBytes);
  const tokens = new Array(count);
  for (let index = 0; index < count; index += 1) {
    tokens[index] = bytes[index].toString(16).toUpperCase().padStart(2, "0");
  }
  let output = tokens.join(separator);
  if (bytes.length > count) {
    output += " …";
  }
  return output;
}

export function formatBytes(value) {
  const bytes = Math.max(0, Number(value) || 0);
  if (bytes < 1024) {
    return `${Math.trunc(bytes)} B`;
  }
  if (bytes < 1024 * 1024) {
    const amount = bytes / 1024;
    return `${amount >= 100 ? amount.toFixed(0) : amount.toFixed(1)} KB`;
  }
  if (bytes < 1024 * 1024 * 1024) {
    const amount = bytes / (1024 * 1024);
    return `${amount >= 100 ? amount.toFixed(0) : amount.toFixed(1)} MB`;
  }
  const amount = bytes / (1024 * 1024 * 1024);
  return `${amount.toFixed(2)} GB`;
}

export function formatCount(value) {
  return Math.max(0, Math.trunc(Number(value) || 0)).toLocaleString("en-US");
}

export function formatTime(epochMs) {
  const date = new Date(epochMs);
  if (Number.isNaN(date.getTime())) {
    return "--:--:--.---";
  }
  const pad = (value, width = 2) => String(value).padStart(width, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(
    date.getMilliseconds(),
    3,
  )}`;
}

export function formatClock(epochMs) {
  const date = new Date(epochMs);
  if (Number.isNaN(date.getTime())) {
    return "--:--:--";
  }
  const pad = (value) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

export function formatDuration(milliseconds) {
  const totalSeconds = Math.max(0, Math.floor((Number(milliseconds) || 0) / 1000));
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  const pad = (value) => String(value).padStart(2, "0");
  return hours > 0 ? `${pad(hours)}:${pad(minutes)}:${pad(seconds)}` : `${pad(minutes)}:${pad(seconds)}`;
}

export function basename(path) {
  const normalized = String(path ?? "").replace(/[\\/]+$/, "");
  const parts = normalized.split(/[\\/]/);
  return parts[parts.length - 1] || normalized;
}

export function debounce(fn, wait = 300) {
  let timer = null;
  return (...args) => {
    if (timer !== null) {
      window.clearTimeout(timer);
    }
    timer = window.setTimeout(() => {
      timer = null;
      fn(...args);
    }, wait);
  };
}

const UNSAFE_DISPLAY =
  /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\u200B-\u200F\u2028\u2029\u202A-\u202E\u2060-\u2064\u2066-\u206F\uFEFF]/g;

export function displaySafeParts(text) {
  const source = String(text ?? "");
  if (!source) {
    return [];
  }
  const parts = [];
  let cursor = 0;
  UNSAFE_DISPLAY.lastIndex = 0;
  let match;
  while ((match = UNSAFE_DISPLAY.exec(source)) !== null) {
    if (match.index > cursor) {
      parts.push({ text: source.slice(cursor, match.index), unsafe: false });
    }
    const code = match[0].codePointAt(0);
    const escaped =
      code <= 0x7f
        ? `\\x${code.toString(16).toUpperCase().padStart(2, "0")}`
        : `\\u{${code.toString(16).toUpperCase().padStart(4, "0")}}`;
    parts.push({ text: escaped, unsafe: true });
    cursor = match.index + match[0].length;
  }
  if (cursor < source.length) {
    parts.push({ text: source.slice(cursor), unsafe: false });
  }
  return parts;
}

export function isIntegerInRange(value, min, max) {
  const number = Number(value);
  return Number.isInteger(number) && number >= min && number <= max;
}

export function parseStopBits(value) {
  return String(value);
}
