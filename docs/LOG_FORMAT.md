# Serial Desk 日志格式

日志文件是 UTF-8 编码、CRLF 换行的文本文件，扩展名 `.log`。每一帧串口数据占一行，发送（TX）和接收（RX）都记录。

## 示例

```text
# Serial Desk session log
# created: 2026-10-19T12:02:05.559+08:00
# port: COM10
# serial: 115200 bps | 8-N-1 | flow: none
# format: utf-8 text, one frame per line
# escapes: \\ = backslash, \r, \n, \t, \xNN = raw byte
# note: control characters and invalid utf-8 are escaped; every payload byte is recoverable
# ---
2026-10-19 12:02:06.486 | TX     24 B | DSH-LOOP-20261019-B8E4\r\n
2026-10-19 12:02:06.521 | RX     24 B | DSH-LOOP-20261019-B8E4\r\n
2026-10-19 12:02:06.618 | TX      5 B | \x00\x01\x7F\x80\xFF
2026-10-19 12:02:06.627 | RX      5 B | \x00\x01\x7F\x80\xFF
# ---
# closed: 2026-10-19T12:02:11.319+08:00 | RX 29 B / 2 frames | TX 29 B / 2 frames
```

## 行结构

```text
<本地时间> | <RX|TX> <字节数> B | <转义后的数据>
```

- 本地时间：`YYYY-MM-DD HH:MM:SS.mmm`，毫秒精度。时区在文件头的 `created` 中给出。
- 方向：`RX` 或 `TX`。
- 字节数：这一帧负载的精确字节数。
- 数据：同一帧原始字节的可逆转义表示。

以 `#` 开头的行是元数据。第一个 `# ---` 之后是数据，第二个 `# ---` 之后是结束汇总。

## 转义规则

| 文本中的形式 | 还原出的字节 |
|---|---|
| `\\` | `0x5C`（反斜杠） |
| `\r` | `0x0D` |
| `\n` | `0x0A` |
| `\t` | `0x09` |
| `\xNN` | 单个字节 `0xNN`，大写十六进制 |
| 其他字符 | 该字符的 UTF-8 编码字节 |

说明：

- 合法的 UTF-8（包括中文）保持原样，便于阅读。
- 非法 UTF-8 字节逐字节写成 `\xNN`，不会丢失。
- 控制字符、双向文本控制符、零宽字符等会以 `\xNN` 或 Unicode 转义形式出现，避免日志被不可见字符伪装。
- 数据中的普通 `|` 不转义；解析时只按**前三个** ` | ` 分隔符拆分，剩余部分全部属于数据。
- 反斜杠一定被写成 `\\`，因此不会与转义序列混淆。

解码后的字节数必须等于行内记录的字节数，可用于自检。

## 还原脚本（Python）

```python
import re

LINE = re.compile(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}) \| (RX|TX) +(\d+) B \| (.*)$")


def unescape(text: str) -> bytes:
    output = bytearray()
    index = 0
    while index < len(text):
        char = text[index]
        if char != "\\":
            output.extend(char.encode("utf-8"))
            index += 1
            continue

        marker = text[index + 1]
        if marker == "\\":
            output.append(0x5C)
            index += 2
        elif marker == "r":
            output.append(0x0D)
            index += 2
        elif marker == "n":
            output.append(0x0A)
            index += 2
        elif marker == "t":
            output.append(0x09)
            index += 2
        elif marker == "x":
            output.append(int(text[index + 2:index + 4], 16))
            index += 4
        else:
            # 当前版本不会产生未知转义；保守地按字面保留。
            output.extend(("\\" + marker).encode("utf-8"))
            index += 2
    return bytes(output)


for raw_line in open("COM10_2026-10-19_12-02-05.log", encoding="utf-8"):
    line = raw_line.rstrip("\r\n")
    match = LINE.match(line)
    if not match:
        continue
    timestamp, direction, byte_count, encoded = match.groups()
    payload = unescape(encoded)
    assert len(payload) == int(byte_count), (line, len(payload), byte_count)
    print(timestamp, direction, len(payload), payload)
```

## 定时保存与落盘

- 每帧到达后立即写入文件（不在应用内累计 30 秒才写），所以打开日志文件通常能马上看到内容。
- 每 N 秒（用户设置，5–3600 秒）执行一次真正的磁盘同步（`sync_data`）。
- “立即保存”会立即同步。
- 断开串口、关闭主窗口时，先停止接收线程，再追加 `# closed` 汇总并同步关闭文件。
- 如果磁盘写入失败，界面会持续显示错误；此后的日志不再被视为完整。不要忽略该提示。
- 强制结束进程、系统断电或休眠时，最多可能丢失最后一个未同步周期的数据。正常关闭窗口不受此影响。

## 文件名

```text
<端口名>_<连接时间>.log
```

例如 `COM10_2026-10-19_12-02-05.log`。同一秒重名时追加 `_02`、`_03` 等后缀；不会覆盖已有日志。

## 用 CLI 读取

命令行工具 `sdesk` 使用同一份解析逻辑，不需要再写脚本处理转义：

```powershell
sdesk log list --json
sdesk log tail --latest --lines 40 --json
sdesk log show COM10_2026-10-19_12-02-05.log --direction RX --grep ERROR --json
```

`--json` 下每行一个帧对象，`text` 就是本文档中的转义文本，`hex` 是解码后的原始字节，`bytes` 为帧长度。两者可以互相校验。`sdesk capture` 写出的日志与本格式完全一致，GUI 与 CLI 可以互相读取。完整说明见 [CLI.md](CLI.md)。
