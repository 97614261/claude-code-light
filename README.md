# claude-code-light

> Claude Code 桌面状态指示灯，把会话状态做成屏幕角落的一个红绿灯小窗口。

![icon](icon.png)

## 这是啥

Claude Code 命令行用着用着，你不知道它在思考还是答完了在等你，得切回 terminal 看。这个工具把状态做成一个 64×64 的悬浮红绿灯：

| 灯 | 含义 | 语音提示 |
|---|---|---|
| 🟡 黄 | Claude 在思考/执行（运行中） | "让我想想" |
| 🔴 红 | 等你回应（AskUserQuestion / 权限对话框） | "你看一下" |
| 🟢 绿 | 答完了 | "好啦" |

整个程序约 4.5 MB，**单 exe 可执行，不需要 Node.js / Python / 任何外部运行时**（Windows 自带 WebView2 即可）。

## 特点

- ⚡ 体积小：单 exe ~4.5 MB（对比 Electron 版本动辄 150 MB）
- 🎯 状态精准：通过 Claude Code 原生 hook 系统驱动
- 🪟 始终置顶：钉在屏幕右上角不挡视线
- 🖱️ 可拖拽：左键拖到任意位置（含副屏），位置自动记忆，副屏拔了自动回主屏
- 🔇 静音可控：右下齿轮里切静音 / 主题
- 🌗 深/浅主题、单灯/三灯样式可切
- 📊 使用统计：托盘菜单看今天/本周的 token 用量（直接读 `~/.claude/projects/` 的 session 文件）

## 安装

### 方式一：下载预编译版（推荐）

去 [Releases](../../releases) 下载最新的 `CC红绿灯.exe`，双击运行。Windows 11 自带 WebView2，零依赖。

首次运行自动注入 hooks 到 `~/.claude/settings.json`。如果 Claude Code 在跑，重启它使 hooks 生效。

### 方式二：从源码构建

需要：
- [Rust toolchain](https://rustup.rs/)
- [Visual Studio Build Tools 2022](https://visualstudio.microsoft.com/visual-cpp-build-tools/) 的 "C++ 桌面开发" 工作负载
- [Node.js 20+](https://nodejs.org/)

```bash
git clone https://github.com/<your-username>/claude-code-light.git
cd claude-code-light
npm install
.\build-portable.bat
```

产物：`src-tauri/target/release/cc-traffic-light.exe`

## 使用

1. 双击 `CC红绿灯.exe`，红绿灯出现在屏幕右上角
2. 首次启动会自动往 `~/.claude/settings.json` 注入 5 条 hooks
3. 重启 Claude Code（如果在跑）
4. 跟 Claude 聊天就能看到灯色变化 + 听到语音

**托盘右键菜单**：手动切换灯色 / 切换 1 或 3 灯样式 / 切换主题 / 重置窗口位置 / 看使用统计 / 重写配置 / 退出。

## Hook 详情

工具往 `~/.claude/settings.json` 注入这 5 条：

| 事件 | matcher | 写入颜色 |
|---|---|---|
| `UserPromptSubmit` | — | yellow |
| `PreToolUse` | `AskUserQuestion` | red |
| `PostToolUse` | `.*` | yellow |
| `PermissionRequest` | `.*` | red |
| `Stop` | — | green |

每条都是简单的 `echo color > $HOME/.claude/cc_traffic_light_state`，由 bash 执行。Rust 后端 300ms 轮询这个文件，变化时刷新窗口 + 播放语音。

如果你已经有自定义 hook，工具**只追加不覆盖**——它会检测每个事件下是否已经有红绿灯相关 hook，有就跳过。

想完全重写配置：托盘菜单 → "重新写入配置"（会清掉旧的红绿灯 hook 再加新的）。

## 改语音

文案 / 声线在 `gen-tts.mjs`：

```js
const lines = {
  red:    "你看一下",
  yellow: "让我想想",
  green:  "好啦",
};
```

跑 `node gen-tts.mjs` 重新生成 3 个 MP3 → `.\build-portable.bat` 烧进新 exe。

## 技术栈

- [**Tauri 2**](https://tauri.app/) — 跨平台桌面壳，比 Electron 小一个数量级
- **Rust** — 后端 / hook 注入 / 状态轮询 / 系统托盘 / 音频播放
- **TypeScript + Vite** — 前端（< 200 行原生 TS，无框架）
- **rodio** — MP3 解码 + 跨平台音频输出
- **Edge TTS** — 仅生成阶段用，合成中文语音

## 项目结构

```
claude-code-light/
├── src/                # 前端 (TS + CSS)
│   ├── main.ts
│   ├── tauri-api.ts    # IPC shim
│   └── styles/index.css
├── src-tauri/          # Rust 后端
│   ├── src/main.rs
│   ├── icons/          # 应用图标
│   ├── sounds/         # 嵌入的 MP3 语音
│   ├── tauri.conf.json
│   └── Cargo.toml
├── public/             # 静态资源 (dog.gif 等)
├── index.html          # Vite 入口
├── gen-tts.mjs         # 重新生成中文语音的脚本
├── icon.png            # 图标源
└── build-portable.bat  # 一键打包脚本
```

## 跨平台支持

| 平台 | 状态 |
|---|---|
| Windows 11 + Git Bash | ✅ 主测试平台 |
| Windows 11 + 仅 WSL bash | ⚠️ Hook 的 `$HOME` 解析到 Linux 子系统，建议装 [Git for Windows](https://git-scm.com/download/win) |
| macOS | 🧪 代码层面支持，未实测 |
| Linux | 🧪 同上 |

## 鸣谢

灵感来自 [Claude-Code-Traffic-Light-Prompt](https://github.com/freed85-xiaozai/Claude-Code-Traffic-Light-Prompt) by 张顽心。本项目是基于 Tauri 2 的重写版本，体积更小（150 MB → 4.5 MB）。

## License

[MIT](LICENSE)
