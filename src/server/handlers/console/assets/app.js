"use strict";

// The anthroxy console. Every value from the router is inserted as text,
// never as markup.

const TOKEN_KEY = "anthroxy.token";
const TABS = [
  ["overview", "Overview"],
  ["requests", "Requests"],
  ["stats", "Statistics"],
  ["tools", "Tools"],
  ["recordings", "Recordings"],
];

const state = {
  token: null,
  status: null,
  tab: "overview",
  arg: null,
  timers: [],
  statsRange: "7d",
  envFormat: "sh",
  requestsFilter: "",
  errorsOnly: false,
  recordingsFilter: "",
  lastProbe: null,
  lastSmoke: null,
  reloadNotice: null,
  /// Milliseconds the browser's clock runs ahead of the router's.
  skew: 0,
};

const root = document.getElementById("app");

class SignedOut extends Error {}

// ---------------------------------------------------------------- DOM

function h(tag, props, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") el.className = value;
    else if (key.startsWith("on") && typeof value === "function") el.addEventListener(key.slice(2), value);
    else if (key === "value") el.value = value;
    else if (value === true) el.setAttribute(key, "");
    else el.setAttribute(key, String(value));
  }
  append(el, children);
  return el;
}

function append(el, children) {
  for (const child of children.flat(Infinity)) {
    if (child === undefined || child === null || child === false) continue;
    el.append(child instanceof Node ? child : String(child));
  }
}

function replace(el, ...children) {
  el.replaceChildren();
  append(el, children);
}

/// Replaces `el`'s content, keeping what the reader did to it: which
/// `<details>` are open and how far tables and code blocks are scrolled.
/// Elements are matched by their panel and their order within it.
function rerender(el, ...children) {
  const saved = new Map();
  walkViewState(el, (key, node, kind) => {
    saved.set(key, kind === "open" ? node.open : [node.scrollLeft, node.scrollTop]);
  });
  replace(el, ...children);
  walkViewState(el, (key, node, kind) => {
    if (!saved.has(key)) return;
    const value = saved.get(key);
    if (kind === "open") node.open = value;
    else [node.scrollLeft, node.scrollTop] = value;
  });
}

function walkViewState(el, visit) {
  const counts = new Map();
  const keyOf = (node, kind) => {
    const section = node.closest("section.panel");
    const prefix = `${section && el.contains(section) ? section.dataset.key : ""}|${kind}`;
    const index = counts.get(prefix) || 0;
    counts.set(prefix, index + 1);
    return `${prefix}|${index}`;
  };
  el.querySelectorAll("details").forEach((node) => visit(keyOf(node, "open"), node, "open"));
  el.querySelectorAll(".table-wrap, pre").forEach((node) => visit(keyOf(node, "scroll"), node, "scroll"));
}

/// The router's clock, from the `now` its answers carry.
function serverNow() {
  return Date.now() - state.skew;
}

function noteServerTime(now) {
  if (now) state.skew = Date.now() - new Date(now).getTime();
}

/// "3m ago" that keeps counting between refreshes.
function rel(at) {
  return h("span", { class: "tick", "data-at": at }, fmt.relative(at));
}

/// Time since `at`, as an uptime ("2h 5m") or a duration ("1.2 s").
function since(at, format) {
  const el = h("span", { class: "tick", "data-since": at, "data-format": format });
  tickOne(el);
  return el;
}

function tickOne(el) {
  if (el.dataset.at) {
    el.textContent = fmt.relative(el.dataset.at);
    return;
  }
  const ms = Math.max(0, serverNow() - new Date(el.dataset.since).getTime());
  el.textContent = el.dataset.format === "uptime" ? fmt.seconds(Math.floor(ms / 1000)) : fmt.ms(ms);
}

function tick() {
  document.querySelectorAll(".tick").forEach(tickOne);
}

function table(headers, rows, options = {}) {
  const head = h("tr", null, headers.map((header) => {
    const [label, cls] = Array.isArray(header) ? header : [header, null];
    return h("th", { class: cls }, label);
  }));
  const body = rows.length
    ? rows
    : [h("tr", null, h("td", { colspan: headers.length, class: "muted" }, options.empty || "Nothing here yet."))];
  return h("div", { class: "table-wrap" }, h("table", null, h("thead", null, head), h("tbody", null, body)));
}

function badge(text, kind) {
  return h("span", { class: `badge ${kind || ""}` }, text);
}

function panel(title, actions, ...content) {
  return h("section", { class: "panel", "data-key": String(title) },
    h("div", { class: "panel-head" },
      h("h2", null, title),
      actions ? h("div", { class: "actions" }, actions) : null),
    content);
}

function banner(message, kind) {
  return h("div", { class: `banner ${kind || ""}`, role: kind === "info" ? "status" : "alert" }, message);
}

function snippet(text) {
  const button = h("button", { class: "small copy", type: "button" }, "Copy");
  button.addEventListener("click", () => copy(text, button));
  return h("div", { class: "snippet" }, h("pre", null, text), button);
}

async function copy(text, button) {
  const label = button.textContent;
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
    } else {
      const area = h("textarea", { class: "offscreen", readonly: true });
      area.value = text;
      document.body.append(area);
      area.select();
      const done = document.execCommand("copy");
      area.remove();
      if (!done) throw new Error("copy refused");
    }
    button.textContent = "Copied";
  } catch {
    button.textContent = "Select and copy";
  }
  setTimeout(() => { button.textContent = label; }, 1500);
}

// ---------------------------------------------------------------- formatting

const fmt = {
  int: (n) => (n === null || n === undefined ? "–" : Number(n).toLocaleString()),
  ms: (ms) => {
    if (ms === null || ms === undefined) return "–";
    if (ms < 1000) return `${ms} ms`;
    if (ms < 60000) return `${(ms / 1000).toFixed(ms < 10000 ? 2 : 1)} s`;
    return `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s`;
  },
  seconds: (s) => {
    if (s === null || s === undefined) return "–";
    const d = Math.floor(s / 86400), hr = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
    if (d) return `${d}d ${hr}h`;
    if (hr) return `${hr}h ${m}m`;
    if (m) return `${m}m ${s % 60}s`;
    return `${s}s`;
  },
  bytes: (b) => {
    if (b === null || b === undefined) return "–";
    if (b < 1024) return `${b} B`;
    if (b < 1048576) return `${(b / 1024).toFixed(1)} KB`;
    return `${(b / 1048576).toFixed(1)} MB`;
  },
  pct: (r) => (r === null || r === undefined ? "–" : `${(r * 100).toFixed(1)}%`),
  rate: (r) => (r === null || r === undefined ? "–" : `${r.toFixed(1)} tok/s`),
  time: (t) => (t ? new Date(t).toLocaleString() : "–"),
  clock: (t) => (t ? new Date(t).toLocaleTimeString() : "–"),
  relative: (t) => {
    if (!t) return "–";
    const diff = (new Date(t).getTime() - serverNow()) / 1000;
    const abs = Math.abs(diff);
    const text = abs < 60 ? `${Math.round(abs)}s`
      : abs < 3600 ? `${Math.round(abs / 60)}m`
      : abs < 86400 ? `${(abs / 3600).toFixed(1)}h`
      : `${(abs / 86400).toFixed(1)}d`;
    return diff >= 0 ? `in ${text}` : `${text} ago`;
  },
};

function outcomeBadge(view) {
  if (!view.outcome) return badge("in flight", "info");
  if (view.outcome === "complete") return badge("complete", "ok");
  if (view.outcome === "client_disconnected") return badge("client left", "warn");
  return badge("error", "err");
}

function statusBadge(status) {
  if (status === null || status === undefined) return badge("–");
  return badge(String(status), status >= 400 ? "err" : status >= 300 ? "warn" : "ok");
}

// ---------------------------------------------------------------- API

function loadToken() {
  try {
    return sessionStorage.getItem(TOKEN_KEY) || localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

function saveToken(token, remember) {
  try {
    sessionStorage.setItem(TOKEN_KEY, token);
    if (remember) localStorage.setItem(TOKEN_KEY, token);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    // Storage can be off; the token then lasts for this page only.
  }
}

function clearToken() {
  try {
    sessionStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(TOKEN_KEY);
  } catch {
    // Nothing stored.
  }
}

async function api(path, { method = "GET", body, raw = false } = {}) {
  const headers = { authorization: `Bearer ${state.token}` };
  if (body !== undefined) headers["content-type"] = "application/json";
  const res = await fetch(path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    cache: "no-store",
  });
  if (res.status === 401) {
    signOut("The router did not accept the token.");
    throw new SignedOut();
  }
  if (raw) {
    if (!res.ok) {
      const error = new Error(`HTTP ${res.status}`);
      error.status = res.status;
      throw error;
    }
    return res;
  }
  if (res.status === 204) return null;
  const text = await res.text();
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = null;
  }
  if (!res.ok) {
    const error = new Error((data && data.error && data.error.message) || `HTTP ${res.status}`);
    error.status = res.status;
    throw error;
  }
  return data;
}

/// Runs `load`, showing its failure in `target` instead of throwing.
async function guarded(target, load) {
  try {
    await load();
  } catch (error) {
    if (error instanceof SignedOut) return;
    replace(target, banner(`Could not load: ${error.message}`));
  }
}

function every(ms, task) {
  const id = setInterval(() => {
    if (!document.hidden) task();
  }, ms);
  state.timers.push(id);
}

function clearTimers() {
  state.timers.forEach(clearInterval);
  state.timers = [];
}

// ---------------------------------------------------------------- shell

function signOut(message) {
  clearTimers();
  clearToken();
  state.token = null;
  renderSignIn(message);
}

function renderSignIn(message) {
  const input = h("input", {
    type: "password",
    autocomplete: "current-password",
    placeholder: "server.token",
    "aria-label": "Router token",
    required: true,
  });
  const remember = h("input", { type: "checkbox" });
  const status = h("p", { class: "error-text", role: "alert" }, message || "");
  const submit = h("button", { class: "primary", type: "submit" }, "Open console");
  const form = h("form", null,
    h("h1", null, "anthroxy"),
    h("p", { class: "muted" },
      "Enter the router token: ", h("code", null, "server.token"), " in the configuration, the same value as ",
      h("code", null, "ANTHROPIC_AUTH_TOKEN"), "."),
    input,
    h("div", { class: "form-row" },
      h("label", null, remember, " Remember on this browser"),
      submit),
    status);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    submit.disabled = true;
    status.textContent = "";
    const token = input.value.trim();
    try {
      const res = await fetch("/api/status", { headers: { authorization: `Bearer ${token}` }, cache: "no-store" });
      if (res.status === 401) {
        status.textContent = "That is not this router's token.";
        return;
      }
      if (!res.ok) {
        status.textContent = `The router answered HTTP ${res.status}.`;
        return;
      }
      saveToken(token, remember.checked);
      state.token = token;
      renderShell();
    } catch (error) {
      status.textContent = `Cannot reach the router: ${error.message}`;
    } finally {
      submit.disabled = false;
    }
  });
  root.replaceChildren(h("div", { class: "signin" }, form));
  input.focus();
}

const header = {
  meta: null,
  nav: null,
};

function renderShell() {
  header.meta = h("div", { class: "top-meta" });
  header.nav = h("nav", { class: "tabs", role: "tablist", "aria-label": "Console sections" });
  const signOutButton = h("button", { class: "small", type: "button", onclick: () => signOut("Signed out.") }, "Sign out");
  const top = h("header", { class: "top" },
    h("div", { class: "top-row" },
      h("span", { class: "brand" }, "anthroxy"),
      header.meta,
      h("span", { class: "spacer" }),
      signOutButton),
    header.nav);
  const main = h("main", { id: "view" });
  root.replaceChildren(top, main);
  refreshHeader();
  every(10000, refreshHeader);
  route();
}

async function refreshHeader() {
  try {
    const status = await api("/api/status");
    state.status = status;
    noteServerTime(status.now);
    replace(header.meta,
      h("span", null, status.version),
      h("span", null, "up ", since(status.started_at, "uptime")),
      h("span", null, status.listen));
  } catch (error) {
    if (!(error instanceof SignedOut)) replace(header.meta, h("span", { class: "error-text" }, `offline: ${error.message}`));
  }
}

function go(tab, arg) {
  const hash = arg ? `#${tab}/${encodeURIComponent(arg)}` : `#${tab}`;
  if (location.hash === hash) route();
  else location.hash = hash;
}

function route() {
  if (!state.token) return;
  const [tab, arg] = location.hash.slice(1).split("/");
  state.tab = TABS.some(([id]) => id === tab) ? tab : "overview";
  state.arg = arg ? decodeURIComponent(arg) : null;
  replace(header.nav, TABS.map(([id, label]) =>
    h("button", {
      type: "button",
      role: "tab",
      "aria-selected": id === state.tab ? "true" : "false",
      onclick: () => go(id),
    }, label)));
  clearTimers();
  document.querySelectorAll(".chart .plot").forEach((plot) => chartObserver.unobserve(plot));
  state.reloadNotice = null;
  every(10000, refreshHeader);
  every(1000, tick);
  const view = document.getElementById("view");
  view.replaceChildren();
  const views = { overview, requests, stats, tools, recordings };
  views[state.tab](view, state.arg);
}

window.addEventListener("hashchange", route);

// ---------------------------------------------------------------- overview

function overview(view) {
  const body = h("div");
  view.append(body);
  // Redrawn only when something changed, so what the reader unfolded or
  // scrolled stays put; ages tick on their own.
  let drawn = null;
  const load = async () => {
    let status;
    try {
      status = await api("/api/status");
    } catch (error) {
      drawn = null;
      throw error;
    }
    state.status = status;
    noteServerTime(status.now);
    const signature = JSON.stringify({ ...status, now: null, uptime_s: null });
    if (signature === drawn) return;
    drawn = signature;
    rerender(body, overviewContent(status, () => {
      drawn = null;
      return guarded(body, load);
    }));
  };
  guarded(body, load);
  every(5000, () => guarded(body, load));
}

function overviewContent(status, reload) {
  const lastReload = status.reloads[status.reloads.length - 1];
  const cards = h("div", { class: "cards" },
    card("Version", status.version, true),
    card("Uptime", since(status.started_at, "uptime")),
    card("Listening on", status.listen, true),
    card("Configuration", status.config_path || "built in memory", true),
    card("Statistics", status.stats ? `${status.stats.dir} (keeps ${status.stats.retention})` : "off", true),
    card("Body recording", status.body_log ? `${status.body_log.dir} (keeps ${status.body_log.retention})` : "off", true));

  const reloadButton = h("button", { type: "button" }, "Reload configuration");
  reloadButton.addEventListener("click", async () => {
    reloadButton.disabled = true;
    try {
      state.reloadNotice = await api("/api/reload", { method: "POST" });
    } catch (error) {
      if (error instanceof SignedOut) return;
      state.reloadNotice = { result: "failed", error: error.message };
    }
    await reload();
  });
  const notice = state.reloadNotice;
  const reloadResult = !notice ? null
    : notice.result === "applied"
      ? banner(`Reloaded: ${notice.backends} backend(s), ${notice.models} model(s).${notice.restart_needed.length ? ` Restart to apply: ${notice.restart_needed.join(", ")}.` : ""}`, "info")
      : banner(`Not reloaded, the running configuration stays: ${notice.error}`);

  const reloads = [...status.reloads].reverse().map((event) =>
    h("tr", null,
      h("td", { class: "nowrap" }, fmt.time(event.at), h("div", { class: "sub" }, rel(event.at))),
      h("td", null, badge(event.trigger)),
      h("td", null, event.result === "applied" ? badge("applied", "ok") : badge("rejected", "err")),
      h("td", { class: "wrap-anywhere" },
        event.result === "applied"
          ? [`${event.backends} backend(s), ${event.models} model(s)`,
            event.restart_needed.length
              ? h("div", { class: "sub" }, `restart needed for: ${event.restart_needed.join(", ")}`)
              : null]
          : h("span", { class: "error-text" }, event.error))));

  const backends = status.backends.map((backend) => {
    const credential = backend.credential;
    const runs = credential.refreshes.slice(-8).map((run) =>
      h("span", {
        class: `run ${run.error ? "failed" : ""}`,
        title: `${fmt.time(run.at)} · ${run.duration_ms} ms${run.error ? ` · ${run.error}` : ""}`,
      }, run.error ? `✗ ${run.duration_ms}ms` : `✓ ${run.duration_ms}ms`));
    const lastRun = credential.refreshes[credential.refreshes.length - 1];
    return h("tr", null,
      h("td", null, h("strong", null, backend.name), h("div", { class: "sub" }, backend.kind)),
      h("td", { class: "wrap-anywhere" }, backend.url,
        backend.drop_fields.length ? h("div", { class: "sub" }, `drop_fields: ${backend.drop_fields.join(", ")}`) : null,
        backend.anthropic_beta.length ? h("div", { class: "sub" }, `anthropic_beta: ${backend.anthropic_beta.join(", ")}`) : null),
      h("td", { class: "wrap-anywhere" }, credential.source),
      h("td", null, credential.masked ? h("code", null, credential.masked) : h("span", { class: "muted" }, credential.source === "none" ? "none" : "not cached")),
      h("td", { class: "nowrap" },
        credential.fetched_at ? h("div", null, "fetched ", rel(credential.fetched_at)) : null,
        credential.expires_at ? h("div", null, "expires ", rel(credential.expires_at)) : null,
        credential.refresh_at ? h("div", { class: "sub" }, "re-run ", rel(credential.refresh_at)) : null),
      h("td", null,
        runs.length ? h("div", { class: "runs" }, runs) : h("span", { class: "muted" }, "–"),
        lastRun && lastRun.error ? h("div", { class: "sub error-text" }, lastRun.error) : null));
  });

  const models = status.models.map((model) =>
    h("tr", null,
      h("td", null, h("strong", { class: "mono" }, model.id),
        model.id === status.default_model ? h("div", null, badge("default", "info")) : null),
      h("td", null, model.display_name),
      h("td", null, model.backend),
      h("td", { class: "mono wrap-anywhere" }, model.upstream_model),
      h("td", { class: "mono wrap-anywhere" }, model.aliases.length ? model.aliases.join(", ") : "–")));

  const names = status.names.map((name) =>
    h("tr", null,
      h("td", { class: "mono wrap-anywhere" }, name.name),
      h("td", { class: "num" }, fmt.int(name.unknown)),
      h("td", { class: "num" }, fmt.int(name.defaulted)),
      h("td", { class: "nowrap" }, rel(name.last_seen))));

  return [
    panel("Router", null, cards),
    panel("Reloads", reloadButton, reloadResult,
      table(["When", "Trigger", "Result", "Detail"], reloads)),
    panel("Backends", null,
      table(["Backend", "URL", "Credential source", "Value", "Freshness", "Recent runs"], backends)),
    panel("Models", null,
      table(["Id", "Picker label", "Backend", "Upstream model", "Aliases"], models)),
    status.names.length ? panel("Model names nothing serves", null,
      h("p", { class: "note" },
        "Claude Code asked for these names. A 404 failed the request; a default sent it to ",
        h("code", null, status.default_model || "routing.default_model"),
        ". Add the names as aliases of a model to serve them on purpose."),
      table(["Name", ["404", "num"], ["Defaulted", "num"], "Last seen"], names),
      aliasesSnippet(status)) : null,
    panel("Configuration in force", null,
      h("p", { class: "note" }, "Secrets are redacted. ", h("code", null, "${ENV}"), " references are shown expanded."),
      h("details", null, h("summary", null, "Show"), h("pre", null, JSON.stringify(status.config, null, 2)))),
  ];
}

function card(label, value, small) {
  return h("div", { class: "card" },
    h("div", { class: "label" }, label),
    h("div", { class: `value ${small ? "small" : ""}` }, value));
}

function aliasesSnippet(status) {
  const target = status.models.find((m) => m.id === status.default_model) || status.models[0];
  if (!target) return null;
  const aliases = [...new Set([...target.aliases, ...status.names.map((n) => n.name)])];
  const text = `# in the [[models]] entry with id = ${JSON.stringify(target.id)}\naliases = [${aliases.map((a) => JSON.stringify(a)).join(", ")}]`;
  return snippet(text);
}

// ---------------------------------------------------------------- requests

function requests(view, selected) {
  const filter = h("input", { type: "text", placeholder: "Filter by model, backend, id or status", value: state.requestsFilter });
  const errorsOnly = h("input", { type: "checkbox" });
  errorsOnly.checked = state.errorsOnly;
  const inFlight = h("div");
  const recent = h("div");
  const detail = h("div");
  let data = null;

  const draw = () => {
    if (!data) return;
    const needle = filter.value.trim().toLowerCase();
    const matches = (v) => {
      if (errorsOnly.checked && v.outcome !== "error") return false;
      if (!needle) return true;
      return [v.id, v.requested_model, v.model, v.backend, v.upstream_model, v.status, v.peer]
        .some((field) => field !== null && field !== undefined && String(field).toLowerCase().includes(needle));
    };
    rerender(inFlight, table(
      ["Started", "Model", "Backend / upstream", "Status", "Elapsed", "First byte", ["Bytes", "num"]],
      data.in_flight.filter(matches).map((v) =>
        h("tr", { class: "clickable", onclick: () => go("requests", v.id) },
          h("td", { class: "nowrap" }, fmt.clock(v.received_at), h("div", { class: "sub mono" }, v.id)),
          h("td", { class: "mono wrap-anywhere" }, modelCell(v)),
          h("td", { class: "wrap-anywhere" }, v.backend || "–", h("div", { class: "sub mono" }, v.upstream_model || "")),
          h("td", null, statusBadge(v.status)),
          h("td", { class: "num" }, since(v.received_at, "ms")),
          h("td", { class: "num" }, v.ttfb_ms === null ? h("span", { class: "muted" }, "waiting") : fmt.ms(v.ttfb_ms)),
          h("td", { class: "num" }, fmt.bytes(v.bytes)))),
      { empty: "No request in flight." }));
    rerender(recent, table(
      ["Time", "Model", "Backend / upstream", "Status", "Attempts", ["First byte", "num"], ["Duration", "num"], ["Tokens in / out, speed", "num"], ["Cache read", "num"], "Outcome"],
      data.recent.filter(matches).map((v) =>
        h("tr", { class: `clickable ${v.id === selected ? "selected" : ""}`, onclick: () => go("requests", v.id) },
          h("td", { class: "nowrap" }, fmt.clock(v.received_at),
            h("div", { class: "sub" }, v.source === "console" ? badge("console", "info") : v.peer || "")),
          h("td", { class: "mono wrap-anywhere" }, modelCell(v), pathNote(v)),
          h("td", { class: "wrap-anywhere" }, v.backend || "–", h("div", { class: "sub mono" }, v.upstream_model || "")),
          h("td", null, statusBadge(v.status)),
          h("td", { class: "num" }, v.attempts > 1 ? badge(String(v.attempts), "warn") : fmt.int(v.attempts)),
          h("td", { class: "num" }, fmt.ms(v.ttfb_ms)),
          h("td", { class: "num" }, fmt.ms(v.duration_ms)),
          h("td", { class: "num" }, v.usage ? `${fmt.int(v.usage.input)} / ${fmt.int(v.usage.output)}` : "–",
            v.output_tokens_per_second ? h("div", { class: "sub" }, fmt.rate(v.output_tokens_per_second)) : null),
          h("td", { class: "num" }, v.usage ? fmt.int(v.usage.cache_read) : "–"),
          h("td", null, outcomeBadge(v), v.hint_count ? h("div", null, badge(`${v.hint_count} hint`, "warn")) : null))),
      { empty: "No finished request since the router started." }));
  };

  let drawn = null;
  const load = async () => {
    let fresh;
    try {
      fresh = await api("/api/requests");
    } catch (error) {
      drawn = null;
      throw error;
    }
    noteServerTime(fresh.now);
    // Elapsed times tick in the page; only real changes redraw.
    const signature = JSON.stringify({
      in_flight: fresh.in_flight.map(({ elapsed_ms, ...rest }) => rest),
      recent: fresh.recent,
    });
    if (signature === drawn) return;
    drawn = signature;
    data = fresh;
    draw();
  };
  filter.addEventListener("input", () => { state.requestsFilter = filter.value; draw(); });
  errorsOnly.addEventListener("change", () => { state.errorsOnly = errorsOnly.checked; draw(); });

  view.append(
    detail,
    panel("In flight", null, inFlight),
    panel("Recent", h("span", { class: "muted" }, "newest first; kept in memory until restart"),
      h("div", { class: "controls" }, filter, h("label", null, errorsOnly, " Errors only")),
      recent));
  guarded(recent, load);
  every(2000, () => guarded(recent, load));
  if (selected) showRequest(detail, selected);
}

/// Everything but a Messages request says what it was.
function pathNote(v) {
  return v.path === "/v1/messages" ? null : h("div", { class: "sub" }, v.path);
}

function modelCell(v) {
  if (!v.requested_model) return "–";
  if (!v.model) return [v.requested_model, h("div", { class: "sub" }, "no route")];
  if (v.model === v.requested_model) return v.model;
  return [v.requested_model, h("div", { class: "sub" }, `→ ${v.model} (${v.matched})`)];
}

async function showRequest(target, id) {
  await guarded(target, async () => {
    const v = await api(`/api/requests/${encodeURIComponent(id)}`);
    const close = h("button", { type: "button", class: "small", onclick: () => go("requests") }, "Close");
    const recording = v.recording
      ? h("button", { type: "button", class: "small", onclick: () => go("recordings", v.recording) }, "Open recording")
      : null;
    replace(target, panel(`Request ${v.id}`, [recording, close],
      h("dl", { class: "facts" },
        fact("Received", `${fmt.time(v.received_at)} from ${v.peer || "–"}${v.source === "console" ? " (console test)" : ""}`),
        fact("Request", `${v.method} ${v.path}${v.stream ? " (stream)" : ""}`),
        fact("Model", v.requested_model ? `${v.requested_model}${v.model ? ` → ${v.model} (${v.matched})` : " (no route)"}` : "–"),
        fact("Backend", v.backend ? `${v.backend} (${v.kind}), upstream model ${v.upstream_model}` : "–"),
        fact("Status", v.status === null ? "–" : `${v.status}${v.attempts ? ` after ${v.attempts} attempt(s)` : ""}`),
        fact("Timing", `headers ${fmt.ms(v.latency_ms)}, first byte ${fmt.ms(v.ttfb_ms)}, total ${fmt.ms(v.duration_ms)}`),
        fact("Body", fmt.bytes(v.bytes)),
        fact("Tokens", v.usage
          ? `input ${fmt.int(v.usage.input)}, output ${fmt.int(v.usage.output)}, cache read ${fmt.int(v.usage.cache_read)}, cache write ${fmt.int(v.usage.cache_creation)}`
          : "not reported"),
        fact("Output speed", v.output_tokens_per_second
          ? fmt.rate(v.output_tokens_per_second)
          : h("span", { class: "muted" }, "measured on streamed answers that complete")),
        fact("Outcome", outcomeBadge(v))),
      v.error ? [h("h3", null, "Error"), h("pre", null, v.error)] : null,
      v.hints.length ? [h("h3", null, "Hints"), v.hints.map((hint) =>
        h("div", { class: "hint" }, hint.summary, hint.snippet ? snippet(hint.snippet) : null))] : null,
      v.error_body ? [h("h3", null, "Upstream error body"), h("pre", null, prettyJson(v.error_body))] : null));
    target.scrollIntoView({ block: "nearest" });
  });
}

function fact(label, value) {
  return [h("dt", null, label), h("dd", null, value)];
}

function prettyJson(text) {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

// ---------------------------------------------------------------- statistics

function stats(view) {
  const ranges = h("span", { class: "segmented", role: "group", "aria-label": "Range" });
  const body = h("div");
  const drawRanges = () => replace(ranges, [["1d", "24 hours"], ["7d", "7 days"], ["30d", "30 days"], ["all", "All kept"]].map(([id, label]) =>
    h("button", {
      type: "button",
      "aria-pressed": state.statsRange === id ? "true" : "false",
      onclick: () => { state.statsRange = id; drawRanges(); guarded(body, load); },
    }, label)));
  let drawn = null;
  const load = async () => {
    let data;
    try {
      data = await api(`/api/stats?range=${state.statsRange}`);
    } catch (error) {
      drawn = null;
      throw error;
    }
    const signature = JSON.stringify({ ...data, report: data.report && { ...data.report, since: null } });
    if (signature === drawn) return;
    drawn = signature;
    body.querySelectorAll(".chart .plot").forEach((plot) => chartObserver.unobserve(plot));
    rerender(body, statsContent(data));
  };
  drawRanges();
  view.append(panel("Statistics", ranges, body));
  guarded(body, load);
  every(30000, () => guarded(body, load));
}

function statsContent(data) {
  if (!data.enabled) {
    return banner(["Statistics are off. Remove ", h("code", null, "enabled = false"), " from ", h("code", null, "[stats]"), " to record them."], "info");
  }
  const report = data.report;
  const total = report.total;
  const errorRate = total.requests ? total.errors / total.requests : null;
  const cards = h("div", { class: "cards" },
    card("Requests", fmt.int(total.requests)),
    card("Errors", `${fmt.int(total.errors)} (${fmt.pct(errorRate)})`),
    card("Input tokens", fmt.int(total.input_tokens)),
    card("Output tokens", fmt.int(total.output_tokens)),
    card("Cache hit rate", fmt.pct(total.cache_hit_rate)),
    card("Output speed", fmt.rate(total.output_tokens_per_second)));
  const modelRows = report.models.map((row) => statsRow(row, h("td", null, h("strong", { class: "mono" }, row.key), h("div", { class: "sub" }, row.backend || ""))));
  const dayRows = report.days.map((row) => statsRow(row, h("td", { class: "nowrap mono" }, row.key)));
  const headers = (first) => [first, ["Requests", "num"], ["Errors", "num"], ["Retried", "num"], ["Left", "num"],
    ["First byte p50 / p95", "num"], ["Duration p50 / p95", "num"], ["Input", "num"], ["Output", "num"],
    ["Cache read", "num"], ["Cache write", "num"], ["Cache hit", "num"], ["Speed", "num"]];
  return [
    cards,
    report.series.length
      ? [h("h3", null, `Over time, per ${BUCKET_WORDS[report.bucket] || report.bucket}`), statsCharts(report), seriesTable(report)]
      : null,
    h("h3", null, "By model"),
    table(headers("Model"), modelRows, { empty: "No request in this range." }),
    h("h3", null, "By day (UTC)"),
    table(headers("Date"), dayRows, { empty: "No request in this range." }),
    h("p", { class: "note" },
      "Latency percentiles count successful, complete requests only; speed is output tokens over the time after the first byte of streamed answers. Files: ",
      h("code", null, data.dir), "."),
  ];
}

function statsRow(row, first) {
  const errorRate = row.requests ? row.errors / row.requests : null;
  return h("tr", null,
    first,
    h("td", { class: "num" }, fmt.int(row.requests)),
    h("td", { class: "num" }, row.errors ? h("span", { class: "error-text" }, `${fmt.int(row.errors)} (${fmt.pct(errorRate)})`) : "0"),
    h("td", { class: "num" }, fmt.int(row.retried)),
    h("td", { class: "num" }, fmt.int(row.disconnects)),
    h("td", { class: "num" }, `${fmt.ms(row.ttfb_p50_ms)} / ${fmt.ms(row.ttfb_p95_ms)}`),
    h("td", { class: "num" }, `${fmt.ms(row.duration_p50_ms)} / ${fmt.ms(row.duration_p95_ms)}`),
    h("td", { class: "num" }, fmt.int(row.input_tokens)),
    h("td", { class: "num" }, fmt.int(row.output_tokens)),
    h("td", { class: "num" }, fmt.int(row.cache_read_tokens)),
    h("td", { class: "num" }, fmt.int(row.cache_creation_tokens)),
    h("td", { class: "num" }, fmt.pct(row.cache_hit_rate)),
    h("td", { class: "num" }, fmt.rate(row.output_tokens_per_second)));
}

// ---------------------------------------------------------------- charts

const SVG_NS = "http://www.w3.org/2000/svg";
const PLOT_HEIGHT = 140;
const BUCKET_WORDS = { "1h": "hour", "6h": "6 hours", "1d": "day (UTC)" };

/// Charts draw at their card's width, and again when it changes.
const chartObserver = new ResizeObserver((entries) => {
  for (const entry of entries) {
    if (entry.target.redraw) entry.target.redraw();
  }
});

function svg(tag, attrs) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value !== null && value !== undefined) el.setAttribute(key, String(value));
  }
  return el;
}

function present(value) {
  return value !== null && value !== undefined;
}

const axisFormat = {
  count: (v) => Number(v).toLocaleString(),
  compact: (v) => (v >= 1e6 ? `${(v / 1e6).toFixed(v >= 1e7 ? 0 : 1)}M` : v >= 1e3 ? `${(v / 1e3).toFixed(v >= 1e4 ? 0 : 1)}K` : String(Math.round(v))),
  ms: (v) => (v >= 1000 ? `${(v / 1000).toFixed(v % 1000 ? 1 : 0)} s` : `${Math.round(v)} ms`),
  rate: (v) => (v < 10 && v % 1 ? v.toFixed(1) : String(Math.round(v))),
};

/// Four ticks or so on clean numbers from zero.
function niceScale(max, integer) {
  if (!(max > 0)) return integer ? { top: 4, step: 1 } : { top: 1, step: 0.25 };
  const rough = max / 4;
  const power = 10 ** Math.floor(Math.log10(rough));
  const multiples = integer ? [1, 2, 5, 10] : [1, 2, 2.5, 5, 10];
  let step = multiples.map((m) => m * power).find((candidate) => candidate >= rough);
  if (integer) step = Math.max(1, Math.round(step));
  return { top: Math.ceil(max / step) * step, step };
}

/// A bucket as an axis label, or with its span for a readout. Days are UTC
/// dates, as the statistics files are; hours are the viewer's local time.
function bucketLabel(key, bucket, long) {
  const start = new Date(key);
  const day = (d, options = {}) => d.toLocaleDateString([], { month: "short", day: "numeric", ...options });
  const time = (d) => d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const hour = (d) => d.toLocaleTimeString([], { hour: "numeric" });
  if (bucket === "1d") return `${day(start, { timeZone: "UTC" })}${long ? " (UTC)" : ""}`;
  const hours = bucket === "6h" ? 6 : 1;
  const end = new Date(start.getTime() + hours * 3600 * 1000);
  if (!long) return bucket === "6h" ? `${day(start)} ${hour(start)}` : hour(start);
  return `${day(start)} ${time(start)}–${time(end)}`;
}

const measuring = document.createElement("canvas").getContext("2d");

/// Rendered width of axis text, so labels are placed by what they occupy.
function textWidth(text, font) {
  measuring.font = font;
  return measuring.measureText(text).width;
}

function columnPath(x, y, width, height, radius) {
  const r = Math.min(radius, width / 2, height);
  return `M${x},${y + height}V${y + r}Q${x},${y} ${x + r},${y}H${x + width - r}Q${x + width},${y} ${x + width},${y + r}V${y + height}Z`;
}

function statsCharts(report) {
  const rows = report.series;
  const keys = rows.map((row) => row.key);
  const bucket = report.bucket;
  return h("div", { class: "charts" },
    chart({
      title: "Requests",
      kind: "bars",
      integer: true,
      axis: axisFormat.count,
      format: fmt.int,
      keys,
      bucket,
      series: [
        { label: "OK", cls: "s1", values: rows.map((row) => row.requests - row.errors) },
        { label: "Errors", cls: "crit", values: rows.map((row) => row.errors) },
      ],
      extra: (i) => (rows[i].disconnects ? [["client left", fmt.int(rows[i].disconnects)]] : []),
    }),
    chart({
      title: "Tokens",
      kind: "bars",
      integer: true,
      axis: axisFormat.compact,
      format: fmt.int,
      keys,
      bucket,
      series: [
        { label: "Prompt", cls: "s1", values: rows.map((row) => row.input_tokens + row.cache_creation_tokens) },
        { label: "Prompt from cache", cls: "s2", values: rows.map((row) => row.cache_read_tokens) },
        { label: "Output", cls: "s3", values: rows.map((row) => row.output_tokens) },
      ],
    }),
    chart({
      title: "Time to first byte",
      kind: "lines",
      axis: axisFormat.ms,
      format: fmt.ms,
      keys,
      bucket,
      series: [
        { label: "p50", cls: "s1", values: rows.map((row) => row.ttfb_p50_ms) },
        { label: "p95", cls: "s2", values: rows.map((row) => row.ttfb_p95_ms) },
      ],
    }),
    chart({
      title: "Output speed",
      subtitle: "tokens per second of streamed answers",
      kind: "lines",
      axis: axisFormat.rate,
      format: fmt.rate,
      keys,
      bucket,
      series: [{ label: "tok/s", cls: "s1", values: rows.map((row) => row.output_tokens_per_second) }],
    }));
}

function chart(spec) {
  const legend = spec.series.length > 1
    ? h("span", { class: "legend" }, spec.series.map((series) =>
      h("span", null, h("span", { class: `legend-key ${spec.kind === "lines" ? "line" : ""} bg-${series.cls}` }), series.label)))
    : h("span", { class: "legend" }, spec.subtitle || "");
  const plot = h("div", { class: "plot" });
  const tip = h("div", { class: "chart-tip", hidden: true, role: "status" });
  const figure = h("figure", { class: "chart" }, h("figcaption", null, h("span", null, spec.title), legend), plot, tip);
  let width = 0;
  plot.redraw = () => {
    const next = Math.floor(plot.clientWidth);
    if (!next || next === width) return;
    width = next;
    drawChart(plot, tip, spec, width);
  };
  chartObserver.observe(plot);
  return figure;
}

function drawChart(plot, tip, spec, width) {
  const n = spec.keys.length;
  const stackTotals = spec.keys.map((_, i) => spec.series.reduce((sum, series) => sum + (series.values[i] || 0), 0));
  const samples = spec.series.flatMap((series) => series.values.filter(present));
  const max = spec.kind === "bars" ? Math.max(0, ...stackTotals) : Math.max(0, ...samples);
  const { top, step } = niceScale(max, spec.integer);
  const ticks = [];
  for (let v = 0; v <= top + step / 2; v += step) ticks.push(v);
  const font = `11px ${getComputedStyle(plot).fontFamily}`;
  const left = Math.ceil(Math.max(...ticks.map((v) => textWidth(spec.axis(v), font)))) + 10;
  const right = 10;
  const above = 8;
  const below = 22;
  const plotWidth = Math.max(10, width - left - right);
  const height = above + PLOT_HEIGHT + below;
  const band = plotWidth / n;
  const y = (v) => above + PLOT_HEIGHT - (v / top) * PLOT_HEIGHT;
  const cx = (i) => left + band * i + band / 2;

  const root = svg("svg", {
    width,
    height,
    viewBox: `0 0 ${width} ${height}`,
    role: "img",
    tabindex: 0,
    "aria-label": `${spec.title} per ${BUCKET_WORDS[spec.bucket] || spec.bucket}. Arrow keys read each point; the table below has every value.`,
  });

  const grid = svg("g", { class: "grid" });
  const axis = svg("g", { class: "axis" });
  for (const v of ticks) {
    const ty = Math.round(y(v)) + 0.5;
    grid.append(svg("line", { x1: left, x2: left + plotWidth, y1: ty, y2: ty }));
    const label = svg("text", { x: left - 6, y: ty + 3.5, "text-anchor": "end" });
    label.textContent = spec.axis(v);
    axis.append(label);
  }
  // Time labels at a regular stride from the latest bucket, each kept inside
  // the chart and clear of its right-hand neighbour; the latest has none.
  const labels = spec.keys.map((key) => bucketLabel(key, spec.bucket, false));
  const widths = labels.map((text) => textWidth(text, font));
  const stride = Math.max(1, Math.ceil((Math.max(...widths) + 12) / band));
  let room = width + 8;
  for (let i = n - 1; i >= 0; i -= stride) {
    const x = Math.max(0, Math.min(cx(i) - widths[i] / 2, width - widths[i]));
    if (x + widths[i] > room - 8) continue;
    const label = svg("text", { x: x.toFixed(1), y: height - 6, "text-anchor": "start" });
    label.textContent = labels[i];
    axis.append(label);
    room = x;
  }

  const hover = svg("rect", { class: "hover-band", x: 0, y: above, width: Math.max(1, band), height: PLOT_HEIGHT, visibility: "hidden" });
  const marks = svg("g");
  if (spec.kind === "bars") {
    const barWidth = Math.max(1, Math.min(24, band - 2));
    spec.keys.forEach((_, i) => {
      const parts = spec.series.map((series) => [series, series.values[i] || 0]).filter(([, v]) => v > 0);
      let base = y(0);
      parts.forEach(([series, v], j) => {
        const gap = j > 0 ? 2 : 0;
        const size = Math.max(1, (v / top) * PLOT_HEIGHT - gap);
        const yTop = base - gap - size;
        const x = cx(i) - barWidth / 2;
        marks.append(j === parts.length - 1
          ? svg("path", { class: `fill-${series.cls}`, d: columnPath(x, yTop, barWidth, size, 4) })
          : svg("rect", { class: `fill-${series.cls}`, x, y: yTop, width: barWidth, height: size }));
        base = yTop;
      });
    });
  } else {
    for (const series of spec.series) {
      let d = "";
      series.values.forEach((v, i) => {
        if (!present(v)) return;
        const joined = i > 0 && present(series.values[i - 1]);
        d += `${joined ? "L" : "M"}${cx(i).toFixed(1)},${y(v).toFixed(1)}`;
        if (!joined && !(i < n - 1 && present(series.values[i + 1]))) {
          marks.append(svg("circle", { class: `dot fill-${series.cls}`, cx: cx(i), cy: y(v), r: 4 }));
        }
      });
      if (d) marks.prepend(svg("path", { class: `line stroke-${series.cls}`, d }));
    }
  }

  const cross = svg("line", { class: "crosshair", x1: 0, x2: 0, y1: above, y2: above + PLOT_HEIGHT, visibility: "hidden" });
  const pointer = svg("g");
  root.append(grid, hover, marks, cross, pointer, axis);
  if (!stackTotals.some((total) => total > 0) && !samples.length) {
    const empty = svg("text", { class: "empty", x: left + plotWidth / 2, y: above + PLOT_HEIGHT / 2, "text-anchor": "middle" });
    empty.textContent = spec.kind === "bars" ? "No requests in this range" : "No samples in this range";
    root.append(empty);
  }

  let active = null;
  const show = (i) => {
    active = i;
    if (spec.kind === "bars") {
      hover.setAttribute("x", left + band * i);
      hover.setAttribute("visibility", "visible");
    } else {
      cross.setAttribute("x1", cx(i));
      cross.setAttribute("x2", cx(i));
      cross.setAttribute("visibility", "visible");
      pointer.replaceChildren(...spec.series.filter((series) => present(series.values[i])).map((series) =>
        svg("circle", { class: `dot fill-${series.cls}`, cx: cx(i), cy: y(series.values[i]), r: 4 })));
    }
    const rows = [...spec.series].reverse().map((series) => h("div", { class: "row" },
      h("span", { class: `tip-key bg-${series.cls}` }),
      h("strong", null, present(series.values[i]) ? series.format ? series.format(series.values[i]) : spec.format(series.values[i]) : "–"),
      series.label));
    const extra = spec.extra ? spec.extra(i).map(([label, value]) =>
      h("div", { class: "row" }, h("span", { class: "tip-key" }), h("strong", null, value), label)) : [];
    replace(tip, h("div", { class: "when" }, bucketLabel(spec.keys[i], spec.bucket, true)), rows, extra);
    tip.hidden = false;
    const figureWidth = plot.parentElement.clientWidth;
    let x = plot.offsetLeft + cx(i) + 12;
    if (x + tip.offsetWidth > figureWidth) x = plot.offsetLeft + cx(i) - 12 - tip.offsetWidth;
    tip.style.left = `${Math.max(0, x)}px`;
    tip.style.top = `${plot.offsetTop + above}px`;
  };
  const hide = () => {
    active = null;
    hover.setAttribute("visibility", "hidden");
    cross.setAttribute("visibility", "hidden");
    pointer.replaceChildren();
    tip.hidden = true;
  };

  const hits = svg("g");
  spec.keys.forEach((_, i) => {
    const hit = svg("rect", { class: "hit", x: left + band * i, y: 0, width: Math.max(1, band), height: above + PLOT_HEIGHT, "data-bucket": i });
    hit.addEventListener("pointerenter", () => show(i));
    hits.append(hit);
  });
  root.append(hits);
  root.addEventListener("pointerleave", hide);
  root.addEventListener("focus", () => show(active === null ? n - 1 : active));
  root.addEventListener("blur", hide);
  root.addEventListener("keydown", (event) => {
    const moves = { ArrowLeft: -1, ArrowRight: 1, Home: -n, End: n };
    if (!(event.key in moves)) return;
    event.preventDefault();
    show(Math.min(n - 1, Math.max(0, (active === null ? n - 1 : active) + moves[event.key])));
  });
  tip.hidden = true;
  plot.replaceChildren(root);
}

/// Every charted value, for reading without hovering.
function seriesTable(report) {
  return h("details", { class: "chart-table" },
    h("summary", null, "Chart data as a table"),
    table(["When", ["Requests", "num"], ["Errors", "num"], ["Prompt", "num"], ["From cache", "num"], ["Output", "num"], ["First byte p50 / p95", "num"], ["Speed", "num"]],
      [...report.series].reverse().map((row) => h("tr", null,
        h("td", { class: "nowrap" }, bucketLabel(row.key, report.bucket, true)),
        h("td", { class: "num" }, fmt.int(row.requests)),
        h("td", { class: "num" }, fmt.int(row.errors)),
        h("td", { class: "num" }, fmt.int(row.input_tokens + row.cache_creation_tokens)),
        h("td", { class: "num" }, fmt.int(row.cache_read_tokens)),
        h("td", { class: "num" }, fmt.int(row.output_tokens)),
        h("td", { class: "num" }, `${fmt.ms(row.ttfb_p50_ms)} / ${fmt.ms(row.ttfb_p95_ms)}`),
        h("td", { class: "num" }, fmt.rate(row.output_tokens_per_second))))));
}

// ---------------------------------------------------------------- tools

function tools(view) {
  const envBox = h("div");
  const probeBox = h("div");
  const smokeBox = h("div");
  view.append(
    h("div", { class: "grid-2" }, smokePanel(smokeBox), envPanel(envBox)),
    probePanel(probeBox, () => guarded(envBox, () => loadEnv(envBox))));
  guarded(envBox, () => loadEnv(envBox));
  if (state.lastProbe) replace(probeBox, probeContent(state.lastProbe));
  if (state.lastSmoke) replace(smokeBox, smokeContent(state.lastSmoke));
}

function smokePanel(result) {
  const models = (state.status && state.status.models) || [];
  const model = h("select", { "aria-label": "Model" }, models.map((m) => h("option", { value: m.id }, `${m.id} (${m.backend})`)));
  const prompt = h("input", { type: "text", placeholder: "Reply with one short sentence.", "aria-label": "Prompt" });
  const stream = h("input", { type: "checkbox" });
  stream.checked = true;
  const send = h("button", { class: "primary", type: "submit" }, "Send");
  const form = h("form", null,
    h("p", { class: "note" }, "Sends one short ", h("code", null, "/v1/messages"),
      " request through the model's route, as Claude Code would. It reaches the backend and costs tokens."),
    h("div", { class: "form-row" }, model, h("label", null, stream, " Stream"), send),
    h("div", { class: "form-row" }, prompt),
    result);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    send.disabled = true;
    replace(result, h("p", { class: "muted" }, "Waiting for the answer…"));
    try {
      const body = { model: model.value, stream: stream.checked };
      if (prompt.value.trim()) body.prompt = prompt.value.trim();
      state.lastSmoke = await api("/api/smoke", { method: "POST", body });
      replace(result, smokeContent(state.lastSmoke));
    } catch (error) {
      if (!(error instanceof SignedOut)) replace(result, banner(error.message));
    } finally {
      send.disabled = false;
    }
  });
  return panel("Test request", null, form);
}

function smokeContent(s) {
  return [
    h("dl", { class: "facts" },
      fact("Status", [statusBadge(s.status), " ", h("button", { type: "button", class: "link", onclick: () => go("requests", s.request_id) }, s.request_id)]),
      fact("Route", s.backend ? `${s.model} → ${s.backend} / ${s.upstream_model}${s.stream ? " (stream)" : ""}` : s.model),
      fact("Timing", `first byte ${fmt.ms(s.ttfb_ms)}, total ${fmt.ms(s.duration_ms)}${s.output_tokens_per_second ? `, ${fmt.rate(s.output_tokens_per_second)}` : ""}`),
      fact("Tokens", s.usage ? `input ${fmt.int(s.usage.input)}, output ${fmt.int(s.usage.output)}, cache read ${fmt.int(s.usage.cache_read)}` : "not reported")),
    s.text ? h("div", { class: "answer" }, s.text)
      : s.status < 400 && !s.error
        ? h("p", { class: "note" }, "The answer had no text. A reasoning model can spend the whole 128-token budget on thinking.")
        : null,
    s.error ? h("pre", { class: "error-text" }, s.error) : null,
    s.hints.map((hint) => h("div", { class: "hint" }, hint.summary, hint.snippet ? snippet(hint.snippet) : null)),
  ];
}

function envPanel(box) {
  return panel("Claude Code environment", null,
    h("p", { class: "note" }, "For the address this browser used to reach the router, which is what the Windows host can use as well."),
    box);
}

async function loadEnv(box) {
  const env = await api("/api/env");
  const formats = [["sh", "bash / zsh"], ["powershell", "PowerShell"], ["json", "settings.json"]];
  const shown = h("div");
  const switcher = h("span", { class: "segmented", role: "group", "aria-label": "Format" });
  const draw = () => {
    replace(switcher, formats.map(([id, label]) => h("button", {
      type: "button",
      "aria-pressed": state.envFormat === id ? "true" : "false",
      onclick: () => { state.envFormat = id; draw(); },
    }, label)));
    replace(shown, snippet(env[state.envFormat]));
  };
  draw();
  replace(box,
    h("div", { class: "controls" }, switcher),
    shown,
    h("p", { class: "note" }, `Base URL ${env.base_url}; this browser is ${env.viewer}. `,
      env.max_context_tokens
        ? ["CLAUDE_CODE_MAX_CONTEXT_TOKENS is the smallest context window the last probe found among configured models."]
        : "Run a probe to add CLAUDE_CODE_MAX_CONTEXT_TOKENS when backends report context windows."));
}

function probePanel(box, afterProbe) {
  const run = h("button", { type: "button", class: "primary" }, "Probe backends");
  run.addEventListener("click", async () => {
    run.disabled = true;
    replace(box, h("p", { class: "muted" }, "Probing every backend…"));
    try {
      state.lastProbe = await api("/api/probe", { method: "POST" });
      replace(box, probeContent(state.lastProbe));
      afterProbe();
    } catch (error) {
      if (!(error instanceof SignedOut)) replace(box, banner(error.message));
    } finally {
      run.disabled = false;
    }
  });
  return panel("Backend probe", run,
    h("p", { class: "note" }, "Acquires each credential and lists each backend's models from inside the running router, with its environment. ",
      "Use it when ", h("code", null, "anthroxy check"), " passes in a shell but the service fails."),
    box);
}

function probeContent(probe) {
  return [
    h("p", { class: "note" }, `Probed ${fmt.time(probe.at)}. `,
      probe.max_context_tokens ? `Smallest context window among configured models: ${fmt.int(probe.max_context_tokens)} tokens.` : "No configured model reported a context window."),
    probe.backends.map((backend) => {
      const models = backend.models;
      let modelLine;
      if (!models) modelLine = h("span", { class: "muted" }, "model list skipped: no credential");
      else if (models.error) modelLine = h("span", { class: "error-text" }, models.error);
      else modelLine = [statusBadge(models.status), ` in ${fmt.ms(models.latency_ms)}`, models.detail ? h("span", { class: "error-text" }, `: ${models.detail}`) : null];
      const listed = models && models.listed ? models.listed.map((m) => {
        const cell = h("td");
        const show = h("button", { type: "button", class: "small" }, "[[models]]");
        show.addEventListener("click", () => replace(cell, snippet(m.snippet)));
        cell.append(show);
        return h("tr", null,
          h("td", { class: "mono wrap-anywhere" }, m.id),
          h("td", { class: "num" }, m.context_length ? fmt.int(m.context_length) : "–"),
          h("td", { class: "mono" }, m.configured_as.length ? m.configured_as.join(", ") : h("span", { class: "muted" }, "not configured")),
          cell);
      }) : [];
      return h("div", null,
        h("h3", null, backend.name, " ", badge(backend.kind), " ", h("span", { class: "muted mono" }, backend.url)),
        h("dl", { class: "facts" },
          fact("Credential", backend.credential.ok ? backend.credential.text : h("span", { class: "error-text" }, backend.credential.text)),
          fact("GET /v1/models", modelLine)),
        listed.length ? table(["Upstream model", ["Context", "num"], "Configured as", "Snippet"], listed) : null);
    }),
  ];
}

// ---------------------------------------------------------------- recordings

function recordings(view, opened) {
  const filter = h("input", { type: "text", placeholder: "Filter by model, backend, id, status or outcome", value: state.recordingsFilter });
  const list = h("div");
  const viewer = h("div");
  const refresh = h("button", { type: "button" }, "Refresh");
  const removeAll = h("button", { type: "button", class: "danger" }, "Delete all");
  let data = null;

  const draw = () => {
    if (!data) return;
    if (!data.dir) {
      removeAll.disabled = true;
      replace(list, banner(["Body recording is off. Set ", h("code", null, "logging.body_dir"), " or pass ", h("code", null, "--body-dir"), " to record each exchange."], "info"));
      return;
    }
    const needle = filter.value.trim().toLowerCase();
    const entries = data.entries.filter((e) => !needle || [e.name, e.model, e.backend, e.status, e.outcome, e.path]
      .some((field) => field !== null && field !== undefined && String(field).toLowerCase().includes(needle)));
    replace(list,
      h("p", { class: "note" }, `${data.entries.length} recording(s) in `, h("code", null, data.dir), `, kept for ${data.retention}. They hold whole conversations.`),
      table(["Time", "Request", "Model", "Status", "Outcome", ["Size", "num"], "Files", ""], entries.map((e) => {
        const remove = h("button", { type: "button", class: "small danger" }, "Delete");
        remove.addEventListener("click", async () => {
          if (!confirm(`Delete recording ${e.name}?`)) return;
          remove.disabled = true;
          try {
            await api(`/api/recordings/${encodeURIComponent(e.name)}`, { method: "DELETE" });
            if (opened === e.name) replace(viewer);
            await load();
          } catch (error) {
            if (!(error instanceof SignedOut)) replace(viewer, banner(error.message));
          }
        });
        return h("tr", { class: e.name === opened ? "selected" : "" },
          h("td", { class: "nowrap" }, fmt.time(e.at)),
          h("td", { class: "mono" }, e.request_id, h("div", { class: "sub" }, e.path || "")),
          h("td", { class: "mono wrap-anywhere" }, e.model || "–", h("div", { class: "sub" }, e.backend || "")),
          h("td", null, statusBadge(e.status)),
          h("td", { class: "wrap-anywhere" }, e.outcome || h("span", { class: "muted" }, "in progress")),
          h("td", { class: "num" }, fmt.bytes(e.bytes)),
          h("td", null, h("div", { class: "runs" }, e.files.map((file) =>
            h("button", { type: "button", class: "small", onclick: () => openFile(viewer, e.name, file, e.files) }, file)))),
          h("td", null, remove));
      }), { empty: needle ? "No recording matches." : "No recording yet." }));
  };

  let openedShown = false;
  const load = async () => {
    data = await api("/api/recordings");
    draw();
    if (opened && !openedShown) {
      openedShown = true;
      const entry = data.entries.find((e) => e.name === opened);
      openFile(viewer, opened, "meta.json", entry ? entry.files : null);
    }
  };

  filter.addEventListener("input", () => { state.recordingsFilter = filter.value; draw(); });
  refresh.addEventListener("click", () => guarded(list, load));
  removeAll.addEventListener("click", async () => {
    if (!confirm("Delete every recording? This cannot be undone.")) return;
    removeAll.disabled = true;
    try {
      const result = await api("/api/recordings", { method: "DELETE" });
      replace(viewer, banner(`Deleted ${result.removed} recording(s).`, "info"));
      await load();
    } catch (error) {
      if (!(error instanceof SignedOut)) replace(viewer, banner(error.message));
    } finally {
      removeAll.disabled = false;
    }
  });

  view.append(
    viewer,
    panel("Recordings", [refresh, removeAll], h("div", { class: "controls" }, filter), list));
  guarded(list, load);
}

/// Displays at most this much of a recorded file.
const SHOWN_BYTES = 2 * 1024 * 1024;

/// `files` are the entry's files, offered as the next ones to open.
async function openFile(viewer, name, file, files) {
  await guarded(viewer, async () => {
    let res;
    try {
      res = await api(`/api/recordings/${encodeURIComponent(name)}/${encodeURIComponent(file)}`, { raw: true });
    } catch (error) {
      if (error.status !== 404) throw error;
      replace(viewer, banner(`Recording ${name} has no ${file}: it was deleted, pruned after its retention, or never written.`, "info"));
      return;
    }
    const text = await res.text();
    const cut = text.length > SHOWN_BYTES;
    const shown = cut ? text.slice(0, SHOWN_BYTES) : text;
    const close = h("button", { type: "button", class: "small", onclick: () => replace(viewer) }, "Close");
    const siblings = (files || [])
      .filter((other) => other !== file)
      .map((other) => h("button", { type: "button", class: "small", onclick: () => openFile(viewer, name, other, files) }, other));
    replace(viewer, panel(`${name} / ${file}`, [siblings, close],
      cut ? banner(`Showing the first ${fmt.bytes(SHOWN_BYTES)} of ${fmt.bytes(text.length)}.`, "info") : null,
      h("pre", null, file.endsWith(".json") && !cut ? prettyJson(shown) : shown)));
    viewer.scrollIntoView({ block: "nearest" });
  });
}

// ---------------------------------------------------------------- start

state.token = loadToken();
if (state.token) renderShell();
else renderSignIn();
