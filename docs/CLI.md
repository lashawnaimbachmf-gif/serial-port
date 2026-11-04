# Serial Desk CLI（`sdesk`）

`sdesk` 是 Serial Desk 的命令行界面，目标是让脚本和 AI 代理在**没有图形界面**的情况下：

1. 直接读取串口工具的会话日志；
2. 后台抓取串口数据并同时落盘；
3. 向正在抓取的串口发送数据，形成“发送 → 观察接收”的闭环；
4. 用稳定的 JSONL 和退出码判断结果，而不是解析给人看的文本。

CLI 与 GUI 共用同一套 Rust 串口核心、日志记录器和转义规则：

- `sdesk` 能读 GUI 写出的 `.log`，`SerialDesk.exe` 也能继续打开 CLI 写出的 `.log`；
- 字节处理、HEX 校验、转义、日志格式完全一致，不存在第二套实现；
- `sdesk.exe` 不链接 Tauri、不需要 WebView2，体积约 0.8 MB，可在无桌面会话的构建机上运行。

## 安装与构建

发布包中 `sdesk.exe` 与 `SerialDesk.exe` 放在同一目录即可，无需安装、无需管理员权限。

从源码构建：

```powershell
cd src-tauri
cargo build --release --bin sdesk
# 产物：src-tauri/target/release/sdesk.exe
```

## 30 秒上手

```powershell
sdesk ports                                  # 本机有哪些串口
sdesk capture --port COM10 --json            # 后台抓取，stdout 输出 JSONL
sdesk send --port COM10 --data "AT+GMR\r\n"  # 发给正在运行的 capture
sdesk stop --port COM10                      # 正常收尾并写入日志结束标记
sdesk log tail --latest --lines 30 --json    # 读取最新日志的最后 30 帧
```

## 输出约定

**stdout 只放数据，提示和错误一律走 stderr。** 因此 `sdesk ... --json > frames.jsonl` 得到的文件可以直接被程序读取，不会混入横幅或警告。

- `--json` 等价于 `--format json`；
- `ports`、`log list`、`sessions` 的 `--json` 输出一个 JSON 数组；
- `capture`、`log show`、`log tail` 的 `--json` 输出 **JSONL**：每行一个 JSON 对象；
- `--format text|json|hex|raw` 用于帧输出；`raw` 只把 RX 原始字节写进 stdout，其余信息走 stderr。
  `raw` 无法承载 `ERR` 帧，串口错误会写到 stderr。

### 退出码

| 码 | 含义 |
|---:|---|
| 0 | 成功 |
| 1 | 运行错误：串口不可用、日志文件不可读、stdout 中断等 |
| 2 | 参数错误：未知选项、取值非法、缺少必填项 |
| 3 | 串口/文件本身正常，但没有匹配的数据，或 `--until` 超时 |

退出码 3 是有意设计的：AI 可以把“没有收到预期日志”和“工具执行失败”区分开。

### 帧对象（frame）

`capture --json` 与 `log show/tail --json` 共用同一结构：

```json
{"type":"frame","source":"live","seq":7,"ts":"2026-10-19 13:33:51.284","tsMs":1792388031284,"dir":"TX","bytes":6,"text":"PING\\r\\n","hex":"50 49 4E 47 0D 0A"}
```

| 字段 | 说明 |
|---|---|
| `type` | 固定为 `frame` |
| `source` | `live`（实时抓取）或 `log`（从日志读回） |
| `seq` | 实时抓取的帧序号；读日志时为 `null` |
| `ts` | 本地墙上时间，毫秒精度，与日志行完全一致 |
| `tsMs` | Unix 毫秒时间戳，便于比较和排序 |
| `dir` | `RX`、`TX` 或 `ERR`（串口错误消息） |
| `bytes` | 该帧负载的精确字节数 |
| `text` | 可逆转义后的文本，与日志行中的内容相同；控制字符显示为 `\r`、`\n`、`\xNN` |
| `hex` | 同样的字节，以大写十六进制空格分隔 |

`text` 与 `hex` 描述的是**同一段字节**。需要精确字节时用 `hex`；需要阅读（尤其是中文）时用 `text`。解码 `text` 的规则见 [LOG_FORMAT.md](LOG_FORMAT.md)。

### 会话与汇总对象

`capture --json` 开始时输出一行 `session`，结束时输出一行 `summary`：

```json
{"type":"session","apiVersion":1,"port":"COM10","baudRate":115200,"framing":"8-N-1","flow":"none","logPath":"C:\\...\\COM10_2026-10-19_13-33-44.log","logIntervalSecs":5,"controlPort":53132,"pid":121124,"startedAtMs":1792388024975,"format":"json"}
{"type":"summary","reason":"closed","port":"COM10","untilMatched":false,"durationMs":19525,"rxBytes":0,"txBytes":27,"rxFrames":0,"txFrames":3,"log":{...},"error":null}
```

`summary.reason` 取值：`signal`（Ctrl+C）、`closed`（`sdesk stop`）、`until`（匹配到 `--until`）、`timeout`、`error`（串口中断）、`log-error`、`output`（stdout 写入失败）。

## 命令

### `sdesk ports`

```text
sdesk ports [--json]
```

列出本机串口。`--json` 输出 `[{"name":"COM10","detail":"USB-SERIAL CH340"}]`。

只有一个串口时，`capture` 和 `send` 都可以省略 `--port`；存在多个串口时会报错并要求显式指定，避免把命令发到错误的设备。

### `sdesk capture`

```text
sdesk capture [--port COM10] [串口参数] [日志与输出选项] [结束条件]
```

打开串口并把每一帧写到 stdout，同时按 GUI 相同的格式写日志。它会在 `%LOCALAPPDATA%\com.serialdesk.app\cli-sessions` 下登记一个会话，使另一个进程可以用 `sdesk send` 写入数据。

串口参数（默认 115200-8-N-1，无流控）：

```text
--port COM10
--baud 115200
--data-bits 5|6|7|8
--parity none|even|odd
--stop-bits 1|2
--flow none|hardware|software
```

日志：

```text
--log-dir DIR          默认：GUI 保存的目录，或 文档\Serial Desk\Logs
--log-interval SECS    磁盘同步间隔，默认 5（范围 5–3600）
--no-log               不写日志（只输出到 stdout）
```

输出：

```text
--format text|json|hex|raw
--json
--quiet                不输出帧，只保留 session/summary；日志照常记录
```

结束条件：

```text
--until TEXT           收到匹配的 RX 数据后结束；支持 \r \n \t \xNN
--timeout SECS         最长运行时间：30、500ms、2m
--no-control           不启动本地控制通道，之后无法用 sdesk send 写入
```

示例：

```powershell
# 后台抓取，JSONL 输出到文件；AI 之后可以读文件
sdesk capture --port COM10 --json --log-dir .\logs > frames.jsonl

# 等待设备启动完成，最多 30 秒；匹配到立即退出
sdesk capture --port COM10 --quiet --until "boot complete" --timeout 30
# 退出码：0 = 匹配到；3 = 30 秒内没匹配到

# 只抓最近一段并保持运行，按 Ctrl+C 收尾
sdesk capture --port COM10 --format text

# 纯落盘，不产生 stdout 流量
sdesk capture --port COM10 --quiet --timeout 10m
```

`--until` 只在 **RX** 数据中匹配，且能跨帧匹配（例如提示文字被拆成两次读取）。匹配发生在日志写入**之后**，因此匹配到的那一帧一定已经在日志里。

Ctrl+C 第一次会正常收尾（停止读取、写 `# closed`、断开串口）；如果驱动卡死，第二次 Ctrl+C 会立即终止进程。

### `sdesk send`

```text
sdesk send [--port COM10] (--data TEXT | --data-hex HEX | --file PATH)
           [--line-ending none|cr|lf|crlf] [--direct] [--json]
```

发送目标的选择顺序：

1. `--session FILE` 指定的会话；
2. 端口匹配的运行中 `capture` 会话；
3. 只有一个会话时可以省略 `--port`；
4. 没有会话时**直接打开串口**发送后关闭（等价于 `--direct`）。

数据来源三选一：

```text
--data "AT\r\n"          文本；支持 \\ \r \n \t \0 \xNN
--data-hex "41 54 0D 0A" HEX；只允许 0-9 A-F 和空白，必须偶数位
--file payload.bin       原始字节文件
```

其他选项：

```text
--line-ending crlf       在数据末尾追加结束符（对三种来源都生效）
--direct                 跳过会话，直接打开串口
--delay-ms 100           直接模式下打开串口后等待再写（设备复位后常用）
--baud / --data-bits / --parity / --stop-bits / --flow   直接模式的串口参数
```

示例：

```powershell
sdesk send --port COM10 --data "AT+GMR\r\n" --json
sdesk send --port COM10 --data-hex "01 03 00 00 00 01 84 0A"
sdesk send --port COM10 --file firmware-cmd.bin --line-ending none
```

直接打开串口会改变 DTR/RTS 电平，部分开发板会因此复位；只要 `capture` 正在运行，就不要用 `--direct`。

### `sdesk sessions`

```text
sdesk sessions [--json] [--prune]
```

列出正在运行的 capture 会话（端口、PID、波特率、日志路径、开始时间）。`--prune` 会删除进程已经退出的状态文件。

### `sdesk stop`

```text
sdesk stop --port COM10 [--json]
sdesk stop --all [--json]
sdesk stop [--json]              # 只有一个会话时
```

请求 capture 正常收尾：停止读线程、写入日志结束标记、关闭串口。比直接结束进程更安全。若会话状态文件已失效（进程崩溃），会顺手清理。

### `sdesk log path`

```text
sdesk log path [--dir DIR] [--json]
```

打印实际使用的日志目录。解析顺序与 GUI 一致：`--dir` → GUI 的 `settings.json` 中的 `logDir` → `%USERPROFILE%\Documents\Serial Desk\Logs`。

### `sdesk log list`

```text
sdesk log list [--dir DIR] [--port COM10] [--limit N] [--json]
```

列出日志文件，按修改时间倒序。文本输出：

```text
FILE                                         PORT    STARTED                   SIZE  FRAMES STATE
COM10_2026-10-19_13-33-44.log                COM10   2026-10-19 13:33:44      577 B       3 closed
```

`STATE` 为 `closed`（有结束标记）、`OPEN`（可能正在写入）或 `invalid(N)`（有 N 行无法解析）。JSON 字段：`file`、`path`、`port`、`serial`、`created`、`createdMs`、`size`、`frames`、`rxBytes`、`txBytes`、`closed`、`invalidLines`、`modifiedMs`。

### `sdesk log show`

```text
sdesk log show <FILE> [筛选] [--limit N] [--offset N] [--format ...] [--json]
sdesk log show --latest [筛选] ...
```

按日志中的顺序输出帧。筛选：

```text
--direction rx|tx|both
--rx / --tx
--since "2026-10-19 12:00:00"    也支持 "12:00:00" 和相对写法 "-5m"
--until "2026-10-19 12:10:00"
--grep TEXT                      在转义后的 text 中做子串匹配
--ignore-case
```

`--limit`、`--offset` 在筛选之后生效。没有匹配时退出码为 3。

```powershell
sdesk log show COM10_2026-10-19_12-02-05.log --direction RX --grep ERROR --json
sdesk log show --latest --since -2m --limit 200 --json
```

`--grep` 匹配的是 `text` 字段（例如 `\r\n` 会以四个字符出现），不是原始字节。需要按字节查找时，先配合 `--json` 用 `hex` 字段自行判断。

### `sdesk log tail`

```text
sdesk log tail [<FILE> | --latest] [--lines N] [--follow] [--timeout SECS] [筛选] [--json]
```

输出最后 N **帧**（默认 50，不是 N 行；日志末尾的 `# ---` / `# closed` 元数据不会占用配额）。`--follow` 持续跟踪新写入的行，直到 Ctrl+C 或 `--timeout`。

```powershell
sdesk log tail --latest --lines 40 --json
sdesk log tail --latest --follow --grep "panic" --json --timeout 60
```

`--follow` 的超时是正常结束，退出码为 0；非 follow 模式下没有任何帧时退出码为 3。

## AI 代理工作流

### 1. 读取 GUI 已经写下的日志

```powershell
sdesk log path --json                 # {"type":"logPath","path":"...","exists":true}
sdesk log list --json                 # 选一个文件
sdesk log tail --latest --lines 60 --json
sdesk log show --latest --grep "ERROR" --ignore-case --json
```

每条 JSONL 都带有 `ts`、`tsMs`、`dir`、`bytes`、`text`、`hex`，AI 不需要自己解析日志转义。

### 2. 后台抓取 + 发送，形成闭环

```powershell
# 终端 A（或后台作业）
sdesk capture --port COM10 --json --log-dir .\logs

# 终端 B
sdesk send --port COM10 --data "AT+CSQ\r\n"
sdesk log tail --latest --dir .\logs --lines 20 --json
sdesk stop --port COM10
```

`capture` 独占串口，`send` 通过本地控制通道进入同一个进程，因此不存在“串口被占用”的竞争。若省略 `--log-dir`，日志会写入默认目录，读取时也不必加 `--dir`。

### 3. 等待启动横幅

```powershell
sdesk capture --port COM10 --quiet --until "WiFi ready" --timeout 15 --json
if ($LASTEXITCODE -eq 0) { ... }   # 0 = 已匹配，3 = 超时
```

### 4. 保存原始字节

```powershell
sdesk capture --port COM10 --format raw > dump.bin
```

stdout 只有 RX 原始字节；`session` / `summary` 以文本形式写 stderr。

### 5. 出错时区分“没有数据”和“工具失败”

```text
退出码 1：串口打不开、日志文件读不了、stdout 被关闭
退出码 2：参数写错
退出码 3：工具执行成功，但没有匹配的帧/日志
```

## 会话控制通道

`capture` 在 `127.0.0.1` 上监听一个临时端口，并把端口号和随机令牌写入：

```text
%LOCALAPPDATA%\com.serialdesk.app\cli-sessions\<PORT>.json
```

- 只绑定回环地址，不接受外部连接；
- 每个请求必须携带状态文件中的随机令牌，令牌每次启动重新生成；
- 支持 `send`、`status`、`ping`、`close` 四个命令；协议是一行 JSON 请求、一行 JSON 响应；
- 进程正常退出（包括 `stop` 和 Ctrl+C）时会删除状态文件；
- 进程被强制结束时会留下状态文件，`sdesk sessions --prune` 或 `sdesk stop` 会清理。

如果不需要被 `send` 写入，可以用 `--no-control` 完全关闭该通道。

## 与 GUI 的关系

- 同一个串口同一时间只能被一个进程打开。GUI 连接期间，`sdesk capture` 会明确报错；需要后台抓取时请先断开 GUI。
- `sdesk log` 是只读的，可以在 GUI 正在写日志时使用。
- CLI 写出的日志与 GUI 完全相同，GUI 可以直接在日志目录里看到它们。
- CLI 的文本输出格式与日志行格式一致，复制到日志里也不会破坏结构。

## 限制与注意事项

- 文本收发按 UTF-8 处理；设备使用 GBK 或其他编码时，用 `--format hex` / `--data-hex`，不要依赖文本替换符。
- 打开串口可能改变 DTR/RTS 电平，导致开发板复位。
- `--grep` 匹配的是转义后的文本，不是原始字节。
- 单帧上限 64 KiB；单次发送上限 1 MiB。
- `capture` 会先写日志再写 stdout。stdout 阻塞（例如下游不读取管道）会反压读线程，但已接收的数据在日志中仍然完整；需要纯落盘时用 `--quiet` 或 `--no-control --quiet`。
- Windows 控制台已设置为 UTF-8（代码页 65001）；重定向到文件时输出为 UTF-8 字节。
- 第一次 Ctrl+C 正常收尾，第二次强制终止。
