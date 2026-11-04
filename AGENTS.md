# AGENTS.md — 串口日志与设备操作

本仓库提供一个 Windows 串口工具。做嵌入式开发或排查设备问题时，**不要自己解析 `.log`**
文件，也不要猜测串口参数，直接使用命令行工具。

在仓库根目录下，可执行文件是 `.\release\sdesk.exe`（刚构建的版本在
`.\src-tauri\target\release\sdesk.exe`）。如果已经把 `release` 加入 PATH，可以把命令里的
路径省略成 `sdesk`。完整参考：[docs/CLI.md](docs/CLI.md)。

## 读取日志（最常用）

```powershell
.\release\sdesk.exe log path --json                    # 日志目录
.\release\sdesk.exe log list --json                    # 文件、端口、帧数、是否正常关闭
.\release\sdesk.exe log tail --latest --lines 60 --json
.\release\sdesk.exe log show --latest --grep ERROR --ignore-case --json
```

- 输出是 JSONL，每行一个帧对象；`text` 是可读文本，`hex` 是精确字节，`bytes` 是帧长度。
- `--json` 下 stdout 只有数据，提示和错误在 stderr。
- 退出码：`0` 成功，`1` 运行错误，`2` 参数错误，`3` 没有匹配的数据。
  退出码 3 表示“工具正常，但设备没有输出预期内容”，不要当成工具失败。

## 驱动设备（抓取 + 发送）

固定套路 A：后台 `capture`，用 `send` 写入同一个会话，结束后 `stop`。同一串口同一时间只能运行一个 capture。

```powershell
# 1. 后台抓取；默认写入与 GUI 相同的日志目录
.\release\sdesk.exe capture --port COM10 --json

# 2. 确认会话
.\release\sdesk.exe sessions --json

# 3. 发送
.\release\sdesk.exe send --port COM10 --data "AT+GMR\r\n"
.\release\sdesk.exe send --port COM10 --data-hex "01 03 00 00 00 01 84 0A"

# 4. 读取刚才的会话日志
.\release\sdesk.exe log tail --latest --lines 40 --json

# 5. 正常结束
.\release\sdesk.exe stop --port COM10
```

如果 `capture` 用了 `--log-dir .\logs`，读取时同样要加 `--dir .\logs`；否则 `--latest` 会去默认目录找。

固定套路 B：只等待某个输出（会独占串口，必须在 A 之前或 A 结束后使用）。

```powershell
.\release\sdesk.exe capture --port COM10 --quiet --until "boot complete" --timeout 30
# 退出码 0 = 匹配到；3 = 30 秒内没匹配到
```

## 规则

- 同一串口同一时间只能被一个进程打开。GUI 连接期间不要运行 `capture`；先断开 GUI。
  `log` 子命令是只读的，GUI 写日志时也可以读。
- 不要直接结束 `capture` 进程；用 `stop`，这样日志才会写入 `# closed` 结束标记。
- 文本按 UTF-8 处理。设备日志不是 UTF-8 时用 `--format hex` / `--data-hex`。
- `--data` 支持 `\r \n \t \xNN` 转义；HEX 必须是偶数位，非法输入会被拒绝而不是忽略。
- 打开串口会改变 DTR/RTS，可能使开发板复位；确认端口后再操作。
- `--grep` 匹配的是转义后的文本（`\r\n` 会显示为四个字符），不是原始字节。

## 修改本仓库代码时

- GUI 与 CLI 共用 `src-tauri/src/{serial,logging,settings}.rs`；改串口或日志逻辑时两边都会受影响。
- `serial.rs` 不能引用 Tauri，表现层通过 `FrameSink` 接入。
- 测试：`cd src-tauri; cargo test --release`；前端：`cd ui; node --test tests/format.test.js`。
