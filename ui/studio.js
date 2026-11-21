// Design studio: build a design from any built-in one or a blank screen, add parts (gauges, numbers,
// text) on top, save it as your own, put it on a screen, and share it as a zip. Uses app.js helpers.
"use strict";

const studio = { c: null, round: false, sel: 0, psel: "", name: "", busy: false, drag: null, picsBusy: false, loading: false };

// the pieces of the chosen full-screen design (Telemetry white shares Telemetry's)
function stPieces(c = studio.c) {
  if (!c || !isFace(c)) return [];
  let face = c.background.slice(5);
  if (face === "telemetry-white") face = "telemetry";
  return (meta.pieces || []).filter((p) => p.face === face);
}
const pieceOf = (id) => studio.c.face_parts?.[id] || { x: 0, y: 0, size: 1, hide: false };
function setPiece(id, fn) {
  const c = studio.c;
  c.face_parts = c.face_parts || {};
  const p = { x: 0, y: 0, size: 1, hide: false, ...(c.face_parts[id] || {}) };
  fn(p);
  if (!p.hide && Math.abs(p.x) < 0.001 && Math.abs(p.y) < 0.001 && Math.abs((p.size || 1) - 1) < 0.001) delete c.face_parts[id]; else c.face_parts[id] = p;
}

function stBlank() {
  const c = clone(defaultScreen("cooler"));
  Object.assign(c, { theme: "", background: "solid", solid: "#05070a", image: "", layout: "custom", widgets: [], slots: ["fps", "gpu.temp"], num_size: 1, label_size: 1, face_color: "", outline: false, plates: false, blur: 0, dim: 0 });
  c.widgets.push({ kind: "ring", sensor: "fps", x: 0.5, y: 0.45, size: 1.4, color: "accent", label: "", plate: false });
  c.widgets.push({ kind: "segments", sensor: "gpu.temp", x: 0.5, y: 0.84, size: 1.1, color: "heat", label: "", plate: false });
  return c;
}

// every design opens as movable parts; a full-screen design keeps drawing itself underneath
async function stLoad(conf, name) {
  const tok = studio.tok = (studio.tok || 0) + 1; // a later load wins over an earlier one still waiting
  const c = clone(conf);
  delete c.enabled;
  if (isFace(c)) { if (c.layout !== "custom") { c.layout = "custom"; c.widgets = []; } }
  else if (c.layout !== "custom") {
    try { c.widgets = await invoke("preset_widgets", { config: c, round: studio.round }); } catch (e) { c.widgets = []; }
    c.layout = "custom";
  }
  if (tok !== studio.tok) return;
  c.widgets = c.widgets || [];
  c.face_parts = c.face_parts || {};
  studio.c = c; studio.name = name || ""; studio.sel = 0; studio.psel = "";
  $("#st-name").value = studio.name;
  renderStudio(true);
}
async function openStudio(conf, name, round) {
  studio.round = !!round;
  go("studio");
  await stLoad(conf, name);
}

function stEdit(fn, full = true) { fn(studio.c); if (full) renderStudio(true); else stOverlay(); }
function stPart(fn, full = true) { stEdit((c) => { const w = c.widgets[studio.sel]; if (w) fn(w, c); }, full); }

// ------------------------------------------------------------------ live preview
function stPreviewCfg() {
  const c = clone(studio.c);
  c.rotation = 0;
  if (!studio.round) c.corner = scr("cooler").corner || 0;
  return c;
}
async function stFrame() {
  if (page !== "studio" || !studio.c || studio.busy || document.hidden) return;
  studio.busy = true;
  try {
    const src = await invoke("studio_frame", { config: stPreviewCfg(), round: studio.round, size: 360, t: performance.now() / 1000 });
    $("#st-stage").src = src;
  } catch (e) { console.error(e); } finally { studio.busy = false; }
}
setInterval(stFrame, 120);

// ------------------------------------------------------------------ start-from pictures
const stPicKey = (round, d) => "st|" + (round ? "rd" : "sq") + "|" + d.id + "|" + JSON.stringify(d.config).length;
async function stPics() {
  if (studio.picsBusy) return;
  const want = allDesigns(studio.round).filter((d) => !designPics[stPicKey(studio.round, d)]).map((d) => [stPicKey(studio.round, d), clone(d.config)]);
  if (!want.length) return;
  studio.picsBusy = true;
  try {
    Object.assign(designPics, await invoke("render_previews", { configs: want, round: studio.round, size: 200 }));
    if (page === "studio") renderStudio(true);
  } catch (e) { console.error(e); } finally { studio.picsBusy = false; }
}

// ------------------------------------------------------------------ drawing the page
function stTitle(w) { return widgetTitle(w); }
function renderStudio(force) {
  if (!snap || !meta) return;
  if (!studio.c) {
    if (studio.loading) return;
    studio.loading = true;
    const c = scr(selScreen); studio.round = curScreen().cur.kind === "fan";
    stLoad(c, c.theme.startsWith("my:") ? c.theme.slice(3) : "").finally(() => { studio.loading = false; });
    return;
  }
  const pg = $("#p-studio");
  const typing = pg.contains(document.activeElement) && document.activeElement.matches("input, select");
  if (!force) {
    $$("[data-live]", pg).forEach((el) => { el.textContent = readingText(el.dataset.live); });
    if (!typing) stTargets();
    return;
  }
  const c = studio.c, round = studio.round;
  setSeg($("#st-shape"), round ? "rd" : "sq");
  $("#st-stage").className = round ? "rd" : "sq";
  $("#st-wrap").className = "stage-wrap " + (round ? "rd" : "sq");

  // start from
  const designs = allDesigns(round);
  const from = $("#st-from");
  from.className = "designs " + (round ? "rd" : "sq");
  from.innerHTML = `<button class="design" data-from="blank"><i class="ph pic">Blank</i><b>Blank screen</b></button>` + designs.map((d) => {
    const pic = designPics[stPicKey(round, d)];
    return `<button class="design" data-from="${esc(d.id)}">${pic ? `<img src="${pic}" alt="">` : `<i class="ph"></i>`}<b>${esc(d.name)}</b></button>`;
  }).join("");
  stPics();

  // base
  const face = isFace(c);
  const faces = meta.themes.filter((t) => (t.config.background || "").startsWith("face:"));
  const bgOpts = `<optgroup label="Full-screen designs">${faces.map((t) => `<option value="${esc(t.config.background)}"${c.background === t.config.background ? " selected" : ""}>${esc(t.name)}</option>`).join("")}</optgroup>
    <optgroup label="Backgrounds">${meta.backgrounds.filter((b) => b.id !== "video").map((b) => `<option value="${esc(b.id)}"${c.background === b.id ? " selected" : ""}>${esc(b.id === "image" ? "Picture or GIF" : b.name)}</option>`).join("")}<option value="video"${c.background === "video" ? " selected" : ""}>Video</option></optgroup>`;
  const media = ["image", "video"].includes(c.background) ? `<div class="field"><span>${c.background === "video" ? "Video" : "Picture or GIF"}</span><div class="inline-name"><span class="small muted" style="flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;align-self:center">${esc(c.image ? c.image.split(/[\\/]/).pop() : "None chosen")}</span><button class="btn small" id="st-media">Choose…</button></div></div>` : "";
  const ns = c.num_size || 1, ls = c.label_size || 1;
  $("#st-base").innerHTML = `
    <label class="field"><span>Background</span><select id="st-bg">${bgOpts}</select></label>
    ${media}
    <div class="row3">
      <label class="field"><span>Accent colour</span><input type="color" id="st-accent" value="${esc(c.accent)}"></label>
      <label class="field"><span>Text colour</span><input type="color" id="st-text" value="${esc(c.text)}"></label>
      ${c.background === "solid" ? `<label class="field"><span>Background colour</span><input type="color" id="st-solid" value="${esc(c.solid)}"></label>` : face ? `<div class="field"><span>Design colour</span><div class="swatches"><input type="color" id="st-fcol" value="${esc(c.face_color || c.accent)}" aria-label="Design colour">${c.face_color ? `<button class="linkish" id="st-fcol-reset">Design's own</button>` : ""}</div></div>` : "<div></div>"}
    </div>
    ${face ? `<div class="row2">
      <label class="field"><span>Number size <b>${Math.round(ns * 100)}%</b></span><input type="range" id="st-num" min="0.6" max="1.6" step="0.05" value="${ns}"></label>
      <label class="field"><span>Label size <b>${Math.round(ls * 100)}%</b></span><input type="range" id="st-lbl" min="0.6" max="1.8" step="0.05" value="${ls}"></label>
    </div>` : ""}
    <div class="row2">
      <label class="field"><span>Animation speed <b>${c.speed.toFixed(1)}×</b></span><input type="range" id="st-speed" min="0" max="3" step="0.1" value="${c.speed}"></label>
      ${face ? "<div></div>" : `<label class="field"><span>Dim background <b>${Math.round(c.dim * 100)}%</b></span><input type="range" id="st-dim" min="0" max="0.8" step="0.05" value="${c.dim}"></label>`}
    </div>`;

  // readings of a full-screen design
  $("#st-read-box").hidden = !face;
  if (face) $("#st-read").innerHTML = faceSlotsHtml(c).replace(/^<div class="slots">|<\/div>$/g, "");

  // pieces of the design itself
  const pieces = stPieces(c);
  $("#st-pieces-box").hidden = !pieces.length;
  if (pieces.length) {
    $("#st-pieces").innerHTML = pieces.map((p) => {
      const v = pieceOf(p.id), k = v.size || 1, changed = v.hide || v.x || v.y || Math.abs(k - 1) > 0.001;
      return `<div class="prow${v.hide ? " off" : ""}" aria-current="${studio.psel === p.id}">
        <button class="pname" data-psel="${esc(p.id)}">${esc(p.name)}</button>
        <input type="range" data-psize="${esc(p.id)}" min="0.3" max="2.5" step="0.05" value="${k}" aria-label="Size of ${esc(p.name)}">
        <b class="pv">${Math.round(k * 100)}%</b>
        <button class="switch" role="switch" data-pshow="${esc(p.id)}" aria-checked="${!v.hide}" aria-label="Show ${esc(p.name)}"></button>
        ${changed ? `<button class="linkish" data-preset="${esc(p.id)}">Reset</button>` : "<span></span>"}
      </div>`;
    }).join("");
  }

  // parts
  const ws = c.widgets;
  if (studio.sel >= ws.length) studio.sel = Math.max(0, ws.length - 1);
  const w = ws[studio.sel];
  const items = ws.map((x, i) => `<button class="witem" data-w="${i}" aria-pressed="${i === studio.sel}"><span>${esc(stTitle(x))}</span>${SENSOR_KINDS.includes(x.kind) ? `<span class="val" data-live="${esc(x.sensor)}">${readingText(x.sensor)}</span>` : ""}</button>`).join("")
    || `<p class="small muted" style="margin:0">${face ? "Only the design itself so far." : "Nothing on the screen yet."} Add a part below.</p>`;
  let ed = "";
  if (w) {
    const colors = ["heat", "accent", "text"];
    const own = !colors.includes(w.color);
    ed = `<div class="wed">
      <div class="row2">
        <label class="field"><span>Type</span><select id="sw-kind">${meta.widgets.map((k) => `<option value="${k.id}"${k.id === w.kind ? " selected" : ""}>${esc(k.name)}</option>`).join("")}</select></label>
        ${["clock", "date"].includes(w.kind) ? "<div></div>" : w.kind === "text" ? `<label class="field"><span>Words</span><input type="text" id="sw-label" value="${esc(w.label)}" maxlength="40"></label>` : `<label class="field"><span>Shows</span><select id="sw-sensor">${sensorOptions(w.sensor, "Choose a reading")}</select></label>`}
      </div>
      <label class="field"><span>Size <b>${Math.round(w.size * 100)}%</b></span><input type="range" id="sw-size" min="0.3" max="3" step="0.05" value="${w.size}"></label>
      <div class="field"><span>Colour</span><div class="swatches"><div class="seg" id="sw-color">${colors.map((k) => `<button data-v="${k}" aria-pressed="${w.color === k}">${COLOR_NAMES[k]}</button>`).join("")}</div><input type="color" id="sw-color-custom" value="${own ? esc(w.color) : esc(c.accent)}" aria-label="Own colour"></div></div>
      ${SENSOR_KINDS.includes(w.kind) ? `<label class="field"><span>Caption</span><input type="text" id="sw-caption" value="${esc(w.label)}" placeholder="Uses the reading's name" maxlength="24"></label>` : ""}
      <div class="setrow"><div class="t"><b>Glass plate behind it</b></div><button class="switch" role="switch" id="sw-plate" aria-checked="${w.plate}"></button></div>
      <div class="addrow"><button class="btn small" id="sw-dup">Duplicate</button><button class="btn small" id="sw-remove">Remove</button></div>
    </div>`;
  }
  $("#st-parts").innerHTML = `<div class="witems">${items}</div>${ed}<div class="field" style="margin-top:14px"><span>Add a part</span><div class="addrow">${meta.widgets.map((k) => `<button class="btn small" data-add="${k.id}">${esc(k.name)}</button>`).join("")}</div></div>`;

  // my designs
  const mine = Object.keys(cfg.my_themes);
  $("#st-mine").innerHTML = mine.length ? mine.map((n) => `<div class="witem" style="align-items:center"><span>${esc(n)}</span><span class="addrow" style="flex-wrap:nowrap"><button class="btn small" data-mine-edit="${esc(n)}">Edit</button><button class="btn small" data-mine-export="${esc(n)}">Export</button><button class="btn small" data-mine-del="${esc(n)}" aria-label="Delete ${esc(n)}">Delete</button></span></div>`).join("")
    : `<p class="small muted" style="margin:0">Designs you save or import show up here.</p>`;

  stTargets(true);
  stOverlay(true);
}

function stTargets(force) {
  const sel = $("#st-target");
  const list = screensOf();
  const opts = [["cooler", "Cooler screen"], ...fanScreens().map((s) => [s.key, s.name])];
  if (fanScreens().length > 1) opts.push(["fans", "All LCD fans"]);
  if (!list.length && !opts.length) return;
  const sig = opts.map((o) => o[0]).join();
  if (!force && sel.dataset.sig === sig) return;
  const want = sel.value || (studio.round ? (fanScreens().length > 1 ? "fans" : fanScreens()[0]?.key) : "cooler");
  sel.dataset.sig = sig;
  sel.innerHTML = opts.map(([k, n]) => `<option value="${esc(k)}"${k === want ? " selected" : ""}>Use on: ${esc(n)}</option>`).join("");
}

function stOverlay(force) {
  const ov = $("#st-overlay");
  const ws = studio.c?.widgets || [];
  const sig = JSON.stringify([ws, studio.sel, studio.psel, studio.c?.face_parts, studio.c?.background]);
  if (!force && ov.dataset.sig === sig) return;
  ov.dataset.sig = sig;
  const pcs = stPieces().map((p) => {
    const v = pieceOf(p.id), k = v.size || 1;
    const bw = Math.min(1, p.w * k) * 100, bh = Math.min(1, p.h * k) * 100;
    const x = (p.x + (v.x || 0)) * 100, y = (p.y + (v.y || 0)) * 100;
    return `<div class="wbox piece${v.hide ? " off" : ""}${studio.psel === p.id ? " on" : ""}" data-p="${esc(p.id)}" tabindex="0" aria-label="${esc(p.name)}" style="left:${x - bw / 2}%;top:${y - bh / 2}%;width:${bw}%;height:${bh}%"><span>${esc(p.name)}</span></div>`;
  }).join("");
  ov.innerHTML = pcs + ws.map((w, i) => {
    const k = meta.widgets.find((x) => x.id === w.kind) || { w: 0.4, h: 0.2 };
    const bw = Math.min(1, k.w * w.size) * 100, bh = Math.min(1, k.h * w.size) * 100;
    return `<div class="wbox${i === studio.sel && !studio.psel ? " on" : ""}" data-w="${i}" tabindex="0" aria-label="${esc(stTitle(w))}" style="left:${w.x * 100 - bw / 2}%;top:${w.y * 100 - bh / 2}%;width:${bw}%;height:${bh}%"><span>${esc(WIDGET_NAMES[w.kind] || w.kind)}</span></div>`;
  }).join("");
}

// ------------------------------------------------------------------ actions
$("#st-shape").onclick = (e) => { const b = e.target.closest("button"); if (!b) return; studio.round = b.dataset.v === "rd"; renderStudio(true); };
$("#st-from").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-from]"); if (!b) return;
  const id = b.dataset.from;
  if (id === "blank") { await stLoad(stBlank(), ""); return; }
  const d = allDesigns(studio.round).find((x) => x.id === id) || allDesigns(!studio.round).find((x) => x.id === id);
  if (d) await stLoad(d.config, d.mine ? d.name : "");
});
$("#st-base").addEventListener("change", async (e) => {
  const t = e.target;
  if (t.id === "st-bg") {
    const v = t.value;
    if (v === "image" || v === "video") {
      const p = await invoke(v === "image" ? "pick_image" : "pick_video");
      if (!p) { renderStudio(true); return; }
      stEdit((c) => { c.background = v; c.image = p; });
      return;
    }
    stEdit((c) => { if (c.background !== v) c.face_parts = {}; c.background = v; if (v.startsWith("face:") && !c.slots[0]) c.slots = ["fps", "gpu.temp"]; });
    return;
  }
  const map = { "st-accent": "accent", "st-text": "text", "st-solid": "solid", "st-fcol": "face_color" };
  if (map[t.id]) stEdit((c) => { c[map[t.id]] = t.value; });
  if (["st-num", "st-lbl", "st-speed", "st-dim"].includes(t.id)) renderStudio(true);
});
$("#st-base").addEventListener("input", (e) => {
  const t = e.target;
  const map = { "st-num": "num_size", "st-lbl": "label_size", "st-speed": "speed", "st-dim": "dim" };
  if (!map[t.id]) return;
  stEdit((c) => { c[map[t.id]] = parseFloat(t.value); }, false);
  const b = t.previousElementSibling.querySelector("b");
  if (b) b.textContent = t.id === "st-speed" ? parseFloat(t.value).toFixed(1) + "×" : Math.round(t.value * 100) + "%";
});
$("#st-base").addEventListener("click", async (e) => {
  if (e.target.closest("#st-fcol-reset")) { stEdit((c) => { c.face_color = ""; }); return; }
  if (e.target.closest("#st-media")) {
    const v = studio.c.background;
    const p = await invoke(v === "video" ? "pick_video" : "pick_image");
    if (p) stEdit((c) => { c.image = p; });
  }
});
$("#st-read").addEventListener("change", (e) => {
  if (e.target.dataset.slot == null) return;
  const i = +e.target.dataset.slot;
  stEdit((c) => { while (c.slots.length <= i) c.slots.push(""); c.slots[i] = e.target.value; });
});
$("#st-parts").addEventListener("click", (e) => {
  const it = e.target.closest("[data-w]"); if (it) { studio.sel = +it.dataset.w; studio.psel = ""; renderStudio(true); return; }
  const add = e.target.closest("[data-add]");
  if (add) {
    const kind = add.dataset.add;
    const sensor = ["clock", "date", "text"].includes(kind) ? "" : PART_SENSOR[kind] || "cpu.load";
    stEdit((c) => { c.widgets.push({ kind, sensor, x: 0.5, y: 0.5, size: 1, color: kind === "clock" ? "accent" : kind === "date" ? "text" : ["value", "gauge", "bar", "graph"].includes(kind) ? "heat" : "accent", label: kind === "text" ? "Vane" : "", plate: false }); studio.sel = c.widgets.length - 1; });
    return;
  }
  const col = e.target.closest("#sw-color button"); if (col) { stPart((w) => { w.color = col.dataset.v; }); return; }
  if (e.target.closest("#sw-plate")) { stPart((w) => { w.plate = !w.plate; }); return; }
  if (e.target.closest("#sw-remove")) { stEdit((c) => { c.widgets.splice(studio.sel, 1); studio.sel = Math.max(0, studio.sel - 1); }); return; }
  if (e.target.closest("#sw-dup")) { stEdit((c) => { const w = clone(c.widgets[studio.sel]); w.x = Math.min(0.9, w.x + 0.06); w.y = Math.min(0.9, w.y + 0.06); c.widgets.push(w); studio.sel = c.widgets.length - 1; }); }
});
$("#st-parts").addEventListener("input", (e) => {
  const t = e.target;
  if (t.id === "sw-size") { stPart((w) => { w.size = parseFloat(t.value); }, false); t.previousElementSibling.querySelector("b").textContent = Math.round(t.value * 100) + "%"; }
  if (t.id === "sw-label" || t.id === "sw-caption") stPart((w) => { w.label = t.value; }, false);
  if (t.id === "sw-color-custom") stPart((w) => { w.color = t.value; }, false);
});
$("#st-parts").addEventListener("change", (e) => {
  const t = e.target;
  if (t.id === "sw-kind") stPart((w) => { w.kind = t.value; if (SENSOR_KINDS.includes(w.kind) && !w.sensor) w.sensor = PART_SENSOR[w.kind] || "cpu.load"; });
  if (t.id === "sw-sensor") stPart((w) => { w.sensor = t.value; });
  if (["sw-label", "sw-caption", "sw-color-custom"].includes(t.id)) renderStudio(true);
});

// drag parts on the preview
$("#st-overlay").addEventListener("pointerdown", (e) => {
  const b = e.target.closest(".wbox"); if (!b) return;
  if (b.dataset.p) {
    const id = b.dataset.p, p = stPieces().find((x) => x.id === id), v = pieceOf(id);
    studio.psel = id;
    const r = $("#st-overlay").getBoundingClientRect();
    studio.drag = { r, piece: p, dx: (e.clientX - r.left) / r.width - (p.x + (v.x || 0)), dy: (e.clientY - r.top) / r.height - (p.y + (v.y || 0)) };
    b.setPointerCapture(e.pointerId); e.preventDefault();
    renderStudio(true);
    return;
  }
  studio.psel = "";
  studio.sel = +b.dataset.w;
  const r = $("#st-overlay").getBoundingClientRect();
  const w = studio.c.widgets[studio.sel];
  studio.drag = { r, dx: (e.clientX - r.left) / r.width - w.x, dy: (e.clientY - r.top) / r.height - w.y };
  b.setPointerCapture(e.pointerId); e.preventDefault();
  renderStudio(true);
});
$("#st-overlay").addEventListener("pointermove", (e) => {
  if (!studio.drag) return;
  const { r, dx, dy } = studio.drag;
  if (studio.drag.piece) {
    const p = studio.drag.piece;
    const x = Math.max(0.02, Math.min(0.98, (e.clientX - r.left) / r.width - dx));
    const y = Math.max(0.02, Math.min(0.98, (e.clientY - r.top) / r.height - dy));
    setPiece(p.id, (v) => { v.x = Math.round((x - p.x) * 200) / 200; v.y = Math.round((y - p.y) * 200) / 200; });
    stOverlay();
    return;
  }
  const w = studio.c.widgets[studio.sel]; if (!w) return;
  w.x = Math.round(Math.max(0.02, Math.min(0.98, (e.clientX - r.left) / r.width - dx)) * 200) / 200;
  w.y = Math.round(Math.max(0.02, Math.min(0.98, (e.clientY - r.top) / r.height - dy)) * 200) / 200;
  stOverlay();
});
$("#st-overlay").addEventListener("pointerup", () => { const was = studio.drag?.piece; studio.drag = null; if (was) renderStudio(true); });
$("#st-overlay").addEventListener("keydown", (e) => {
  const step = e.shiftKey ? 0.05 : 0.01, m = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[e.key];
  if (!m) return; e.preventDefault();
  if (studio.psel) { setPiece(studio.psel, (v) => { v.x = Math.round((v.x + m[0]) * 200) / 200; v.y = Math.round((v.y + m[1]) * 200) / 200; }); stOverlay(); return; }
  stPart((w) => { w.x = Math.max(0.02, Math.min(0.98, w.x + m[0])); w.y = Math.max(0.02, Math.min(0.98, w.y + m[1])); }, false);
});

// save, use, share
$("#st-save").onclick = () => {
  const name = $("#st-name").value.trim();
  if (!name) { $("#st-name").focus(); toast("Type a name for the design first"); return; }
  const had = !!cfg.my_themes[name];
  const c = clone(studio.c); c.theme = "my:" + name;
  cfg.my_themes[name] = c; studio.name = name; studio.c.theme = c.theme;
  save(); renderStudio(true);
  toast(had ? `Updated "${name}"` : `Saved "${name}" to your designs`);
};
$("#st-use").onclick = () => {
  const target = $("#st-target").value || "cooler";
  const keys = target === "fans" ? fanScreens().map((s) => s.key) : [target];
  const theme = studio.name && cfg.my_themes[studio.name] ? "my:" + studio.name : studio.c.theme || "";
  for (const k of keys) {
    const cur = scr(k);
    const keep = { enabled: cur.enabled, brightness: cur.brightness, rotation: cur.rotation, fps: cur.fps, corner: cur.corner, contrast: cur.contrast, saturation: cur.saturation, gamma: cur.gamma, black: cur.black };
    cfg.screens[k] = Object.assign(clone(studio.c), keep, { theme });
  }
  save();
  toast(target === "fans" ? "Now showing on all LCD fans" : `Now showing on ${$("#st-target").selectedOptions[0]?.textContent.replace("Use on: ", "") || "the screen"}`);
};
async function stExport(name, conf) {
  try {
    const path = await invoke("export_design", { name, config: conf });
    if (path) toast("Exported to " + path.split(/[\\/]/).pop());
  } catch (e) { toast("Couldn't export: " + e); }
}
$("#st-export").onclick = () => stExport($("#st-name").value.trim() || studio.name || "My design", studio.c);
$("#st-import").onclick = async () => {
  try {
    const r = await invoke("import_design");
    if (!r) return;
    const [name, conf] = r;
    let n = name, i = 2;
    while (cfg.my_themes[n]) n = `${name} ${i++}`;
    conf.theme = "my:" + n;
    cfg.my_themes[n] = conf;
    save();
    await stLoad(conf, n);
    toast(`Imported "${n}"`);
  } catch (e) { toast(String(e)); }
};
$("#st-mine").addEventListener("click", async (e) => {
  const ed = e.target.closest("[data-mine-edit]"); if (ed) { const n = ed.dataset.mineEdit; await stLoad(cfg.my_themes[n], n); return; }
  const ex = e.target.closest("[data-mine-export]"); if (ex) { const n = ex.dataset.mineExport; stExport(n, cfg.my_themes[n]); return; }
  const del = e.target.closest("[data-mine-del]");
  if (del) {
    const n = del.dataset.mineDel;
    if (del.dataset.sure !== "1") { del.dataset.sure = "1"; del.textContent = "Sure?"; setTimeout(() => { if (del.isConnected) { del.dataset.sure = ""; del.textContent = "Delete"; } }, 3000); return; }
    delete cfg.my_themes[n]; save();
    if (studio.name === n) studio.name = "";
    renderStudio(true); toast(`Deleted "${n}"`);
  }
});

// design pieces: select, resize, show or hide, put back
$("#st-pieces").addEventListener("click", (e) => {
  const sel = e.target.closest("[data-psel]"); if (sel) { studio.psel = sel.dataset.psel; renderStudio(true); return; }
  const sh = e.target.closest("[data-pshow]"); if (sh) { const id = sh.dataset.pshow; setPiece(id, (v) => { v.hide = !v.hide; }); studio.psel = id; renderStudio(true); return; }
  const rs = e.target.closest("[data-preset]"); if (rs) { delete studio.c.face_parts[rs.dataset.preset]; renderStudio(true); }
});
$("#st-pieces").addEventListener("input", (e) => {
  const t = e.target.closest("[data-psize]"); if (!t) return;
  const id = t.dataset.psize;
  setPiece(id, (v) => { v.size = parseFloat(t.value); });
  studio.psel = id;
  t.nextElementSibling.textContent = Math.round(t.value * 100) + "%";
  stOverlay();
});
$("#st-pieces").addEventListener("change", (e) => { if (e.target.closest("[data-psize]")) renderStudio(true); });
$("#st-pieces-reset").onclick = () => { studio.c.face_parts = {}; studio.psel = ""; renderStudio(true); toast("Every piece is back in its place"); };
