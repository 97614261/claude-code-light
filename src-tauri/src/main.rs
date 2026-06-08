#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Duration as ChronoDur, Local};
use serde_json::{json, Value};
use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WindowEvent};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

// =============== 路径常量 ===============

fn home() -> PathBuf { dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")) }

/// 状态/主题/静音/样式文件的存放目录。
/// 三平台统一用 ~/.claude/，跟 Claude Code 自己的配置同目录，也跟我们注入的
/// hook 命令 `echo X > $HOME/.claude/cc_traffic_light_state` 完全对齐。
fn tmp_dir() -> PathBuf {
    home().join(".claude")
}

fn state_file() -> PathBuf { tmp_dir().join("cc_traffic_light_state") }
fn theme_file() -> PathBuf { tmp_dir().join("cc_traffic_light_theme") }
fn mute_file()  -> PathBuf { tmp_dir().join("cc_traffic_light_mute") }
fn style_file() -> PathBuf { tmp_dir().join("cc_traffic_light_style") }
fn stats_file() -> PathBuf { home().join(".claude").join("cc_traffic_light_stats.json") }
fn pos_file()   -> PathBuf { std::env::temp_dir().join("cc_traffic_light_pos") }

const TRAY_PNG: &[u8] = include_bytes!("../icons/tray.png");

// =============== 配置 IO ===============

fn read_str(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_theme() -> String {
    match read_str(&theme_file()).as_deref() {
        Some("light") => "light".into(),
        _ => "dark".into(),
    }
}

fn read_mute() -> bool {
    read_str(&mute_file()).as_deref() == Some("true")
}

// =============== 声音 ===============
// 用 Edge-TTS 预生成的中文语音 MP3，编译时嵌入二进制；
// 走 rodio + cpal，绕开浏览器自动播放策略，零交互即可发声。

const RED_MP3:    &[u8] = include_bytes!("../sounds/red.mp3");
const YELLOW_MP3: &[u8] = include_bytes!("../sounds/yellow.mp3");
const GREEN_MP3:  &[u8] = include_bytes!("../sounds/green.mp3");

fn play_sound(state: &str) {
    if read_mute() { return; }
    let bytes: &'static [u8] = match state {
        "red"    => RED_MP3,
        "yellow" => YELLOW_MP3,
        "green"  => GREEN_MP3,
        _ => return,
    };
    // 单开线程：解码 + 播放都是同步阻塞操作，不能挡轮询
    std::thread::spawn(move || {
        use rodio::{Decoder, OutputStream, Sink};
        use std::io::Cursor;
        if let Ok((_stream, handle)) = OutputStream::try_default() {
            if let Ok(sink) = Sink::try_new(&handle) {
                if let Ok(src) = Decoder::new(Cursor::new(bytes)) {
                    sink.append(src);
                    sink.sleep_until_end();
                }
            }
        }
    });
}

fn read_style() -> String {
    match read_str(&style_file()).as_deref() {
        Some("single") => "single".into(),
        _ => "triple".into(),
    }
}

// =============== 使用统计 ===============

fn today_key() -> String { Local::now().format("%Y-%m-%d").to_string() }

fn read_stats() -> Value {
    fs::read_to_string(stats_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_stats(stats: &Value) {
    if let Some(p) = stats_file().parent() {
        let _ = fs::create_dir_all(p);
    }
    if let Ok(s) = serde_json::to_string_pretty(stats) {
        let _ = fs::write(stats_file(), s);
    }
}

fn record_state_change(new_state: &str, prev_state: &str, red_start: Option<u128>) {
    let key = today_key();
    let mut stats = read_stats();
    let mut day = stats
        .get(&key)
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let get_u = |day: &serde_json::Map<String, Value>, k: &str| -> u64 {
        day.get(k).and_then(|v| v.as_u64()).unwrap_or(0)
    };
    if new_state == "red" {
        let c = get_u(&day, "redCount") + 1;
        day.insert("redCount".into(), json!(c));
    } else if new_state == "green" {
        let c = get_u(&day, "greenCount") + 1;
        day.insert("greenCount".into(), json!(c));
        if prev_state == "red" {
            if let Some(start) = red_start {
                let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
                let dur = (now_ms.saturating_sub(start)) as u64;
                let d = get_u(&day, "redDuration") + dur;
                day.insert("redDuration".into(), json!(d));
            }
        }
    }
    stats[&key] = Value::Object(day);
    save_stats(&stats);
}

// ---- 官方 API 用量统计：扫 ~/.claude/projects/*/*.jsonl 里的 assistant 消息 ----

#[derive(Default, Clone, Debug)]
struct TokenStats {
    calls: u32,      // assistant 消息次数 ≈ API 调用次数
    input: u64,      // 输入 token（含缓存读 + 缓存创建）
    output: u64,     // 输出 token
    cache_hit: u64,  // 缓存命中（已计入 input）
}

impl TokenStats {
    fn add(&mut self, o: &Self) {
        self.calls += o.calls;
        self.input += o.input;
        self.output += o.output;
        self.cache_hit += o.cache_hit;
    }
}

/// 扫近 8 天的 Claude session 文件，按日期分桶
fn collect_claude_token_usage() -> std::collections::HashMap<String, TokenStats> {
    use std::collections::HashMap;
    let mut buckets: HashMap<String, TokenStats> = HashMap::new();
    let root = home().join(".claude").join("projects");
    if !root.exists() { return buckets; }
    let cutoff = SystemTime::now().checked_sub(Duration::from_secs(8 * 86400));

    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() { stack.push(p); continue; }
            if p.extension().and_then(|x| x.to_str()) != Some("jsonl") { continue; }
            if let (Some(c), Ok(meta)) = (cutoff, e.metadata()) {
                if let Ok(mtime) = meta.modified() {
                    if mtime < c { continue; }
                }
            }
            let Ok(content) = fs::read_to_string(&p) else { continue };
            for line in content.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                if v.get("type").and_then(|t| t.as_str()) != Some("assistant") { continue; }
                let Some(msg) = v.get("message") else { continue };
                if msg.get("model").and_then(|m| m.as_str()) == Some("<synthetic>") { continue; }
                let Some(usage) = msg.get("usage") else { continue };

                let input        = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let output       = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let cache_read   = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let cache_create = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                if input + output + cache_read + cache_create == 0 { continue; }

                let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) else { continue };
                let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) else { continue };
                let key = dt.with_timezone(&Local).format("%Y-%m-%d").to_string();
                let s = buckets.entry(key).or_default();
                s.calls += 1;
                s.input += input + cache_read + cache_create;
                s.output += output;
                s.cache_hit += cache_read;
            }
        }
    }
    buckets
}

fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let bytes: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 { out.push(','); }
        out.push(*c);
    }
    out
}

fn fmt_token_stats(s: &TokenStats) -> String {
    let pct = if s.input > 0 { s.cache_hit * 100 / s.input } else { 0 };
    format!(
        "API 调用：{} 次\n输入：{} tokens\n输出：{} tokens\n缓存命中：{} tokens ({}%)",
        s.calls,
        fmt_num(s.input),
        fmt_num(s.output),
        fmt_num(s.cache_hit),
        pct
    )
}

// =============== Claude Code hooks 注入 ===============

fn claude_settings_path() -> PathBuf {
    let mut candidates = vec![home().join(".claude").join("settings.json")];
    if cfg!(windows) {
        if let Ok(appdata) = std::env::var("APPDATA") {
            candidates.push(PathBuf::from(&appdata).join("Claude Code").join("settings.json"));
            candidates.push(PathBuf::from(&appdata).join("Claude").join("settings.json"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates.push(PathBuf::from(&local).join("Claude Code").join("settings.json"));
            candidates.push(PathBuf::from(&local).join("AnthropicClaude").join("settings.json"));
        }
    }
    for p in &candidates {
        if p.exists() { return p.clone(); }
    }
    candidates.into_iter().next().unwrap()
}

/// 我们的 hook 三元组 (event, matcher, color)
///
/// A 方案（符合交通灯标准）：
///   🟡 黄 = Claude 在思考/执行（运行中）
///   🔴 红 = 等用户回应（需要你介入）
///   🟢 绿 = 完成
fn hooks_spec() -> [(&'static str, Option<&'static str>, &'static str); 5] {
    [
        ("UserPromptSubmit",  None,                    "yellow"),
        ("Stop",              None,                    "green"),
        // Claude 用 AskUserQuestion 问你 → 切红等回应
        ("PreToolUse",        Some("AskUserQuestion"), "red"),
        // 任何工具完成都回黄（含 AskUserQuestion 答完后 Claude 继续思考）
        ("PostToolUse",       Some(".*"),              "yellow"),
        // PermissionRequest：弹"Do you want to proceed?"时变红，提示用户该处理
        // 副作用：每次工具的权限检查都会触发（PostToolUse 紧跟着补回黄），用户能接受
        ("PermissionRequest", Some(".*"),              "red"),
    ]
}

fn state_cmd(color: &str) -> String {
    format!(r#"echo {} > "$HOME/.claude/cc_traffic_light_state""#, color)
}

/// 检查某个事件下是否已经有任意含红绿灯魔法字符串的 hook
fn has_cc_hook(arr: &[Value]) -> bool {
    arr.iter().any(|h| {
        h.get("hooks").and_then(|h| h.as_array()).map_or(false, |inner| {
            inner.iter().any(|hh| {
                hh.get("command").and_then(|c| c.as_str())
                    .map_or(false, |c| c.contains("cc_traffic_light_state"))
            })
        })
    })
}

/// 启动时调用：纯增量注入。只在某事件 *完全没有* 红绿灯 hook 时才加。
/// 不动用户的其他字段、不清理任何东西。
fn setup_claude_hooks() -> PathBuf {
    let path = claude_settings_path();

    let mut settings: Value = match fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        Some(v) if v.is_object() => v,
        _ => json!({}),
    };
    if !settings.get("hooks").map(|h| h.is_object()).unwrap_or(false) {
        settings["hooks"] = json!({});
    }

    let mut changed = false;
    {
        let hooks = settings["hooks"].as_object_mut().unwrap();
        for (event, matcher, color) in hooks_spec().iter() {
            // 已经有红绿灯 hook（不管是哪版）就完全不动，避免覆盖用户修改
            let already = hooks.get(*event).and_then(|v| v.as_array())
                .map_or(false, |arr| has_cc_hook(arr));
            if already { continue; }

            let mut entry = json!({
                "hooks": [{ "type": "command", "command": state_cmd(color) }]
            });
            if let Some(m) = matcher { entry["matcher"] = json!(m); }
            hooks.entry(event.to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut().unwrap()
                .push(entry);
            changed = true;
        }
    }

    if changed {
        if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
        if let Ok(s) = serde_json::to_string_pretty(&settings) {
            let _ = fs::write(&path, s);
        }
    }
    path
}

/// 用户主动点"重新写入配置"时调用：先清掉所有红绿灯相关 hook，再写一份新的。
/// 这是唯一会"破坏"既有 hook 的入口，所以单独成函数。
fn rewrite_claude_hooks() -> PathBuf {
    let path = claude_settings_path();
    let events_to_clean = ["UserPromptSubmit", "Stop", "PreToolUse", "PostToolUse",
                           "PermissionRequest", "Notification", "Elicitation"];

    let mut settings: Value = match fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        Some(v) if v.is_object() => v,
        _ => json!({}),
    };
    if !settings.get("hooks").map(|h| h.is_object()).unwrap_or(false) {
        settings["hooks"] = json!({});
    }

    {
        let hooks = settings["hooks"].as_object_mut().unwrap();
        // 清掉所有含红绿灯魔法字符串的 hook 条目
        for event in events_to_clean.iter() {
            if let Some(arr) = hooks.get(*event).and_then(|v| v.as_array()).cloned() {
                let filtered: Vec<Value> = arr.into_iter().filter(|h| {
                    !h.get("hooks").and_then(|h| h.as_array()).map_or(false, |inner| {
                        inner.iter().any(|hh| {
                            hh.get("command").and_then(|c| c.as_str())
                                .map_or(false, |c| c.contains("cc_traffic_light_state"))
                        })
                    })
                }).collect();
                if filtered.is_empty() { hooks.remove(*event); }
                else { hooks.insert(event.to_string(), Value::Array(filtered)); }
            }
        }
        // 再加新的
        for (event, matcher, color) in hooks_spec().iter() {
            let mut entry = json!({
                "hooks": [{ "type": "command", "command": state_cmd(color) }]
            });
            if let Some(m) = matcher { entry["matcher"] = json!(m); }
            hooks.entry(event.to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut().unwrap()
                .push(entry);
        }
    }

    if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
    if let Ok(s) = serde_json::to_string_pretty(&settings) {
        let _ = fs::write(&path, s);
    }
    path
}

// =============== IPC 命令 ===============

#[tauri::command] fn get_theme() -> String { read_theme() }

#[tauri::command]
fn set_theme(app: AppHandle, theme: String) {
    let _ = fs::write(theme_file(), &theme);
    let _ = app.emit("theme-change", &theme);
    update_tray_menu(&app);
}

#[tauri::command] fn get_style() -> String { read_style() }

#[tauri::command]
fn set_style(app: AppHandle, style: String) {
    let _ = fs::write(style_file(), &style);
    let _ = app.emit("style-change", &style);
    update_tray_menu(&app);
}

#[tauri::command] fn get_mute() -> bool { read_mute() }

#[tauri::command]
fn set_mute(muted: bool) {
    let _ = fs::write(mute_file(), if muted { "true" } else { "false" });
}

#[tauri::command]
fn set_state(state: String) { let _ = fs::write(state_file(), state); }

#[tauri::command]
fn get_state() -> String {
    fs::read_to_string(state_file()).ok()
        .map(|s| s.trim().to_string())
        .filter(|s| s == "red" || s == "yellow" || s == "green")
        .unwrap_or_else(|| "green".into())
}

#[tauri::command]
fn focus_app(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") { let _ = w.set_focus(); }
}

/// 手动启动窗口拖动，替代 CSS 的 -webkit-app-region: drag
/// 主要好处：右键不再被 Windows 当标题栏处理、不弹系统菜单
#[tauri::command]
fn start_drag(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") { let _ = w.start_dragging(); }
}

#[tauri::command]
fn set_window_height(app: AppHandle, h: u32) {
    if let Some(w) = app.get_webview_window("main") {
        // 必须用 LogicalSize：tauri.conf.json 里 width/height 是逻辑像素，
        // 用 PhysicalSize 会在高 DPI 屏（笔记本 125%/150%）上把窗口缩小
        let _ = w.set_size(LogicalSize::new(76, h));
    }
}

#[tauri::command] fn quit(app: AppHandle) { app.exit(0); }

// =============== 托盘菜单 ===============

fn build_tray_menu(app: &AppHandle, theme: &str, style: &str) -> tauri::Result<Menu<tauri::Wry>> {
    let style_label = if style == "single" { "切换到三灯样式" } else { "切换到单灯样式" };
    let theme_label = if theme == "dark"   { "切换浅色模式"  } else { "切换深色模式"  };
    MenuBuilder::new(app)
        .text("light:red",     "🔴  切换到红灯")
        .text("light:yellow",  "🟡  切换到黄灯")
        .text("light:green",   "🟢  切换到绿灯")
        .separator()
        .text("style:toggle",  style_label)
        .separator()
        .text("config:view",    "查看配置路径")
        .text("config:rewrite", "重新写入配置")
        .text("config:copy",    "复制手动配置")
        .separator()
        .text("theme:toggle",  theme_label)
        .separator()
        .text("window:reset",  "重置窗口位置")
        .text("report:week",   "使用统计")
        .separator()
        .text("quit",          "退出")
        .build()
}

fn update_tray_menu(app: &AppHandle) {
    let theme = read_theme();
    let style = read_style();
    if let Some(tray) = app.tray_by_id("main") {
        if let Ok(menu) = build_tray_menu(app, &theme, &style) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

// =============== 窗口位置持久化 ===============

fn read_pos() -> Option<(i32, i32)> {
    let s = fs::read_to_string(pos_file()).ok()?;
    let parts: Vec<&str> = s.trim().split(',').collect();
    if parts.len() != 2 { return None; }
    let x: i32 = parts[0].parse().ok()?;
    let y: i32 = parts[1].parse().ok()?;
    // 拒绝离谱坐标（Windows 最小化标记 -32000，或多屏负值过大）
    if x < -10000 || y < -10000 || x > 20000 || y > 20000 { return None; }
    Some((x, y))
}

fn write_pos(x: i32, y: i32) {
    let _ = fs::write(pos_file(), format!("{},{}", x, y));
}

/// 主显示器右上角，作为兜底/重置目标。
/// 按 DPI 缩放因子算窗口实际像素宽，避免 150%/175% 屏幕上跑出可见区。
fn default_window_pos(win: &tauri::WebviewWindow) -> (i32, i32) {
    if let Ok(Some(m)) = win.primary_monitor() {
        let pos = m.position();
        let size = m.size();
        let scale = win.scale_factor().unwrap_or(1.0);
        // 窗口逻辑宽 76，物理宽 = 76 * scale；右侧再留 20 物理像素余量
        let win_w_physical = (76.0 * scale).ceil() as i32;
        let x = pos.x + size.width as i32 - win_w_physical - 20;
        let y = pos.y + 60;
        return (x, y);
    }
    (1800, 60)
}

/// 检查 (x, y) 是否落在任何一块当前可见的显示器内（多屏拔了后用来过滤无效位置）
fn pos_on_any_monitor(win: &tauri::WebviewWindow, x: i32, y: i32) -> bool {
    win.available_monitors().ok().map_or(false, |monitors| {
        monitors.iter().any(|m| {
            let pos = m.position();
            let size = m.size();
            // 容忍少量边界偏差，但要求至少 50 像素落在显示器范围内
            x >= pos.x - 10 && x < pos.x + size.width as i32 - 50 &&
            y >= pos.y - 10 && y < pos.y + size.height as i32 - 50
        })
    })
}

// =============== 托盘事件处理 ===============

fn handle_tray_event(app: &AppHandle, id: &str) {
    match id {
        "light:red"    => { let _ = fs::write(state_file(), "red"); }
        "light:yellow" => { let _ = fs::write(state_file(), "yellow"); }
        "light:green"  => { let _ = fs::write(state_file(), "green"); }
        "style:toggle" => {
            let next = if read_style() == "single" { "triple" } else { "single" };
            let _ = fs::write(style_file(), next);
            let _ = app.emit("style-change", next);
            update_tray_menu(app);
        }
        "theme:toggle" => {
            let next = if read_theme() == "dark" { "light" } else { "dark" };
            let _ = fs::write(theme_file(), next);
            let _ = app.emit("theme-change", next);
            update_tray_menu(app);
        }
        "config:view" => {
            let p = claude_settings_path();
            let s = p.display().to_string();
            let app2 = app.clone();
            let s2 = s.clone();
            app.dialog()
                .message(format!(
                    "Hooks 已写入以下文件：\n{}\n\n如果红绿灯不响应，请确认 Claude Code 读取的是这个文件。",
                    s
                ))
                .title("CC 红绿灯 — 配置路径")
                .kind(MessageDialogKind::Info)
                .buttons(MessageDialogButtons::OkCancelCustom("打开文件".into(), "关闭".into()))
                .show(move |ok| {
                    if ok { let _ = app2.opener().open_path(s2, None::<&str>); }
                });
        }
        "config:rewrite" => {
            let p = rewrite_claude_hooks();
            update_tray_menu(app);
            app.dialog()
                .message(format!("配置已重新写入\n{}", p.display()))
                .title("CC 红绿灯")
                .kind(MessageDialogKind::Info)
                .show(|_| {});
        }
        "config:copy" => {
            // 构造一个独立的 settings.json 片段（完整 hooks 对象 + 顶层大括号）
            // 用户可以直接合并到自己的 settings.json，比裸键值对更不易出错
            let mut hooks_obj = serde_json::Map::new();
            for (event, matcher, color) in hooks_spec().iter() {
                let mut entry = json!({
                    "hooks": [{ "type": "command", "command": state_cmd(color) }]
                });
                if let Some(m) = matcher { entry["matcher"] = json!(m); }
                hooks_obj.insert(event.to_string(), json!([entry]));
            }
            let snippet_json = json!({ "hooks": Value::Object(hooks_obj) });
            let snippet = serde_json::to_string_pretty(&snippet_json).unwrap_or_default();

            let _ = app.clipboard().write_text(snippet);
            let p = claude_settings_path();
            let s = p.display().to_string();
            let app2 = app.clone();
            let s2 = s.clone();
            app.dialog()
                .message(format!(
                    "已复制完整 hooks 配置到剪贴板。\n\n用法：打开 settings.json，把其中的 \"hooks\" 整段替换为剪贴板内容里的 \"hooks\" 部分（如果原来没有 hooks，就把这段添加到顶层大括号内）。其他字段（env / model 等）保持原样。\n\n配置文件路径：{}",
                    s
                ))
                .title("CC 红绿灯")
                .kind(MessageDialogKind::Info)
                .buttons(MessageDialogButtons::OkCancelCustom("打开配置文件".into(), "关闭".into()))
                .show(move |ok| {
                    if ok { let _ = app2.opener().open_path(s2, None::<&str>); }
                });
        }
        "report:week" => {
            // 现扫 ~/.claude/projects/**/*.jsonl 提取真实的 API 用量
            let buckets = collect_claude_token_usage();
            let today_k = today_key();
            let today_stats = buckets.get(&today_k).cloned().unwrap_or_default();

            let today = Local::now().date_naive();
            let weekday = today.weekday().number_from_monday();
            let mut week_stats = TokenStats::default();
            for i in 0..weekday {
                let d = today - ChronoDur::days(i as i64);
                let key = d.format("%Y-%m-%d").to_string();
                if let Some(s) = buckets.get(&key) { week_stats.add(s); }
            }

            app.dialog()
                .message(format!(
                    "【今天】\n{}\n\n【本周】\n{}\n\n（数据来源：~/.claude/projects/）",
                    fmt_token_stats(&today_stats),
                    fmt_token_stats(&week_stats),
                ))
                .title("使用统计")
                .kind(MessageDialogKind::Info)
                .show(|_| {});
        }
        "window:reset" => {
            if let Some(win) = app.get_webview_window("main") {
                let (x, y) = default_window_pos(&win);
                let _ = win.set_position(PhysicalPosition::new(x, y));
                write_pos(x, y);
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

// =============== main ===============

fn main() {
    // 启动时强制把 hook 写一遍（兼容旧版本残留）
    let _ = setup_claude_hooks();
    let _ = fs::create_dir_all(tmp_dir());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_theme, set_theme, get_style, set_style,
            get_mute, set_mute, get_state, set_state, focus_app,
            start_drag, set_window_height, quit,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // === 窗口位置 ===
            if let Some(win) = app.get_webview_window("main") {
                let default_xy = default_window_pos(&win);
                // 存的坐标必须落在当前任意一块显示器上，否则丢弃用默认值
                // （副屏拔掉的情形下，老坐标会指向不存在的显示器）
                let (x, y) = read_pos()
                    .filter(|(x, y)| pos_on_any_monitor(&win, *x, *y))
                    .unwrap_or(default_xy);
                let _ = win.set_position(PhysicalPosition::new(x, y));

                win.on_window_event(|evt| {
                    if let WindowEvent::Moved(pos) = evt {
                        // 过滤 Windows 最小化时的伪坐标 (-32000, -32000)
                        // 以及任何明显离谱的负值
                        if pos.x > -10000 && pos.y > -10000 {
                            write_pos(pos.x, pos.y);
                        }
                    }
                });
            }

            // === 托盘 ===
            let theme0 = read_theme();
            let style0 = read_style();
            let menu = build_tray_menu(app.handle(), &theme0, &style0)?;
            let icon = Image::from_bytes(TRAY_PNG)?;
            TrayIconBuilder::with_id("main")
                .icon(icon)
                .tooltip("CC 红绿灯")
                .menu(&menu)
                .on_menu_event(|app, event| handle_tray_event(app, event.id.as_ref()))
                .build(app)?;

            // === 状态文件轮询 ===
            std::thread::spawn(move || {
                // 用当前状态文件内容做 last 的初值：避免启动时把已有状态再 emit 一次
                // （否则前端会因首次 state-change 而播放声音）
                let mut last = fs::read_to_string(state_file())
                    .ok().map(|s| s.trim().to_string()).unwrap_or_default();
                let mut red_start: Option<u128> = None;
                loop {
                    if let Ok(s) = fs::read_to_string(state_file()) {
                        let s = s.trim().to_string();
                        if !s.is_empty() && s != last {
                            record_state_change(&s, &last, red_start);
                            red_start = if s == "red" {
                                Some(SystemTime::now()
                                    .duration_since(UNIX_EPOCH).unwrap().as_millis())
                            } else { None };
                            last = s.clone();
                            let _ = handle.emit("state-change", &s);
                            play_sound(&s);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
