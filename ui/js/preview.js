const FRAME_HEADER_BYTES = 20;
const KIND_RX = 1;
const KIND_TX = 2;

const DEMO_PORTS = [
  { name: "COM8", detail: "Silicon Labs CP210x USB to UART Bridge" },
  { name: "COM10", detail: "USB-SERIAL CH340" },
];

const DEFAULT_SETTINGS = {
  portName: "COM8",
  baudRate: 115200,
  dataBits: 8,
  parity: "none",
  stopBits: "1",
  flowControl: "none",
  viewMode: "text",
  showTimestamps: true,
  autoScroll: true,
  sendMode: "text",
  lineEnding: "none",
  clearAfterSend: false,
  autoLog: true,
  logIntervalSecs: 30,
  logDir: "C:\\Users\\Public\\Documents\\Serial Desk\\Logs",
};

class PreviewChannel {
  constructor() {
    this.onmessage = null;
  }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function encodeUtf8(value) {
  return new TextEncoder().encode(value);
}

function toHex(bytes) {
  return Array.from(bytes, (byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(" ");
}

function previewLogDocument(path) {
  const scripted = [
    { dir: "rx", text: "LOG DEMO · RX 中文\r\n" },
    { dir: "tx", text: "AT+LOG?\r\n" },
    { dir: "rx", bytes: Uint8Array.from([0x00, 0x01, 0x7f, 0x80, 0xff]) },
  ];
  let rxBytes = 0;
  let txBytes = 0;
  let rxFrames = 0;
  let txFrames = 0;
  const base = Date.now() - 60_000;
  const frames = scripted.map((item, index) => {
    const bytes = item.bytes || encodeUtf8(item.text);
    if (item.dir === "rx") {
      rxBytes += bytes.length;
      rxFrames += 1;
    } else {
      txBytes += bytes.length;
      txFrames += 1;
    }
    return {
      dir: item.dir,
      tsMs: base + index * 1_000,
      bytes: bytes.length,
      hex: toHex(bytes),
    };
  });
  return {
    path,
    file: "COM8_2026-10-19_11-24-38.log",
    port: "COM8",
    serial: "115200 bps | 8-N-1 | flow: none",
    created: "2026-10-19T11:24:38.000+08:00",
    createdMs: base,
    closed: true,
    totalFrames: frames.length,
    returnedFrames: frames.length,
    rxFrames,
    txFrames,
    rxBytes,
    txBytes,
    size: 1280,
    invalidLines: 0,
    modifiedMs: base + 5_000,
    truncated: false,
    frames,
  };
}

export async function createPreviewApi() {
  let session = null;
  let sequence = 0;
  let timer = null;
  let tick = 0;
  let settings = { ...DEFAULT_SETTINGS };
  const bootTime = Date.now();

  function buildFrame(kind, payload, timestampMs = Date.now()) {
    sequence = (sequence + 1) >>> 0;
    if (sequence === 0) {
      sequence = 1;
    }
    const frame = new Uint8Array(FRAME_HEADER_BYTES + payload.length);
    frame[0] = 0x53;
    frame[1] = 0x44;
    frame[2] = 0x30;
    frame[3] = 0x31;
    new DataView(frame.buffer).setFloat64(4, timestampMs, true);
    new DataView(frame.buffer).setUint32(12, sequence, true);
    frame[16] = kind;
    frame[17] = 0;
    frame.set(payload, FRAME_HEADER_BYTES);
    return frame.buffer;
  }

  function makeLog() {
    if (!settings.autoLog) {
      return null;
    }
    return {
      path: `${settings.logDir}\\COM8_2026-10-19_11-24-38.log`,
      intervalSecs: settings.logIntervalSecs,
      startedAtMs: bootTime,
      lastSavedAtMs: Date.now() - 18_000,
      bytesLogged: 0,
      entries: 0,
      rxBytes: 0,
      txBytes: 0,
      fileSize: 486,
      error: null,
      closed: false,
    };
  }

  function sendPayload(kind, payload) {
    if (!session) {
      return;
    }
    if (kind === KIND_TX) {
      session.txBytes += payload.length;
      session.txFrames += 1;
    } else {
      session.rxBytes += payload.length;
      session.rxFrames += 1;
    }
    if (session.log) {
      session.log.bytesLogged += payload.length;
      session.log.entries += 1;
      session.log.fileSize += payload.length + 46;
      if (kind === KIND_TX) {
        session.log.txBytes += payload.length;
      } else {
        session.log.rxBytes += payload.length;
      }
    }
    session.channel.onmessage?.(buildFrame(kind, payload));
  }

  function sendData(kind, text) {
    sendPayload(kind, encodeUtf8(text));
  }

  function emitTranscript() {
    if (!session) {
      return;
    }
    const scripted = [
      { kind: KIND_TX, text: "AT+VER?\r\n", wait: 60 },
      { kind: KIND_RX, text: "AT+VER?\r\n", wait: 140 },
      { kind: KIND_RX, text: "Serial Desk Fixture v1.4.2\r\n", wait: 210 },
      { kind: KIND_RX, text: "BOOT OK · 2 channels online\r\n", wait: 250 },
      { kind: KIND_TX, text: "AT+READ?\r\n", wait: 340 },
      { kind: KIND_RX, text: "TEMP=23.6C  HUM=41.2%  SEQ=00128\r\n", wait: 520 },
      { kind: KIND_RX, text: "采样完成 · 通道 A 稳定 · 噪声 0.08\r\n", wait: 560 },
      { kind: KIND_TX, text: "AT+MODE=STREAM\r\n", wait: 690 },
      { kind: KIND_RX, text: "STREAM ON\r\n", wait: 740 },
      { kind: KIND_RX, text: "00129 23.7 41.1  0.07\r\n", wait: 1100 },
      { kind: KIND_RX, text: "00130 23.6 41.2  0.08\r\n", wait: 1680 },
    ];
    for (const item of scripted) {
      window.setTimeout(() => {
        if (session && session.open) {
          sendData(item.kind, item.text);
        }
      }, item.wait);
    }
    timer = window.setInterval(() => {
      if (!session || !session.open) {
        return;
      }
      tick += 1;
      const temp = (23.6 + Math.sin(tick / 3) * 0.4).toFixed(1);
      const hum = (41.2 + Math.cos(tick / 4) * 0.5).toFixed(1);
      const noise = (0.06 + Math.abs(Math.sin(tick / 2)) * 0.03).toFixed(2);
      sendData(KIND_RX, `${String(130 + tick).padStart(5, "0")} ${temp} ${hum}  ${noise}\r\n`);
    }, 2600);
  }

  return {
    isPreview: true,
    invoke: async (command, args = {}) => {
      switch (command) {
        case "bootstrap":
          return {
            settings: { ...settings },
            ports: DEMO_PORTS,
            warnings: [],
            version: "preview",
          };
        case "list_ports":
          return DEMO_PORTS;
        case "save_settings":
          settings = { ...args.settings };
          if (session?.log) {
            session.log.intervalSecs = settings.logIntervalSecs;
          }
          return null;
        case "pick_log_dir":
          return "C:\\Users\\Public\\Documents\\Serial Desk\\Logs";
        case "pick_log_file":
          return `${settings.logDir || "C:\\Users\\Public\\Documents\\Serial Desk\\Logs"}\\COM8_2026-10-19_11-24-38.log`;
        case "read_log_file":
          return previewLogDocument(args.path || "");
        case "open_log_folder":
          return null;
        case "open_port": {
          if (session?.open) {
            throw "已有串口连接，请先关闭当前连接。";
          }
          await delay(280);
          const channel = args.dataChannel;
          session = {
            open: true,
            channel,
            startedAtMs: Date.now(),
            rxBytes: 0,
            txBytes: 0,
            rxFrames: 0,
            txFrames: 0,
            log: makeLog(),
          };
          sequence = 0;
          emitTranscript();
          return {
            info: {
              portName: args.settings.portName || "COM8",
              baudRate: Number(args.settings.baudRate) || 115200,
              dataBits: Number(args.settings.dataBits) || 8,
              parity: args.settings.parity || "none",
              stopBits: String(args.settings.stopBits || "1"),
              flowControl: args.settings.flowControl || "none",
              framing: `${args.settings.dataBits || 8}-${
                { none: "N", even: "E", odd: "O", mark: "M", space: "S" }[args.settings.parity || "none"] || "N"
              }-${args.settings.stopBits || "1"}`,
              flowLabel: "none",
              openedAtMs: session.startedAtMs,
            },
            log: session.log,
          };
        }
        case "get_session_status": {
          if (!session?.open) {
            return {
              connected: false,
              info: null,
              rxBytes: 0,
              txBytes: 0,
              rxFrames: 0,
              txFrames: 0,
              fatalError: null,
              log: null,
            };
          }
          if (session.log) {
            session.log.lastSavedAtMs = Date.now();
          }
          return {
            connected: true,
            info: {
              portName: settings.portName || "COM8",
              baudRate: Number(settings.baudRate) || 115200,
              dataBits: Number(settings.dataBits) || 8,
              parity: settings.parity || "none",
              stopBits: String(settings.stopBits || "1"),
              flowControl: settings.flowControl || "none",
              framing: `${settings.dataBits || 8}-${
                { none: "N", even: "E", odd: "O", mark: "M", space: "S" }[settings.parity || "none"] || "N"
              }-${settings.stopBits || "1"}`,
              flowLabel: "none",
              openedAtMs: session.startedAtMs,
            },
            rxBytes: session.rxBytes,
            txBytes: session.txBytes,
            rxFrames: session.rxFrames,
            txFrames: session.txFrames,
            fatalError: null,
            log: session.log,
          };
        }
        case "send_data": {
          if (!session?.open) {
            throw "当前没有打开的串口。";
          }
          const bytes = Array.isArray(args.bytes) ? args.bytes : [];
          const payload = Uint8Array.from(bytes);
          sendPayload(KIND_TX, payload);
          const text = new TextDecoder().decode(payload).trim();
          if (text.startsWith("AT")) {
            window.setTimeout(() => {
              if (session?.open) {
                sendData(KIND_RX, "OK\r\n");
              }
            }, 240);
          }
          return {
            bytesSent: payload.length,
            timestampMs: Date.now(),
            txBytesTotal: session.txBytes,
          };
        }
        case "close_port": {
          if (!session?.open) {
            throw "当前没有打开的串口。";
          }
          if (timer !== null) {
            window.clearInterval(timer);
            timer = null;
          }
          session.open = false;
          const log = session.log
            ? {
                ...session.log,
                lastSavedAtMs: Date.now(),
                closed: true,
              }
            : null;
          const report = {
            log,
            rxBytes: session.rxBytes,
            txBytes: session.txBytes,
            rxFrames: session.rxFrames,
            txFrames: session.txFrames,
            fatalError: null,
            durationMs: Date.now() - session.startedAtMs,
          };
          session = null;
          return report;
        }
        case "save_log_now":
          if (!session?.log) {
            throw "本次连接未启用日志记录。";
          }
          session.log.lastSavedAtMs = Date.now();
          return { ...session.log };
        default:
          throw `预览环境不支持命令：${command}`;
      }
    },
    Channel: PreviewChannel,
    getWindow: null,
  };
}
