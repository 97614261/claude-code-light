import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

const onEvent = <T = string>(name: string, cb: (v: T) => void): (() => void) => {
  let off: UnlistenFn | null = null;
  listen<T>(name, e => cb(e.payload)).then(unlisten => { off = unlisten; });
  return () => { off?.(); };
};

(window as any).electronAPI = {
  onStateChange:  (cb: (s: string) => void) => onEvent("state-change", cb),
  onThemeChange:  (cb: (t: string) => void) => onEvent("theme-change", cb),
  onStyleChange:  (cb: (s: string) => void) => onEvent("style-change", cb),
  getState:       () => invoke<string>("get_state"),
  setState:       (s: string) => { invoke("set_state", { state: s }); },
  quit:           () => { invoke("quit"); },
  getTheme:       () => invoke<string>("get_theme"),
  setTheme:       (t: string) => { invoke("set_theme", { theme: t }); },
  getStyle:       () => invoke<string>("get_style"),
  setStyle:       (s: string) => { invoke("set_style", { style: s }); },
  focusApp:       () => { invoke("focus_app"); },
  startDrag:      () => { invoke("start_drag"); },
  getMute:        () => invoke<boolean>("get_mute"),
  setMute:        (m: boolean) => { invoke("set_mute", { muted: m }); },
  setWindowHeight:(h: number) => { invoke("set_window_height", { h }); },
};
