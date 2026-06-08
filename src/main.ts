import "./tauri-api";
import "./styles/index.css";

type Light = "red" | "yellow" | "green";
type Theme = "dark" | "light";
type Style = "triple" | "single";

declare global {
  interface Window {
    electronAPI: {
      onStateChange: (cb: (s: string) => void) => () => void;
      onThemeChange: (cb: (t: string) => void) => () => void;
      onStyleChange: (cb: (s: string) => void) => () => void;
      setState: (s: string) => void;
      getState: () => Promise<string>;
      quit: () => void;
      getTheme: () => Promise<string>;
      setTheme: (t: string) => void;
      getStyle: () => Promise<string>;
      setStyle: (s: string) => void;
      focusApp: () => void;
      startDrag: () => void;
      getMute: () => Promise<boolean>;
      setMute: (m: boolean) => void;
      setWindowHeight: (h: number) => void;
    };
  }
}

const ORDER: Light[] = ["red", "yellow", "green"];
const TIPS: Record<Light, string> = {
  red:    "红灯 · 等你回应",
  yellow: "黄灯 · 思考中",
  green:  "绿灯 · 已完成",
};
const isLight = (s: string): s is Light => s === "red" || s === "yellow" || s === "green";

// ============== state ==============
let active: Light = "yellow";
let theme: Theme = "dark";
let style: Style = "triple";
let greenSteady = false;
let muted = false;
let showSettings = false;
let timer: number | null = null;

// ============== DOM ==============
const root = document.getElementById("root") as HTMLDivElement;

const bulbHTML = (l: Light) =>
  `<div class="bulb" data-light="${l}" title="${TIPS[l]}">
    <img class="dog" alt="" src="${l === "green" ? "dog-green.gif" : "dog.gif"}" />
    <div class="bulb-outer"><div class="bulb-inner"></div></div>
  </div>`;

function render() {
  const bulbs = style === "single" ? bulbHTML(active) : ORDER.map(bulbHTML).join("");
  root.innerHTML =
    `<div class="housing ${style}">
      <div class="bulbs">${bulbs}</div>
      <button class="gear" title="设置">⚙</button>
    </div>
    <div class="settings${showSettings ? " open" : ""}">
      <label class="row" data-act="mute" title="${muted ? "点击开启提示音" : "点击静音"}">
        <span class="icon">${muted ? "🔇" : "🔔"}</span>
        <div class="toggle${muted ? "" : " on"}"><div class="knob"></div></div>
      </label>
      <label class="row" data-act="theme" title="${theme === "dark" ? "切换浅色模式" : "切换深色模式"}">
        <span class="icon">${theme === "dark" ? "🌙" : "☀️"}</span>
        <div class="toggle${theme === "dark" ? " on" : ""}"><div class="knob"></div></div>
      </label>
    </div>`;
  bind();
  paintBulbs();
}

function bind() {
  root.querySelector<HTMLButtonElement>(".gear")?.addEventListener("click", () => {
    showSettings = !showSettings;
    root.querySelector(".settings")?.classList.toggle("open", showSettings);
    syncHeight();
  });
  root.querySelector<HTMLLabelElement>('.row[data-act="mute"]')?.addEventListener("click", e => {
    e.preventDefault();
    muted = !muted;
    window.electronAPI.setMute(muted);
    render();  // redraw settings (icon + tooltip)
  });
  root.querySelector<HTMLLabelElement>('.row[data-act="theme"]')?.addEventListener("click", e => {
    e.preventDefault();
    theme = theme === "dark" ? "light" : "dark";
    window.electronAPI.setTheme(theme);
    document.body.className = theme;
    render();
  });
}

function paintBulbs() {
  // Only updates active/blink/breathe classes — no DOM rebuild
  root.querySelectorAll<HTMLDivElement>(".bulb").forEach(el => {
    const light = el.dataset.light as Light;
    const isActive = style === "single" ? true : light === active;
    const effective = style === "single" ? active : light;
    // A 方案：红=等你介入 → 快闪+脉冲圈（更醒目）；黄=思考中 → 缓慢呼吸（柔和）
    const blink   = isActive && (effective === "red" || (effective === "green" && !greenSteady));
    const breathe = isActive && effective === "yellow";
    el.classList.toggle("active", isActive);
    el.classList.toggle("blink", blink);
    el.classList.toggle("breathe", breathe);
  });
}

function syncHeight() {
  const base = style === "single" ? 88 : 168;
  const open = style === "single" ? 160 : 240;
  window.electronAPI.setWindowHeight(showSettings ? open : base);
}

function apply(s: Light) {
  if (timer !== null) { clearTimeout(timer); timer = null; }
  greenSteady = false;
  const wasSingle = style === "single";
  active = s;
  if (wasSingle) {
    // single mode: bulb DOM is keyed by color — rebuild to switch
    render();
  } else {
    paintBulbs();
  }
  if (s === "green") {
    timer = window.setTimeout(() => { greenSteady = true; paintBulbs(); }, 2500);
  }
}

// ============== init ==============
document.body.className = theme;
render();
syncHeight();

// 左键非交互区域 → 手动拖窗（替代 -webkit-app-region: drag）
// 右键无任何动作，且禁掉 WebView2 默认右键菜单
document.addEventListener("mousedown", e => {
  window.electronAPI.focusApp();
  if (e.button !== 0) return;
  const t = e.target as HTMLElement;
  // 命中齿轮 / 设置面板 / 开关 → 让它正常 click
  if (t.closest('.gear, .settings, .toggle, .row')) return;
  window.electronAPI.startDrag();
});
document.addEventListener("contextmenu", e => e.preventDefault());

const { electronAPI } = window;
electronAPI.getTheme().then(t => {
  if ((t === "light" || t === "dark") && t !== theme) {
    theme = t;
    document.body.className = theme;
    render();
  }
});
electronAPI.getMute().then(m => { if (m !== muted) { muted = m; render(); } });
electronAPI.getStyle().then(s => {
  if ((s === "single" || s === "triple") && s !== style) {
    style = s;
    render();
    syncHeight();
  }
});
// 同步真实状态，避免一直闪默认黄灯；不调 apply()，免得启动响一次声
electronAPI.getState().then(s => {
  if (isLight(s) && s !== active) {
    active = s;
    if (s === "green") greenSteady = true;  // 启动前的绿灯肯定已稳定
    if (style === "single") render(); else paintBulbs();
  }
});

electronAPI.onThemeChange(t => {
  if ((t === "light" || t === "dark") && t !== theme) {
    theme = t as Theme;
    document.body.className = theme;
    render();
  }
});
electronAPI.onStyleChange(s => {
  if ((s === "single" || s === "triple") && s !== style) {
    style = s as Style;
    render();
    syncHeight();
  }
});
electronAPI.onStateChange(s => { if (isLight(s)) apply(s); });
