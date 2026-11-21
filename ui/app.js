// Vane window logic. Talks to the Rust engine through Tauri commands; the engine keeps running when this window closes.
"use strict";
const T = window.__TAURI__;
const invoke = T.core.invoke;
const appWin = T.window.getCurrentWindow();
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const clone = (o) => JSON.parse(JSON.stringify(o));

let meta = null;      // themes, backgrounds, layouts, presets, version
let snap = null;      // latest status + sensors from the engine
let cfg = null;       // working copy of the settings
let dirtyUntil = 0;   // while editing, don't let the engine's copy overwrite ours
let page = "overview";
let selScreen = "cooler";
let selFan = null;
let lcConfirm = false;

const ACCENTS = ["#00ff6a", "#2cc4bc", "#4f9dff", "#b07cff", "#ff6fa8", "#ffb347", "#ff5a4f", "#ffffff"];
const BG_SWATCH = {
  matrix: (a) => `linear-gradient(180deg, #000 0%, ${a}55 50%, #000 100%)`,
  aurora: (a) => `linear-gradient(160deg, #061018, ${a}aa, #2a1450)`,
  nebula: (a) => `radial-gradient(circle at 30% 40%, ${a}cc, #1a0830 60%, #05030a)`,
  synthwave: (a) => `linear-gradient(180deg, #14002a 0%, ${a} 55%, #ffb347 56%, #14002a 100%)`,
  waves: (a) => `repeating-linear-gradient(160deg, #031018 0 8px, ${a}66 8px 10px)`,
  pulse: (a) => `radial-gradient(circle, ${a} 0 10%, #021014 11% 30%, ${a}88 31% 33%, #021014 34%)`,
  radar: (a) => `conic-gradient(from 0deg, ${a}cc, #001a08 25%, #000)`,
  particles: (a) => `radial-gradient(circle at 20% 30%, ${a} 0 2px, transparent 3px), radial-gradient(circle at 70% 60%, ${a} 0 2px, transparent 3px), #08060f`,
  carbon: () => `repeating-linear-gradient(45deg, #15181b 0 4px, #0c0e10 4px 8px)`,
  solid: (a, s) => s || "#0b1a22",
  image: () => `linear-gradient(135deg, #333, #777)`,
};

// ------------------------------------------------------------------ helpers
function toast(msg) {
  const t = $("#toast");
  t.textContent = msg;
  t.classList.add("on");
  clearTimeout(toast.h);
  toast.h = setTimeout(() => t.classList.remove("on"), 2200);
}
let saveTimer = 0;
function save() {
  dirtyUntil = Date.now() + 2500;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => invoke("set_config", { config: cfg }).catch((e) => toast("Couldn't save: " + e)), 200);
}
function smart(id) { return snap?.sensors.smart.find((s) => s.id === id); }
function rawReading(id) {
  for (const list of Object.values(snap?.sensors.groups || {})) { const r = list.find((x) => x.id === id); if (r) return r; }
  return null;
}
function reading(id) { return smart(id) || rawReading(id); }
function fmt(v, unit) {
  if (v === null || v === undefined || Number.isNaN(v)) return null;
  if (unit === "V") return v.toFixed(3);
  if (unit === "ms") return v.toFixed(1);
  if (unit === "GB") return v.toFixed(1);
  return Math.round(v).toLocaleString("en-US").replace(/,/g, " ");
}
function heat(v, unit) { if (unit !== "°C" || v == null) return ""; return v >= 80 ? "heat-hot" : v >= 65 ? "heat-warm" : ""; }
function setSwitch(el, on) { el.setAttribute("aria-checked", on ? "true" : "false"); }
function setSeg(el, v) { $$("button", el).forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.v === String(v)))); }
function fanSvg(rpm) {
  const dur = rpm > 0 ? Math.max(0.25, (60 / rpm) * 8) : 0;
  const blades = [0, 72, 144, 216, 288].map((a) => `<path transform="rotate(${a} 32 32)" d="M32 32 C31 20 35 12 42 10 C44 18 40 27 32 32Z" fill="currentColor" opacity=".85"/>`).join("");
  return `<circle cx="32" cy="32" r="29" fill="none" stroke="currentColor" stroke-opacity=".35" stroke-width="2"/><g class="blades" style="animation-duration:${dur}s;animation-play-state:${dur ? "running" : "paused"}">${blades}</g><circle cx="32" cy="32" r="5" fill="currentColor"/>`;
}
const screensOf = () => snap?.status.screens || [];
const fanScreens = () => screensOf().filter((s) => s.kind === "fan");
function defaultScreen(key) {
  const id = key === "cooler" ? "matrix-fps" : "matrix-rain";
  const c = clone(meta.themes.find((t) => t.id === id).config);
  if (key !== "cooler") c.fps = 15;
  return c;
}
function scr(key) { return cfg.screens[key] || defaultScreen(key); }
function editScreen(fn) {
  const keys = [selScreen];
  if (cfg.link_fan_screens && selScreen !== "cooler") fanScreens().forEach((s) => { if (!keys.includes(s.key)) keys.push(s.key); });
  for (const k of keys) { const c = clone(scr(k)); fn(c, k); cfg.screens[k] = c; }
  save();
  renderScreens();
}

// ------------------------------------------------------------------ app theme + matrix rain
function applyTheme() {
  const t = cfg?.app_theme || "system";
  if (t === "system") delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = t;
  try { localStorage.setItem("vane.theme", t); } catch (e) {}
  rain.toggle(t === "matrix");
}
const rain = (() => {
  const cv = $("#rain"); const cx = cv.getContext("2d");
  let on = false, cols = [], h = 0, last = 0;
  const glyphs = "ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉ0123456789";
  function size() { cv.width = innerWidth; cv.height = innerHeight; cols = Array.from({ length: Math.ceil(innerWidth / 16) }, () => Math.random() * innerHeight / 16); }
  function frame(ts) {
    if (!on) return;
    h = requestAnimationFrame(frame);
    if (document.hidden || ts - last < 66) return;
    last = ts;
    cx.fillStyle = "rgba(1, 6, 3, 0.12)"; cx.fillRect(0, 0, cv.width, cv.height);
    cx.fillStyle = "#00ff6a"; cx.font = "15px 'MS Gothic', monospace";
    cols.forEach((y, i) => { cx.fillText(glyphs[(Math.random() * glyphs.length) | 0], i * 16, y * 16); cols[i] = y * 16 > cv.height && Math.random() > 0.975 ? 0 : y + 1; });
  }
  addEventListener("resize", () => on && size());
  return { toggle(v) {
    const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
    v = v && !reduce;
    if (v === on) return; on = v;
    if (on) { size(); h = requestAnimationFrame(frame); } else { cancelAnimationFrame(h); cx.clearRect(0, 0, cv.width, cv.height); }
  } };
})();

// ------------------------------------------------------------------ navigation + window buttons
function go(p) {
  page = p;
  $$(".navbtn").forEach((b) => { if (b.dataset.page === p) b.setAttribute("aria-current", "page"); else b.removeAttribute("aria-current"); });
  $$(".page").forEach((s) => s.classList.toggle("on", s.id === "p-" + p));
  $("#main").scrollTop = 0;
  renderAll();
  pullPreviews();
}
$$(".navbtn").forEach((b) => b.addEventListener("click", () => go(b.dataset.page)));
$("#w-min").onclick = () => appWin.minimize();
$("#w-max").onclick = () => appWin.toggleMaximize();
$("#w-close").onclick = () => { if (cfg && !cfg.close_to_tray) invoke("quit"); else appWin.close(); };
$("#p-overview").addEventListener("click", (e) => {
  const part = e.target.closest(".part"); if (!part) return;
  if (part.dataset.screen) { selScreen = part.dataset.screen === "fan" ? (e.target.closest("[data-key]")?.dataset.key || fanScreens()[0]?.key || "cooler") : "cooler"; go("screens"); }
  else if (part.dataset.go === "fans") {
    const w = snap?.status.wireless || [];
    const d = part.dataset.kind === "aio" ? w.find((x) => x.is_aio) : w.find((x) => !x.is_aio && !x.kind.includes("LCD"));
    if (d) selFan = d.mac;
    go("fans");
  }
});

// ------------------------------------------------------------------ banners and side status
function lcBanner() {
  const st = snap.status;
  if (st.lconnect_running) return `<div class="banner bad"><p><b>L-Connect 3 is running.</b> <span class="muted">It holds the screens and fans, so Vane can't reach them.</span></p><button class="btn primary" data-act="lc-off">Turn L-Connect off</button></div>`;
  if (st.paused) return `<div class="banner"><p><b>Vane has handed the devices to L-Connect.</b></p><button class="btn primary" data-act="resume">Take them back</button></div>`;
  return "";
}
document.addEventListener("click", async (e) => {
  const a = e.target.closest("[data-act]")?.dataset.act; if (!a) return;
  if (a === "lc-off") await lconnect(false);
  if (a === "resume") { await invoke("resume"); toast("Vane is driving the devices again"); }
});
async function lconnect(on) {
  try { await invoke("lconnect", { on }); toast(on ? "Starting L-Connect 3. Windows asks for admin first." : "Turning L-Connect 3 off. Windows asks for admin first."); }
  catch (e) { toast(String(e)); }
}
function renderSide() {
  const st = snap.status;
  const conn = st.screens.filter((s) => s.connected).length;
  const fans = st.wireless.length;
  $("#side-status").innerHTML =
    `<div><span class="dot ${conn === st.screens.length && conn ? "" : "warn"}"></span>${conn} of ${st.screens.length || 4} screens connected</div>` +
    `<div><span class="dot ${st.wireless_error ? "hot" : fans ? "" : "warn"}"></span>${st.wireless_error ? "Wireless dongle not reachable" : fans + " wireless fan group" + (fans === 1 ? "" : "s")}</div>` +
    `<div><span class="dot ${st.lconnect_running ? "hot" : ""}"></span>L-Connect is ${st.lconnect_running ? "running" : "off"}</div>`;
}

// ------------------------------------------------------------------ overview
const STATS = ["fps", "frametime", "gpu.temp", "gpu.voltage", "cpu.temp", "cpu.load"];
function spark(h) {
  if (!h || h.length < 2) return "";
  const lo = Math.min(...h), hi = Math.max(...h), span = hi - lo || 1;
  const pts = h.map((v, i) => `${(i / (h.length - 1)) * 100},${24 - ((v - lo) / span) * 22}`).join(" ");
  return `<svg viewBox="0 0 100 26" preserveAspectRatio="none"><polyline points="${pts}"/></svg>`;
}
function renderOverview() {
  const st = snap.status;
  $("#ov-banner").innerHTML = lcBanner();
  const conn = st.screens.filter((s) => s.connected).length;
  const chip = $("#ov-chip");
  if (st.lconnect_running) { chip.className = "chip hot"; chip.innerHTML = `<span class="dot hot"></span>Waiting for L-Connect to stop`; }
  else if (!screensOf().length || !conn) { chip.className = "chip warn"; chip.innerHTML = `<span class="dot warn"></span>Looking for screens`; }
  else if (!cfg.screens_on) { chip.className = "chip warn"; chip.innerHTML = `<span class="dot warn"></span>Screens are turned off`; }
  else { chip.className = "chip ok"; chip.innerHTML = `<span class="dot"></span>${conn} screen${conn === 1 ? "" : "s"} updating`; }

  const w = st.wireless;
  const aio = w.find((d) => d.is_aio);
  const lcd = w.find((d) => d.kind.includes("LCD"));
  const other = w.find((d) => !d.is_aio && !d.kind.includes("LCD"));
  const radN = aio ? Math.max(1, Math.min(3, aio.fan_count)) : 3;
  const radRpm = aio ? aio.rpm.slice(0, radN) : [0, 0, 0];
  $("#ov-rad").innerHTML = radRpm.map((r) => `<div><svg class="fanicon" viewBox="0 0 64 64">${fanSvg(r)}</svg><div class="fan-rpm num">${r ? r + " rpm" : "–"}</div></div>`).join("");
  $("#ov-rad-avg").textContent = aio ? Math.round(radRpm.reduce((a, b) => a + b, 0) / radN) + " rpm avg" : "not found";
  const cool = smart("coolant"), pump = smart("pump");
  $("#ov-coolant").textContent = cool?.value != null ? cool.value.toFixed(0) + " °C" : "–";
  $("#ov-pump").textContent = pump?.value != null ? Math.round(pump.value) : "–";

  const fs = fanScreens();
  const stack = $("#ov-fanscreens");
  if (stack.dataset.keys !== fs.map((s) => s.key).join()) {
    stack.dataset.keys = fs.map((s) => s.key).join();
    stack.innerHTML = fs.length ? fs.map((s) => `<img class="lcd rd" data-key="${esc(s.key)}" data-preview="${esc(s.key)}" alt="${esc(s.name)}">`).join("") : `<span class="small muted">No LCD fans found</span>`;
  }
  $("#ov-lcd-avg").textContent = lcd ? Math.round(lcd.rpm.slice(0, Math.max(1, lcd.fan_count)).reduce((a, b) => a + b, 0) / Math.max(1, lcd.fan_count)) + " rpm avg" : "";
  $("#ov-inf").hidden = !other;
  if (other) {
    $("#ov-inf-name").textContent = cfg.fans[other.mac]?.name || other.kind;
    $("#ov-inf svg").innerHTML = fanSvg(other.rpm[0]);
    $("#ov-inf-rpm").textContent = other.rpm[0] || "–";
  }

  $("#ov-stats").innerHTML = STATS.map((id) => {
    const s = smart(id) || { label: id, unit: "" };
    const v = fmt(s.value, s.unit);
    return `<div class="stat"><span class="k">${esc(s.label)}</span>${v === null ? `<span class="v none">No reading</span>` : `<span class="v ${heat(s.value, s.unit)}">${v}<small>${esc(s.unit === "FPS" ? "" : s.unit)}</small></span>`}${spark(s.history)}</div>`;
  }).join("");

  const devs = st.screens.map((s) => {
    const chip = !s.connected ? `<span class="chip warn"><span class="dot warn"></span>Not found</span>` : s.error ? `<span class="chip hot"><span class="dot hot"></span>Error</span>` : `<span class="chip ok"><span class="dot"></span>${s.fps ? s.fps.toFixed(0) + " fps" : "Connected"}</span>`;
    return `<div class="dev"><span class="n">${esc(s.name)}</span><span class="id">${esc(s.error || (s.kind === "cooler" ? "HydroShift II LCD-S · 480 × 480" : "SL V3 LCD · 400 × 400") + (s.firmware ? " · " + s.firmware : ""))}</span>${chip}</div>`;
  });
  devs.push(`<div class="dev"><span class="n">Wireless dongle</span><span class="id">${esc(st.wireless_error || w.map((d) => cfg.fans[d.mac]?.name || d.kind).join(", ") || "Searching for fans")}</span>${st.wireless_error ? `<span class="chip hot"><span class="dot hot"></span>Error</span>` : `<span class="chip ok"><span class="dot"></span>${w.length} group${w.length === 1 ? "" : "s"}</span>`}</div>`);
  $("#ov-devices").innerHTML = devs.join("");
  $("#mem-vane").textContent = st.memory_mb ? st.memory_mb.toFixed(0) + " MB" : "–";
  $("#mem-vane-bar").style.width = Math.min(100, (st.memory_mb / 480) * 100) + "%";
}

// ------------------------------------------------------------------ screens
function sensorOptions(selected) {
  const s = snap.sensors;
  let h = `<option value="">Empty</option><optgroup label="Common">`;
  h += s.smart.map((r) => `<option value="${esc(r.id)}"${r.id === selected ? " selected" : ""}>${esc(r.label)}${r.value == null ? " (no reading)" : ""}</option>`).join("") + `</optgroup>`;
  for (const [g, list] of Object.entries(s.groups)) {
    h += `<optgroup label="${esc(g)}">` + list.map((r) => `<option value="${esc(r.id)}"${r.id === selected ? " selected" : ""}>${esc(r.label)}</option>`).join("") + `</optgroup>`;
  }
  return h;
}
function renderScreens() {
  const list = screensOf().length ? screensOf() : [{ key: "cooler", name: "Cooler screen", kind: "cooler", connected: false }];
  if (!list.find((s) => s.key === selScreen)) selScreen = list[0].key;
  const cur = list.find((s) => s.key === selScreen);
  const isFan = cur.kind === "fan";
  const c = scr(selScreen);

  const pick = $("#scr-pick");
  const sig = list.map((s) => s.key + s.name + s.connected).join() + selScreen;
  if (pick.dataset.sig !== sig) {
    pick.dataset.sig = sig;
    pick.innerHTML = list.map((s) => `<button class="pickbtn ${s.kind === "fan" ? "rd" : "sq"}" data-key="${esc(s.key)}" aria-pressed="${s.key === selScreen}"><img data-preview="${esc(s.key)}" alt=""><span><b>${esc(s.name)}</b><span>${s.connected ? (s.kind === "fan" ? "400 × 400, round" : "480 × 480") : "Not found"}</span></span></button>`).join("");
  }
  const stage = $("#scr-stage");
  stage.className = isFan ? "rd" : "sq";
  stage.dataset.preview = selScreen;
  $("#scr-cap").textContent = cur.connected ? `${cur.name} · live${cur.fps ? " · " + cur.fps.toFixed(0) + " fps" : ""}` : `${cur.name} isn't connected right now`;
  setSwitch($("#scr-enabled"), c.enabled);

  const all = [...meta.themes.filter((t) => (isFan ? t.fans : t.cooler)).map((t) => ({ id: t.id, name: t.name, config: t.config, mine: false })),
    ...Object.entries(cfg.my_themes).map(([name, conf]) => ({ id: "my:" + name, name, config: conf, mine: true }))];
  $("#scr-themes").innerHTML = all.map((t) => `<button class="theme" data-theme="${esc(t.id)}" aria-pressed="${c.theme === t.id}"><i style="background:${(BG_SWATCH[t.config.background] || BG_SWATCH.solid)(t.config.accent, t.config.solid)}"></i><b>${esc(t.name)}</b>${t.mine ? `<span class="del" data-del="${esc(t.name)}" role="button" aria-label="Delete ${esc(t.name)}">×</span>` : ""}</button>`).join("");

  $("#scr-layout").innerHTML = meta.layouts.map((l) => `<option value="${l.id}"${l.id === c.layout ? " selected" : ""}>${esc(l.name)}</option>`).join("");
  $("#scr-bg").innerHTML = meta.backgrounds.map((b) => `<option value="${b.id}"${b.id === c.background ? " selected" : ""}>${esc(b.name)}</option>`).join("");
  const n = meta.layouts.find((l) => l.id === c.layout)?.slots ?? 0;
  $("#scr-slot-hint").textContent = n ? `${n} slot${n === 1 ? "" : "s"}` : "this layout shows no readings";
  const slots = $("#scr-slots");
  const sSig = c.layout + "|" + (c.slots || []).join() + "|" + Object.keys(snap.sensors.groups).length + "|" + selScreen;
  if (slots.dataset.sig !== sSig) {
    slots.dataset.sig = sSig;
    slots.innerHTML = Array.from({ length: n }, (_, i) => `<div class="slotrow"><span class="n">${i + 1}</span><select data-slot="${i}" aria-label="Reading ${i + 1}">${sensorOptions(c.slots[i] || "")}</select><span class="val" data-val="${i}"></span></div>`).join("");
  }
  $$("[data-val]", slots).forEach((el) => { const id = c.slots[+el.dataset.val]; const r = id && reading(id); el.textContent = r && r.value != null ? fmt(r.value, r.unit) + " " + (r.unit || "") : "–"; });
  $("#scr-image-row").hidden = c.background !== "image";
  $("#scr-image-name").textContent = c.image ? c.image.split(/[\\/]/).pop() : "Nothing chosen yet";

  $("#scr-accent").innerHTML = ACCENTS.map((a) => `<button class="sw" data-accent="${a}" style="background:${a}" aria-label="Accent ${a}" aria-pressed="${c.accent.toLowerCase() === a}"></button>`).join("") + `<input type="color" id="scr-accent-custom" value="${c.accent}" aria-label="Custom accent colour">`;
  $("#scr-text").value = c.text;
  $("#scr-solid").value = c.solid;
  $("#scr-solid-f").hidden = c.background !== "solid";
  setSeg($("#scr-clock"), c.clock_24h ? 24 : 12);
  $("#scr-speed").value = c.speed; $("#scr-speed-v").textContent = c.speed.toFixed(1) + "×";
  $("#scr-dim").value = c.dim; $("#scr-dim-v").textContent = Math.round(c.dim * 100) + "%";
  setSwitch($("#scr-plates"), c.plates);
  $("#scr-bright").value = c.brightness; $("#scr-bright-v").textContent = c.brightness + "%";
  $("#scr-fps").value = String(c.fps);
  setSeg($("#scr-rot"), c.rotation);
  $("#scr-link-row").hidden = !isFan;
  setSwitch($("#scr-link"), cfg.link_fan_screens);
}
$("#scr-pick").addEventListener("click", (e) => { const b = e.target.closest("[data-key]"); if (b) { selScreen = b.dataset.key; renderScreens(); pullPreviews(); } });
$("#scr-themes").addEventListener("click", (e) => {
  const del = e.target.closest("[data-del]");
  if (del) { delete cfg.my_themes[del.dataset.del]; save(); renderScreens(); toast("Theme deleted"); return; }
  const b = e.target.closest("[data-theme]"); if (!b) return;
  const id = b.dataset.theme;
  const base = id.startsWith("my:") ? cfg.my_themes[id.slice(3)] : meta.themes.find((t) => t.id === id)?.config;
  if (!base) return;
  editScreen((c) => { const keep = { enabled: c.enabled, brightness: c.brightness, rotation: c.rotation, fps: c.fps }; Object.assign(c, clone(base), keep, { theme: id }); });
});
$("#scr-layout").onchange = (e) => editScreen((c) => { c.layout = e.target.value; });
$("#scr-bg").onchange = (e) => editScreen((c) => { c.background = e.target.value; });
$("#scr-slots").addEventListener("change", (e) => { const i = +e.target.dataset.slot; editScreen((c) => { while (c.slots.length <= i) c.slots.push(""); c.slots[i] = e.target.value; }); });
$("#scr-pick-image").onclick = async () => { const p = await invoke("pick_image"); if (p) editScreen((c) => { c.image = p; c.background = "image"; }); };
$("#scr-accent").addEventListener("click", (e) => { const b = e.target.closest("[data-accent]"); if (b) editScreen((c) => { c.accent = b.dataset.accent; }); });
$("#scr-accent").addEventListener("change", (e) => { if (e.target.id === "scr-accent-custom") editScreen((c) => { c.accent = e.target.value; }); });
$("#scr-text").onchange = (e) => editScreen((c) => { c.text = e.target.value; });
$("#scr-solid").onchange = (e) => editScreen((c) => { c.solid = e.target.value; });
$("#scr-clock").onclick = (e) => { const b = e.target.closest("button"); if (b) editScreen((c) => { c.clock_24h = b.dataset.v === "24"; }); };
for (const [id, key, num] of [["#scr-speed", "speed", parseFloat], ["#scr-dim", "dim", parseFloat], ["#scr-bright", "brightness", (v) => parseInt(v, 10)]]) {
  $(id).oninput = (e) => editScreen((c) => { c[key] = num(e.target.value); });
}
$("#scr-fps").onchange = (e) => editScreen((c) => { c.fps = parseInt(e.target.value, 10); });
$("#scr-rot").onclick = (e) => { const b = e.target.closest("button"); if (b) editScreen((c) => { c.rotation = parseInt(b.dataset.v, 10); }); };
$("#scr-plates").onclick = () => editScreen((c) => { c.plates = !c.plates; });
$("#scr-enabled").onclick = () => { const c = clone(scr(selScreen)); c.enabled = !c.enabled; cfg.screens[selScreen] = c; save(); renderScreens(); };
$("#scr-link").onclick = () => {
  cfg.link_fan_screens = !cfg.link_fan_screens;
  if (cfg.link_fan_screens && selScreen !== "cooler") { const src = scr(selScreen); fanScreens().forEach((s) => { const c = clone(src); c.enabled = scr(s.key).enabled; cfg.screens[s.key] = c; }); }
  save(); renderScreens();
};
$("#scr-identify").onclick = () => { invoke("identify"); toast("Each screen shows its name for a few seconds"); };
$("#scr-save-theme").onclick = () => {
  const name = $("#scr-theme-name").value.trim();
  if (!name) { $("#scr-theme-name").focus(); toast("Type a name for the theme first"); return; }
  const c = clone(scr(selScreen)); delete c.enabled;
  c.theme = "my:" + name;
  cfg.my_themes[name] = c;
  editScreen((x) => { x.theme = "my:" + name; });
  $("#scr-theme-name").value = "";
  toast(`Saved "${name}" to your themes`);
};

// ------------------------------------------------------------------ fans
function temps() {
  const s = snap.sensors;
  const opts = s.smart.filter((r) => r.unit === "°C").map((r) => ({ id: r.id, label: r.label, value: r.value }));
  for (const [g, list] of Object.entries(s.groups)) for (const r of list) if (r.unit === "°C" && !r.id.startsWith("lianli.")) opts.push({ id: r.id, label: `${g}: ${r.label}`, value: r.value });
  return opts;
}
function renderFans() {
  const st = snap.status, w = st.wireless;
  $("#fan-banner").innerHTML = lcBanner() || (st.wireless_error ? `<div class="banner bad"><p><b>The wireless dongle isn't answering.</b> <span class="muted">${esc(st.wireless_error)}</span></p></div>` : "");
  if (!w.length) {
    $("#fan-groups").innerHTML = `<div class="box empty">Searching for wireless fans…</div>`;
    $("#fan-editor").innerHTML = `<div class="empty">Fan groups appear here once the dongle finds them.</div>`;
    return;
  }
  if (!w.find((d) => d.mac === selFan)) selFan = w[0].mac;
  $("#fan-groups").innerHTML = w.map((d) => {
    const g = cfg.fans[d.mac] || {};
    const n = Math.max(1, Math.min(d.is_aio ? 3 : 4, d.fan_count || 1));
    const avg = Math.round(d.rpm.slice(0, n).reduce((a, b) => a + b, 0) / n);
    const mode = !g.mode ? "Setting up" : g.mode === "fixed" ? `Fixed ${g.fixed}%` : g.mode === "auto" ? "Left to the fans" : `Follows ${reading(g.follow)?.label || g.follow}`;
    return `<button class="group" data-mac="${esc(d.mac)}" aria-pressed="${d.mac === selFan}"><b>${esc(g.name || d.kind)}</b><span class="sub">${esc(d.kind)} · ${n} fan${n === 1 ? "" : "s"}${d.is_aio ? " + pump" : ""} · ${esc(mode)}</span><span class="rpm">${avg}<small>rpm</small></span></button>`;
  }).join("");
  renderFanEditor();
}
function renderFanEditor(soft) {
  const d = snap.status.wireless.find((x) => x.mac === selFan); if (!d) return;
  const g = cfg.fans[d.mac]; if (!g) return;
  const ed = $("#fan-editor");
  const n = Math.max(1, Math.min(d.is_aio ? 3 : 4, d.fan_count || 1));
  const src = reading(g.follow);
  const t = src?.value;
  const pct = g.mode === "fixed" ? g.fixed : t != null ? curveAt(g.curve, t) : 60;
  if (soft && ed.dataset.mac === d.mac) {
    // live values only, so a drag or a focused field isn't interrupted
    $$("[data-rpm]", ed).forEach((el) => { el.textContent = d.rpm[+el.dataset.rpm] + " rpm"; });
    const now = $("#fan-now"); if (now) now.innerHTML = nowLine(g, t);
    const ft = $("#fan-temp"); if (ft) ft.textContent = t != null ? t.toFixed(0) + " °C" : "no reading";
    const fp = $("#fan-pct"); if (fp) fp.textContent = Math.round(pct) + "%";
    return;
  }
  ed.dataset.mac = d.mac;
  const tOpts = temps();
  if (!tOpts.find((o) => o.id === g.follow)) tOpts.unshift({ id: g.follow, label: g.follow + " (no reading)" });
  ed.innerHTML = `
    <div style="display:grid;gap:16px">
      <div class="row2">
        <label class="field"><span>Name</span><input type="text" id="fan-name" value="${esc(g.name)}" maxlength="40"></label>
        <div class="field"><span>Speed</span><div class="seg" id="fan-mode"><button data-v="curve">Curve</button><button data-v="fixed">Fixed</button><button data-v="auto">Leave alone</button></div></div>
      </div>
      <div id="fan-curve-box" ${g.mode === "curve" ? "" : "hidden"}>
        <div class="row2" style="margin-bottom:10px">
          <label class="field"><span>Follows</span><select id="fan-follow">${tOpts.map((o) => `<option value="${esc(o.id)}"${o.id === g.follow ? " selected" : ""}>${esc(o.label)}${o.value != null ? " · " + o.value.toFixed(0) + " °C" : ""}</option>`).join("")}</select></label>
          <div class="field"><span>Presets</span><div class="seg" id="fan-presets">${meta.presets.map((p) => `<button data-v="${p.id}">${{ quiet: "Quiet", balanced: "Balanced", perf: "Performance", full: "Full" }[p.id]}</button>`).join("")}</div></div>
        </div>
        <svg class="curve" id="fan-curve" viewBox="0 0 520 250" aria-label="Fan curve, drag the points"></svg>
        <div class="curve-foot"><span>Now <b id="fan-temp">${t != null ? t.toFixed(0) + " °C" : "no reading"}</b></span><span>Speed <b id="fan-pct">${Math.round(pct)}%</b></span><span class="muted small" style="align-self:center">${t == null ? "No reading, so the fans run at 60%." : "Drag the points to shape the curve."}</span></div>
      </div>
      <label class="field" id="fan-fixed-box" ${g.mode === "fixed" ? "" : "hidden"}><span>Fixed speed <b id="fan-fixed-v">${g.fixed}%</b></span><input type="range" id="fan-fixed" min="0" max="100" step="1" value="${g.fixed}"></label>
      ${g.mode === "auto" ? `<p class="small muted" style="margin:0">Vane sends nothing to this group; the fans keep their last speed.</p>` : ""}
      <div class="field"><span>Fans in this group</span><div class="fanrows">${d.rpm.slice(0, n).map((_, i) => `<div class="fanrow"><b class="num" data-rpm="${i}">${d.rpm[i]} rpm</b><span>Fan ${i + 1} · ${d.pwm[i]}% sent</span></div>`).join("")}${d.is_aio ? `<div class="fanrow"><b class="num" data-rpm="3">${d.rpm[3]} rpm</b><span>Pump</span></div>` : ""}</div></div>
      ${d.is_aio ? `<label class="field"><span>Pump speed <b id="fan-pump-v">${g.pump_fixed}%</b></span><input type="range" id="fan-pump" min="30" max="100" step="1" value="${g.pump_fixed}"><span class="small muted" style="font-weight:400">The pump never goes below 30%, so the CPU stays cooled.</span></label>` : ""}
    </div>`;
  setSeg($("#fan-mode"), g.mode);
  if (g.mode === "curve") drawCurve(g, t);
}
function curveAt(c, t) {
  if (!c.length) return 50;
  if (t <= c[0].t) return c[0].p;
  for (let i = 1; i < c.length; i++) if (t <= c[i].t) return c[i - 1].p + ((c[i].p - c[i - 1].p) * (t - c[i - 1].t)) / Math.max(0.01, c[i].t - c[i - 1].t);
  return c[c.length - 1].p;
}
const CV = { x0: 44, x1: 505, y0: 220, y1: 14 };
function tRange(g) { return g.follow === "coolant" ? [20, 60] : [20, 100]; }
function px(g, t) { const [a, b] = tRange(g); return CV.x0 + ((t - a) / (b - a)) * (CV.x1 - CV.x0); }
function py(p) { return CV.y0 - (p / 100) * (CV.y0 - CV.y1); }
function nowLine(g, t) {
  if (t == null) return "";
  const p = curveAt(g.curve, t), x = px(g, Math.max(tRange(g)[0], Math.min(tRange(g)[1], t)));
  return `<line x1="${x}" x2="${x}" y1="${CV.y1}" y2="${CV.y0}"/><circle cx="${x}" cy="${py(p)}" r="4"/><text x="${x + 6}" y="${CV.y1 + 12}">${t.toFixed(0)} °C</text>`;
}
function drawCurve(g, t) {
  const svg = $("#fan-curve"); if (!svg) return;
  const [a, b] = tRange(g);
  let grid = "", axis = "";
  for (let p = 0; p <= 100; p += 25) { grid += `<line x1="${CV.x0}" x2="${CV.x1}" y1="${py(p)}" y2="${py(p)}"/>`; axis += `<text x="${CV.x0 - 8}" y="${py(p) + 4}" text-anchor="end">${p}%</text>`; }
  const step = (b - a) / 4;
  for (let v = a; v <= b; v += step) { grid += `<line y1="${CV.y1}" y2="${CV.y0}" x1="${px(g, v)}" x2="${px(g, v)}"/>`; axis += `<text x="${px(g, v)}" y="${CV.y0 + 18}" text-anchor="middle">${Math.round(v)} °C</text>`; }
  const pts = g.curve.map((c) => `${px(g, c.t)},${py(c.p)}`).join(" ");
  svg.innerHTML = `<g class="grid">${grid}</g><g class="axis">${axis}</g>
    <polygon class="area" points="${px(g, g.curve[0].t)},${CV.y0} ${pts} ${px(g, g.curve.at(-1).t)},${CV.y0}"/>
    <polyline class="ln" points="${pts}"/><g class="now" id="fan-now">${nowLine(g, t)}</g>
    ${g.curve.map((c, i) => `<circle class="pt" data-i="${i}" cx="${px(g, c.t)}" cy="${py(c.p)}" r="7"><title>${c.t.toFixed(0)} °C → ${c.p.toFixed(0)}%</title></circle>`).join("")}`;
}
let drag = null;
$("#fan-editor").addEventListener("pointerdown", (e) => {
  const pt = e.target.closest(".pt"); if (!pt) return;
  drag = { i: +pt.dataset.i }; pt.setPointerCapture(e.pointerId); e.preventDefault();
});
$("#fan-editor").addEventListener("pointermove", (e) => {
  if (!drag) return;
  const g = cfg.fans[selFan], svg = $("#fan-curve");
  const r = svg.getBoundingClientRect(), sx = 520 / r.width, sy = 250 / r.height;
  const x = (e.clientX - r.left) * sx, y = (e.clientY - r.top) * sy;
  const [a, b] = tRange(g);
  let t = a + ((x - CV.x0) / (CV.x1 - CV.x0)) * (b - a);
  const lo = drag.i ? g.curve[drag.i - 1].t + 1 : a, hi = drag.i < g.curve.length - 1 ? g.curve[drag.i + 1].t - 1 : b;
  t = Math.round(Math.max(lo, Math.min(hi, t)));
  const p = Math.round(Math.max(0, Math.min(100, ((CV.y0 - y) / (CV.y0 - CV.y1)) * 100)));
  g.curve[drag.i] = { t, p };
  drawCurve(g, reading(g.follow)?.value);
  dirtyUntil = Date.now() + 2500;
});
$("#fan-editor").addEventListener("pointerup", () => { if (drag) { drag = null; save(); renderFanEditor(); } });
$("#fan-editor").addEventListener("click", (e) => {
  const g = cfg.fans[selFan]; if (!g) return;
  const m = e.target.closest("#fan-mode button"); if (m) { g.mode = m.dataset.v; save(); renderFans(); return; }
  const p = e.target.closest("#fan-presets button"); if (p) { g.curve = clone(meta.presets.find((x) => x.id === p.dataset.v).curve); save(); renderFanEditor(); }
});
$("#fan-editor").addEventListener("input", (e) => {
  const g = cfg.fans[selFan]; if (!g) return;
  if (e.target.id === "fan-fixed") { g.fixed = +e.target.value; $("#fan-fixed-v").textContent = g.fixed + "%"; save(); }
  if (e.target.id === "fan-pump") { g.pump_fixed = +e.target.value; g.pump_mode = "fixed"; $("#fan-pump-v").textContent = g.pump_fixed + "%"; save(); }
  if (e.target.id === "fan-name") { g.name = e.target.value; save(); }
});
$("#fan-editor").addEventListener("change", (e) => {
  const g = cfg.fans[selFan]; if (!g) return;
  if (e.target.id === "fan-follow") { g.follow = e.target.value; save(); renderFanEditor(); }
  if (e.target.id === "fan-name") renderFans();
});
$("#fan-groups").addEventListener("click", (e) => { const b = e.target.closest("[data-mac]"); if (b) { selFan = b.dataset.mac; renderFans(); } });

// ------------------------------------------------------------------ settings
function renderSettings() {
  setSeg($("#set-theme"), cfg.app_theme);
  setSwitch($("#set-auto"), cfg.start_with_windows);
  setSwitch($("#set-tray"), cfg.close_to_tray);
  setSwitch($("#set-screens"), cfg.screens_on);
  const st = snap.status;
  $("#lc-state").textContent = st.lconnect_running ? "L-Connect 3 is running" : st.paused ? "L-Connect 3 has the devices" : "L-Connect 3 is off";
  $("#lc-sub").textContent = st.lconnect_running ? "Turn it off so Vane can drive the screens and fans. Windows asks for admin." : "Its services and startup task stay disabled until you turn it back on.";
  $("#lc-btn").textContent = st.lconnect_running ? "Turn off" : lcConfirm ? "Click again to hand over" : "Turn back on";
  $("#lc-btn").className = st.lconnect_running ? "btn primary" : "btn";

  const src = snap.sensors.sources;
  const gpuV = smart("gpu.voltage")?.value != null, cpuT = smart("cpu.temp")?.value != null;
  $("#set-sources").innerHTML = [
    [src.rtss, "RivaTuner (RTSS)", src.rtss ? (src.game ? "Reading " + src.game : "Running, no game in front") : "Not running. FPS and frame time need it."],
    [src.hwinfo_shared_memory, "HWiNFO shared memory", src.hwinfo_shared_memory ? "On: every HWiNFO reading is available" : "Off"],
    [src.hwinfo_gadget > 0, "HWiNFO gadget values", src.hwinfo_gadget ? src.hwinfo_gadget + " readings shared" : "None shared"],
    [true, "Windows", "CPU load and RAM"],
    [st.wireless.some((d) => d.is_aio), "HydroShift II", "Coolant and pump"],
  ].map(([ok, n, s]) => `<div class="src"><span class="dot ${ok ? "" : "off"}"></span>${esc(n)}<span class="muted">${esc(s)}</span></div>`).join("");
  const tip = $("#set-tip");
  if (src.hwinfo_shared_memory && cpuT && gpuV) { tip.hidden = true; }
  else {
    tip.hidden = false;
    tip.innerHTML = `<b>${!cpuT && !gpuV ? "CPU temperature and GPU voltage aren't coming through yet." : !cpuT ? "CPU temperature isn't coming through yet." : !gpuV ? "GPU voltage isn't coming through yet." : "Share more readings from HWiNFO."}</b> Pick one of these in HWiNFO:
      <ol><li>Settings → <b>Shared Memory Support</b> on. Vane then sees every reading (the free version turns this off after 12 hours).</li>
      <li>Or, in the sensors window, right-click <b>CPU (Tctl/Tdie)</b> and <b>GPU Core Voltage</b> → Customize values → tick <b>Report value in Gadget</b>. This has no time limit.</li></ol>`;
  }
  const all = $("#set-all");
  if (all.closest("details").open) {
    all.innerHTML = Object.entries(snap.sensors.groups).map(([g, list]) => `<h4>${esc(g)}</h4>` + list.map((r) => `<div><span>${esc(r.label)}</span><span>${fmt(r.value, r.unit) ?? "–"} ${esc(r.unit)}</span></div>`).join("")).join("") ||
      `<span class="muted">Only Windows readings so far.</span>`;
  }
  $("#about").innerHTML = `Vane ${esc(meta.version)}. Drives the HydroShift II LCD-S screen, the SL V3 LCD fan screens and the wireless fans directly over USB, without L-Connect. Device protocol details come from the MIT-licensed lian-li-linux project by sgtaziz. Settings live in %APPDATA%\\Vane.`;
}
$("#set-theme").onclick = (e) => { const b = e.target.closest("button"); if (!b) return; cfg.app_theme = b.dataset.v; applyTheme(); save(); renderSettings(); };
$("#set-auto").onclick = () => { cfg.start_with_windows = !cfg.start_with_windows; save(); renderSettings(); toast(cfg.start_with_windows ? "Vane starts with Windows" : "Vane won't start with Windows"); };
$("#set-tray").onclick = () => { cfg.close_to_tray = !cfg.close_to_tray; save(); renderSettings(); };
$("#set-screens").onclick = () => { cfg.screens_on = !cfg.screens_on; save(); renderSettings(); };
$("#set-all").closest("details").addEventListener("toggle", renderSettings);
$("#lc-btn").onclick = async () => {
  if (snap.status.lconnect_running) { await lconnect(false); return; }
  if (!lcConfirm) { lcConfirm = true; renderSettings(); setTimeout(() => { lcConfirm = false; snap && renderSettings(); }, 4000); return; }
  lcConfirm = false; await lconnect(true);
};

// ------------------------------------------------------------------ loop
function renderAll(soft) {
  if (!snap || !meta) return;
  renderSide();
  if (page === "overview") renderOverview();
  if (page === "screens" && !(document.activeElement?.closest("#p-screens select, #p-screens input"))) renderScreens();
  if (page === "fans") {
    const busy = drag || document.activeElement?.closest("#fan-editor input, #fan-editor select");
    const same = $$("#fan-groups .group").map((b) => b.dataset.mac).join() === snap.status.wireless.map((d) => d.mac).join();
    if ((soft || busy) && same && snap.status.wireless.length) { renderFanEditor(true); refreshGroupRpm(); } else renderFans();
  }
  if (page === "settings") renderSettings();
}
function refreshGroupRpm() {
  const w = snap.status.wireless;
  $$("#fan-groups .group").forEach((b) => {
    const d = w.find((x) => x.mac === b.dataset.mac); if (!d) return;
    const n = Math.max(1, Math.min(d.is_aio ? 3 : 4, d.fan_count || 1));
    b.querySelector(".rpm").innerHTML = Math.round(d.rpm.slice(0, n).reduce((a, c) => a + c, 0) / n) + "<small>rpm</small>";
  });
}
async function poll() {
  try {
    const s = await invoke("snapshot");
    const first = !snap;
    snap = s;
    if (first || Date.now() > dirtyUntil) { cfg = s.config; if (first) applyTheme(); }
    renderAll(!first);
  } catch (e) { console.error(e); }
}
let previewBusy = false;
async function pullPreviews() {
  if (previewBusy || document.hidden) return;
  previewBusy = true;
  try {
    const imgs = $$(".page.on img[data-preview]");
    const keys = [...new Set(imgs.map((i) => i.dataset.preview))];
    const got = await Promise.all(keys.map((k) => invoke("preview", { key: k }).catch(() => null)));
    keys.forEach((k, i) => imgs.filter((im) => im.dataset.preview === k).forEach((im) => {
      if (got[i]) { im.src = got[i]; im.classList.remove("off"); } else im.classList.add("off");
    }));
  } finally { previewBusy = false; }
}
// the cooler pump preview is a fixed image element; give it its key once
$("#ov-img-cooler").dataset.preview = "cooler";

(async () => {
  meta = await invoke("meta");
  await poll();
  setInterval(poll, 1000);
  setInterval(pullPreviews, 200);
})();
