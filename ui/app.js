import { TerminalView } from "./js/terminal.js";
import {
  basename,
  buildSendPayload,
  formatBytes,
  formatClock,
  formatCount,
  formatDuration,
  isIntegerInRange,
  toHexString,
} from "./js/format.js";

const DEFAULT_SETTINGS = Object.freeze({
  portName: "",
  baudRate: 115200,
  dataBits: 8,
  parity: "none",
  stopBits: "1",
  flowControl: "none",
  viewMode: "text",
  receiveEncoding: "utf-8",
  showTimestamps: true,
  autoScroll: true,
  sendMode: "text",
  lineEnding: "none",
  clearAfterSend: false,
  autoLog: true,
  logIntervalSecs: 30,
  logDir: "",
});

const FLOW_LABELS = {
  none: "无流控",
  hardware: "RTS/CTS",
  software: "XON/XOFF",
};

const PARITY_CODES = {
  none: "N",
  even: "E",
  odd: "O",
};

const DATA_BITS = [5, 6, 7, 8];
const PARITIES = ["none", "even", "odd"];
const STOP_BITS = ["1", "2"];
const FLOW_CONTROLS = ["none", "hardware", "software"];
const LINE_ENDINGS = ["none", "cr", "lf", "crlf"];
const SEND_MODES = ["text", "hex"];
const VIEW_MODES = ["text", "hex"];
const ENCODINGS = ["utf-8", "gbk"];
const BAUD_PRESETS = [
  110, 300, 600, 1200, 2400, 4800, 9600, 14400, 19200, 38400,
  57600, 115200, 128000, 230400, 256000, 460800, 921600, 1000000,
  2000000, 3000000,
];
const FRAME_HEADER_BYTES = 20;
const FRAME_MAGIC = [0x53, 0x44, 0x30, 0x31]; // "SD01"
const KIND_RX = 1;
const KIND_TX = 2;
const KIND_ERROR = 3;

function byId(id) {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error(`界面元素缺失：${id}`);
  }
  return element;
}

const el = {
  portSelect: byId("portSelect"),
  refreshPorts: byId("refreshPorts"),
  portDetail: byId("portDetail"),
  baudInput: byId("baudInput"),
  baudMenuButton: byId("baudMenuButton"),
  baudMenu: byId("baudMenu"),
  dataBitsSelect: byId("dataBitsSelect"),
  paritySelect: byId("paritySelect"),
  stopBitsSelect: byId("stopBitsSelect"),
  flowSelect: byId("flowSelect"),
  connectBtn: byId("connectBtn"),
  connectBtnLabel: byId("connectBtnLabel"),
  connectionError: byId("connectionError"),
  autoLogToggle: byId("autoLogToggle"),
  logIntervalInput: byId("logIntervalInput"),
  pickLogDir: byId("pickLogDir"),
  logDirLabel: byId("logDirLabel"),
  logFilePath: byId("logFilePath"),
  logPanel: byId("logPanel"),
  logStatusDot: byId("logStatusDot"),
  logStatusText: byId("logStatusText"),
  logSizeText: byId("logSizeText"),
  logSaveText: byId("logSaveText"),
  saveLogNow: byId("saveLogNow"),
  openLogFile: byId("openLogFile"),
  openLogFolder: byId("openLogFolder"),
  logViewBar: byId("logViewBar"),
  logViewName: byId("logViewName"),
  logViewMeta: byId("logViewMeta"),
  exitLogView: byId("exitLogView"),
  logError: byId("logError"),
  logLockNote: byId("logLockNote"),
  notice: byId("notice"),
  noticeText: byId("noticeText"),
  noticeDismiss: byId("noticeDismiss"),
  streamState: byId("streamState"),
  terminalSubtitle: byId("terminalSubtitle"),
  viewTextBtn: byId("viewTextBtn"),
  viewHexBtn: byId("viewHexBtn"),
  encodingSelect: byId("encodingSelect"),
  timestampToggle: byId("timestampToggle"),
  followToggle: byId("followToggle"),
  clearTerminal: byId("clearTerminal"),
  terminalScroll: byId("terminalScroll"),
  terminalFlow: byId("terminalFlow"),
  terminalEmpty: byId("terminalEmpty"),
  terminalEmptyTitle: byId("terminalEmptyTitle"),
  terminalEmptyCopy: byId("terminalEmptyCopy"),
  terminalNotice: byId("terminalNotice"),
  terminalNoticeText: byId("terminalNoticeText"),
  jumpLatest: byId("jumpLatest"),
  toggleLogPane: byId("toggleLogPane"),
  sendTextBtn: byId("sendTextBtn"),
  sendHexBtn: byId("sendHexBtn"),
  lineEndingSelect: byId("lineEndingSelect"),
  clearAfterSend: byId("clearAfterSend"),
  sendInput: byId("sendInput"),
  inputModeHint: byId("inputModeHint"),
  sendBtn: byId("sendBtn"),
  sendMeta: byId("sendMeta"),
  sendPreview: byId("sendPreview"),
  sendError: byId("sendError"),
  sessionDot: byId("sessionDot"),
  sessionState: byId("sessionState"),
  sessionPort: byId("sessionPort"),
  sessionParams: byId("sessionParams"),
  rxCounter: byId("rxCounter"),
  txCounter: byId("txCounter"),
  sessionDuration: byId("sessionDuration"),
  logFooter: byId("logFooter"),
  statusMessage: byId("statusMessage"),
  bootCover: byId("bootCover"),
};

const terminalPane = document.querySelector(".terminal-pane");
const workspace = document.querySelector(".workspace");
const sendBtnLabel = el.sendBtn.querySelector("span");
const LOG_PANE_KEY = "serialdesk.logPaneHidden";
let statusMessageTimer = null;

const state = {
  api: null,
  isPreview: false,
  version: "",
  bootWarnings: [],
  ports: [],
  settings: { ...DEFAULT_SETTINGS },
  connected: false,
  opening: false,
  closing: false,
  sending: false,
  session: null,
  lastSession: null,
  fatalError: null,
  pendingFatal: null,
  handledFatal: false,
  channel: null,
  generation: 0,
  lastSeq: 0,
  log: null,
  lastLog: null,
  lastLogPort: "",
  logIntervalSecs: 30,
  logDir: "",
  autoLog: true,
  viewMode: "text",
  receiveEncoding: "utf-8",
  showTimestamps: true,
  autoScroll: true,
  sendMode: "text",
  lineEnding: "none",
  clearAfterSend: false,
  logPaneHidden: false,
  logView: null,
  logViewLoading: false,
  logViewSnapshot: null,
  logViewCounters: null,
  logViewPrevAutoScroll: true,
  connectionError: "",
  counters: {
    rxBytes: 0,
    txBytes: 0,
    rxFrames: 0,
    txFrames: 0,
    openedAtMs: 0,
    durationMs: 0,
  },
  pollTimer: null,
  pollFailures: 0,
  polling: false,
  portRefreshInFlight: false,
  portAutoTimer: null,
  settingsTimer: null,
  settingsChain: Promise.resolve(),
  sendError: "",
  currentSend: { bytes: new Uint8Array(0), error: null, empty: true },
  terminal: null,
  lastLogErrorNotice: "",
  protocolWarnedGeneration: -1,
  lastNoticeText: "",
};

function setText(node, text) {
  const value = String(text ?? "");
  if (node && node.textContent !== value) {
    node.textContent = value;
  }
}

function errorText(error) {
  if (typeof error === "string") {
    return error;
  }
  if (error instanceof Error) {
    return error.message;
  }
  if (error && typeof error.message === "string") {
    return error.message;
  }
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function isSerialBusy() {
  return state.opening || state.closing;
}

function isActiveSession() {
  return state.connected || isSerialBusy();
}

function isAbsoluteWindowsPath(path) {
  const value = String(path ?? "").trim();
  return /^[a-zA-Z]:[\\/]/.test(value) || /^\\\\[^\\/]+[\\/][^\\/]+/.test(value);
}

function populateBaudMenu() {
  if (el.baudMenu.childElementCount > 0) {
    return;
  }
  const fragment = document.createDocumentFragment();
  for (const rate of BAUD_PRESETS) {
    const option = document.createElement("button");
    option.type = "button";
    option.className = "combo-option";
    option.setAttribute("role", "option");
    option.dataset.value = String(rate);
    option.textContent = String(rate);
    option.addEventListener("click", () => chooseBaud(rate));
    fragment.appendChild(option);
  }
  el.baudMenu.appendChild(fragment);
}

function syncBaudMenuSelection() {
  const value = el.baudInput.value.trim();
  for (const option of el.baudMenu.querySelectorAll(".combo-option")) {
    const selected = option.dataset.value === value;
    option.classList.toggle("is-current", selected);
    option.setAttribute("aria-selected", String(selected));
  }
}

function openBaudMenu() {
  if (el.baudInput.disabled || el.baudMenuButton.disabled) {
    return;
  }
  populateBaudMenu();
  syncBaudMenuSelection();
  el.baudMenu.hidden = false;
  el.baudInput.setAttribute("aria-expanded", "true");
  el.baudMenuButton.setAttribute("aria-expanded", "true");
  const current =
    el.baudMenu.querySelector(".combo-option.is-current") ||
    el.baudMenu.querySelector(".combo-option");
  current?.focus();
  current?.scrollIntoView({ block: "nearest" });
  // Align the menu to whole 24 px rows so the first visible row is not clipped.
  if (el.baudMenu.scrollTop > 0) {
    el.baudMenu.scrollTop =
      2 + Math.floor((el.baudMenu.scrollTop - 2) / 24) * 24;
  }
}

function closeBaudMenu({ restoreFocus = false } = {}) {
  if (el.baudMenu.hidden) {
    return;
  }
  el.baudMenu.hidden = true;
  el.baudInput.setAttribute("aria-expanded", "false");
  el.baudMenuButton.setAttribute("aria-expanded", "false");
  if (restoreFocus) {
    el.baudInput.focus();
  }
}

function chooseBaud(rate) {
  el.baudInput.value = String(rate);
  closeBaudMenu();
  el.baudInput.dispatchEvent(new Event("input", { bubbles: true }));
  el.baudInput.dispatchEvent(new Event("change", { bubbles: true }));
  el.baudInput.focus();
}

function normalizeSettings(raw) {
  const merged = { ...DEFAULT_SETTINGS, ...(raw && typeof raw === "object" ? raw : {}) };
  const baudRate = Number(merged.baudRate);
  const logIntervalSecs = Number(merged.logIntervalSecs);
  return {
    portName: typeof merged.portName === "string" ? merged.portName : "",
    baudRate: isIntegerInRange(baudRate, 50, 4_000_000) ? baudRate : 115200,
    dataBits: DATA_BITS.includes(Number(merged.dataBits)) ? Number(merged.dataBits) : 8,
    parity: PARITIES.includes(merged.parity) ? merged.parity : "none",
    stopBits: STOP_BITS.includes(String(merged.stopBits)) ? String(merged.stopBits) : "1",
    flowControl: FLOW_CONTROLS.includes(merged.flowControl) ? merged.flowControl : "none",
    viewMode: VIEW_MODES.includes(merged.viewMode) ? merged.viewMode : "text",
    receiveEncoding: ENCODINGS.includes(merged.receiveEncoding)
      ? merged.receiveEncoding
      : "utf-8",
    showTimestamps: merged.showTimestamps !== false,
    autoScroll: merged.autoScroll !== false,
    sendMode: SEND_MODES.includes(merged.sendMode) ? merged.sendMode : "text",
    lineEnding: LINE_ENDINGS.includes(merged.lineEnding) ? merged.lineEnding : "none",
    clearAfterSend: Boolean(merged.clearAfterSend),
    autoLog: merged.autoLog !== false,
    logIntervalSecs: isIntegerInRange(logIntervalSecs, 5, 3600) ? logIntervalSecs : 30,
    logDir: typeof merged.logDir === "string" ? merged.logDir : "",
  };
}

function applySettingsToForm(settings) {
  state.settings = settings;
  el.baudInput.value = String(settings.baudRate);
  el.dataBitsSelect.value = String(settings.dataBits);
  el.paritySelect.value = settings.parity;
  el.stopBitsSelect.value = settings.stopBits;
  el.flowSelect.value = settings.flowControl;
  el.logIntervalInput.value = String(settings.logIntervalSecs);
  el.autoLogToggle.checked = settings.autoLog;
  el.clearAfterSend.checked = settings.clearAfterSend;
  el.lineEndingSelect.value = settings.lineEnding;
  el.timestampToggle.checked = settings.showTimestamps;
  el.followToggle.checked = settings.autoScroll;
  el.encodingSelect.value = settings.receiveEncoding;

  state.viewMode = settings.viewMode;
  state.receiveEncoding = settings.receiveEncoding;
  state.showTimestamps = settings.showTimestamps;
  state.autoScroll = settings.autoScroll;
  state.sendMode = settings.sendMode;
  state.lineEnding = settings.lineEnding;
  state.clearAfterSend = settings.clearAfterSend;
  state.autoLog = settings.autoLog;
  state.logIntervalSecs = settings.logIntervalSecs;
  state.logDir = settings.logDir;
}

function currentFraming() {
  const dataBits = el.dataBitsSelect.value || "8";
  const parity = PARITY_CODES[el.paritySelect.value] ?? "N";
  const stopBits = el.stopBitsSelect.value || "1";
  return `${dataBits}-${parity}-${stopBits}`;
}

function buildSettingsPayload() {
  const baudRate = Number(el.baudInput.value.trim());
  const dataBits = Number(el.dataBitsSelect.value);
  const logIntervalSecs = Number(el.logIntervalInput.value);
  const portName = el.portSelect.value;
  const parity = el.paritySelect.value;
  const stopBits = String(el.stopBitsSelect.value);
  const flowControl = el.flowSelect.value;
  const logDir = state.logDir.trim();

  if (!isIntegerInRange(baudRate, 50, 4_000_000)) {
    return { error: "波特率需为 50–4000000 之间的整数。", field: el.baudInput };
  }
  if (!DATA_BITS.includes(dataBits)) {
    return { error: "数据位设置无效。", field: el.dataBitsSelect };
  }
  if (!PARITIES.includes(parity)) {
    return { error: "校验位设置无效。", field: el.paritySelect };
  }
  if (!STOP_BITS.includes(stopBits)) {
    return { error: "停止位设置无效。", field: el.stopBitsSelect };
  }
  if (!FLOW_CONTROLS.includes(flowControl)) {
    return { error: "流控设置无效。", field: el.flowSelect };
  }
  if (!isIntegerInRange(logIntervalSecs, 5, 3600)) {
    return {
      error: "日志保存间隔需为 5–3600 秒之间的整数。",
      field: el.logIntervalInput,
    };
  }
  if (!logDir || !isAbsoluteWindowsPath(logDir)) {
    return { error: "请选择有效的日志保存文件夹。", field: el.pickLogDir };
  }

  return {
    settings: {
      portName,
      baudRate,
      dataBits,
      parity,
      stopBits,
      flowControl,
      viewMode: state.viewMode,
      receiveEncoding: state.receiveEncoding,
      showTimestamps: state.showTimestamps,
      autoScroll: state.autoScroll,
      sendMode: state.sendMode,
      lineEnding: state.lineEnding,
      clearAfterSend: state.clearAfterSend,
      autoLog: state.autoLog,
      logIntervalSecs,
      logDir,
    },
  };
}

async function persistSettings(settings, { quiet = true } = {}) {
  const run = () => state.api.invoke("save_settings", { settings });
  const request = state.settingsChain.then(run, run);
  state.settingsChain = request.catch(() => {});
  try {
    await request;
    state.settings = settings;
    return true;
  } catch (error) {
    if (!quiet) {
      showToast(`设置保存失败：${errorText(error)}`, "error");
    }
    return false;
  }
}

async function saveCurrentSettings({ quiet = true } = {}) {
  const payload = buildSettingsPayload();
  if (payload.error) {
    return false;
  }
  return persistSettings(payload.settings, { quiet });
}

function scheduleSettingsSave() {
  if (state.settingsTimer !== null) {
    window.clearTimeout(state.settingsTimer);
  }
  state.settingsTimer = window.setTimeout(() => {
    state.settingsTimer = null;
    const payload = buildSettingsPayload();
    if (!payload.error) {
      state.settings = payload.settings;
      void persistSettings(payload.settings, { quiet: true });
    }
  }, 350);
}

function renderLogDir() {
  const path = state.logDir || "尚未选择保存文件夹";
  setText(el.logDirLabel, compactPath(path));
  el.pickLogDir.title = path;
  el.logDirLabel.title = path;
}

function compactPath(value, maxLength = 36) {
  const path = String(value ?? "");
  if (path.length <= maxLength) {
    return path;
  }
  const drive = path.match(/^[a-zA-Z]:/)?.[0] || "";
  const parts = path.split(/[\\/]+/).filter(Boolean);
  const tail = parts.slice(-2).join("\\");
  if (drive) {
    return `${drive}\\…\\${tail}`;
  }
  return `…\\${tail}`;
}

function updatePortDetail() {
  const selected = state.ports.find((port) => port.name === el.portSelect.value);
  if (state.opening) {
    setText(el.portDetail, `正在打开 ${el.portSelect.value}…`);
  } else if (state.connected && state.session) {
    const detail = selected?.detail ? ` · ${selected.detail}` : "";
    setText(el.portDetail, `${state.session.portName}${detail}`);
    el.portDetail.title = `${state.session.portName}${detail}`;
  } else if (selected) {
    setText(el.portDetail, selected.detail || "串口设备");
    el.portDetail.title = `${selected.name} · ${selected.detail || "串口设备"}`;
  } else if (state.ports.length === 0) {
    setText(el.portDetail, "未检测到串口，请连接设备并检查驱动");
    el.portDetail.title = "未检测到串口";
  } else {
    setText(el.portDetail, "请选择一个串口");
    el.portDetail.title = "请选择一个串口";
  }
}

function showConnectionError(message, field = null) {
  state.connectionError = String(message ?? "");
  el.connectionError.hidden = state.connectionError.length === 0;
  setText(el.connectionError, state.connectionError);
  updateConnectionUi();
  if (field && typeof field.focus === "function") {
    field.focus();
    field.select?.();
  }
}

function clearConnectionError() {
  if (!state.connectionError && el.connectionError.hidden) {
    return;
  }
  state.connectionError = "";
  el.connectionError.hidden = true;
  setText(el.connectionError, "");
}

function activeLogStatus() {
  if (state.connected && state.log) {
    return state.log;
  }
  if (!state.connected && state.lastLog) {
    return state.lastLog;
  }
  return null;
}

function renderLogUi() {
  const active = activeLogStatus();
  const connected = state.connected;
  const busy = isActiveSession();
  const hasError = Boolean(active?.error);
  const autoLog = state.autoLog;

  let dotState = "idle";
  let statusText = autoLog ? "连接后自动开始" : "自动记录已关闭";
  let sizeText = "—";
  let saveText = autoLog
    ? `每 ${state.logIntervalSecs} 秒写入磁盘`
    : "连接前可开启自动记录";
  let footer = autoLog ? "待连接" : "未启用";

  if (isSerialBusy() || connected) {
    if (state.opening) {
      statusText = "准备记录…";
      saveText = "正在建立日志会话";
    } else if (connected && active) {
      dotState = hasError ? "error" : "recording";
      statusText = hasError ? "保存异常" : "正在记录";
      sizeText = formatBytes(active.fileSize);
      if (active.lastSavedAtMs) {
        saveText = `每 ${active.intervalSecs ?? state.logIntervalSecs} 秒保存 · 最近 ${formatClock(
          active.lastSavedAtMs,
        )}`;
      } else {
        saveText = `每 ${active.intervalSecs ?? state.logIntervalSecs} 秒写入磁盘`;
      }
      footer = hasError ? "日志异常" : "记录中";
    } else if (connected && !active) {
      statusText = "本次连接未记录";
      saveText = "自动记录已关闭，可在断开后修改";
      footer = "未记录";
    }
  } else if (active) {
    dotState = hasError ? "error" : "saved";
    statusText = hasError ? "日志有异常" : "会话日志已保存";
    sizeText = formatBytes(active.fileSize);
    saveText = active.lastSavedAtMs
      ? `已写入 ${formatCount(active.entries)} 条 · ${formatClock(active.lastSavedAtMs)}`
      : `已写入 ${formatCount(active.entries)} 条记录`;
    footer = hasError ? "日志异常" : "已保存";
  }

  el.logStatusDot.dataset.state = dotState;
  setText(el.logStatusText, statusText);
  setText(el.logSizeText, sizeText);
  setText(el.logSaveText, saveText);
  setText(el.logFooter, footer);
  const logFile = active?.path ? basename(active.path) : "—";
  setText(el.logFilePath, logFile);
  el.logFilePath.title = active?.path || "";
  el.autoLogToggle.checked = autoLog;
  el.autoLogToggle.disabled = busy;
  el.pickLogDir.disabled = busy;
  el.logIntervalInput.disabled = state.opening || state.closing;
  el.logLockNote.hidden = !busy;
  el.saveLogNow.disabled = !(connected && active && !state.closing);
  el.pickLogDir.title = busy ? "串口连接期间不可更改保存目录" : state.logDir;
  el.logPanel.classList.toggle("is-locked", busy);

  const logErrorMessage = active?.error || "";
  el.logError.hidden = !logErrorMessage;
  setText(el.logError, logErrorMessage);
  renderLogDir();
}

function updateCounterText(node, bytes, frames) {
  setText(node, formatBytes(bytes));
  node.title = `${formatCount(bytes)} 字节 · ${formatCount(frames)} 帧`;
}

function currentDurationMs() {
  if (state.connected && state.counters.openedAtMs) {
    return Math.max(0, Date.now() - state.counters.openedAtMs);
  }
  return state.counters.durationMs;
}

function updateCountersUi() {
  const counters = state.counters;
  updateCounterText(el.rxCounter, counters.rxBytes, counters.rxFrames);
  updateCounterText(el.txCounter, counters.txBytes, counters.txFrames);
  setText(el.sessionDuration, formatDuration(currentDurationMs()));
}

function updateConnectionUi() {
  const port = el.portSelect.value;
  const framing = state.session
    ? `${state.session.dataBits}-${PARITY_CODES[state.session.parity] ?? "N"}-${state.session.stopBits}`
    : currentFraming();

  if (state.logView) {
    el.sessionDot.dataset.state = "off";
    setText(el.streamState, "日志回放");
    el.streamState.dataset.state = "";
    setText(el.terminalSubtitle, `${state.logView.file} · 只读`);
    el.terminalSubtitle.title = state.logView.path;
    setText(el.sessionState, "日志回放");
    setText(el.sessionPort, state.logView.port || "—");
    setText(el.sessionParams, "只读");
    updateCountersUi();
    setText(el.sessionDuration, "—");
    updatePortDetail();
    updateFormLocking();
    renderLogUi();
    setText(el.logFooter, "查看日志");
    return;
  }
  el.terminalSubtitle.title = "";

  let led = "off";
  if (state.fatalError) {
    led = "error";
  } else if (state.opening || state.closing) {
    led = "connecting";
  } else if (state.connected) {
    led = "on";
  } else if (state.connectionError) {
    led = "error";
  }
  el.sessionDot.dataset.state =
    led === "on" ? "on" : led === "connecting" ? "connecting" : led === "error" ? "error" : "off";

  let streamState = "未连接";
  let streamTone = "";
  if (state.opening) {
    streamState = "连接中";
  } else if (state.closing) {
    streamState = "关闭中";
  } else if (state.connected && state.fatalError) {
    streamState = "中断";
    streamTone = "error";
  } else if (state.connected) {
    streamState = "已连接";
    streamTone = "live";
  } else if (state.lastSession) {
    streamState = "已断开";
  }
  setText(el.streamState, streamState);
  el.streamState.dataset.state = streamTone;

  if (state.connected && state.session) {
    setText(
      el.terminalSubtitle,
      `${state.session.portName} · ${state.session.baudRate} bps · ${framing} · ${FLOW_LABELS[state.session.flowControl] ?? "无流控"}`,
    );
  } else if (state.opening) {
    setText(el.terminalSubtitle, `${port || "串口"} · 正在建立会话`);
  } else if (state.lastSession) {
    setText(el.terminalSubtitle, "会话已结束，记录保留至清空");
  } else {
    setText(el.terminalSubtitle, "打开串口后显示收发数据");
  }

  let sessionState = "未连接";
  if (state.opening) {
    sessionState = "正在连接";
  } else if (state.closing) {
    sessionState = "正在关闭";
  } else if (state.connected) {
    sessionState = state.fatalError ? "连接中断" : "已连接";
  } else if (state.lastSession) {
    sessionState = "已断开";
  }
  setText(el.sessionState, sessionState);
  setText(el.sessionPort, state.connected && state.session ? state.session.portName : "—");
  setText(
    el.sessionParams,
    state.connected && state.session
      ? `${framing} · ${FLOW_LABELS[state.session.flowControl] ?? "无流控"}`
      : "",
  );
  updateCountersUi();
  updatePortDetail();
  updateFormLocking();
  renderLogUi();
}

function updateFormLocking() {
  const locked = isActiveSession();
  const logView = Boolean(state.logView);
  const noPort = state.ports.length === 0;
  const serialControls = [
    el.portSelect,
    el.baudInput,
    el.dataBitsSelect,
    el.paritySelect,
    el.stopBitsSelect,
    el.flowSelect,
  ];

  for (const control of serialControls) {
    control.disabled = locked || (control === el.portSelect && noPort);
  }
  el.refreshPorts.disabled = locked;
  el.baudMenuButton.disabled = locked;
  if (locked) {
    closeBaudMenu();
  }

  const canClose = state.connected && !state.closing;
  const canOpen =
    !state.connected &&
    !locked &&
    !logView &&
    !state.logViewLoading &&
    !noPort &&
    Boolean(el.portSelect.value);
  el.connectBtn.disabled =
    state.opening || state.closing || logView || state.logViewLoading || (!state.connected && !canOpen);
  el.connectBtn.classList.toggle("is-connected", canClose || (state.closing && state.connected));
  if (state.opening) {
    setText(el.connectBtnLabel, "正在打开…");
  } else if (state.closing) {
    setText(el.connectBtnLabel, "正在关闭…");
  } else if (state.connected) {
    setText(el.connectBtnLabel, "关闭串口");
  } else if (logView) {
    setText(el.connectBtnLabel, "日志回放中");
  } else {
    setText(el.connectBtnLabel, "打开串口");
  }
  el.connectBtn.title = logView ? "请先退出日志回放" : "";

  el.clearTerminal.disabled = logView;
  el.clearTerminal.title = logView ? "请先退出日志回放" : "";
  el.openLogFile.disabled = logView || state.logViewLoading || isActiveSession();
  el.openLogFile.title = isActiveSession()
    ? "串口连接期间不可打开日志"
    : logView
      ? "请先退出当前日志回放"
      : "打开已保存的 .log 日志文件";

  el.lineEndingSelect.disabled = state.sendMode === "hex";
  updateSendUi();
}

function setCounterFromStatus(status) {
  state.counters = {
    rxBytes: Number(status.rxBytes) || 0,
    txBytes: Number(status.txBytes) || 0,
    rxFrames: Number(status.rxFrames) || 0,
    txFrames: Number(status.txFrames) || 0,
    openedAtMs: Number(status.info?.openedAtMs) || state.counters.openedAtMs,
    durationMs: state.counters.durationMs,
  };
}

function applyStatus(status) {
  if (!status) {
    return;
  }
  state.pollFailures = 0;
  if (status.info) {
    state.session = status.info;
  }
  state.log = status.log || null;
  setCounterFromStatus(status);
  updateConnectionUi();
  if (state.log?.error) {
    maybeShowLogErrorNotice(state.log.error);
  }
  if (status.fatalError) {
    handleFatal(status.fatalError);
  }
}

function stopStatusPolling() {
  if (state.pollTimer !== null) {
    window.clearTimeout(state.pollTimer);
    state.pollTimer = null;
  }
}

function startStatusPolling() {
  stopStatusPolling();
  state.pollFailures = 0;
  state.pollTimer = window.setTimeout(pollStatus, 260);
}

async function pollStatus() {
  state.pollTimer = null;
  if (!state.connected || state.closing) {
    return;
  }
  const generation = state.generation;
  try {
    const status = await state.api.invoke("get_session_status");
    if (generation !== state.generation || !state.connected) {
      return;
    }
    applyStatus(status);
  } catch (error) {
    if (generation !== state.generation) {
      return;
    }
    state.pollFailures += 1;
    if (state.pollFailures === 3) {
      showNotice(`无法读取串口状态：${errorText(error)}`);
    }
  }
  if (
    state.connected &&
    !state.closing &&
    generation === state.generation
  ) {
    state.pollTimer = window.setTimeout(pollStatus, 500);
  }
}

function normalizeChannelMessage(message) {
  if (message instanceof ArrayBuffer) {
    return new Uint8Array(message);
  }
  if (ArrayBuffer.isView(message)) {
    return new Uint8Array(message.buffer, message.byteOffset, message.byteLength);
  }
  if (Array.isArray(message)) {
    return Uint8Array.from(message);
  }
  if (message && typeof message === "object" && "message" in message) {
    return normalizeChannelMessage(message.message);
  }
  return null;
}

function parseFrame(bytes) {
  if (!bytes || bytes.byteLength < FRAME_HEADER_BYTES) {
    return null;
  }
  if (
    bytes[0] !== FRAME_MAGIC[0] ||
    bytes[1] !== FRAME_MAGIC[1] ||
    bytes[2] !== FRAME_MAGIC[2] ||
    bytes[3] !== FRAME_MAGIC[3]
  ) {
    return null;
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return {
    timestampMs: view.getFloat64(4, true),
    sequence: view.getUint32(12, true),
    kind: view.getUint8(16),
    payload: bytes.subarray(FRAME_HEADER_BYTES),
  };
}

function warnProtocolGap(expected, actual) {
  if (state.protocolWarnedGeneration === state.generation) {
    return;
  }
  state.protocolWarnedGeneration = state.generation;
  showNotice(
    `界面数据流序号不连续（期望 ${expected}，收到 ${actual}）。已继续显示收到的数据，串口日志不受影响。`,
  );
  state.terminal?.appendSystem("界面数据流发生序号缺口，请检查日志文件完整性。");
}

function handleIncomingFrame(bytes, generation) {
  if (generation !== state.generation) {
    return;
  }
  const frame = parseFrame(bytes);
  if (!frame) {
    if (state.protocolWarnedGeneration !== state.generation) {
      state.protocolWarnedGeneration = state.generation;
      showNotice("收到无法识别的界面数据帧，已忽略。串口日志不受影响。");
    }
    return;
  }

  const expected = state.lastSeq === 0 ? 1 : (state.lastSeq + 1) >>> 0;
  if (frame.sequence !== expected) {
    warnProtocolGap(expected, frame.sequence);
  }
  state.lastSeq = frame.sequence >>> 0;

  if (frame.kind === KIND_RX || frame.kind === KIND_TX) {
    state.terminal?.appendFrame(
      frame.kind === KIND_TX ? "tx" : "rx",
      frame.timestampMs,
      frame.payload,
    );
    return;
  }

  if (frame.kind === KIND_ERROR) {
    const message = new TextDecoder("utf-8", { fatal: false }).decode(frame.payload);
    handleFatal(message);
  }
}

function createDataChannel(generation) {
  const channel = new state.api.Channel();
  channel.onmessage = (message) => {
    const bytes = normalizeChannelMessage(message);
    if (bytes) {
      handleIncomingFrame(bytes, generation);
    }
  };
  return channel;
}

function resetCounters() {
  state.counters = {
    rxBytes: 0,
    txBytes: 0,
    rxFrames: 0,
    txFrames: 0,
    openedAtMs: 0,
    durationMs: 0,
  };
  updateCountersUi();
}

async function openPort() {
  if (state.connected || isSerialBusy()) {
    return;
  }
  if (state.logView || state.logViewLoading) {
    showToast("请先退出日志回放，再连接串口。", "error");
    return;
  }
  clearConnectionError();
  const portName = el.portSelect.value;
  if (!portName) {
    showConnectionError("没有可用的串口。请先连接设备并刷新串口列表。", el.refreshPorts);
    return;
  }

  const payload = buildSettingsPayload();
  if (payload.error) {
    showConnectionError(payload.error, payload.field);
    return;
  }
  if (state.autoLog && (!state.logDir || !isAbsoluteWindowsPath(state.logDir))) {
    showConnectionError("请选择有效的日志保存文件夹。", el.pickLogDir);
    return;
  }

  const settings = payload.settings;
  const generation = state.generation + 1;
  if (state.settingsTimer !== null) {
    window.clearTimeout(state.settingsTimer);
    state.settingsTimer = null;
  }
  state.generation = generation;
  state.opening = true;
  state.connectionError = "";
  state.fatalError = null;
  state.pendingFatal = null;
  state.lastSeq = 0;
  state.protocolWarnedGeneration = -1;
  el.connectionError.hidden = true;
  resetCounters();
  updateConnectionUi();
  updatePortDetail();

  let channel = null;
  try {
    // Persist the exact configuration before opening so a restart can restore
    // it, and so a failed settings write is surfaced before hardware is touched.
    if (!(await persistSettings(settings, { quiet: false }))) {
      throw new Error("连接设置未能保存，已取消打开串口。");
    }
    state.settings = settings;
    state.logDir = settings.logDir;
    state.logIntervalSecs = settings.logIntervalSecs;
    state.autoLog = settings.autoLog;

    channel = createDataChannel(generation);
    const report = await state.api.invoke("open_port", {
      settings,
      dataChannel: channel,
    });
    if (generation !== state.generation) {
      return;
    }

    const pendingFatal = state.pendingFatal;
    state.opening = false;
    state.connected = true;
    state.session = report.info;
    state.lastSession = report.info;
    state.log = report.log || null;
    state.lastLog = null;
    state.lastLogPort = "";
    state.fatalError = null;
    state.pendingFatal = null;
    state.handledFatal = false;
    state.lastLogErrorNotice = "";
    state.channel = channel;
    setCounterFromStatus({
      rxBytes: 0,
      txBytes: 0,
      rxFrames: 0,
      txFrames: 0,
      info: report.info,
    });
    state.counters.openedAtMs = report.info.openedAtMs;
    state.counters.durationMs = 0;

    hideNotice();
    state.terminal?.appendSystem(
      `${report.info.portName} 已连接 · ${report.info.baudRate} bps · ${report.info.framing} · ${FLOW_LABELS[report.info.flowControl] ?? "无流控"}`,
    );
    updateConnectionUi();
    startStatusPolling();
    updateSendUi();
    if (state.log?.error) {
      maybeShowLogErrorNotice(state.log.error);
    }
    if (pendingFatal) {
      handleFatal(pendingFatal);
    }
  } catch (error) {
    if (generation === state.generation) {
      state.generation += 1;
    }
    state.opening = false;
    state.connected = false;
    state.session = null;
    state.channel = null;
    state.log = null;
    state.pendingFatal = null;
    showConnectionError(errorText(error), el.connectBtn);
    updateConnectionUi();
    void refreshPorts({ manual: false, silent: true });
  } finally {
    if (channel && !state.connected) {
      state.channel = null;
    }
    updateFormLocking();
  }
}

async function closePort({ reason = "user", silent = false } = {}) {
  if (!state.connected || state.closing) {
    return;
  }
  state.closing = true;
  stopStatusPolling();
  updateConnectionUi();

  try {
    const report = await state.api.invoke("close_port");
    const previousSession = state.session;
    const previousFatal = state.fatalError;
    const log = report.log || null;

    state.generation += 1;
    state.closing = false;
    state.connected = false;
    state.channel = null;
    state.lastSeq = 0;
    state.lastSession = previousSession;
    state.session = null;
    state.status = null;
    state.log = null;
    state.lastLog = log;
    state.lastLogPort = previousSession?.portName || "";
    state.fatalError = report.fatalError || previousFatal || null;
    state.handledFatal = true;
    state.pendingFatal = null;
    state.logIntervalSecs = Number(el.logIntervalInput.value) || state.logIntervalSecs;
    state.counters = {
      ...state.counters,
      rxBytes: Number(report.rxBytes) || 0,
      txBytes: Number(report.txBytes) || 0,
      rxFrames: Number(report.rxFrames) || 0,
      txFrames: Number(report.txFrames) || 0,
      durationMs: Number(report.durationMs) || 0,
    };

    const portName = previousSession?.portName || "串口";
    if (log) {
      if (log.error) {
        state.terminal?.appendSystem(
          `${portName} 已断开 · 日志保存异常 · ${basename(log.path)}`,
        );
      } else {
        state.terminal?.appendSystem(
          `${portName} 已断开 · 日志已保存 · ${basename(log.path)} · ${formatBytes(log.fileSize)}`,
        );
      }
      maybeShowLogErrorNotice(log.error);
    } else {
      state.terminal?.appendSystem(`${portName} 已断开 · 本次未记录日志`);
    }

    updateConnectionUi();
    updateFormLocking();
    updateSendUi();

    if (reason === "fatal" || state.fatalError) {
      showNotice(
        `串口连接已中断：${state.fatalError || "读取失败"}${
          log ? "。日志已保存。" : "。本次未启用日志，已接收的数据仅保留在终端。"
        }`,
      );
    } else if (log?.error) {
      showNotice(`日志保存异常：${log.error}`);
    }
    if (!silent && reason === "user") {
      showToast(log ? `${portName} 已关闭，日志已保存` : `${portName} 已关闭`);
    }

    if (el.portSelect.value !== portName) {
      el.portSelect.value = portName;
    }
    void refreshPorts({ manual: false, silent: true });
    el.connectBtn.focus();
  } catch (error) {
    state.closing = false;
    updateConnectionUi();
    showNotice(`关闭串口失败：${errorText(error)}。请重试。`);
    if (state.connected) {
      startStatusPolling();
    }
  }
}

function handleFatal(message) {
  const text = String(message || "串口连接已中断。");
  if (state.fatalError) {
    return;
  }
  state.fatalError = text;
  state.handledFatal = false;
  updateConnectionUi();
  showNotice(`串口连接异常：${text} 正在保存日志并关闭连接…`);
  if (state.opening) {
    state.pendingFatal = text;
    return;
  }
  if (state.connected && !state.closing) {
    void closePort({ reason: "fatal" });
  }
}

function maybeShowLogErrorNotice(error) {
  const text = String(error || "");
  if (!text || text === state.lastLogErrorNotice) {
    return;
  }
  state.lastLogErrorNotice = text;
  showNotice(`日志写入异常：${text} 请检查磁盘空间和保存目录。`);
}

function updateSendUi() {
  const value = el.sendInput.value;
  const payload = buildSendPayload(state.sendMode, value, state.lineEnding);
  state.currentSend = payload;

  el.sendTextBtn.classList.toggle("active", state.sendMode === "text");
  el.sendHexBtn.classList.toggle("active", state.sendMode === "hex");
  el.sendTextBtn.setAttribute("aria-pressed", String(state.sendMode === "text"));
  el.sendHexBtn.setAttribute("aria-pressed", String(state.sendMode === "hex"));
  el.inputModeHint.textContent = state.sendMode === "hex" ? "HEX" : "UTF-8";
  el.sendInput.placeholder =
    state.sendMode === "hex"
      ? "输入 HEX 字节，例如 41 54 0D 0A…"
      : "输入要发送的数据…";
  el.lineEndingSelect.disabled = state.sendMode === "hex";

  const hasValue = value.length > 0;
  let error = "";
  if (payload.error) {
    error = payload.error;
  } else if (state.sendError) {
    error = state.sendError;
  }
  if (!hasValue && !payload.error) {
    error = state.sendError;
  }
  el.sendError.hidden = error.length === 0;
  setText(el.sendError, error);

  if (payload.error) {
    setText(el.sendMeta, "无法发送");
    setText(el.sendPreview, "");
  } else {
    setText(el.sendMeta, `${payload.bytes.length} 字节`);
    setText(
      el.sendPreview,
      payload.bytes.length > 0 ? toHexString(payload.bytes, 16) : "",
    );
  }

  const canSend =
    state.connected &&
    !state.closing &&
    !state.opening &&
    !state.sending &&
    !payload.error &&
    !payload.empty;
  el.sendBtn.disabled = !canSend;
  setText(sendBtnLabel, state.sending ? "发送中" : "发送");
  el.sendBtn.setAttribute("aria-busy", String(state.sending));
}

function resizeSendInput() {
  const node = el.sendInput;
  node.style.height = "auto";
  const max = window.matchMedia("(max-height: 700px)").matches ? 90 : 120;
  const next = Math.min(Math.max(node.scrollHeight, 50), max);
  node.style.height = `${next}px`;
  node.style.overflowY = node.scrollHeight > max ? "auto" : "hidden";
}

function setViewMode(mode, { save = true } = {}) {
  const next = VIEW_MODES.includes(mode) ? mode : "text";
  state.viewMode = next;
  el.viewTextBtn.classList.toggle("active", next === "text");
  el.viewHexBtn.classList.toggle("active", next === "hex");
  el.viewTextBtn.setAttribute("aria-pressed", String(next === "text"));
  el.viewHexBtn.setAttribute("aria-pressed", String(next === "hex"));
  el.encodingSelect.disabled = next === "hex";
  state.terminal?.setMode(next);
  if (save) {
    scheduleSettingsSave();
  }
}

function setTimestamps(enabled, { save = true } = {}) {
  state.showTimestamps = Boolean(enabled);
  el.timestampToggle.checked = state.showTimestamps;
  state.terminal?.setTimestamps(state.showTimestamps);
  if (save) {
    scheduleSettingsSave();
  }
}

function setEncoding(encoding, { save = true } = {}) {
  const next = ENCODINGS.includes(encoding) ? encoding : "utf-8";
  state.receiveEncoding = next;
  el.encodingSelect.value = next;
  el.encodingSelect.disabled = state.viewMode === "hex";
  state.terminal?.setEncoding(next);
  if (save) {
    scheduleSettingsSave();
  }
}

function setAutoFollow(enabled, { save = true, scroll = true } = {}) {
  state.autoScroll = Boolean(enabled);
  el.followToggle.checked = state.autoScroll;
  state.terminal?.setFollow(state.autoScroll, { scroll });
  if (save) {
    scheduleSettingsSave();
  }
}

function readLogPanePreference() {
  try {
    return window.localStorage.getItem(LOG_PANE_KEY) === "1";
  } catch {
    return false;
  }
}

function setLogPaneHidden(hidden, { persist = true } = {}) {
  const next = Boolean(hidden);
  state.logPaneHidden = next;
  workspace?.classList.toggle("is-log-hidden", next);
  const label = next ? "展开日志面板（不影响日志记录）" : "收起日志面板（不影响日志记录）";
  el.toggleLogPane.classList.toggle("is-collapsed", next);
  el.toggleLogPane.setAttribute("aria-expanded", String(!next));
  el.toggleLogPane.title = label;
  el.toggleLogPane.setAttribute("aria-label", label);
  if (persist) {
    try {
      window.localStorage.setItem(LOG_PANE_KEY, next ? "1" : "0");
    } catch {
      // A restricted webview can ignore this UI-only preference.
    }
  }
}

function setSendMode(mode, { save = true } = {}) {
  state.sendMode = SEND_MODES.includes(mode) ? mode : "text";
  state.sendError = "";
  if (save) {
    scheduleSettingsSave();
  }
  updateSendUi();
}

function setTerminalEmptyCopy(title, copy) {
  setText(el.terminalEmptyTitle, title);
  setText(el.terminalEmptyCopy, copy);
}

function hexToBytes(value) {
  const text = String(value ?? "").trim();
  if (!text) {
    return new Uint8Array(0);
  }
  const parts = text.split(/\s+/);
  const bytes = new Uint8Array(parts.length);
  for (let index = 0; index < parts.length; index += 1) {
    const byte = Number.parseInt(parts[index], 16);
    bytes[index] = Number.isFinite(byte) ? byte & 0xff : 0;
  }
  return bytes;
}

function logViewMetaText(document) {
  const parts = [];
  if (document.port) {
    parts.push(document.port);
  }
  if (document.serial) {
    parts.push(document.serial);
  }
  parts.push(`${formatCount(document.totalFrames)} 帧`);
  parts.push(`RX ${formatBytes(document.rxBytes)} / TX ${formatBytes(document.txBytes)}`);
  parts.push(document.closed ? "已关闭" : "可能仍在写入");
  if (document.invalidLines > 0) {
    parts.push(`无效行 ${formatCount(document.invalidLines)}`);
  }
  if (document.truncated) {
    parts.push(`仅显示最近 ${formatCount(document.returnedFrames)} 帧`);
  }
  return parts.join(" · ");
}

function enterLogView(document) {
  if (state.logView) {
    return;
  }
  state.logViewSnapshot = state.terminal?.snapshot() || null;
  state.logViewCounters = { ...state.counters };
  state.logViewPrevAutoScroll = state.autoScroll;
  state.logView = document;
  state.connectionError = "";
  hideNotice();
  el.connectionError.hidden = true;

  el.logViewBar.hidden = false;
  setText(el.logViewName, document.file);
  el.logViewName.title = document.path;
  setText(el.logViewMeta, logViewMetaText(document));
  el.logViewMeta.title = logViewMetaText(document);
  el.logViewMeta.classList.toggle(
    "is-warning",
    !document.closed || document.invalidLines > 0 || document.truncated,
  );

  setTerminalEmptyCopy("日志为空", "该日志没有可显示的数据帧。");
  state.counters = {
    rxBytes: document.rxBytes,
    txBytes: document.txBytes,
    rxFrames: document.rxFrames,
    txFrames: document.txFrames,
    openedAtMs: 0,
    durationMs: 0,
  };
  setAutoFollow(false, { save: false, scroll: false });
  state.terminal?.replaceFrames(
    document.frames.map((frame) => ({
      dir: frame.dir,
      ts: Number(frame.tsMs) || 0,
      bytes: hexToBytes(frame.hex),
    })),
  );
  updateConnectionUi();
  updateSendUi();
  updateFormLocking();
  el.exitLogView.focus();
  showToast(`已打开日志：${document.file}`);
}

function exitLogView() {
  if (!state.logView) {
    return;
  }
  const snapshot = state.logViewSnapshot;
  const counters = state.logViewCounters;
  const previousAutoScroll = state.logViewPrevAutoScroll;
  state.logView = null;
  state.logViewSnapshot = null;
  state.logViewCounters = null;
  el.logViewBar.hidden = true;
  setText(el.logViewName, "");
  setText(el.logViewMeta, "");
  el.logViewName.title = "";
  el.logViewMeta.title = "";
  el.logViewMeta.classList.remove("is-warning");
  setTerminalEmptyCopy("未连接", "选择串口并打开，收发数据会显示在这里。");

  if (snapshot) {
    state.terminal?.restore(snapshot);
  } else {
    state.terminal?.clear();
  }
  if (counters) {
    state.counters = { ...counters };
  }
  setAutoFollow(previousAutoScroll, { save: false, scroll: false });
  updateConnectionUi();
  updateSendUi();
  updateFormLocking();
  el.openLogFile.focus();
}

async function openLogFromDialog() {
  if (state.logViewLoading) {
    return;
  }
  if (state.logView) {
    return;
  }
  if (isActiveSession()) {
    showToast("请先断开串口，再打开保存的日志。", "error");
    return;
  }
  state.logViewLoading = true;
  el.openLogFile.disabled = true;
  el.openLogFile.setAttribute("aria-busy", "true");
  setText(el.openLogFile, "读取中…");
  updateFormLocking();
  try {
    const path = await state.api.invoke("pick_log_file");
    if (!path) {
      return;
    }
    const document = await state.api.invoke("read_log_file", { path });
    enterLogView(document);
  } catch (error) {
    showToast(`打开日志失败：${errorText(error)}`, "error");
  } finally {
    state.logViewLoading = false;
    el.openLogFile.removeAttribute("aria-busy");
    setText(el.openLogFile, "打开日志");
    updateFormLocking();
  }
}

async function sendData() {
  if (state.sending || !state.connected || state.closing) {
    return;
  }
  state.sendError = "";
  updateSendUi();
  const payload = buildSendPayload(state.sendMode, el.sendInput.value, state.lineEnding);
  if (payload.error) {
    state.sendError = payload.error;
    updateSendUi();
    el.sendInput.focus();
    return;
  }
  if (payload.empty) {
    state.sendError = "没有可发送的数据。";
    updateSendUi();
    el.sendInput.focus();
    return;
  }

  state.sending = true;
  updateSendUi();
  try {
    await state.api.invoke("send_data", { bytes: Array.from(payload.bytes) });
    state.sendError = "";
    if (state.clearAfterSend) {
      el.sendInput.value = "";
      resizeSendInput();
    }
    updateSendUi();
    void pollStatusOnce();
  } catch (error) {
    state.sendError = `发送失败：${errorText(error)}`;
    updateSendUi();
  } finally {
    state.sending = false;
    updateSendUi();
  }
}

async function pollStatusOnce() {
  if (!state.connected || state.closing) {
    return;
  }
  const generation = state.generation;
  try {
    const status = await state.api.invoke("get_session_status");
    if (generation === state.generation && state.connected) {
      applyStatus(status);
    }
  } catch {
    // The regular poller reports persistent failures.
  }
}

async function saveLogNow() {
  if (!state.connected || !state.log || state.closing) {
    return;
  }
  el.saveLogNow.disabled = true;
  try {
    const status = await state.api.invoke("save_log_now");
    if (status) {
      state.log = status;
      renderLogUi();
      if (status.error) {
        maybeShowLogErrorNotice(status.error);
      } else {
        showToast("日志已写入磁盘");
      }
    }
  } catch (error) {
    showToast(`立即保存失败：${errorText(error)}`, "error");
  } finally {
    renderLogUi();
  }
}

function applyPorts(ports) {
  state.ports = Array.isArray(ports) ? ports : [];
  const previous = el.portSelect.value || state.settings.portName;
  if (state.ports.length === 0) {
    const placeholder = new Option("未发现串口", "");
    placeholder.disabled = true;
    el.portSelect.replaceChildren(placeholder);
    el.portSelect.disabled = true;
    updatePortDetail();
    updateFormLocking();
    return;
  }

  const fragment = document.createDocumentFragment();
  for (const port of state.ports) {
    const option = new Option(port.name, port.name);
    option.title = `${port.name}${port.detail ? ` · ${port.detail}` : ""}`;
    fragment.appendChild(option);
  }
  el.portSelect.replaceChildren(fragment);

  const candidate =
    state.ports.find((port) => port.name === previous) ||
    state.ports.find((port) => port.name === state.settings.portName) ||
    state.ports[0];
  el.portSelect.value = candidate.name;
  if (!state.connected) {
    el.portSelect.disabled = false;
  }
  updatePortDetail();
  updateFormLocking();
}

async function refreshPorts({ manual = false, silent = true } = {}) {
  if (isActiveSession() || state.portRefreshInFlight) {
    return;
  }
  state.portRefreshInFlight = true;
  if (manual) {
    el.refreshPorts.classList.add("spinning");
    el.refreshPorts.disabled = true;
  }
  try {
    const ports = await state.api.invoke("list_ports");
    if (!isActiveSession()) {
      applyPorts(ports);
    }
  } catch (error) {
    if (manual && !silent) {
      showToast(`扫描串口失败：${errorText(error)}`, "error");
    }
    if (state.ports.length === 0) {
      setText(el.portDetail, "扫描串口失败，请检查驱动");
    }
  } finally {
    state.portRefreshInFlight = false;
    if (manual) {
      el.refreshPorts.classList.remove("spinning");
      el.refreshPorts.disabled = isActiveSession();
    }
  }
}

function startPortAutoRefresh() {
  if (state.portAutoTimer !== null) {
    return;
  }
  state.portAutoTimer = window.setInterval(() => {
    if (
      document.visibilityState !== "visible" ||
      isActiveSession() ||
      state.portRefreshInFlight ||
      document.activeElement === el.portSelect
    ) {
      return;
    }
    void refreshPorts({ manual: false, silent: true });
  }, 2500);
}

function showNotice(message) {
  const text = String(message || "");
  if (!text) {
    hideNotice();
    return;
  }
  state.lastNoticeText = text;
  setText(el.noticeText, text);
  el.notice.hidden = false;
}

function hideNotice() {
  el.notice.hidden = true;
  state.lastNoticeText = "";
}

function showToast(message, tone = "info") {
  const text = String(message || "");
  if (!text) {
    return;
  }
  if (statusMessageTimer !== null) {
    window.clearTimeout(statusMessageTimer);
  }
  setText(el.statusMessage, text);
  el.statusMessage.title = text;
  el.statusMessage.dataset.tone = tone === "error" ? "error" : "info";
  statusMessageTimer = window.setTimeout(() => {
    statusMessageTimer = null;
    setText(el.statusMessage, "");
    el.statusMessage.removeAttribute("data-tone");
    el.statusMessage.title = "";
  }, tone === "error" ? 6000 : 3500);
}

function bindEvents() {
  el.connectBtn.addEventListener("click", () => {
    if (state.connected) {
      void closePort({ reason: "user" });
    } else {
      void openPort();
    }
  });
  el.refreshPorts.addEventListener("click", () =>
    refreshPorts({ manual: true, silent: false }),
  );

  const configControls = [
    el.portSelect,
    el.baudInput,
    el.dataBitsSelect,
    el.paritySelect,
    el.stopBitsSelect,
    el.flowSelect,
  ];
  for (const control of configControls) {
    control.addEventListener("change", () => {
      clearConnectionError();
      updatePortDetail();
      updateConnectionUi();
      scheduleSettingsSave();
    });
    if (control === el.baudInput) {
      control.addEventListener("input", () => {
        clearConnectionError();
        updatePortDetail();
        if (!el.baudMenu.hidden) {
          syncBaudMenuSelection();
        }
      });
    }
  }
  el.portSelect.addEventListener("change", updatePortDetail);

  el.baudMenuButton.addEventListener("click", () => {
    if (el.baudMenu.hidden) {
      openBaudMenu();
    } else {
      closeBaudMenu();
    }
  });
  el.baudInput.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      openBaudMenu();
    } else if (event.key === "Escape") {
      closeBaudMenu();
    }
  });
  el.baudMenu.addEventListener("keydown", (event) => {
    const options = Array.from(el.baudMenu.querySelectorAll(".combo-option"));
    const index = options.indexOf(document.activeElement);
    if (event.key === "ArrowDown") {
      event.preventDefault();
      options[Math.min(index + 1, options.length - 1)]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      options[Math.max(index - 1, 0)]?.focus();
    } else if (event.key === "Home") {
      event.preventDefault();
      options[0]?.focus();
    } else if (event.key === "End") {
      event.preventDefault();
      options[options.length - 1]?.focus();
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeBaudMenu({ restoreFocus: true });
    } else if (event.key === "Tab") {
      closeBaudMenu();
    }
  });
  document.addEventListener("pointerdown", (event) => {
    if (!el.baudMenu.hidden && !event.target.closest(".baud-field")) {
      closeBaudMenu();
    }
  });

  el.autoLogToggle.addEventListener("change", () => {
    if (isActiveSession()) {
      el.autoLogToggle.checked = state.autoLog;
      return;
    }
    state.autoLog = el.autoLogToggle.checked;
    renderLogUi();
    scheduleSettingsSave();
  });

  el.logIntervalInput.addEventListener("input", () => {
    const value = Number(el.logIntervalInput.value);
    if (isIntegerInRange(value, 5, 3600)) {
      state.logIntervalSecs = value;
      renderLogUi();
      scheduleSettingsSave();
    }
  });
  el.logIntervalInput.addEventListener("change", () => {
    const value = Number(el.logIntervalInput.value);
    if (!isIntegerInRange(value, 5, 3600)) {
      el.logIntervalInput.value = String(state.logIntervalSecs);
      el.logIntervalInput.setAttribute("aria-invalid", "true");
      showToast("日志保存间隔需为 5–3600 秒。", "error");
    } else {
      el.logIntervalInput.removeAttribute("aria-invalid");
      state.logIntervalSecs = value;
      renderLogUi();
      scheduleSettingsSave();
    }
  });

  el.pickLogDir.addEventListener("click", async () => {
    if (isActiveSession()) {
      return;
    }
    el.pickLogDir.disabled = true;
    try {
      const selected = await state.api.invoke("pick_log_dir", {
        initial: state.logDir || null,
      });
      if (selected) {
        state.logDir = selected;
        renderLogDir();
        renderLogUi();
        scheduleSettingsSave();
      }
    } catch (error) {
      showToast(`选择文件夹失败：${errorText(error)}`, "error");
    } finally {
      renderLogUi();
    }
  });

  el.openLogFolder.addEventListener("click", async () => {
    try {
      await state.api.invoke("open_log_folder");
    } catch (error) {
      showToast(`打开文件夹失败：${errorText(error)}`, "error");
    }
  });

  el.saveLogNow.addEventListener("click", () => void saveLogNow());

  el.viewTextBtn.addEventListener("click", () => setViewMode("text"));
  el.viewHexBtn.addEventListener("click", () => setViewMode("hex"));
  el.encodingSelect.addEventListener("change", () =>
    setEncoding(el.encodingSelect.value),
  );
  el.timestampToggle.addEventListener("change", () =>
    setTimestamps(el.timestampToggle.checked),
  );
  el.followToggle.addEventListener("change", () =>
    setAutoFollow(el.followToggle.checked, { save: !state.logView }),
  );
  el.clearTerminal.addEventListener("click", () => {
    if (state.logView) {
      return;
    }
    state.terminal?.clear();
  });
  el.openLogFile.addEventListener("click", () => void openLogFromDialog());
  el.exitLogView.addEventListener("click", exitLogView);
  el.toggleLogPane.addEventListener("click", () => {
    setLogPaneHidden(!state.logPaneHidden);
  });

  el.sendTextBtn.addEventListener("click", () => setSendMode("text"));
  el.sendHexBtn.addEventListener("click", () => setSendMode("hex"));
  el.lineEndingSelect.addEventListener("change", () => {
    state.lineEnding = el.lineEndingSelect.value;
    scheduleSettingsSave();
    updateSendUi();
  });
  el.clearAfterSend.addEventListener("change", () => {
    state.clearAfterSend = el.clearAfterSend.checked;
    scheduleSettingsSave();
  });
  el.sendInput.addEventListener("input", () => {
    state.sendError = "";
    resizeSendInput();
    updateSendUi();
  });
  el.sendInput.addEventListener("keydown", (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      void sendData();
    }
  });
  el.sendBtn.addEventListener("click", () => void sendData());

  el.noticeDismiss.addEventListener("click", hideNotice);

  window.addEventListener("keydown", (event) => {
    const key = String(event.key || "").toLowerCase();
    if (
      !state.isPreview &&
      (event.key === "F5" || ((event.ctrlKey || event.metaKey) && key === "r"))
    ) {
      event.preventDefault();
      showToast("已阻止刷新，避免丢失当前终端内容。");
    }
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      const target = event.target;
      if (target === el.logIntervalInput || target === el.baudInput) {
        return;
      }
      event.preventDefault();
      void sendData();
    }
    if (event.key === "Escape" && state.logView) {
      event.preventDefault();
      exitLogView();
      return;
    }
    if (event.key === "Escape" && !el.notice.hidden) {
      hideNotice();
    }
  });

  window.addEventListener("dragover", (event) => event.preventDefault());
  window.addEventListener("drop", (event) => event.preventDefault());

  let resizeTimer = null;
  window.addEventListener("resize", () => {
    closeBaudMenu();
    if (resizeTimer !== null) {
      window.clearTimeout(resizeTimer);
    }
    resizeTimer = window.setTimeout(() => {
      resizeTimer = null;
      resizeSendInput();
    }, 100);
  });
  window.addEventListener("blur", () => closeBaudMenu());

  startPortAutoRefresh();
}

function setInitialUiFromSettings() {
  renderLogDir();
  setViewMode(state.viewMode, { save: false });
  setEncoding(state.receiveEncoding, { save: false });
  setTimestamps(state.showTimestamps, { save: false });
  setAutoFollow(state.autoScroll, { save: false, scroll: false });
  setLogPaneHidden(readLogPanePreference(), { persist: false });
  el.clearAfterSend.checked = state.clearAfterSend;
  el.lineEndingSelect.value = state.lineEnding;
  updateSendUi();
  updateConnectionUi();
  updateFormLocking();
  resizeSendInput();
}

async function createApi() {
  const preview = new URLSearchParams(window.location.search).get("preview") === "1";
  if (preview) {
    const { createPreviewApi } = await import("./js/preview.js");
    const api = await createPreviewApi();
    api.isPreview = true;
    return api;
  }
  const tauri = globalThis.__TAURI__;
  if (!tauri || !tauri.core || typeof tauri.core.invoke !== "function") {
    throw new Error("未检测到桌面运行环境。请通过 SerialDesk.exe 启动应用。");
  }
  return {
    isPreview: false,
    invoke: (command, args) => tauri.core.invoke(command, args),
    Channel: tauri.core.Channel,
  };
}

function showBootError(error) {
  const message = errorText(error);
  el.bootCover.classList.add("is-error");
  el.bootCover.replaceChildren();
  const title = document.createElement("div");
  title.className = "boot-error-title";
  title.textContent = "应用无法启动";
  const copy = document.createElement("p");
  copy.className = "boot-error-copy";
  copy.textContent = message;
  const hint = document.createElement("p");
  hint.className = "boot-error-hint";
  hint.textContent = "请从 SerialDesk.exe 启动，或重新启动应用。";
  el.bootCover.append(title, copy, hint);
}

function hideBootCover() {
  requestAnimationFrame(() => {
    el.bootCover.classList.add("is-hidden");
  });
}

function bindTerminalCallbacks() {
  state.terminal = new TerminalView({
    scrollEl: el.terminalScroll,
    flowEl: el.terminalFlow,
    emptyEl: el.terminalEmpty,
    jumpBtn: el.jumpLatest,
    noticeEl: el.terminalNotice,
    noticeTextEl: el.terminalNoticeText,
    cardEl: terminalPane,
    onFollowChange: (follow) => {
      state.autoScroll = Boolean(follow);
      el.followToggle.checked = state.autoScroll;
      scheduleSettingsSave();
    },
    onTrim: ({ retainedBytes }) => {
      const log = activeLogStatus();
      let message = `终端仅保留最近 ${formatBytes(retainedBytes)} 数据，较早内容已从显示缓存移除。`;
      if (log && !log.error) {
        message += "完整记录仍在日志文件中。";
      } else if (state.connected) {
        message += "本次日志不可用，较早数据无法恢复。";
      }
      state.terminal?.setNotice(message);
    },
  });
}

async function initialize() {
  try {
    state.api = await createApi();
    state.isPreview = Boolean(state.api.isPreview);
    if (!state.isPreview) {
      // The window is created hidden to avoid a white flash. Show it now with
      // the boot cover still visible, instead of waiting for the page-load
      // event (unreliable on Windows) or the 3 s fallback.
      try {
        await state.api.invoke("show_main_window");
      } catch {
        // The backend fallback timer will still reveal the window.
      }
    }
    const boot = await state.api.invoke("bootstrap");
    state.version = String(boot.version || "");
    state.bootWarnings = Array.isArray(boot.warnings) ? boot.warnings.filter(Boolean) : [];
    applySettingsToForm(normalizeSettings(boot.settings));
    state.ports = Array.isArray(boot.ports) ? boot.ports : [];
    renderLogDir();
    applyPorts(state.ports);
    bindTerminalCallbacks();
    bindEvents();
    setInitialUiFromSettings();
    updatePortDetail();
    updateFormLocking();
    renderLogUi();
    updateConnectionUi();
    startPortAutoRefresh();

    if (state.bootWarnings.length > 0) {
      showNotice(state.bootWarnings.join(" "));
    }
    await new Promise((resolve) => requestAnimationFrame(resolve));
    hideBootCover();
    const previewAuto = new URLSearchParams(window.location.search).get("autoconnect") === "1";
    if (state.isPreview && previewAuto) {
      window.setTimeout(() => void openPort(), 500);
    }
  } catch (error) {
    showBootError(error);
  }
}

void initialize();
