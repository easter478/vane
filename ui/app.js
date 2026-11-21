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
  if (p === "screens") { renderDesignPics(); if (!library) loadLibrary(); }
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
  const g = e.target.closest("[data-goto]"); if (g) { e.stopPropagation(); go(g.dataset.goto); return; }
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
    const needsHw = id === "cpu.temp" || id === "gpu.voltage";
    return `<div class="stat"><span class="k">${esc(s.label)}</span>${v === null ? (needsHw ? `<button class="v none linkish" data-goto="settings">Turn on in HWiNFO</button>` : `<span class="v none">No reading</span>`) : `<span class="v ${heat(s.value, s.unit)}">${v}<small>${esc(s.unit === "FPS" ? "" : s.unit)}</small></span>`}${spark(s.history)}</div>`;
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
let editing = false;   // layout editor on
let selWidget = 0;
let library = null;    // imported L-Connect videos
let importing = false;
const designPics = {}; // "kind|id|sig" -> rendered picture
let designBusy = false;
const WIDGET_NAMES = { value: "Number", gauge: "Gauge", bar: "Bar", clock: "Clock", date: "Date", text: "Text", graph: "Graph" };
const COLOR_NAMES = { heat: "Warms with heat", accent: "Accent", text: "Text colour" };

function sensorOptions(selected, empty = "Empty") {
  const s = snap.sensors;
  let h = `<option value="">${esc(empty)}</option><optgroup label="Common">`;
  h += s.smart.map((r) => `<option value="${esc(r.id)}"${r.id === selected ? " selected" : ""}>${esc(r.label)}${r.value == null ? " (no reading)" : ""}</option>`).join("") + `</optgroup>`;
  for (const [g, list] of Object.entries(s.groups)) {
    h += `<optgroup label="${esc(g)}">` + list.map((r) => `<option value="${esc(r.id)}"${r.id === selected ? " selected" : ""}>${esc(r.label)}</option>`).join("") + `</optgroup>`;
  }
  return h;
}
function curScreen() {
  const list = screensOf().length ? screensOf() : [{ key: "cooler", name: "Cooler screen", kind: "cooler", connected: false }];
  if (!list.find((s) => s.key === selScreen)) selScreen = list[0].key;
  return { list, cur: list.find((s) => s.key === selScreen) };
}
function allDesigns(isFan) {
  return [...meta.themes.filter((t) => (isFan ? t.fans : t.cooler)).map((t) => ({ id: t.id, name: t.name, config: t.config, mine: false })),
    ...Object.entries(cfg.my_themes).map(([name, conf]) => ({ id: "my:" + name, name, config: conf, mine: true }))];
}
const picKey = (isFan, id, conf) => (isFan ? "f|" : "c|") + id + "|" + JSON.stringify(conf).length;

// render the design gallery and background tiles in the engine, with live readings
async function renderDesignPics(force) {
  if (designBusy || !snap) return;
  const { cur } = curScreen();
  const isFan = cur.kind === "fan";
  const c = scr(selScreen);
  const want = [];
  for (const d of allDesigns(isFan)) { const k = picKey(isFan, d.id, d.config); if (force || !designPics[k]) want.push([k, d.config]); }
  for (const b of meta.backgrounds) {
    if (b.id === "image" || b.id === "video") continue;
    const conf = clone(c); conf.background = b.id; conf.layout = "none"; conf.rotation = 0; conf.dim = b.id === "matrix" ? 0.25 : 0;
    const k = (isFan ? "f|bg|" : "c|bg|") + b.id + "|" + c.accent + c.solid + c.speed;
    if (force || !designPics[k]) want.push([k, conf]);
  }
  if (!want.length) return;
  designBusy = true;
  try {
    const got = await invoke("render_previews", { configs: want, round: isFan, size: 200 });
    Object.assign(designPics, got);
    if (page === "screens") renderScreens(true);
  } catch (e) { console.error(e); } finally { designBusy = false; }
}
async function loadLibrary() {
  try { library = await invoke("lianli_library"); } catch (e) { library = []; }
  if (page === "screens") renderScreens(true);
}

function renderScreens(force) {
  const { list, cur } = curScreen();
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
  $("#scr-wrap").className = "stage-wrap " + (isFan ? "rd" : "sq");
  stage.dataset.preview = selScreen;
  $("#scr-cap").textContent = editing ? "Drag the items to move them. Click one to change it." : cur.connected ? `${cur.name} · live${cur.fps ? " · " + cur.fps.toFixed(0) + " fps" : ""}` : `${cur.name} isn't connected right now`;
  setSwitch($("#scr-enabled"), c.enabled);

  // designs, as real pictures
  const designs = allDesigns(isFan);
  const dSig = designs.map((d) => picKey(isFan, d.id, d.config) + (designPics[picKey(isFan, d.id, d.config)] ? 1 : 0)).join() + c.theme;
  const dEl = $("#scr-designs");
  if (force || dEl.dataset.sig !== dSig) {
    dEl.dataset.sig = dSig;
    dEl.className = "designs " + (isFan ? "rd" : "sq");
    dEl.innerHTML = designs.map((d) => {
      const pic = designPics[picKey(isFan, d.id, d.config)];
      return `<button class="design" data-theme="${esc(d.id)}" aria-pressed="${c.theme === d.id}">${pic ? `<img src="${pic}" alt="">` : `<i class="ph"></i>`}<b>${esc(d.name)}</b>${d.mine ? `<span class="del" data-del="${esc(d.name)}" role="button" aria-label="Delete ${esc(d.name)}">×</span>` : ""}</button>`;
    }).join("");
  }

  renderLayoutBox(c, isFan, force);
  renderOverlay(c);
  renderBackgrounds(c, isFan, force);

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

function readingText(id) { const r = id && reading(id); return r && r.value != null ? fmt(r.value, r.unit) + " " + (r.unit === "FPS" ? "FPS" : r.unit || "") : "no reading"; }
function widgetTitle(w) {
  if (w.kind === "clock") return "Clock";
  if (w.kind === "date") return "Date";
  if (w.kind === "text") return w.label ? `Text: ${w.label}` : "Text";
  return `${WIDGET_NAMES[w.kind] || w.kind}: ${reading(w.sensor)?.label || w.sensor || "nothing chosen"}`;
}
function renderLayoutBox(c, isFan, force) {
  const box = $("#scr-layout-box");
  $("#scr-edit-layout").textContent = editing ? "Done" : "Edit layout";
  $("#scr-edit-layout").className = editing ? "btn small primary" : "btn small";
  const custom = c.layout === "custom";
  const sig = JSON.stringify([c.layout, c.slots, c.widgets, editing, selWidget, selScreen, Object.keys(snap.sensors.groups).length]);
  const typing = box.contains(document.activeElement) && document.activeElement.matches("input, select");
  if (!force && (box.dataset.sig === sig || typing)) {
    $$("[data-live]", box).forEach((el) => { el.textContent = readingText(el.dataset.live); });
    return;
  }
  box.dataset.sig = sig;
  if (!custom) {
    const n = meta.layouts.find((l) => l.id === c.layout)?.slots ?? 0;
    const name = meta.layouts.find((l) => l.id === c.layout)?.name || c.layout;
    box.innerHTML = `<p class="small muted" style="margin:0 0 10px">This design uses the <b>${esc(name)}</b> arrangement${n ? ". Choose what each spot shows, or click Edit layout to move things, resize them and add more." : ", which shows no readings. Click Edit layout to add some."}</p>
      <div class="slots">${Array.from({ length: n }, (_, i) => `<div class="slotrow"><span class="n">${i + 1}</span><select data-slot="${i}" aria-label="Spot ${i + 1}">${sensorOptions(c.slots[i] || "")}</select><span class="val" data-live="${esc(c.slots[i] || "")}">${readingText(c.slots[i])}</span></div>`).join("")}</div>`;
    return;
  }
  const ws = c.widgets || [];
  if (selWidget >= ws.length) selWidget = Math.max(0, ws.length - 1);
  const w = ws[selWidget];
  const items = ws.map((x, i) => `<button class="witem" data-w="${i}" aria-pressed="${editing && i === selWidget}"><span>${esc(widgetTitle(x))}</span>${["clock", "date", "text"].includes(x.kind) ? "" : `<span class="val" data-live="${esc(x.sensor)}">${readingText(x.sensor)}</span>`}</button>`).join("") || `<p class="small muted" style="margin:0">Nothing on the screen yet. Add something below.</p>`;
  const add = `<div class="field"><span>Add to the screen</span><div class="addrow">${meta.widgets.map((k) => `<button class="btn small" data-add="${k.id}">${esc(k.name)}</button>`).join("")}</div></div>`;
  let ed = "";
  if (editing && w) {
    const colors = ["heat", "accent", "text"];
    const custom = !colors.includes(w.color);
    ed = `<div class="wed">
      <div class="row2">
        <div class="field"><span>Type</span><select id="w-kind">${meta.widgets.map((k) => `<option value="${k.id}"${k.id === w.kind ? " selected" : ""}>${esc(k.name)}</option>`).join("")}</select></div>
        ${["clock", "date"].includes(w.kind) ? "<div></div>" : w.kind === "text" ? `<label class="field"><span>Words</span><input type="text" id="w-label" value="${esc(w.label)}" maxlength="40"></label>` : `<label class="field"><span>Shows</span><select id="w-sensor">${sensorOptions(w.sensor, "Choose a reading")}</select></label>`}
      </div>
      <label class="field"><span>Size <b>${Math.round(w.size * 100)}%</b></span><input type="range" id="w-size" min="0.3" max="3" step="0.05" value="${w.size}"></label>
      <div class="field"><span>Colour</span><div class="swatches"><div class="seg" id="w-color">${colors.map((k) => `<button data-v="${k}" aria-pressed="${w.color === k}">${COLOR_NAMES[k]}</button>`).join("")}</div><input type="color" id="w-color-custom" value="${custom ? w.color : c.accent}" aria-label="Own colour"${custom ? ' class="on"' : ""}></div></div>
      ${["value", "gauge", "bar", "graph"].includes(w.kind) ? `<label class="field"><span>Caption</span><input type="text" id="w-caption" value="${esc(w.label)}" placeholder="Uses the reading's name" maxlength="24"></label>` : ""}
      <div class="setrow"><div class="t"><b>Glass plate behind it</b></div><button class="switch" role="switch" id="w-plate" aria-checked="${w.plate}"></button></div>
      <div class="addrow"><button class="btn small" id="w-dup">Duplicate</button><button class="btn small" id="w-remove">Remove</button></div>
    </div>`;
  }
  box.innerHTML = `<div class="witems">${items}</div>${editing ? ed + add : `<p class="small muted" style="margin:10px 0 0">Click Edit layout to move, resize, change or add items.</p>`}`;
}
function renderOverlay(c) {
  const ov = $("#scr-overlay");
  const on = editing && c.layout === "custom";
  ov.hidden = !on;
  if (!on) return;
  const sig = JSON.stringify([c.widgets, selWidget]);
  if (ov.dataset.sig === sig) return;
  ov.dataset.sig = sig;
  ov.innerHTML = (c.widgets || []).map((w, i) => {
    const k = meta.widgets.find((x) => x.id === w.kind) || { w: 0.4, h: 0.2 };
    const bw = Math.min(1, k.w * w.size) * 100, bh = Math.min(1, k.h * w.size) * 100;
    return `<div class="wbox${i === selWidget ? " on" : ""}" data-w="${i}" tabindex="0" aria-label="${esc(widgetTitle(w))}" style="left:${w.x * 100 - bw / 2}%;top:${w.y * 100 - bh / 2}%;width:${bw}%;height:${bh}%"><span>${esc(WIDGET_NAMES[w.kind] || w.kind)}</span></div>`;
  }).join("");
}
function renderBackgrounds(c, isFan, force) {
  const el = $("#scr-bgs");
  const lib = (library || []).filter((x) => x.kind === (isFan ? "fan" : "cooler"));
  const sig = JSON.stringify([c.background, c.image, c.accent, c.solid, isFan, lib.length, meta.backgrounds.map((b) => !!designPics[(isFan ? "f|bg|" : "c|bg|") + b.id + "|" + c.accent + c.solid + c.speed])]);
  if (force || el.dataset.sig !== sig) {
    el.dataset.sig = sig;
    el.className = "bgs " + (isFan ? "rd" : "sq");
    const tiles = meta.backgrounds.filter((b) => b.id !== "video").map((b) => {
      let pic = designPics[(isFan ? "f|bg|" : "c|bg|") + b.id + "|" + c.accent + c.solid + c.speed];
      const on = c.background === b.id;
      if (b.id === "image") return `<button class="bg" data-bg="image" aria-pressed="${on}"><i class="ph pic">${on && c.image ? esc(c.image.split(/[\\/]/).pop()) : "Choose…"}</i><b>Picture or GIF</b></button>`;
      return `<button class="bg" data-bg="${b.id}" aria-pressed="${on}">${pic ? `<img src="${pic}" alt="">` : `<i class="ph"></i>`}<b>${esc(b.name)}</b></button>`;
    });
    const ownVideo = c.background === "video" && !lib.some((x) => x.path === c.image);
    tiles.push(`<button class="bg" data-bg="video" aria-pressed="${ownVideo}"><i class="ph pic">${ownVideo ? esc(c.image.split(/[\\/]/).pop()) : "Choose…"}</i><b>Your video</b></button>`);
    el.innerHTML = tiles.join("");
  }
  const ll = $("#scr-lianli");
  const lSig = JSON.stringify([lib.map((x) => x.path), c.image, c.background, importing, meta.lianli_available]);
  if (!force && ll.dataset.sig === lSig) return;
  ll.dataset.sig = lSig;
  if (lib.length) {
    ll.innerHTML = `<div class="field" style="margin-top:14px"><span>From L-Connect <button class="linkish" id="ll-import">${importing ? "Importing…" : "Import again"}</button></span><div class="bgs vids ${isFan ? "rd" : "sq"}">${lib.map((x) => `<button class="bg" data-video="${esc(x.path)}" aria-pressed="${c.background === "video" && c.image === x.path}">${x.thumb ? `<img src="${x.thumb}" alt="">` : `<i class="ph"></i>`}<b>${esc(x.name)}</b></button>`).join("")}</div></div>`;
  } else if (meta.lianli_available) {
    ll.innerHTML = `<div class="tip" style="margin-top:14px"><b>L-Connect's videos are on this PC.</b> Import them to use as backgrounds. They're copied into Vane's folder, so they keep working if you uninstall L-Connect.<div style="margin-top:8px"><button class="btn primary small" id="ll-import"${importing ? " disabled" : ""}>${importing ? "Importing…" : "Import from L-Connect"}</button></div></div>`;
  } else ll.innerHTML = "";
}

$("#scr-pick").addEventListener("click", (e) => { const b = e.target.closest("[data-key]"); if (b) { selScreen = b.dataset.key; selWidget = 0; renderScreens(true); pullPreviews(); renderDesignPics(); } });
$("#scr-designs").addEventListener("click", (e) => {
  const del = e.target.closest("[data-del]");
  if (del) { delete cfg.my_themes[del.dataset.del]; save(); renderScreens(true); toast("Design deleted"); return; }
  const b = e.target.closest("[data-theme]"); if (!b) return;
  const id = b.dataset.theme;
  const base = id.startsWith("my:") ? cfg.my_themes[id.slice(3)] : meta.themes.find((t) => t.id === id)?.config;
  if (!base) return;
  editing = false;
  editScreen((c) => { const keep = { enabled: c.enabled, brightness: c.brightness, rotation: c.rotation, fps: c.fps }; Object.assign(c, clone(base), keep, { theme: id }); });
  renderDesignPics();
});
$("#scr-edit-layout").onclick = async () => {
  if (editing) { editing = false; renderScreens(true); return; }
  const { cur } = curScreen();
  const c = scr(selScreen);
  if (c.layout !== "custom") {
    const widgets = await invoke("preset_widgets", { config: c, round: cur.kind === "fan" });
    editScreen((x) => { x.widgets = widgets; x.layout = "custom"; });
  }
  editing = true; selWidget = 0;
  renderScreens(true);
};
function editWidget(fn, full = true) {
  editScreen((c) => { const w = c.widgets[selWidget]; if (w) fn(w, c); });
  if (full) renderScreens(true);
}
$("#scr-layout-box").addEventListener("click", (e) => {
  const it = e.target.closest("[data-w]"); if (it) { selWidget = +it.dataset.w; if (!editing) $("#scr-edit-layout").click(); else renderScreens(true); return; }
  const add = e.target.closest("[data-add]");
  if (add) {
    const kind = add.dataset.add;
    const sensor = ["clock", "date", "text"].includes(kind) ? "" : "cpu.load";
    editScreen((c) => { c.widgets.push({ kind, sensor, x: 0.5, y: 0.5, size: 1, color: kind === "clock" ? "accent" : kind === "date" ? "text" : "heat", label: kind === "text" ? "Vane" : "", plate: c.plates }); selWidget = c.widgets.length - 1; });
    renderScreens(true); return;
  }
  const col = e.target.closest("#w-color button"); if (col) { editWidget((w) => { w.color = col.dataset.v; }); return; }
  if (e.target.closest("#w-plate")) { editWidget((w) => { w.plate = !w.plate; }); return; }
  if (e.target.closest("#w-remove")) { editScreen((c) => { c.widgets.splice(selWidget, 1); }); selWidget = Math.max(0, selWidget - 1); renderScreens(true); return; }
  if (e.target.closest("#w-dup")) { editScreen((c) => { const w = clone(c.widgets[selWidget]); w.x = Math.min(0.9, w.x + 0.06); w.y = Math.min(0.9, w.y + 0.06); c.widgets.push(w); selWidget = c.widgets.length - 1; }); renderScreens(true); }
});
$("#scr-layout-box").addEventListener("input", (e) => {
  if (e.target.id === "w-size") { editWidget((w) => { w.size = parseFloat(e.target.value); }, false); e.target.previousElementSibling.querySelector("b").textContent = Math.round(e.target.value * 100) + "%"; renderOverlay(scr(selScreen)); }
  if (e.target.id === "w-label" || e.target.id === "w-caption") editWidget((w) => { w.label = e.target.value; }, false);
  if (e.target.id === "w-color-custom") editWidget((w) => { w.color = e.target.value; }, false);
});
$("#scr-layout-box").addEventListener("change", (e) => {
  if (e.target.dataset.slot != null) { const i = +e.target.dataset.slot; editScreen((c) => { while (c.slots.length <= i) c.slots.push(""); c.slots[i] = e.target.value; }); return; }
  if (e.target.id === "w-kind") editWidget((w) => { w.kind = e.target.value; if (["value", "gauge", "bar", "graph"].includes(w.kind) && !w.sensor) w.sensor = "cpu.load"; });
  if (e.target.id === "w-sensor") editWidget((w) => { w.sensor = e.target.value; });
  if (["w-label", "w-caption", "w-color-custom"].includes(e.target.id)) renderScreens(true);
});
// drag items on the preview
let wdrag = null;
$("#scr-overlay").addEventListener("pointerdown", (e) => {
  const b = e.target.closest(".wbox"); if (!b) return;
  selWidget = +b.dataset.w;
  const r = $("#scr-overlay").getBoundingClientRect();
  const w = scr(selScreen).widgets[selWidget];
  wdrag = { r, dx: (e.clientX - r.left) / r.width - w.x, dy: (e.clientY - r.top) / r.height - w.y };
  b.setPointerCapture(e.pointerId); e.preventDefault();
  renderScreens(true);
});
$("#scr-overlay").addEventListener("pointermove", (e) => {
  if (!wdrag) return;
  const { r, dx, dy } = wdrag;
  const x = Math.max(0.02, Math.min(0.98, (e.clientX - r.left) / r.width - dx));
  const y = Math.max(0.02, Math.min(0.98, (e.clientY - r.top) / r.height - dy));
  const c = clone(scr(selScreen)); c.widgets[selWidget].x = Math.round(x * 200) / 200; c.widgets[selWidget].y = Math.round(y * 200) / 200;
  cfg.screens[selScreen] = c;
  if (cfg.link_fan_screens && selScreen !== "cooler") fanScreens().forEach((s) => { if (s.key !== selScreen) { const o = clone(c); o.enabled = scr(s.key).enabled; cfg.screens[s.key] = o; } });
  save(); renderOverlay(c);
});
$("#scr-overlay").addEventListener("pointerup", () => { wdrag = null; });
$("#scr-overlay").addEventListener("keydown", (e) => {
  const step = e.shiftKey ? 0.05 : 0.01, m = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[e.key];
  if (!m) return; e.preventDefault();
  editWidget((w) => { w.x = Math.max(0.02, Math.min(0.98, w.x + m[0])); w.y = Math.max(0.02, Math.min(0.98, w.y + m[1])); }, false); renderOverlay(scr(selScreen));
});
$("#scr-bgs").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-bg]"); if (!b) return;
  const id = b.dataset.bg;
  if (id === "image") { const p = await invoke("pick_image"); if (p) editScreen((c) => { c.image = p; c.background = "image"; }); return; }
  if (id === "video") { const p = await invoke("pick_video"); if (p) editScreen((c) => { c.image = p; c.background = "video"; }); return; }
  editScreen((c) => { c.background = id; });
});
$("#scr-lianli").addEventListener("click", async (e) => {
  const v = e.target.closest("[data-video]"); if (v) { editScreen((c) => { c.image = v.dataset.video; c.background = "video"; }); return; }
  if (e.target.closest("#ll-import") && !importing) {
    importing = true; renderScreens(true); toast("Copying L-Connect's videos. This takes a moment.");
    try { const n = await invoke("import_lianli"); toast(`Imported ${n} L-Connect video${n === 1 ? "" : "s"}`); } catch (err) { toast(String(err)); }
    importing = false; await loadLibrary();
  }
});
$("#scr-accent").addEventListener("click", (e) => { const b = e.target.closest("[data-accent]"); if (b) { editScreen((c) => { c.accent = b.dataset.accent; }); renderDesignPics(); } });
$("#scr-accent").addEventListener("change", (e) => { if (e.target.id === "scr-accent-custom") { editScreen((c) => { c.accent = e.target.value; }); renderDesignPics(); } });
$("#scr-text").onchange = (e) => editScreen((c) => { c.text = e.target.value; });
$("#scr-solid").onchange = (e) => { editScreen((c) => { c.solid = e.target.value; }); renderDesignPics(); };
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
  if (!name) { $("#scr-theme-name").focus(); toast("Type a name for the design first"); return; }
  const c = clone(scr(selScreen)); delete c.enabled;
  c.theme = "my:" + name;
  cfg.my_themes[name] = c;
  editScreen((x) => { x.theme = "my:" + name; });
  $("#scr-theme-name").value = "";
  toast(`Saved "${name}" to your designs`);
  renderDesignPics();
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
      <div class="field"><span>Fans in this group</span><div class="fanrows">${d.rpm.slice(0, n).map((_, i) => `<div class="fanrow"><b class="num" data-rpm="${i}">${d.rpm[i]} rpm</b><span>Fan ${i + 1} · ${Math.round(d.pwm[i] / 2.55)}% now</span></div>`).join("")}${d.is_aio ? `<div class="fanrow"><b class="num" data-rpm="3">${d.rpm[3]} rpm</b><span>Pump</span></div>` : ""}</div></div>
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

// ------------------------------------------------------------------ lighting
let selLight = null;
const LT_COLORS = ["#2cc4bc", "#b07cff", "#ff6fa8", "#4f9dff", "#ffb347", "#00ff6a", "#ff5a4f", "#ffffff"];
function lightOf(mac) { return cfg.lighting[mac] || { effect: "auto", colors: ["#2cc4bc", "#b07cff"], speed: 5, brightness: 80, reverse: false }; }
function lightDevices() { return (snap.status.wireless || []).filter((d) => zonesOf(d).length); }
function zonesOf(d) {
  const b = d.fan_types.find((x) => x) || 0;
  const per = d.is_aio ? 24 : b >= 20 && b <= 26 ? 40 : (b >= 27 && b <= 35) || (b >= 51 && b <= 58) ? 26 : (b >= 36 && b <= 39) || (b >= 43 && b <= 50) ? 44 : b >= 40 && b <= 42 ? 24 : b >= 59 && b <= 62 ? 52 : 0;
  if (!per || !d.fan_count) return [];
  return [...(d.is_aio ? [24] : []), ...Array(d.fan_count).fill(per)];
}
function effectName(id) { return meta.effects.find((e) => e.id === id)?.name || id; }
function renderLighting() {
  const st = snap.status, w = lightDevices();
  $("#lt-banner").innerHTML = lcBanner() || (st.wireless_error ? `<div class="banner bad"><p><b>The wireless dongle isn't answering.</b> <span class="muted">${esc(st.wireless_error)}</span></p></div>` : "");
  // OpenRGB box
  const live = st.openrgb?.startsWith("Ready");
  $("#orgb-dot").className = "dot " + (live ? (st.openrgb_clients ? "" : "warn") : "off");
  $("#orgb-state").textContent = !cfg.openrgb_server ? "Off" : live ? (st.openrgb_clients ? `OpenRGB connected (${st.openrgb_clients})` : "Waiting for OpenRGB to connect") : st.openrgb || "Starts with Vane";
  $("#orgb-port").textContent = cfg.openrgb_port;
  setSwitch($("#orgb-on"), cfg.openrgb_server);
  if (!w.length) {
    $("#lt-groups").innerHTML = `<div class="box empty">Searching for wireless fans…</div>`;
    $("#lt-editor").innerHTML = `<div class="empty">Lighting groups appear here once the dongle finds them.</div>`;
    return;
  }
  if (!w.find((d) => d.mac === selLight)) selLight = w[0].mac;
  const gSig = JSON.stringify([w.map((d) => d.mac), selLight, cfg.lighting, st.lighting, cfg.fans]);
  const groups = $("#lt-groups");
  if (groups.dataset.sig !== gSig) {
    groups.dataset.sig = gSig;
    groups.innerHTML = w.map((d) => {
      const l = lightOf(d.mac), note = st.lighting?.[d.mac];
      const n = d.fan_count;
      return `<button class="group" data-mac="${esc(d.mac)}" aria-pressed="${d.mac === selLight}"><b>${esc(cfg.fans[d.mac]?.name || d.kind)}</b><span class="sub">${n} fan${n === 1 ? "" : "s"}${d.is_aio ? " + pump" : ""} · ${esc(effectName(l.effect))}${note && note !== "Playing" ? " · " + esc(note) : ""}</span><span class="ring" style="background:${ringCss(l)}"></span></button>`;
    }).join("");
  }
  renderLightEditor();
}
function ringCss(l) {
  if (l.effect === "off") return "var(--surface-hi)";
  if (["rainbow", "wave", "temperature"].includes(l.effect)) return "conic-gradient(#ff3b3b, #ffd43b, #3bff6f, #3bd1ff, #6f3bff, #ff3bd4, #ff3b3b)";
  if (l.effect === "auto" || l.effect === "openrgb") return "conic-gradient(var(--ink-3), var(--surface-hi), var(--ink-3))";
  const c = l.colors.length ? l.colors : ["#ffffff"];
  return c.length === 1 ? c[0] : `conic-gradient(${[...c, c[0]].join(",")})`;
}
function renderLightEditor() {
  const d = lightDevices().find((x) => x.mac === selLight); if (!d) return;
  const l = lightOf(d.mac);
  const ed = $("#lt-editor");
  const note = snap.status.lighting?.[d.mac];
  const sig = JSON.stringify([d.mac, l, note]);
  if (ed.dataset.sig === sig) return;
  if (ed.contains(document.activeElement) && document.activeElement.matches("input")) return;
  ed.dataset.sig = sig;
  const eff = meta.effects.find((e) => e.id === l.effect) || meta.effects[0];
  const anim = !["auto", "static", "off", "temperature", "openrgb"].includes(l.effect);
  ed.innerHTML = `<div style="display:grid;gap:16px">
    <div class="ltview" aria-hidden="true"><canvas id="lt-canvas" width="560" height="170"></canvas></div>
    ${note ? `<div class="src"><span class="dot ${note === "Playing" ? "" : note.startsWith("Waiting") || note.startsWith("Turning") ? "warn" : "hot"}"></span>${esc(note === "Playing" ? "Playing on the fans" : note)}</div>` : ""}
    <div class="field"><span>Effect</span><div class="effects">${meta.effects.map((e) => `<button class="effect" data-eff="${e.id}" aria-pressed="${e.id === l.effect}">${esc(e.name)}</button>`).join("")}</div></div>
    ${l.effect === "auto" ? `<p class="small muted" style="margin:0">Vane sends nothing, so the fans keep whatever lighting they have now.</p>` : ""}
    ${l.effect === "openrgb" ? `<p class="small muted" style="margin:0">OpenRGB sets the colours. Pick another effect to take the lights back.</p>` : ""}
    ${l.effect === "temperature" ? `<p class="small muted" style="margin:0">Blue when cool, through green and amber, to red when hot. Follows the hottest of CPU and GPU.</p>` : ""}
    ${eff.colors ? `<div class="field"><span>Colours</span><div class="swatches" id="lt-colors">${l.colors.map((c, i) => `<span class="lc"><input type="color" data-ci="${i}" value="${c}" aria-label="Colour ${i + 1}">${l.colors.length > 1 ? `<button class="x" data-cdel="${i}" aria-label="Remove colour ${i + 1}">×</button>` : ""}</span>`).join("")}${l.colors.length < 4 ? `<button class="btn small" id="lt-cadd">Add colour</button>` : ""}</div><div class="swatches" style="margin-top:6px">${LT_COLORS.map((c) => `<button class="sw sm" data-cset="${c}" style="background:${c}" aria-label="Use ${c}"></button>`).join("")}</div></div>` : ""}
    ${!["auto", "openrgb"].includes(l.effect) ? `<div class="row2">
      ${anim ? `<label class="field"><span>Speed <b>${l.speed}</b></span><input type="range" id="lt-speed" min="1" max="10" step="1" value="${l.speed}"></label>` : "<div></div>"}
      <label class="field"><span>Brightness <b>${l.brightness}%</b></span><input type="range" id="lt-bright" min="0" max="100" step="5" value="${l.brightness}"></label>
    </div>` : ""}
    ${anim ? `<div class="setrow"><div class="t"><b>Reverse direction</b></div><button class="switch" role="switch" id="lt-rev" aria-checked="${l.reverse}"></button></div>` : ""}
  </div>`;
  ltAnim.set(d, l);
}
// in-window preview of the effect on rings of LEDs, using the same maths as the fans
const ltAnim = (() => {
  let dev = null, light = null, h = 0, t0 = performance.now();
  const hsv = (h) => { h = ((h % 1) + 1) % 1 * 6; const x = 1 - Math.abs((h % 2) - 1); return [[1, x, 0], [x, 1, 0], [0, 1, x], [0, x, 1], [x, 0, 1], [1, 0, x]][Math.floor(h) % 6]; };
  const hex = (s) => [1, 3, 5].map((i) => parseInt(s.slice(i, i + 2), 16) / 255 || 0);
  const heatCol = (t) => { const st = [[35, [0, .35, 1]], [48, [0, .8, .75]], [60, [.1, 1, .2]], [72, [1, .65, 0]], [82, [1, 0, 0]]]; if (t <= 35) return st[0][1]; for (let i = 1; i < st.length; i++) if (t <= st[i][0]) { const f = (t - st[i - 1][0]) / (st[i][0] - st[i - 1][0]); return st[i - 1][1].map((v, k) => v + (st[i][1][k] - v) * f); } return st[4][1]; };
  function color(l, z, nz, i, len, k, n, led) {
    const cols = (l.colors.length ? l.colors : ["#ffffff"]).map(hex), t = k / n, dir = l.reverse ? -1 : 1, pos = i / len;
    switch (l.effect) {
      case "off": return [0, 0, 0];
      case "static": return cols[z % cols.length];
      case "temperature": return heatCol(smart("hottest")?.value ?? 40);
      case "breathing": { const per = n / Math.min(4, cols.length), ci = Math.floor(k / per) % cols.length, v = 0.5 - 0.5 * Math.cos(((k % per) / per) * Math.PI * 2); return cols[ci].map((c) => c * (0.03 + 0.97 * v)); }
      case "cycle": { if (cols.length < 2) return hsv(t * dir); const x = t * cols.length, a = Math.floor(x) % cols.length, f = x % 1, b = cols[(a + 1) % cols.length]; return cols[a].map((c, j) => c + (b[j] - c) * f); }
      case "rainbow": return hsv(pos - t * dir);
      case "wave": return hsv((z / nz) * 0.8 + pos * 0.12 - t * dir);
      case "meteor": { const ii = l.reverse ? len - 1 - i : i, d = (((t * len - ii) % len) + len) % len, v = d < 1 ? 1 : Math.pow(Math.max(0, 1 - d / (len * 0.4)), 1.6); return cols[z % cols.length].map((c) => c * v); }
      case "runway": { const seg = Math.max(1, Math.floor(len / 4)), sh = Math.floor(t * len) * dir, idx = Math.floor((((i + sh) % len) + len) % len / seg) % 2; return idx === 0 ? cols[0] : cols[1] || [0, 0, 0]; }
      case "twinkle": { const r = Math.sin(led * 12.9898) * 43758.5453 % 1, s = Math.abs(r) < 0.35 ? Math.floor(Math.abs(Math.sin(led * 78.233) * 9999 % 1) * n) : -1, age = s < 0 ? 99 : (k + n - s) % n, v = age < 14 ? 0.06 + 0.94 * (1 - age / 14) : 0.06; return cols[led % cols.length].map((c) => c * v); }
      default: return [0.35, 0.4, 0.45];
    }
  }
  function frame(ts) {
    h = requestAnimationFrame(frame);
    const cv = $("#lt-canvas"); if (!cv || !dev || page !== "lighting") return;
    const cx = cv.getContext("2d"), zones = zonesOf(dev), nz = zones.length, per = zones[nz - 1];
    const pace = Math.pow(1.45, 5 - light.speed), info = { breathing: [64 * Math.min(4, light.colors.length || 1), 40], cycle: [96, 50], rainbow: [per, 2000 / per], meteor: [per, 2000 / per], runway: [per, 2000 / per], wave: [90, 40], twinkle: [96, 50] }[light.effect] || [1, 1000];
    const [n, ms] = info, k = Math.floor((ts - t0) / (ms * pace)) % n;
    cx.clearRect(0, 0, cv.width, cv.height);
    const gap = cv.width / nz, rad = Math.min(gap * 0.38, 66), bright = light.brightness / 100;
    let led = 0;
    zones.forEach((len, z) => {
      const cxp = gap * (z + 0.5), cyp = cv.height / 2, isPump = dev.is_aio && z === 0, r = isPump ? rad * 0.62 : rad;
      cx.strokeStyle = "rgba(128,140,150,.35)"; cx.lineWidth = 1; cx.beginPath(); cx.arc(cxp, cyp, r + 9, 0, Math.PI * 2); cx.stroke();
      for (let i = 0; i < len; i++) {
        const c = light.effect === "auto" || light.effect === "openrgb" ? [0.45, 0.5, 0.55] : color(light, z, nz, i, len, k, n, led);
        const a = (i / len) * Math.PI * 2 - Math.PI / 2;
        const rgb = c.map((v) => Math.round(Math.min(1, v * (light.effect === "auto" || light.effect === "openrgb" ? 1 : bright)) * 255));
        cx.fillStyle = `rgb(${rgb})`; cx.shadowColor = `rgb(${rgb})`; cx.shadowBlur = 8;
        cx.beginPath(); cx.arc(cxp + Math.cos(a) * r, cyp + Math.sin(a) * r, isPump ? 3 : 3.6, 0, Math.PI * 2); cx.fill();
        led++;
      }
      cx.shadowBlur = 0; cx.fillStyle = "rgba(128,140,150,.8)"; cx.font = "12px Segoe UI, sans-serif"; cx.textAlign = "center";
      cx.fillText(isPump ? "Pump" : `Fan ${dev.is_aio ? z : z + 1}`, cxp, cyp + 4);
    });
  }
  return { set(d, l) { dev = d; light = l; if (!h && !matchMedia("(prefers-reduced-motion: reduce)").matches) h = requestAnimationFrame(frame); else if (!h) frame(performance.now()); } };
})();
function editLight(fn, full = true) {
  const l = clone(lightOf(selLight)); fn(l); cfg.lighting[selLight] = l; save();
  if (full) renderLighting(); else ltAnim.set(lightDevices().find((x) => x.mac === selLight), l);
}
$("#lt-groups").addEventListener("click", (e) => { const b = e.target.closest("[data-mac]"); if (b) { selLight = b.dataset.mac; renderLighting(); } });
$("#lt-editor").addEventListener("click", (e) => {
  const ef = e.target.closest("[data-eff]"); if (ef) { editLight((l) => { l.effect = ef.dataset.eff; }); return; }
  const del = e.target.closest("[data-cdel]"); if (del) { editLight((l) => { l.colors.splice(+del.dataset.cdel, 1); }); return; }
  if (e.target.closest("#lt-cadd")) { editLight((l) => { l.colors.push(LT_COLORS[l.colors.length % LT_COLORS.length]); }); return; }
  const set = e.target.closest("[data-cset]"); if (set) { editLight((l) => { if (!l.colors.length) l.colors.push(set.dataset.cset); else l.colors[l.colors.length - 1] = set.dataset.cset; }); return; }
  if (e.target.closest("#lt-rev")) editLight((l) => { l.reverse = !l.reverse; });
});
$("#lt-editor").addEventListener("input", (e) => {
  if (e.target.id === "lt-speed") { editLight((l) => { l.speed = +e.target.value; }, false); e.target.previousElementSibling.querySelector("b").textContent = e.target.value; }
  if (e.target.id === "lt-bright") { editLight((l) => { l.brightness = +e.target.value; }, false); e.target.previousElementSibling.querySelector("b").textContent = e.target.value + "%"; }
  if (e.target.dataset.ci != null) editLight((l) => { l.colors[+e.target.dataset.ci] = e.target.value; }, false);
});
$("#lt-editor").addEventListener("change", () => renderLighting());
$("#lt-all").onclick = () => {
  const src = lightOf(selLight);
  lightDevices().forEach((d) => { cfg.lighting[d.mac] = clone(src); });
  save(); renderLighting(); toast(`${effectName(src.effect)} on every group`);
};
$("#orgb-on").onclick = () => { cfg.openrgb_server = !cfg.openrgb_server; save(); renderLighting(); toast("Takes effect the next time Vane starts"); };

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
    [!!src.nvidia, "NVIDIA driver", src.nvidia ? `${src.nvidia}: temperature, power, load, clocks, VRAM, fan` : "No NVIDIA GPU found"],
    [src.hwinfo_shared_memory, "HWiNFO shared memory", src.hwinfo_shared_memory ? "On: every HWiNFO reading is available" : "Off"],
    [src.hwinfo_gadget > 0, "HWiNFO gadget values", src.hwinfo_gadget ? src.hwinfo_gadget + " readings shared" : "None shared"],
    [true, "Windows", "CPU load and RAM"],
    [st.wireless.some((d) => d.is_aio), "HydroShift II", "Coolant and pump"],
  ].map(([ok, n, s]) => `<div class="src"><span class="dot ${ok ? "" : "off"}"></span><span class="nm">${esc(n)}</span><span class="muted">${esc(s)}</span></div>`).join("");
  const tip = $("#set-tip");
  const missing = [!cpuT && ["CPU temperature", "CPU (Tctl/Tdie)", "under the CPU's Enhanced sensors"], !gpuV && ["GPU voltage", "GPU Core Voltage", "under the GPU"]].filter(Boolean);
  if (!missing.length) { tip.hidden = true; }
  else {
    tip.hidden = false;
    tip.innerHTML = `<b>${missing.map((m) => m[0]).join(" and ")} ${missing.length > 1 ? "aren't" : "isn't"} coming through yet.</b> Windows only hands these to programs with a hardware driver, so Vane reads them from HWiNFO. Turn them on there:
      <ol><li>Open HWiNFO's <b>Sensors</b> window.</li>
      ${missing.map((m) => `<li>Find <b>${m[1]}</b> (${m[2]}), right-click it, choose <b>Customize values</b> and tick <b>Report value in Gadget</b>.</li>`).join("")}
      <li>Click OK. The reading shows up here within a few seconds. No time limit, and it survives restarts.</li></ol>
      <span class="small">Or turn on <b>Shared Memory Support</b> in HWiNFO's settings to share every reading (the free version switches it off every 12 hours).</span>
      ${!gpuV && src.nvidia ? `<p class="small" style="margin:8px 0 0">Your Stream Deck's GPU button shows <b>power in watts</b> from the NVIDIA driver. Vane reads that too, as <b>GPU power</b>. NVIDIA doesn't give voltage to apps, which is why it needs HWiNFO.</p>` : ""}`;
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
  if (page === "lighting") renderLighting();
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
  const sp = await invoke("start_page").catch(() => null);
  if (sp) { const [pg, extra] = sp.split(":"); go(pg); if (extra === "edit") setTimeout(() => $("#scr-edit-layout").click(), 300); if (extra === "fan") { const f = fanScreens()[0]; if (f) { selScreen = f.key; renderScreens(true); renderDesignPics(); } } }
  // designs show live readings; refresh their pictures now and then while the page is open
  setInterval(() => { if (page === "screens" && !document.hidden && !editing) renderDesignPics(true); }, 20000);
})();
