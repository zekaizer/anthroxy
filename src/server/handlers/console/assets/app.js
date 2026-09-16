"use strict";

// The anthroxy console. Every value from the router is inserted as text,
// never as markup.

const TOKEN_KEY = "anthroxy.token";
const SMOKE_MODEL_KEY = "anthroxy.smoke.model";
const TABS = [
  ["overview", "Overview"],
  ["requests", "Requests"],
  ["stats", "Statistics"],
  ["tools", "Tools"],
  ["recordings", "Recordings"],
];

/// Rows a list draws at first, and adds per "Show more". Enough to see what
/// the router has been doing without a page nine screens long; the filter is
/// how a list of hundreds is searched, not the scrollbar.
const LIST_PAGE = 25;

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
  recordingPart: "request",
  recordingView: "sections",
  requestSection: "prompt",
  requestFind: "",
  /// Recording `requestFind` was typed in.
  inspected: null,
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

/// A row that opens something: reachable and operable by keyboard, with the
/// buttons inside it keeping their own handling.
function openRow(props, open, ...cells) {
  const tr = h("tr", {
    ...props,
    tabindex: 0,
    onclick: open,
    onkeydown: (event) => {
      if (event.target !== tr || (event.key !== "Enter" && event.key !== " ")) return;
      event.preventDefault();
      open();
    },
  }, cells);
  return tr;
}

/// What something is doing or how it ended.
function badge(text, kind) {
  return h("span", { class: `badge state ${kind || ""}` }, text);
}

/// What kind of thing it is: a backend's dialect, a reload's trigger, where a
/// header came from. A classification carries no judgement, so no colour.
function kindBadge(text) {
  return h("span", { class: "badge kind" }, text);
}

/// An annotation on something a client sent, not on the router's own state.
function tag(text) {
  return h("span", { class: "badge tag" }, text);
}

const HEALTH_KIND = { ok: "ok", attention: "warn", trouble: "err" };

/// The header's standing answer to "is anything wrong", on every tab, and the
/// way to the account of it.
function healthPill(health) {
  if (!health) return null;
  const count = health.problems.length;
  return h("button", {
    type: "button",
    class: `badge state ${HEALTH_KIND[health.level] || ""}`,
    title: count ? health.problems.map((problem) => problem.summary).join("\n") : "Nothing to report",
    onclick: () => go("overview"),
  }, count ? `${fmt.int(count)} need${count === 1 ? "s" : ""} attention` : "All backends ready");
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
    // Inside the clocks' own disagreement, so neither "0s ago" nor "in 0s".
    if (abs < 1.5) return "just now";
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

/// An attempt count is a number, not a state. Only an abnormal one is marked.
function attemptsCell(attempts) {
  if (!attempts || attempts <= 1) return fmt.int(attempts);
  return h("strong", { class: "warn-text" }, `×${fmt.int(attempts)}`);
}

function statusBadge(status) {
  if (status === null || status === undefined) return h("span", { class: "muted" }, "–");
  const kind = status >= 400 ? "err" : status >= 300 ? "warn" : "ok";
  return h("span", { class: `badge http ${kind}` }, String(status));
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

/// What a form field keeps between visits. Storage can be off, and a choice
/// that does not survive is only an inconvenience.
const remembers = {
  get(key) {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  set(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Nothing to remember by.
    }
  },
};

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

/// `load` as a poll: once something has been read, a later failure says above
/// it how old the reading is instead of taking it away. A refresh that missed
/// is not a reason to empty the page a reader is looking at.
function polled(target, load) {
  let fresh = null;
  let notice = null;
  return async () => {
    try {
      await load();
      fresh = serverNow();
      notice?.remove();
      notice = null;
    } catch (error) {
      if (error instanceof SignedOut) return;
      if (fresh === null) {
        replace(target, banner(`Could not load: ${error.message}`));
        return;
      }
      if (!notice) {
        notice = h("div", { class: "banner warn", role: "status" });
        target.parentNode.insertBefore(notice, target);
      }
      replace(notice, "Showing the last reading from ", rel(new Date(fresh).toISOString()),
        `. The router did not answer the refresh: ${error.message}`);
    }
  };
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

async function renderShell() {
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
  // The roles above promise the tab pattern; these keys are the rest of it.
  // One tab is in the tab order and the arrows move between them, as a screen
  // reader announcing a tablist tells its user they will.
  header.nav.addEventListener("keydown", (event) => {
    const moves = { ArrowLeft: -1, ArrowRight: 1, Home: -TABS.length, End: TABS.length };
    if (!(event.key in moves)) return;
    event.preventDefault();
    const at = TABS.findIndex(([id]) => id === state.tab);
    const next = Math.min(TABS.length - 1, Math.max(0, at + moves[event.key]));
    // The hash change rebuilds the tabs, so the focus has to wait for it or it
    // lands on a button that is about to be replaced.
    focusTabAfterRoute = true;
    go(TABS[next][0]);
  });
  const main = h("main", { id: "view", role: "tabpanel" });
  root.replaceChildren(top, main);
  // Tabs read the model list from the first status, so it comes first.
  await refreshHeader();
  route();
}

async function refreshHeader() {
  try {
    const status = await api("/api/status");
    state.status = status;
    noteServerTime(status.now);
    replace(header.meta,
      healthPill(status.health),
      // Every tab refreshes itself; this says whether what is on screen is
      // still being fed, which nothing did.
      h("span", null, "read ", rel(new Date(serverNow()).toISOString())),
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

/// Functions taking a new argument for the tab drawn in a view element, for
/// tabs that return one.
const retargets = new WeakMap();

/// Set when the arrow keys chose the tab, so the new one takes the focus.
let focusTabAfterRoute = false;

function route() {
  if (!state.token) return;
  const [hashTab, hashArg] = location.hash.slice(1).split("/");
  const tab = TABS.some(([id]) => id === hashTab) ? hashTab : "overview";
  const arg = hashArg ? decodeURIComponent(hashArg) : null;
  const view = document.getElementById("view");
  // The same hash again rebuilds the tab, which is how a tab is reloaded.
  if (tab === state.tab && arg !== state.arg && retargets.has(view)) {
    state.arg = arg;
    retargets.get(view)(arg);
    return;
  }
  state.tab = tab;
  state.arg = arg;
  replace(header.nav, TABS.map(([id, label]) =>
    h("button", {
      type: "button",
      role: "tab",
      id: `tab-${id}`,
      "aria-controls": "view",
      "aria-selected": id === state.tab ? "true" : "false",
      // Roving: the tablist holds one tab stop, and the arrows move within it.
      tabindex: id === state.tab ? 0 : -1,
      onclick: () => go(id),
    }, label)));
  view.setAttribute("aria-labelledby", `tab-${state.tab}`);
  if (focusTabAfterRoute) {
    focusTabAfterRoute = false;
    header.nav.children[TABS.findIndex(([id]) => id === state.tab)].focus();
  }
  clearTimers();
  document.querySelectorAll(".chart .plot").forEach((plot) => chartObserver.unobserve(plot));
  state.reloadNotice = null;
  every(10000, refreshHeader);
  every(1000, tick);
  view.replaceChildren();
  const views = { overview, requests, stats, tools, recordings };
  const retarget = views[state.tab](view, state.arg);
  if (retarget) retargets.set(view, retarget);
  else retargets.delete(view);
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
      return refresh();
    }));
  };
  const refresh = polled(body, load);
  refresh();
  every(5000, refresh);
}

function overviewContent(status, reload) {
  const lastReload = status.reloads[status.reloads.length - 1];
  const cards = h("div", { class: "cards" },
    card("Version", status.version, { small: true }),
    card("Uptime", since(status.started_at, "uptime")),
    card("Listening on", status.listen, { small: true }),
    card("Configuration", status.config_path || "built in memory", { small: true }),
    card("Statistics", status.stats ? `${status.stats.dir} (${kept(status.stats.retention)})` : "off", { small: true }),
    card("Body recording", status.body_log ? `${status.body_log.dir} (${kept(status.body_log.retention)})` : "off", { small: true }));

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
      h("td", null, kindBadge(event.trigger)),
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
        backend.drop_headers.length ? h("div", { class: "sub" }, `drop_headers: ${backend.drop_headers.join(", ")}`) : null,
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
    attentionPanel(status.health),
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

/// The account behind the header's pill. Absent when there is nothing to
/// say, so its presence is the news.
function attentionPanel(health) {
  if (!health || !health.problems.length) return null;
  return panel("Needs attention", null,
    health.problems.map((problem) =>
      h("div", { class: "problem" },
        badge(problem.backend || problem.kind, HEALTH_KIND[problem.level] || ""),
        h("div", null,
          problem.summary,
          problem.detail ? h("div", { class: "sub mono wrap-anywhere" }, problem.detail) : null),
        problem.kind === "errors"
          ? h("button", { type: "button", class: "small", onclick: () => { state.errorsOnly = true; go("requests"); } }, "Show the requests")
          : null)));
}

/// A retention as the configuration spells it; "0s" turns pruning off.
function kept(retention) {
  return retention === "0s" ? "kept indefinitely" : `kept for ${retention}`;
}

/// A stat tile: one label, one value, and a sub-line when the number needs
/// its denominator or a caveat. `small` is for a value that is a path or a
/// sentence rather than a number.
function card(label, value, { small, sub } = {}) {
  return h("div", { class: "card" },
    h("div", { class: "label" }, label),
    h("div", { class: `value ${small ? "small" : ""}` }, value),
    sub ? h("div", { class: "sub" }, sub) : null);
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
  const inFlight = h("div", { class: "strip" });
  const recent = h("div");
  const detail = h("aside", { class: "detail" });
  const master = h("div", { class: "master" });
  let data = null;
  /// Rows the table draws, kept across the two-second poll so a refresh does
  /// not fold back what the reader asked to see.
  let shown = LIST_PAGE;

  const draw = () => {
    if (!data) return;
    const needle = filter.value.trim().toLowerCase();
    const matches = (v) => {
      if (errorsOnly.checked && v.outcome !== "error") return false;
      if (!needle) return true;
      return [v.id, v.requested_model, v.model, v.backend, v.upstream_model, v.status, v.peer]
        .some((field) => field !== null && field !== undefined && String(field).toLowerCase().includes(needle));
    };
    const running = data.in_flight.filter(matches);
    // Nothing in flight is the usual state, and a table header over an empty
    // row is a third of the first screen spent saying so.
    rerender(inFlight, running.length
      ? table(
        ["Started", "Model", "Backend / upstream", "Status", "Elapsed", "First byte", ["Bytes", "num"]],
        running.map((v) =>
          openRow({ class: "clickable", "aria-label": `Request ${v.id}` }, () => go("requests", v.id),
            h("td", { class: "nowrap" }, fmt.clock(v.received_at), h("div", { class: "sub mono" }, v.id)),
            h("td", { class: "mono wrap-anywhere" }, modelCell(v)),
            h("td", { class: "wrap-anywhere" }, v.backend || "–", h("div", { class: "sub mono" }, v.upstream_model || "")),
            h("td", null, statusBadge(v.status)),
            h("td", { class: "num" }, since(v.received_at, "ms")),
            h("td", { class: "num" }, v.ttfb_ms === null ? h("span", { class: "muted" }, "waiting") : fmt.ms(v.ttfb_ms)),
            h("td", { class: "num" }, fmt.bytes(v.bytes)))))
      : h("p", { class: "idle" }, badge("idle", "ok"), " Nothing in flight.",
        data.recent.length ? [" The last request finished ", rel(data.recent[0].received_at), "."] : null));

    const rows = [];
    let day = null;
    const matching = data.recent.filter(matches);
    for (const v of matching.slice(0, shown)) {
      // A clock alone is ambiguous once the router has run past midnight, and
      // a date on every row is the same date seventeen times.
      const at = new Date(v.received_at).toDateString();
      if (at !== day) {
        day = at;
        rows.push(h("tr", { class: "daybar" }, h("td", { colspan: 9 }, dayLabel(v.received_at))));
      }
      rows.push(openRow({ class: `clickable ${v.id === selected ? "selected" : ""}`, "aria-label": `Request ${v.id}` }, () => go("requests", v.id),
        h("td", { class: "nowrap" }, fmt.clock(v.received_at), h("div", { class: "sub" }, rel(v.received_at))),
        h("td", { class: "mono model" }, modelCell(v), pathNote(v),
          v.source === "console" ? h("div", null, kindBadge("console")) : null),
        h("td", { class: "wrap-anywhere" }, v.backend || "–", h("div", { class: "sub mono" }, v.upstream_model || "")),
        h("td", null, statusBadge(v.status), v.attempts > 1 ? [" ", attemptsCell(v.attempts)] : null),
        h("td", { class: "num" }, fmt.ms(v.ttfb_ms)),
        h("td", { class: "num" }, fmt.ms(v.duration_ms)),
        h("td", { class: "num" }, v.usage ? `${fmt.int(v.usage.input)} / ${fmt.int(v.usage.output)}` : "–",
          v.output_tokens_per_second ? h("div", { class: "sub" }, fmt.rate(v.output_tokens_per_second)) : null),
        h("td", { class: "num" }, v.usage && cacheKnown(v.usage) ? fmt.int(v.usage.cache_read) : "–",
          cacheShare(v.usage) ? h("div", { class: "sub" }, cacheShare(v.usage)) : null),
        h("td", null, outcomeBadge(v),
          v.error ? h("div", { class: "sub one-line", title: v.error }, v.error) : null,
          v.hint_count ? h("div", null, badge(`${v.hint_count} hint`, "warn")) : null)));
    }
    const hidden = matching.length - Math.min(shown, matching.length);
    const more = h("button", { type: "button", class: "small" },
      `Show ${fmt.int(Math.min(LIST_PAGE, hidden))} more of ${fmt.int(hidden)} not shown`);
    more.addEventListener("click", () => { shown += LIST_PAGE; draw(); });
    rerender(recent, table(
      ["Time", "Model", "Backend / upstream", "Status", ["First byte", "num"], ["Duration", "num"], ["Tokens in / out, speed", "num"], ["Cache read", "num"], "Outcome"],
      rows,
      { empty: "No finished request since the router started." }),
      hidden ? h("p", { class: "note" }, more) : null);
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
    // An exchange still running keeps changing, so its detail is read again
    // with the list rather than frozen at the moment it was opened.
    if (selected && data && data.in_flight.some((v) => v.id === selected)) showRequest(detail, selected, false);
    if (signature === drawn) return;
    drawn = signature;
    data = fresh;
    draw();
    if (selected) showRequest(detail, selected, false);
  };
  filter.addEventListener("input", () => { state.requestsFilter = filter.value; shown = LIST_PAGE; draw(); });
  errorsOnly.addEventListener("change", () => { state.errorsOnly = errorsOnly.checked; shown = LIST_PAGE; draw(); });

  replace(master,
    panel("Recent", h("span", { class: "muted" }, "newest first; kept in memory until restart"),
      h("div", { class: "controls" }, filter, h("label", null, errorsOnly, " Errors only")),
      recent),
    detail);
  view.append(inFlight, master);

  /// Opens `id` beside the list, or closes what is open for null. The list
  /// stays where it is: a detail drawn above it sent every click back to the
  /// top of the page.
  const open = (id) => {
    selected = id;
    draw();
    master.classList.toggle("open", Boolean(id));
    if (id) showRequest(detail, id, true);
    else replace(detail);
  };

  const refresh = polled(recent, load);
  refresh();
  every(2000, refresh);
  open(selected);
  return open;
}

/// "Today", "Yesterday", or the date.
function dayLabel(at) {
  const day = new Date(at);
  const today = new Date(serverNow());
  const days = Math.round((today.setHours(0, 0, 0, 0) - new Date(day).setHours(0, 0, 0, 0)) / 86400000);
  const date = day.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
  if (days === 0) return `Today — ${date}`;
  if (days === 1) return `Yesterday — ${date}`;
  return date;
}

/// How much of the prompt the backend read from its cache, which is the
/// number the raw count is usually being compared against. A backend that
/// said nothing about caching gets no number: it is not a backend that
/// cached nothing, and saying 0% would be an answer it never gave.
function cacheShare(usage) {
  if (!usage) return null;
  if (!cacheKnown(usage)) return h("span", { class: "faint", title: "The backend reported no cache counters" }, "not reported");
  const prompt = usage.input + usage.cache_read + usage.cache_creation;
  return prompt ? fmt.pct(usage.cache_read / prompt) : null;
}

/// Whether anything is known about caching, by the same rule the router
/// uses: a line written before the flag existed answers with its counts.
function cacheKnown(usage) {
  return Boolean(usage.cache_reported || usage.cache_read > 0 || usage.cache_creation > 0);
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

async function showRequest(target, id, scroll) {
  await guarded(target, async () => {
    const v = await api(`/api/requests/${encodeURIComponent(id)}`);
    const close = h("button", { type: "button", class: "small", onclick: () => go("requests") }, "Close");
    const recording = v.recording
      ? h("button", { type: "button", class: "small", onclick: () => go("recordings", v.recording) }, "Open recording")
      : null;
    rerender(target, panel(`Request ${v.id}`, [recording, close],
      h("dl", { class: "facts" },
        fact("Received", `${fmt.time(v.received_at)} from ${v.peer || "–"}${v.source === "console" ? " (console test)" : ""}`),
        fact("Request", `${v.method} ${v.path}${v.stream ? " (stream)" : ""}`),
        fact("Model", v.requested_model ? `${v.requested_model}${v.model ? ` → ${v.model} (${v.matched})` : " (no route)"}` : "–"),
        fact("Backend", v.backend ? `${v.backend} (${v.kind}), upstream model ${v.upstream_model}` : "–"),
        fact("Status", v.status === null ? "–" : `${v.status}${v.attempts ? ` after ${v.attempts} attempt(s)` : ""}${v.credential_refreshed ? ", one re-sent with a re-acquired credential" : ""}`),
        fact("Timing", `headers ${fmt.ms(v.latency_ms)}, first byte ${fmt.ms(v.ttfb_ms)}, total ${fmt.ms(v.duration_ms)}`),
        fact("Body", fmt.bytes(v.bytes)),
        fact("Tokens", v.usage
          ? `input ${fmt.int(v.usage.input)}, output ${fmt.int(v.usage.output)}`
          : "not reported"),
        v.usage ? fact("Prompt cache", cacheKnown(v.usage)
          ? `${fmt.pct(v.usage.cache_read / (v.usage.input + v.usage.cache_read + v.usage.cache_creation))} read from cache — ${fmt.int(v.usage.cache_read)} read, ${fmt.int(v.usage.cache_creation)} written, of ${fmt.int(v.usage.input + v.usage.cache_read + v.usage.cache_creation)} prompt tokens`
          : h("span", { class: "muted" }, "the backend reported no cache counters")) : null,
        // Measured only on streamed answers that complete.
        v.output_tokens_per_second ? fact("Output speed", fmt.rate(v.output_tokens_per_second)) : null,
        fact("Outcome", outcomeBadge(v))),
      v.error ? [h("h3", null, "Error"), h("pre", null, v.error)] : null,
      v.hints.length ? [h("h3", null, "Hints"), v.hints.map((hint) =>
        h("div", { class: "hint" }, hint.summary, hint.snippet ? snippet(hint.snippet) : null))] : null,
      v.error_body ? [h("h3", null, "Upstream error body"), h("pre", null, prettyJson(v.error_body))] : null));
    if (scroll) target.scrollIntoView({ block: "nearest" });
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
      onclick: () => { state.statsRange = id; drawRanges(); refresh(); },
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
  const refresh = polled(body, load);
  drawRanges();
  view.append(panel("Statistics", ranges, body));
  refresh();
  every(30000, refresh);
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
    card("Fallbacks", fmt.int(total.defaulted)),
    card("Credential re-sends", fmt.int(total.credential_refreshed)),
    card("Input tokens", fmt.int(total.input_tokens)),
    card("Output tokens", fmt.int(total.output_tokens)),
    card("Cache hit rate", fmt.pct(total.cache_hit_rate), {
      sub: total.cache_silent
        ? `${fmt.int(total.cache_silent)} request(s) reported no cache counters`
        : null,
    }),
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
    report.fallbacks.length
      ? [h("h3", null, "Names no route serves"),
        h("p", { class: "note" }, "Claude Code asked for these names. The default model served them, or nothing did and the request failed with 404. Give a model the name as an alias to serve it on purpose."),
        table(["Requested", "Served by", ["Requests", "num"], "Last seen"], report.fallbacks.map((f) =>
          h("tr", null,
            h("td", { class: "mono wrap-anywhere" }, f.requested),
            h("td", { class: "mono" }, f.model || h("span", { class: "error-text" }, "nothing (404)")),
            h("td", { class: "num" }, fmt.int(f.requests)),
            h("td", { class: "nowrap" }, rel(f.last_seen)))))]
      : null,
    h("h3", null, "By day (UTC)"),
    table(headers("Date"), dayRows, { empty: "No request in this range." }),
    h("p", { class: "note" },
      "Latency percentiles count successful, complete requests only; speed is output tokens over the time after the first byte of streamed answers. ",
      "A hit rate counts only the requests whose backend reported cache counters at all — vLLM ships with prefix caching on and ",
      h("code", null, "--enable-prompt-tokens-details"), " off, and SGLang with ",
      h("code", null, "--enable-cache-report"), " off, so a silent backend is not one that never hit. Files: ",
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
    h("td", { class: "num" }, fmt.pct(row.cache_hit_rate),
      row.cache_silent ? h("div", { class: "sub" }, `${fmt.int(row.cache_silent)} silent`) : null),
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
  const remembered = remembers.get(SMOKE_MODEL_KEY);
  if (models.some((m) => m.id === remembered)) model.value = remembered;
  model.addEventListener("change", () => remembers.set(SMOKE_MODEL_KEY, model.value));
  const prompt = h("input", { type: "text", placeholder: "Reply with one short sentence.", "aria-label": "Prompt" });
  const stream = h("input", { type: "checkbox" });
  stream.checked = true;
  const send = h("button", { class: "primary", type: "submit", disabled: !models.length }, "Send");
  const form = h("form", null,
    h("p", { class: "note" }, "Sends one short ", h("code", null, "/v1/messages"),
      " request through the model's route, as Claude Code would. It reaches the backend and costs tokens."),
    models.length ? null : h("p", { class: "note error-text" }, "The model list did not load. Reload the page to try again."),
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
      const sent = backend.request.headers;
      const received = models && models.headers ? models.headers : [];
      return h("div", null,
        h("h3", null, backend.name, " ", kindBadge(backend.kind), " ", h("span", { class: "muted mono" }, backend.url)),
        h("dl", { class: "facts" },
          fact("Credential", backend.credential.ok ? backend.credential.text : h("span", { class: "error-text" }, backend.credential.text)),
          fact("Route", backend.proxy ? ["through proxy ", h("code", null, backend.proxy)] : "direct"),
          fact(`GET ${backend.request.path}`, modelLine)),
        sent.length ? h("details", null, h("summary", null, `Request headers (${sent.length})`),
          h("p", { class: "note" }, "Every header the backend receives. Backend-forced values and the credential are masked."),
          table(["Header", "Value", "From"], sent.map((x) =>
            h("tr", null, h("td", { class: "mono" }, x.name), h("td", { class: "mono wrap-anywhere" }, x.value), h("td", null, kindBadge(x.source)))))) : null,
        received.length ? h("details", null, h("summary", null, `Response headers (${received.length})`),
          table(["Header", "Value"], received.map((x) =>
            h("tr", null, h("td", { class: "mono" }, x.name), h("td", { class: "mono wrap-anywhere" }, x.value))))) : null,
        listed.length ? table(["Upstream model", ["Context", "num"], "Configured as", "Snippet"], listed) : null);
    }),
  ];
}

// ---------------------------------------------------------------- start

state.token = loadToken();
if (state.token) renderShell();
else renderSignIn();
