"use strict";

// The Recordings tab: the body log listed, and one exchange read either as
// sections or as its files exactly as recorded. Loaded before app.js; it only
// defines functions, which use app.js's helpers when they run.

/// Rows the list draws at first, and adds per "Show more".
const LIST_PAGE = 100;
/// Requests this far apart are never one request tried again.
const RETRY_WINDOW_MS = 10 * 60 * 1000;
/// Past this many characters a recorded response is folded only for the view
/// that shows it, not for the summary above it.
const SUMMARY_FOLD_CHARS = 1024 * 1024;
/// Characters of a recorded file the raw view draws at most.
const SHOWN_CHARS = 2 * 1024 * 1024;
/// Messages after the last prompt up to this size start unfolded.
const OPEN_BYTES = 4 * 1024;
/// Earlier recordings of the same session a request is compared with, nearest
/// first.
const COMPARED = 5;
/// Items holding the Find text that start unfolded, per section.
const UNFOLDED_MATCHES = 10;
/// Occurrences of the Find text marked in one text.
const MARKS = 200;
/// Label the router writes before images it moves out of a tool result for a
/// Chat Completions backend.
const ROUTER_LABEL = /^Images? from tool call \S+:$/;
/// How a recorded outcome that failed upstream begins.
const UPSTREAM_ERROR = "upstream_error:";

const PARTS = [["request", "Request"], ["response", "Response"], ["meta", "Meta"]];
const VIEWS = [["sections", "Sections"], ["raw", "Raw"]];
const REMINDER = /<system-reminder>([\s\S]*?)<\/system-reminder>/g;
/// How a reminder carrying a message the user sent mid-turn begins.
const QUEUED = "The user sent a new message while you were working:";
/// Text blocks Claude Code adds to a user message when the user stops a turn.
const INTERRUPTED = ["[Request interrupted by user]", "[Request interrupted by user for tool use]"];
/// How user text Claude Code writes itself begins, and what it is. Matched at
/// the start only, so the user's own words that mention one stay text. Kept
/// in step with `anthropic::summary` on the router.
const NOTICES = [
  ["<local-command-caveat>", "command output"],
  ["<local-command-stdout>", "command output"],
  ["<local-command-stderr>", "command output"],
  ["<bash-stdout>", "shell output"],
  ["<bash-stderr>", "shell output"],
  ["<task-notification>", "task notification"],
  ["<ide_opened_file>", "ide context"],
  ["<ide_selection>", "ide context"],
  ["Stop hook feedback:", "hook feedback"],
  ["Goal check-in:", "goal check-in"],
  ["A session-scoped Stop hook is now active", "goal set"],
  ["This session is being continued from a previous conversation", "compaction summary"],
  ["Base directory for this skill:", "skill"],
  ["Another Claude session sent a message:", "agent message"],
  ["Continue from where you left off.", "continue"],
  ["[Your previous response had no visible output.", "continue"],
  ["[Image: original ", "image note"],
];
const utf8 = new TextEncoder();

// ---------------------------------------------------------------- list

/// Returns the function that opens entry `name`, or closes the open one for
/// null, without redrawing the list.
function recordings(view, opened) {
  const filter = h("input", { type: "text", placeholder: "Filter by prompt, tool, session, model, backend, id, status or outcome", value: state.recordingsFilter });
  const list = h("div");
  const inspector = h("div");
  const refresh = h("button", { type: "button" }, "Refresh");
  const removeAll = h("button", { type: "button", class: "danger" }, "Delete all");
  let data = null;
  let limit = LIST_PAGE;
  const rows = new Map();
  /// Reveals the entry of a name that a group holds folded.
  const reveals = new Map();

  const shown = () => {
    const needle = filter.value.trim().toLowerCase();
    return data.entries.filter((e) => !needle || [e.name, e.prompt, e.step, e.session, e.model, e.backend, e.status, e.outcome, e.path]
      .some((field) => field !== null && field !== undefined && String(field).toLowerCase().includes(needle)));
  };
  /// One row per attempt. A request Claude Code sent again unchanged — after
  /// a stream it could not read, as a rule — follows the one it repeats, so
  /// the newest stands for the group and the rest fold under it.
  const groupRows = (group) => {
    const [newest, ...repeats] = group;
    const lead = row(newest);
    if (!repeats.length) return [lead];
    const folded = repeats.map((e) => {
      const tr = row(e);
      tr.classList.add("attempt");
      tr.hidden = true;
      return tr;
    });
    const toggle = h("button", { type: "button", class: "link small" });
    const draw = () => replace(toggle, folded[0].hidden ? `${group.length} attempts` : "hide attempts");
    const show = (open) => {
      folded.forEach((tr) => { tr.hidden = !open; });
      draw();
    };
    toggle.addEventListener("click", (event) => {
      event.stopPropagation();
      show(folded[0].hidden);
    });
    draw();
    // The facts line, not the prompt above it: that one is clipped to a line,
    // which would hide the toggle for an entry that shows a step.
    lead.querySelector("td.prompt-cell .sub.entry-facts").append(" · ", toggle);
    for (const e of repeats) reveals.set(e.name, () => show(true));
    return [lead, ...folded];
  };

  /// Requests of one session that ask the same thing within
  /// [`RETRY_WINDOW_MS`], newest first, as groups of attempts. Another
  /// session's requests in between do not break a group, since sessions
  /// sharing the router interleave.
  const groups = (entries) => {
    const key = (e) => JSON.stringify([e.session, e.model, e.path, e.messages, e.prompt, e.step]);
    const open = new Map();
    const out = [];
    for (const e of entries) {
      // Without a session there is nothing saying one client sent both, and
      // two clients asking the same thing are not one request tried again.
      if (!e.session) {
        out.push([e]);
        continue;
      }
      const id = key(e);
      const group = open.get(id);
      if (group && Math.abs(new Date(group[group.length - 1].at) - new Date(e.at)) <= RETRY_WINDOW_MS) {
        group.push(e);
        continue;
      }
      const started = [e];
      open.set(id, started);
      out.push(started);
    }
    return out;
  };

  const row = (e) => {
    const remove = h("button", { type: "button", class: "small danger" }, "Delete");
    remove.addEventListener("click", async (event) => {
      event.stopPropagation();
      if (!confirm(`Delete recording ${e.name}?`)) return;
      remove.disabled = true;
      try {
        await api(`/api/recordings/${encodeURIComponent(e.name)}`, { method: "DELETE" });
        await load();
        if (opened === e.name) go("recordings");
      } catch (error) {
        // The row survives a failed delete, so it takes another click.
        remove.disabled = false;
        if (!(error instanceof SignedOut)) replace(inspector, banner(error.message));
      }
    });
    const tr = h("tr", { class: `clickable ${e.name === opened ? "selected" : ""}`, onclick: () => go("recordings", e.name) },
      h("td", { class: "nowrap" }, fmt.time(e.at)),
      h("td", { class: "prompt-cell" },
        entryPrompt(e),
        h("div", { class: "sub entry-facts" }, entryFacts(e, (session) => { filter.value = session; state.recordingsFilter = session; refilter(); }))),
      h("td", { class: "mono nowrap" }, e.model || "–", h("div", { class: "sub" }, e.backend || "")),
      h("td", null, statusBadge(e.status)),
      h("td", null, entryOutcome(e)),
      h("td", { class: "num" }, fmt.bytes(e.bytes)),
      h("td", null, remove));
    rows.set(e.name, tr);
    return tr;
  };
  const draw = () => {
    if (!data) return;
    rows.clear();
    reveals.clear();
    if (!data.dir) {
      removeAll.disabled = true;
      replace(list, banner(["Body recording is off. Set ", h("code", null, "logging.body_dir"), " or pass ", h("code", null, "--body-dir"), " to record each exchange."], "info"));
      return;
    }
    const needle = filter.value.trim();
    const entries = shown();
    const more = h("button", { type: "button", class: "small" });
    const moreLine = h("p", { class: "note" }, more);
    // A group is one row with its attempts; both counts are in groups, so
    // "50 more" adds 50 rows.
    const label = () => {
      moreLine.hidden = limit >= grouped.length;
      replace(more, `Show ${fmt.int(Math.min(LIST_PAGE, grouped.length - limit))} more of ${fmt.int(grouped.length - limit)} not shown`);
    };
    const grouped = groups(entries);
    const drawn = table(["Time", "Prompt", "Model", "Status", "Outcome", ["Size", "num"], ""], grouped.slice(0, limit).flatMap(groupRows),
      { empty: needle ? "No recording matches." : "No recording yet." });
    more.addEventListener("click", () => {
      append(drawn.querySelector("tbody"), grouped.slice(limit, limit + LIST_PAGE).flatMap(groupRows));
      limit += LIST_PAGE;
      label();
    });
    label();
    // The rows can be fewer than the recordings twice over, so both say so.
    const folded = entries.length - grouped.length;
    replace(list,
      h("p", { class: "note" }, `${fmt.int(data.total)} recording(s) in `, h("code", null, data.dir), `, ${kept(data.retention)}. They hold whole conversations.`,
        data.total > data.entries.length ? ` The newest ${fmt.int(data.entries.length)} are listed.` : "",
        needle ? ` ${fmt.int(entries.length)} match(es) the filter.` : "",
        folded ? ` ${fmt.int(folded)} grouped under a later attempt.` : ""),
      drawn,
      moreLine);
  };
  const refilter = () => {
    limit = LIST_PAGE;
    draw();
  };
  // A box per opening, so an earlier opening still loading fills nothing shown.
  const open = (name) => {
    rows.get(opened)?.classList.remove("selected");
    opened = name;
    reveals.get(name)?.();
    rows.get(opened)?.classList.add("selected");
    const box = h("div");
    replace(inspector, box);
    if (!opened || !data) return;
    // Newer and older follow the list as filtered, or the whole list when
    // the filter hides the opened entry.
    const filtered = shown();
    const entries = filtered.some((e) => e.name === opened) ? filtered : data.entries;
    const at = entries.findIndex((e) => e.name === opened);
    inspect(box, opened, at < 0 ? null : entries[at].files, { newer: entries[at - 1], older: at < 0 ? undefined : entries[at + 1] }, data.entries);
  };

  const load = async () => {
    data = await api("/api/recordings");
    draw();
    // Every load rebuilds the rows, so the open recording is opened again:
    // its row is re-selected, an attempt it folds under is revealed, and what
    // it shows is read from the entry as it now stands.
    if (opened) open(opened);
  };

  let typing = null;
  filter.addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(() => {
      state.recordingsFilter = filter.value;
      refilter();
    }, 150);
  });
  refresh.addEventListener("click", () => guarded(list, load));
  removeAll.addEventListener("click", async () => {
    if (!confirm("Delete every recording? This cannot be undone.")) return;
    removeAll.disabled = true;
    try {
      const result = await api("/api/recordings", { method: "DELETE" });
      // Closed first: the open recording is one of the deleted ones, and a
      // reload would otherwise open it again to say it is not there.
      open(null);
      history.replaceState(null, "", "#recordings");
      await load();
      replace(inspector, banner(`Deleted ${result.removed} recording(s).`, "info"));
    } catch (error) {
      if (!(error instanceof SignedOut)) replace(inspector, banner(error.message));
    } finally {
      removeAll.disabled = false;
    }
  });

  view.append(
    inspector,
    panel("Recordings", [refresh, removeAll], h("div", { class: "controls" }, filter), list));
  guarded(list, load);
  return open;
}

/// How a recording ended: a relay that finished still failed the request when
/// the status did. The list and the open recording read it the same way.
function recordedOutcome(m) {
  if (m.unreadable) return badge("unreadable", "warn");
  if (!m.outcome) return badge("in progress", "info");
  if (m.outcome.startsWith(UPSTREAM_ERROR)) return badge("error", "err");
  if (m.outcome === "client_disconnected") return badge("client left", "warn");
  if (m.status >= 400) return badge("error", "err");
  if (m.outcome === "complete") return badge("complete", "ok");
  return badge(m.outcome, "err");
}

/// The badge, plus an upstream failure's reason on one line; the open
/// recording has it in full.
function entryOutcome(e) {
  if (!e.outcome || !e.outcome.startsWith(UPSTREAM_ERROR)) return recordedOutcome(e);
  const reason = e.outcome.slice(UPSTREAM_ERROR.length).trim();
  return [recordedOutcome(e), h("div", { class: "sub one-line", title: reason }, reason)];
}

/// The entry's prompt; for a later request of a turn, what it sends with the
/// turn's prompt under it.
function entryPrompt(e) {
  if (e.step) return [h("div", { class: "mono" }, e.step), e.prompt ? h("div", { class: "sub one-line", title: e.prompt }, e.prompt) : null];
  if (e.prompt) return h("div", { class: "clamp" }, e.prompt);
  return h("span", { class: "muted" }, e.messages === null ? "–" : "no prompt of its own");
}

/// Message count, session and request id under an entry's prompt; the
/// session filters the list to its entries.
function entryFacts(e, filterBy) {
  const facts = [];
  if (e.messages !== null && e.messages !== undefined) facts.push(`${e.messages} message(s)`);
  if (e.session) {
    facts.push(h("button", {
      type: "button",
      class: "link small mono",
      title: `Show only session ${e.session}`,
      onclick: (event) => { event.stopPropagation(); filterBy(e.session); },
    }, `session ${e.session.length > 8 ? `${e.session.slice(0, 8)}…` : e.session}`));
  }
  if (e.stream !== null && e.stream !== undefined) facts.push(e.stream ? "stream" : "whole response");
  if (e.path && !/^\/v1\/(messages|chat\/completions)(\?|$)/.test(e.path)) facts.push(h("span", { class: "mono" }, e.path));
  facts.push(h("span", { class: "mono" }, e.request_id));
  return facts.flatMap((fact, i) => (i ? [" · ", fact] : [fact]));
}

// ---------------------------------------------------------------- inspector

/// `files` are the entry's files as listed; without them every known name is
/// tried. `neighbors` holds the listed entries just `newer` and `older`;
/// `listed` is the whole list, where earlier requests of the session are found.
async function inspect(target, name, files, neighbors, listed) {
  // Find text belongs to the recording it was typed in: carried into another
  // one it hides everything and reads as a recording with nothing in it.
  if (state.inspected !== name) {
    state.requestFind = "";
    state.inspected = name;
  }
  await guarded(target, async () => {
    const responses = (files || RESPONSE_FILES).filter((file) => file.startsWith("response."));
    const [meta, request, response] = await Promise.all([
      recordedFile(name, "meta.json"),
      recordedFile(name, "request.json"),
      firstRecordedFile(name, responses),
    ]);
    const close = h("button", { type: "button", class: "small", onclick: () => go("recordings") }, "Close");
    if (!meta && !request && !response) {
      replace(target, panel(`Recording ${name}`, [close],
        banner("There is no such recording: it was deleted, pruned after its retention, or recorded by another router.", "info")));
      return;
    }
    const exchange = {
      name,
      files: { meta, request, response },
      meta: meta ? parseJson(meta.text) : null,
      parsed: {},
    };
    if (exchange.meta instanceof Error) exchange.meta = null;
    exchange.dialect = dialectOf(
      exchange.meta && exchange.meta.path,
      request ? parseJson(request.text) : null,
    );
    const session = exchange.meta && exchange.meta.session;
    exchange.earlier = session
      ? listed.filter((e) => e.session === session && e.name < name && e.path === exchange.meta.path).slice(0, COMPARED)
      : [];

    const partSwitch = h("span", { class: "segmented", role: "group", "aria-label": "File" });
    const viewSwitch = h("span", { class: "segmented", role: "group", "aria-label": "View" });
    const body = h("div");
    const draw = () => {
      replace(partSwitch, PARTS.map(([id, label]) =>
        h("button", { type: "button", "aria-pressed": state.recordingPart === id ? "true" : "false", onclick: () => { state.recordingPart = id; draw(); } }, label)));
      replace(viewSwitch, VIEWS.map(([id, label]) =>
        h("button", { type: "button", "aria-pressed": state.recordingView === id ? "true" : "false", onclick: () => { state.recordingView = id; draw(); } }, label)));
      const file = exchange.files[state.recordingPart];
      replace(body, state.recordingView === "raw" ? rawView(exchange, file) : sectionsView(exchange, state.recordingPart));
    };
    const step = (label, entry) => h("button", {
      type: "button",
      class: "small",
      disabled: !entry,
      title: entry ? entry.prompt || entry.request_id : null,
      onclick: () => go("recordings", entry.name),
    }, label);
    const title = exchange.meta ? exchange.meta.request_id : name;
    replace(target, panel(`Recording ${title}`, [step("Newer", neighbors.newer), step("Older", neighbors.older), close],
      summaryFacts(exchange),
      h("div", { class: "controls" }, partSwitch, viewSwitch),
      body));
    draw();
    target.scrollIntoView({ block: "nearest" });
  });
}

/// The request format an upstream path speaks.
/// Which wire format the recorded files are in. `meta.json` names the path
/// the router sent; without it the body says, since a Chat Completions
/// request is the only one carrying `messages[].tool_calls` or a `tools[]`
/// whose entries wrap a `function`. Guessing wrong reads an `openai`
/// recording as Anthropic and drops every tool call from the view.
function dialectOf(path, body) {
  if (path) return String(path).startsWith("/v1/chat/completions") ? "openai" : "anthropic";
  const openai = body && typeof body === "object"
    && ((Array.isArray(body.tools) && body.tools.some((t) => t && t.function))
      || (Array.isArray(body.messages) && body.messages.some((m) => m && (m.tool_calls || m.role === "tool"))));
  return openai ? "openai" : "anthropic";
}

const RESPONSE_FILES = ["response.json", "response.sse", "response.bin"];

/// The first of `names` the entry holds, or null.
async function firstRecordedFile(name, names) {
  for (const file of names) {
    const found = await recordedFile(name, file);
    if (found) return found;
  }
  return null;
}

/// A recorded file as `{ name, text }`, or null when the entry has none.
async function recordedFile(name, file) {
  try {
    const res = await api(`/api/recordings/${encodeURIComponent(name)}/${encodeURIComponent(file)}`, { raw: true });
    return { name: file, text: await res.text() };
  } catch (error) {
    if (error.status === 404) return null;
    throw error;
  }
}

function parseJson(text) {
  try {
    return JSON.parse(text);
  } catch (error) {
    return error;
  }
}

/// Parses once per exchange; a parse failure is kept as the Error it threw.
function parsedOnce(exchange, key, parse) {
  if (!(key in exchange.parsed)) {
    try {
      exchange.parsed[key] = parse();
    } catch (error) {
      exchange.parsed[key] = error;
    }
  }
  return exchange.parsed[key];
}

function summaryFacts(exchange) {
  const m = exchange.meta;
  if (!m) return banner(`Recording ${exchange.name} has no readable meta.json.`, "info");
  const route = [m.requested_model];
  if (m.model && m.model !== m.requested_model) route.push(`→ ${m.model}`);
  if (m.upstream_model && m.upstream_model !== m.model) route.push(`→ ${m.upstream_model}`);
  // Folding a recorded stream costs a JSON.parse per event, too much to pay
  // for one line of a summary; a big one is folded when the response is the
  // part being read, and the tokens wait for that.
  const file = exchange.files.response;
  const heavy = file && file.text.length > SUMMARY_FOLD_CHARS && state.recordingPart !== "response";
  const response = file && !heavy ? parsedOnce(exchange, "response", () => foldResponse(file, exchange.dialect)) : null;
  const usage = response && !(response instanceof Error) && response.usage ? usageText(response.usage) : null;
  return h("dl", { class: "facts" },
    fact("Request", `${m.method} ${m.path}${m.stream ? " (stream)" : ""}, ${fmt.time(m.received_at)}`),
    fact("Route", [h("span", { class: "mono" }, route.join(" ")), ` on ${m.backend}`]),
    fact("Result", [statusBadge(m.status), " ", recordedOutcome(m), m.outcome && m.outcome !== "complete" ? ` ${m.outcome}` : "",
      m.attempts > 1 ? `, ${m.attempts} attempts` : "",
      m.latency_ms !== undefined ? `, headers ${fmt.ms(m.latency_ms)}` : "",
      m.duration_ms !== undefined ? `, total ${fmt.ms(m.duration_ms)}` : ""]),
    fact("Size", `request ${fmt.bytes(exchange.files.request ? byteSize(exchange.files.request.text) : null)}, response ${fmt.bytes(m.response_bytes)}`),
    usage ? fact("Tokens", usage) : null,
    heavy ? fact("Tokens", h("span", { class: "muted" }, "counted when the response is opened")) : null);
}

function rawView(exchange, file) {
  if (!file) return missingFile(exchange);
  // Cut by characters, said in characters: one byte count for the file is
  // enough, and a multibyte file would make two of them disagree.
  const cut = file.text.length > SHOWN_CHARS;
  const shown = cut ? file.text.slice(0, SHOWN_CHARS) : file.text;
  return [
    h("p", { class: "note" }, h("code", null, file.name), `, ${fmt.bytes(byteSize(file.text))}, exactly as recorded.`),
    cut ? banner(`Showing the first ${fmt.int(SHOWN_CHARS)} of ${fmt.int(file.text.length)} characters.`, "info") : null,
    h("pre", null, file.name.endsWith(".json") && !cut ? prettyJson(shown) : shown),
  ];
}

function missingFile(exchange) {
  if (state.recordingPart === "response") {
    return banner("No response was recorded: the exchange is in flight, the client left before the headers, or the file was pruned.", "info");
  }
  return banner(`Recording ${exchange.name} has no ${state.recordingPart} file: it was deleted, pruned after its retention, or never written.`, "info");
}

function sectionsView(exchange, part) {
  const file = exchange.files[part];
  if (!file) return missingFile(exchange);
  if (part === "request") return requestView(exchange);
  if (part === "response") return responseView(exchange);
  return metaView(exchange);
}

// ---------------------------------------------------------------- request

function requestView(exchange) {
  const doc = parsedOnce(exchange, "request", () => readRequest(JSON.parse(exchange.files.request.text), exchange.dialect));
  if (doc instanceof Error) return banner(`request.json does not read as a request (${doc.message}); Raw shows it as recorded.`, "info");

  const prompt = lastPrompt(doc.messages);
  const sum = (items) => items.reduce((total, item) => total + item.bytes, 0);
  const matching = (items, needle) => (needle ? items.filter((item) => item.find.includes(needle)) : items);
  const sections = [
    // `filters: false`: these two draw the same thing whatever Find says, so
    // they keep their own count rather than claiming a number of hits.
    { id: "prompt", label: "Last prompt", count: prompt ? `#${doc.messages[prompt.position].index}` : "none", items: prompt ? doc.messages.slice(prompt.position) : [], filters: false, render: (ctx) => promptSection(doc, prompt, ctx) },
    { id: "messages", label: "Messages", count: doc.messages.length, items: doc.messages, render: (ctx) => messagesSection(doc, matching(doc.messages, ctx.needle), ctx) },
    { id: "system", label: "System", count: doc.system.length, items: doc.system, render: (ctx) => systemSection(doc, matching(doc.system, ctx.needle), ctx) },
    { id: "tools", label: "Tools", count: doc.tools.length, items: doc.tools, render: (ctx) => toolsSection(doc, matching(doc.tools, ctx.needle), ctx) },
    { id: "params", label: "Parameters", count: doc.params.length, items: doc.params, render: (ctx) => paramsSection(matching(doc.params, ctx.needle), ctx) },
    { id: "compare", label: "Compared", count: exchange.earlier.length ? `${exchange.earlier.length} earlier` : "none", items: [], sized: false, filters: false, render: (ctx) => compareSection(exchange, doc, ctx) },
  ];
  const total = sum(doc.messages) + sum(doc.system) + sum(doc.tools) + sum(doc.params) || 1;
  const find = h("input", { type: "text", placeholder: "Find in this request", "aria-label": "Find in the request", value: state.requestFind });
  const found = h("span", { class: "muted" });
  const nav = h("nav", { class: "section-nav", "aria-label": "Request sections" });
  const content = h("div", { class: "section-body" });
  const links = toolLinks(doc);
  const show = (id) => {
    state.requestSection = id;
    const needle = state.requestFind.trim().toLowerCase();
    const ctx = { ...links, needle, reveal };
    replace(found, needle
      ? `${sections.reduce((n, s) => n + (s.filters === false ? 0 : matching(s.items, needle).length), 0)} matching item(s)`
      : "");
    replace(nav, sections.map((s) => {
      const fill = h("span");
      fill.style.width = `${Math.min(100, (sum(s.items) / total) * 100)}%`;
      const hits = needle && s.filters !== false ? matching(s.items, needle).length : null;
      return h("button", { type: "button", class: hits === 0 ? "empty" : null, "aria-current": s.id === id ? "true" : null, onclick: () => show(s.id) },
        h("span", { class: "label" }, s.label),
        h("span", { class: "count" }, hits === null ? String(s.count) : `${hits} found`),
        s.sized === false ? null : [h("span", { class: "size" }, fmt.bytes(sum(s.items))), h("span", { class: "bar" }, fill)]);
    }));
    replace(content, sections.find((s) => s.id === id).render(ctx));
  };
  const reveal = (index) => {
    let target = content.querySelector(`[data-message="${index}"]`);
    if (!target) {
      state.requestFind = "";
      find.value = "";
      show("messages");
      target = content.querySelector(`[data-message="${index}"]`);
    }
    if (!target) return;
    target.open = true;
    target.scrollIntoView({ block: "start", behavior: "smooth" });
    target.classList.add("flash");
    setTimeout(() => target.classList.remove("flash"), 1500);
  };
  let typing = null;
  find.addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(() => {
      state.requestFind = find.value;
      show(state.requestSection);
    }, 150);
  });
  show(sections.some((s) => s.id === state.requestSection) ? state.requestSection : "prompt");
  return [h("div", { class: "controls" }, find, found), h("div", { class: "inspect" }, nav, content)];
}

/// A Messages or Chat Completions body as parameters, system blocks, tools
/// and messages. Messages keep their index in the body's `messages`. Each
/// item carries its size and the lowercased text Find searches.
function readRequest(body, dialect) {
  if (!body || typeof body !== "object" || Array.isArray(body)) throw new Error("not a JSON object");
  const sectioned = new Set(["system", "tools", "messages"]);
  const params = Object.entries(body)
    .filter(([key]) => !sectioned.has(key))
    .map(([key, value]) => {
      const item = indexed({ key, value }, { [key]: value });
      item.find = `${key.toLowerCase()}\n${item.find}`;
      return item;
    });
  const messages = Array.isArray(body.messages) ? body.messages : [];
  const tools = Array.isArray(body.tools) ? body.tools : [];
  if (dialect === "openai") {
    // Chat Completions carries the system text as leading system messages.
    let lead = 0;
    while (lead < messages.length && messages[lead] && messages[lead].role === "system") lead++;
    return {
      params,
      system: messages.slice(0, lead).map((m, i) => indexed({ label: `messages[${i}]`, blocks: openaiContent(m.content) }, m)),
      tools: tools.map((t) => {
        const fn = t.function || {};
        return indexed({ name: fn.name || t.type || "?", description: fn.description || "", schema: fn.parameters, extra: omit(t, ["type", "function"]) }, t);
      }),
      messages: messages.slice(lead).map((m, i) => indexed({ index: lead + i, role: m.role, blocks: markNotices(m.role, openaiMessage(m)) }, m)),
    };
  }
  const system = typeof body.system === "string" ? [{ type: "text", text: body.system }] : Array.isArray(body.system) ? body.system : [];
  return {
    params,
    system: system.map((b, i) => indexed({ label: `system[${i}]`, blocks: [anthropicBlock(b)] }, b)),
    tools: tools.map((t) => indexed({ name: t.name || t.type || "?", description: t.description || "", schema: t.input_schema, extra: omit(t, ["name", "description", "input_schema"]) }, t)),
    messages: messages.map((m, i) => indexed({ index: i, role: m.role, blocks: markNotices(m.role, anthropicContent(m.content)) }, m)),
  };
}

function indexed(item, source) {
  item.source = source;
  item.bytes = byteSize(source);
  item.find = findText(source);
  return item;
}

/// Every string or scalar value under `value`, lowercased. Keys are left out,
/// since the same few ("type", "text") are in every item, and so are encoded
/// payloads (`data`, `signature`).
function findText(value) {
  const parts = [];
  const walk = (v) => {
    if (typeof v === "string") parts.push(v);
    else if (typeof v === "number" || typeof v === "boolean") parts.push(String(v));
    else if (Array.isArray(v)) v.forEach(walk);
    else if (v && typeof v === "object") {
      for (const [key, inner] of Object.entries(v)) {
        if (key !== "data" && key !== "signature") walk(inner);
      }
    }
  };
  walk(value);
  return parts.join("\n").toLowerCase();
}

function omit(object, keys) {
  const rest = Object.fromEntries(Object.entries(object).filter(([key]) => !keys.includes(key)));
  return Object.keys(rest).length ? rest : null;
}

function anthropicContent(content) {
  if (typeof content === "string") return [{ kind: "text", text: content }];
  return Array.isArray(content) ? content.map(anthropicBlock) : [];
}

function anthropicBlock(b) {
  if (typeof b === "string") return { kind: "text", text: b };
  const cache = Boolean(b && b.cache_control);
  switch (b && b.type) {
    case "text": return { kind: "text", text: String(b.text ?? ""), cache };
    case "thinking": return { kind: "thinking", text: String(b.thinking ?? ""), cache };
    case "redacted_thinking": return { kind: "redacted_thinking", cache };
    case "tool_use":
    case "server_tool_use":
    case "mcp_tool_use":
      return { kind: "tool_use", id: b.id, name: b.name, input: b.input, cache };
    case "tool_result":
    case "mcp_tool_result":
      return { kind: "tool_result", id: b.tool_use_id, isError: Boolean(b.is_error), content: anthropicContent(b.content), cache };
    case "image": return { kind: "image", source: b.source, cache };
    case "document": return { kind: "document", title: b.title, source: b.source, cache };
    default: return { kind: "other", type: String((b && b.type) || typeof b), raw: b, cache };
  }
}

function openaiContent(content) {
  if (typeof content === "string") return [{ kind: "text", text: content }];
  if (!Array.isArray(content)) return [];
  const images = content.some((part) => part && part.type === "image_url");
  return content.map((part) => {
    if (part && part.type === "text") {
      const text = String(part.text ?? "");
      return images && ROUTER_LABEL.test(text) ? { kind: "router_label", text } : { kind: "text", text };
    }
    if (part && part.type === "image_url") return { kind: "image", url: part.image_url && part.image_url.url };
    return { kind: "other", type: String((part && part.type) || typeof part), raw: part };
  });
}

function openaiMessage(m) {
  const blocks = [];
  const reasoning = m.reasoning_content || m.reasoning;
  if (reasoning) blocks.push({ kind: "thinking", text: String(reasoning) });
  if (m.role === "tool") {
    blocks.push({ kind: "tool_result", id: m.tool_call_id, isError: false, content: openaiContent(m.content) });
    return blocks;
  }
  blocks.push(...openaiContent(m.content));
  for (const call of m.tool_calls || []) blocks.push(openaiCall(call));
  return blocks;
}

function openaiCall(call) {
  const fn = call.function || {};
  const input = parseJson(fn.arguments ?? "");
  return { kind: "tool_use", id: call.id, name: fn.name, input: input instanceof Error ? fn.arguments : input };
}

/// The user's last prompt as `{ position, text, queued }`: `position` in
/// `messages` of the last user message with text the user typed beside system
/// reminders and Claude Code's notices, or with a reminder through which
/// Claude Code delivers a message the user sent while the model was working
/// (`queued`); null when there is neither.
function lastPrompt(messages) {
  for (let position = messages.length - 1; position >= 0; position--) {
    if (messages[position].role !== "user") continue;
    let text = "";
    let queued = false;
    const pieces = typedPieces(messages[position].blocks);
    for (const b of messages[position].blocks) {
      if (b.kind !== "text") continue;
      for (const match of [...b.text.matchAll(REMINDER), null]) {
        const typed = pieces.shift();
        if (typed) {
          if (queued) text = "";
          queued = false;
          text += `${typed}\n\n`;
        }
        if (!match) break;
        const inner = match[1].trim();
        if (inner.startsWith(QUEUED) && inner.slice(QUEUED.length).trim()) {
          text = inner.slice(QUEUED.length).trim();
          queued = true;
        }
      }
    }
    if (text.trim()) return { position, text: text.trim(), queued };
  }
  return null;
}

/// What a trimmed stretch of user text outside reminders is: `{ prompt }` for
/// text the user typed, `{ prompt, command: true }` for a slash or shell
/// command read as typed, `{ notice }` for Claude Code's own; null when empty.
function classifyText(text) {
  if (!text) return null;
  if (INTERRUPTED.includes(text)) return { notice: "interrupted" };
  const found = NOTICES.find(([start]) => text.startsWith(start));
  if (found) return { notice: found[1] };
  const name = text.startsWith("<command-name>") || text.startsWith("<command-message>") ? tagged(text, "command-name") : null;
  if (name !== null) return { prompt: `${name} ${tagged(text, "command-args") || ""}`.trim(), command: true };
  const shell = text.startsWith("<bash-input>") ? tagged(text, "bash-input") : null;
  if (shell !== null) return { prompt: `! ${shell}`, command: true };
  return { prompt: text };
}

/// A user message's text blocks cut at their reminders, in order, each as
/// what the user typed or null: notices, and the text a slash command expands
/// to (right after the command, before any notice), are null.
function typedPieces(blocks) {
  const pieces = [];
  let command = false;
  for (const b of blocks) {
    if (b.kind !== "text") continue;
    let at = 0;
    for (const match of [...b.text.matchAll(REMINDER), null]) {
      const piece = classifyText(b.text.slice(at, match ? match.index : undefined).trim());
      if (!piece) pieces.push(null);
      else if (piece.notice || (command && !piece.command)) {
        command = false;
        pieces.push(null);
      } else {
        command = Boolean(piece.command);
        pieces.push(piece.prompt);
      }
      if (!match) break;
      at = match.index + match[0].length;
    }
  }
  return pieces;
}

/// The trimmed text between the first `<tag>` and its closing tag, or null.
function tagged(text, tag) {
  const open = `<${tag}>`;
  const start = text.indexOf(open);
  if (start < 0) return null;
  const end = text.indexOf(`</${tag}>`, start + open.length);
  return end < 0 ? null : text.slice(start + open.length, end).trim();
}

/// Marks each text block of a user message that is one of Claude Code's
/// notices with its `notice`, and a slash or shell command with `command`;
/// blocks inside tool results are left alone.
function markNotices(role, blocks) {
  if (role !== "user") return blocks;
  let command = false;
  for (const b of blocks) {
    if (b.kind !== "text" || b.text.includes("<system-reminder>")) continue;
    const piece = classifyText(b.text.trim());
    if (!piece) continue;
    if (piece.notice) b.notice = piece.notice;
    else if (command && !piece.command) b.notice = "command text";
    else if (piece.command) b.command = true;
    command = Boolean(piece.command);
  }
  return blocks;
}

/// Where each tool call and its result sit, so either can lead to the other.
function toolLinks(doc) {
  const calls = new Map();
  const results = new Map();
  for (const m of doc.messages) {
    for (const b of m.blocks) {
      if (b.kind === "tool_use" && b.id) calls.set(b.id, { index: m.index, name: b.name });
      if (b.kind === "tool_result" && b.id) results.set(b.id, m.index);
    }
  }
  return { calls, results };
}

function promptSection(doc, prompt, ctx) {
  if (!prompt) return h("p", { class: "note" }, "No user message carries a prompt.");
  const m = doc.messages[prompt.position];
  const after = doc.messages.slice(prompt.position + 1);
  const calls = after.reduce((n, next) => n + next.blocks.filter((b) => b.kind === "tool_use").length, 0);
  const reminders = m.blocks.reduce((n, b) => n + (b.kind === "text" ? (b.text.match(REMINDER) || []).length : 0), 0);
  const told = [`Message #${m.index} of ${doc.messages.length}`];
  if (prompt.queued) told.push("sent while the model was working, inside a system reminder");
  else if (reminders) told.push(`sent with ${reminders} system reminder(s)`);
  told.push(after.length ? `followed by ${after.length} message(s) with ${calls} tool call(s)` : "the last message");
  let unfolded = 0;
  const opens = (item, fallback) => {
    const open = ctx.needle ? item.find.includes(ctx.needle) : fallback;
    return open && unfolded++ < UNFOLDED_MATCHES;
  };
  return [
    h("div", { class: "prompt text" }, marked(prompt.text, ctx.needle)),
    h("p", { class: "note" }, `${told.join(", ")}.`),
    messageItem(m, ctx, opens(m, prompt.queued && m.bytes <= OPEN_BYTES)),
    after.map((next) => messageItem(next, ctx, opens(next, next.bytes <= OPEN_BYTES))),
  ];
}

function messagesSection(doc, shown, ctx) {
  return [
    ctx.needle ? h("p", { class: "note" }, matchNote(shown.length, `of ${doc.messages.length} message(s) contain the text`)) : null,
    shown.map((m, i) => messageItem(m, ctx, Boolean(ctx.needle) && i < UNFOLDED_MATCHES)),
  ];
}

function matchNote(count, what) {
  return `${count} ${what}${count > UNFOLDED_MATCHES ? `; the first ${UNFOLDED_MATCHES} are unfolded` : ""}.`;
}

function systemSection(doc, shown, ctx) {
  if (!doc.system.length) return h("p", { class: "note" }, "The request has no system prompt.");
  if (!shown.length) return h("p", { class: "note" }, "No system block contains the text.");
  return shown.map((s, i) => {
    const text = s.blocks.filter((b) => b.kind === "text").map((b) => b.text).join("\n");
    return lazyDetails({ class: "item" },
      [h("span", { class: "mono muted" }, s.label), s.blocks.some((b) => b.cache) ? badge("cache", "info") : null,
        h("span", { class: "preview" }, marked(firstLine(text), ctx.needle)), h("span", { class: "size" }, fmt.bytes(s.bytes))],
      () => s.blocks.map((b) => blockView(b, ctx)), Boolean(ctx.needle) && i < UNFOLDED_MATCHES);
  });
}

function toolsSection(doc, shown, ctx) {
  if (!doc.tools.length) return h("p", { class: "note" }, "The request offers no tools.");
  if (!shown.length) return h("p", { class: "note" }, "No tool contains the text.");
  const groups = new Map();
  for (const tool of shown) {
    const group = toolGroup(tool.name);
    if (!groups.has(group.label)) groups.set(group.label, { ...group, tools: [] });
    groups.get(group.label).tools.push(tool);
  }
  const ordered = [...groups.values()].sort((a, b) => a.order - b.order || a.label.localeCompare(b.label));
  // Groups start folded so every MCP server shows in one screen.
  const open = Boolean(ctx.needle) || ordered.length === 1;
  let unfolded = 0;
  return [
    ctx.needle ? h("p", { class: "note" }, matchNote(shown.length, `of ${doc.tools.length} tool(s) contain the text`)) : null,
    ordered.map((group) =>
      lazyDetails({ class: "item group" },
        [h("strong", null, group.label), h("span", { class: "kinds" }, `${group.tools.length} tool(s)`),
          h("span", { class: "preview" }, group.tools.map((t) => shortToolName(t, group.prefix)).join(", ")),
          h("span", { class: "size" }, fmt.bytes(group.tools.reduce((n, t) => n + t.bytes, 0)))],
        () => group.tools.map((tool) => toolItem(tool, group.prefix, ctx, Boolean(ctx.needle) && unfolded++ < UNFOLDED_MATCHES)), open)),
  ];
}

/// MCP tools are named `mcp__<server>__<tool>`.
function toolGroup(name) {
  const mcp = /^mcp__(.+?)__/.exec(name);
  if (mcp) return { label: `MCP ${mcp[1]}`, prefix: mcp[0], order: 1 };
  return { label: "Built-in", prefix: "", order: 0 };
}

function shortToolName(tool, prefix) {
  return prefix && tool.name.startsWith(prefix) ? tool.name.slice(prefix.length) : tool.name;
}

function toolItem(tool, prefix, ctx, open) {
  return lazyDetails({ class: "item" },
    [h("strong", { class: "mono", title: tool.name }, marked(shortToolName(tool, prefix), ctx.needle)),
      h("span", { class: "preview" }, marked(firstLine(tool.description), ctx.needle)),
      h("span", { class: "size" }, fmt.bytes(tool.bytes))],
    () => [
      tool.description ? h("div", { class: "text" }, marked(tool.description, ctx.needle)) : null,
      tool.schema !== undefined ? [h("h4", null, "Input schema"), h("pre", null, marked(JSON.stringify(tool.schema, null, 2), ctx.needle))] : null,
      tool.extra ? [h("h4", null, "Other fields"), h("pre", null, marked(JSON.stringify(tool.extra, null, 2), ctx.needle))] : null,
    ], open);
}

function paramsSection(shown, ctx) {
  return table(["Field", "Value"], shown.map((p) =>
    h("tr", null,
      h("td", { class: "mono nowrap" }, marked(p.key, ctx.needle)),
      h("td", { class: "mono wrap-anywhere" }, jsonValue(p.value, ctx.needle)))),
  { empty: ctx.needle ? "No field contains the text." : "The request has no other fields." });
}

// ---------------------------------------------------------------- compare

/// An earlier request of the same session set against this one in the order
/// a prompt cache reads them: tools, system, messages. The earlier request
/// that shares the longest prefix is chosen first.
function compareSection(exchange, doc, ctx) {
  const box = h("div");
  if (!exchange.earlier.length) {
    replace(box, h("p", { class: "note" }, "No earlier recording of this Claude Code session to compare with."));
    return box;
  }
  replace(box, h("p", { class: "note" }, "Reading the session's earlier requests…"));
  if (!exchange.parsed.earlier) exchange.parsed.earlier = earlierRequests(exchange, doc);
  guarded(box, async () => {
    const earlier = await exchange.parsed.earlier;
    if (!earlier.length) {
      replace(box, h("p", { class: "note" }, "The session's earlier recordings could not be read."));
      return;
    }
    // The choice outlives the section, which Find redraws.
    if (exchange.parsed.compareWith === undefined) {
      exchange.parsed.compareWith = earlier.indexOf(earlier.reduce((a, b) => (b.diff.shared > a.diff.shared ? b : a)));
    }
    const picker = h("select", { "aria-label": "Earlier request" });
    for (const [i, e] of earlier.entries()) {
      const option = h("option", { value: String(i) },
        `${fmt.clock(e.entry.at)}, ${e.entry.messages ?? "?"} message(s), shares ${fmt.pct(e.diff.shared / (e.diff.total || 1))}: ${e.entry.prompt || e.entry.request_id}`);
      option.selected = i === exchange.parsed.compareWith;
      picker.append(option);
    }
    const open = h("button", { type: "button", class: "small" }, "Open");
    const view = h("div");
    const draw = () => {
      const chosen = earlier[Number(picker.value)];
      open.onclick = () => go("recordings", chosen.entry.name);
      replace(view, diffView(doc, chosen.doc, chosen.diff, ctx));
    };
    picker.addEventListener("change", () => {
      exchange.parsed.compareWith = Number(picker.value);
      draw();
    });
    replace(box, h("div", { class: "controls" }, h("label", null, "Against ", picker), open), view);
    draw();
  });
  return box;
}

async function earlierRequests(exchange, doc) {
  const read = await Promise.all(exchange.earlier.map(async (entry) => {
    try {
      const file = await recordedFile(entry.name, "request.json");
      if (!file) return null;
      const before = readRequest(JSON.parse(file.text), dialectOf(entry.path));
      return { entry, doc: before, diff: requestDiff(doc, before) };
    } catch (error) {
      if (error instanceof SignedOut) throw error;
      return null;
    }
  }));
  return read.filter(Boolean);
}

/// Where `now` stops repeating `before`. Items compare as JSON without
/// `cache_control`, which Claude Code moves to the newest message each turn.
function requestDiff(now, before) {
  const key = (item) => JSON.stringify(item.source, (name, value) => (name === "cache_control" ? undefined : value));
  const sum = (items) => items.reduce((n, item) => n + item.bytes, 0);
  const prefix = (a, b) => {
    let n = 0;
    while (n < a.length && n < b.length && key(a[n]) === key(b[n])) n++;
    return n;
  };
  const names = (tools) => new Map(tools.map((t) => [t.name, key(t)]));
  const nowTools = names(now.tools);
  const beforeTools = names(before.tools);
  const tools = {
    same: now.tools.length === before.tools.length && prefix(now.tools, before.tools) === now.tools.length,
    added: [...nowTools.keys()].filter((name) => !beforeTools.has(name)),
    removed: [...beforeTools.keys()].filter((name) => !nowTools.has(name)),
    changed: [...nowTools].filter(([name, k]) => beforeTools.has(name) && beforeTools.get(name) !== k).map(([name]) => name),
  };
  const system = prefix(now.system, before.system);
  const systemSame = system === now.system.length && system === before.system.length;
  const messages = prefix(now.messages, before.messages);
  const params = [...new Set([...now.params, ...before.params].map((p) => p.key))].filter((name) => {
    const a = now.params.find((p) => p.key === name);
    const b = before.params.find((p) => p.key === name);
    return !a || !b || key(a) !== key(b);
  });
  let shared = 0;
  let breaks;
  if (!tools.same) {
    breaks = "the tools";
  } else {
    shared += sum(now.tools) + sum(now.system.slice(0, system));
    if (!systemSame) {
      breaks = system < now.system.length ? now.system[system].label : "the system blocks the earlier request had after them";
    } else {
      shared += sum(now.messages.slice(0, messages));
      if (messages < before.messages.length) breaks = `message #${messages < now.messages.length ? now.messages[messages].index : before.messages[messages].index}`;
    }
  }
  return { tools, system, systemSame, messages, params, shared, total: sum(now.tools) + sum(now.system) + sum(now.messages), breaks };
}

function diffView(now, before, diff, ctx) {
  const changes = (label, list) => (list.length ? `${label} ${list.join(", ")}` : null);
  const toolText = diff.tools.same
    ? badge("same", "ok")
    : [badge("changed", "warn"), " ", [changes("added", diff.tools.added), changes("removed", diff.tools.removed), changes("changed", diff.tools.changed)].filter(Boolean).join("; ") || "reordered"];
  const systemText = diff.systemSame
    ? [badge("same", "ok"), ` ${now.system.length} block(s)`]
    : [badge("changed", "warn"), ` the first ${diff.system} of ${now.system.length} block(s) repeat; the earlier request had ${before.system.length}`];
  const repeated = diff.messages === 0 ? "none repeat"
    : diff.messages === 1 ? `#${now.messages[0].index} repeats`
    : `#${now.messages[0].index}–#${now.messages[diff.messages - 1].index} repeat`;
  const newer = now.messages.slice(diff.messages);
  const dropped = before.messages.length - diff.messages;
  // Nothing dropped and nothing new is the same list, not an extension of it.
  const messageLabel = dropped ? "changed" : newer.length ? "extended" : "same";
  const messageText = [badge(messageLabel, dropped ? "warn" : "ok"),
    ` ${repeated}, ${newer.length} new`, dropped ? `, ${dropped} of the earlier request's differ or are gone` : ""];
  return [
    h("dl", { class: "facts" },
      fact("Shared prefix", [`${fmt.bytes(diff.shared)} of ${fmt.bytes(diff.total)} (${fmt.pct(diff.shared / (diff.total || 1))})`,
        diff.breaks ? `, first difference in ${diff.breaks}` : ", the earlier request is a prefix of this one"]),
      fact("Tools", toolText),
      fact("System", systemText),
      fact("Messages", messageText),
      fact("Parameters", diff.params.length ? [badge("changed", "warn"), ` ${diff.params.join(", ")}`] : badge("same", "ok"))),
    // Only the first difference unfolds: a session's newest request adds a
    // whole turn, and unfolding all of it buries the summary above.
    newer.length ? [h("h3", null, `Messages after the repeated ones (${newer.length})`),
      newer.map((m, i) => messageItem(m, ctx, i === 0 && m.bytes <= OPEN_BYTES))] : null,
    dropped ? [h("h3", null, "The earlier request's messages from there"),
      before.messages.slice(diff.messages).map((m) => messageItem(m, { ...NO_CONTEXT, needle: ctx.needle }, false))] : null,
  ];
}

// ---------------------------------------------------------------- messages

function messageItem(m, ctx, open) {
  return lazyDetails({ class: `item message role-${m.role}`, "data-message": m.index },
    [roleBadge(m.role), h("span", { class: "mono muted" }, `#${m.index}`),
      h("span", { class: "kinds" }, blockKinds(m.blocks, ctx)),
      h("span", { class: "preview" }, marked(messagePreview(m.blocks), ctx.needle)),
      h("span", { class: "size" }, fmt.bytes(m.bytes))],
    () => m.blocks.map((b) => blockView(b, ctx)), open);
}

function roleBadge(role) {
  const kind = { user: "info", assistant: "ok", system: "warn" }[role];
  return badge(String(role), kind);
}

function blockKinds(blocks, ctx) {
  const counts = new Map();
  for (const b of blocks) {
    let label = b.kind === "other" ? b.type : b.kind.replace("_", " ");
    if (b.kind === "tool_use") label = `→ ${b.name}`;
    if (b.kind === "tool_result") label = `← ${(ctx.calls.get(b.id) || {}).name || "result"}${b.isError ? " (error)" : ""}`;
    if (b.kind === "text" && b.text.includes("<system-reminder>")) label = b.text.replace(REMINDER, "").trim() ? "text + reminder" : "reminder";
    if (b.notice) label = b.notice;
    if (b.command) label = "command";
    counts.set(label, (counts.get(label) || 0) + 1);
  }
  return [...counts].map(([label, n]) => (n > 1 ? `${label} ×${n}` : label)).join(" · ");
}

function messagePreview(blocks) {
  for (const b of blocks) {
    if (b.kind === "text" && !b.notice) {
      const piece = classifyText(b.text.replace(REMINDER, "").trim());
      if (piece && piece.prompt) return firstLine(piece.prompt);
    }
  }
  const notice = blocks.find((b) => b.notice);
  if (notice) return firstLine(notice.text);
  const call = blocks.find((b) => b.kind === "tool_use");
  if (call) return firstLine(typeof call.input === "string" ? call.input : JSON.stringify(call.input));
  const result = blocks.find((b) => b.kind === "tool_result");
  if (result) return firstLine(result.content.filter((b) => b.kind === "text").map((b) => b.text).join(" "));
  return "";
}

/// A block rendering context carries `calls` and `results` from toolLinks,
/// the `reveal(index)` that opens a message, and the Find `needle` to mark;
/// this one has none of them.
const NO_CONTEXT = { calls: new Map(), results: new Map(), reveal: null, needle: "" };

function blockView(b, ctx) {
  const needle = ctx.needle;
  const cache = b.cache ? badge("cache breakpoint", "info") : null;
  switch (b.kind) {
    case "text":
      if (b.notice) {
        return lazyDetails({ class: "item notice" },
          [badge(b.notice, "warn"), cache, h("span", { class: "preview" }, marked(firstLine(b.text), needle)), h("span", { class: "size" }, fmt.bytes(byteSize(b.text)))],
          () => h("div", { class: "text" }, marked(b.text, needle)), contains(b.text, needle));
      }
      return h("div", { class: "block" }, cache, textView(b.text, needle));
    case "thinking":
      return lazyDetails({ class: "item thinking" },
        [badge("thinking"), cache, h("span", { class: "preview" }, marked(firstLine(b.text), needle)), h("span", { class: "size" }, fmt.bytes(byteSize(b.text)))],
        () => h("div", { class: "text" }, marked(b.text, needle)), contains(b.text, needle));
    case "redacted_thinking":
      return h("div", { class: "block" }, badge("redacted thinking"), cache);
    case "router_label":
      return h("div", { class: "block" }, h("div", { class: "block-head" }, badge("added by the router"), h("span", { class: "muted" }, b.text)));
    case "tool_use": {
      const result = ctx.reveal ? ctx.results.get(b.id) : undefined;
      return h("div", { class: "block tool-use" },
        h("div", { class: "block-head" }, badge("tool call", "info"), h("strong", { class: "mono" }, marked(b.name || "?", needle)),
          h("span", { class: "mono muted" }, b.id || ""), cache,
          result !== undefined ? h("button", { type: "button", class: "link small", onclick: () => ctx.reveal(result) }, `result in #${result}`) : null),
        h("pre", null, marked(typeof b.input === "string" ? b.input : JSON.stringify(b.input, null, 2), needle)));
    }
    case "tool_result": {
      const call = ctx.calls.get(b.id);
      return h("div", { class: `block tool-result ${b.isError ? "error" : ""}` },
        h("div", { class: "block-head" }, badge(b.isError ? "tool error" : "tool result", b.isError ? "err" : "ok"),
          call ? h("strong", { class: "mono" }, call.name) : null, h("span", { class: "mono muted" }, b.id || ""), cache,
          call && ctx.reveal ? h("button", { type: "button", class: "link small", onclick: () => ctx.reveal(call.index) }, `call in #${call.index}`) : null),
        b.content.length ? b.content.map((inner) => blockView(inner, ctx)) : h("span", { class: "muted" }, "empty"));
    }
    case "image":
      return h("div", { class: "block" }, h("div", { class: "block-head" }, badge("image"), cache), imageView(b));
    case "document":
      return h("div", { class: "block" }, h("div", { class: "block-head" }, badge("document"), b.title || "", cache,
        h("span", { class: "muted" }, b.source ? `${b.source.media_type || b.source.type || ""}, ${fmt.bytes(byteSize(b.source))}` : "")));
    default:
      return h("div", { class: "block" }, h("div", { class: "block-head" }, badge(b.type), cache), h("pre", null, marked(JSON.stringify(b.raw, null, 2), needle)));
  }
}

/// Text with each system reminder folded under its first line; a reminder
/// holding the Find needle starts unfolded.
function textView(text, needle) {
  const pieces = [];
  let at = 0;
  for (const match of text.matchAll(REMINDER)) {
    const before = text.slice(at, match.index);
    if (before.trim()) pieces.push(h("div", { class: "text" }, marked(before.replace(/^\n+|\n+$/g, ""), needle)));
    const inner = match[1].replace(/^\n+|\n+$/g, "");
    pieces.push(lazyDetails({ class: "item reminder" },
      [badge("system reminder", "warn"), h("span", { class: "preview" }, marked(firstLine(inner), needle)), h("span", { class: "size" }, fmt.bytes(byteSize(match[0])))],
      () => h("div", { class: "text" }, marked(inner, needle)), contains(inner, needle)));
    at = match.index + match[0].length;
  }
  const rest = text.slice(at);
  if (rest.trim() || !pieces.length) pieces.push(h("div", { class: "text" }, marked(at ? rest.replace(/^\n+|\n+$/g, "") : rest, needle)));
  return pieces;
}

function contains(text, needle) {
  return Boolean(needle) && text.toLowerCase().includes(needle);
}

/// `text` with each case-insensitive occurrence of `needle` in a `<mark>`.
function marked(text, needle) {
  if (!needle) return text;
  const lower = text.toLowerCase();
  // Lowercasing that changes length would misplace the marks.
  if (lower.length !== text.length) return text;
  const out = [];
  let at = 0;
  for (let i = lower.indexOf(needle); i >= 0 && out.length < 2 * MARKS; i = lower.indexOf(needle, at)) {
    if (i > at) out.push(text.slice(at, i));
    out.push(h("mark", null, text.slice(i, i + needle.length)));
    at = i + needle.length;
  }
  out.push(text.slice(at));
  return out;
}

function imageView(b) {
  let src = null;
  if (b.source && b.source.type === "base64" && /^image\//.test(b.source.media_type || "")) src = `data:${b.source.media_type};base64,${b.source.data}`;
  else if (typeof b.url === "string" && b.url.startsWith("data:image/")) src = b.url;
  if (src) return h("img", { class: "thumb", src, alt: "image from the request" });
  const where = b.url || (b.source && (b.source.url || b.source.type)) || "";
  return h("span", { class: "mono muted wrap-anywhere" }, where);
}

// ---------------------------------------------------------------- response

function responseView(exchange) {
  const file = exchange.files.response;
  if (file.name === "response.bin") return h("p", { class: "note" }, "The response is neither JSON nor an event stream; Raw shows it as recorded.");
  const folded = parsedOnce(exchange, "response", () => foldResponse(file, exchange.dialect));
  if (folded instanceof Error) return banner(`${file.name} does not read as a response (${folded.message}); Raw shows it as recorded.`, "info");
  // An error document carries nothing else worth a line.
  const bare = folded.error && !folded.blocks.length && !folded.stop;
  return [
    folded.error ? banner(`${folded.error.type || "error"}: ${folded.error.message || JSON.stringify(folded.error)}`) : null,
    bare ? null : h("dl", { class: "facts" },
      fact("Stop reason", folded.stop ? h("span", { class: "mono" }, folded.stop) : h("span", { class: "muted" }, "none")),
      folded.usage ? fact("Usage", usageText(folded.usage)) : null,
      folded.model ? fact("Model", h("span", { class: "mono" }, folded.model)) : null,
      folded.events ? fact("Events", [...folded.events].map(([type, n]) => `${type} ×${n}`).join(", ")) : null,
      folded.malformed ? fact("Unreadable frames", String(folded.malformed)) : null),
    bare ? null : [
      h("h3", null, "Content"),
      folded.blocks.length ? folded.blocks.map((b) => blockView(b, NO_CONTEXT)) : h("p", { class: "note" }, "No content."),
    ],
  ];
}

/// The response folded into one message: content blocks, stop reason, usage
/// and error. A stream is replayed event by event.
function foldResponse(file, dialect) {
  const frames = file.name === "response.sse" ? sseData(file.text) : null;
  if (dialect === "openai") return frames ? foldOpenaiStream(frames) : openaiDocument(JSON.parse(file.text));
  return frames ? foldAnthropicStream(frames) : anthropicDocument(JSON.parse(file.text));
}

/// The `data` of each event, joined across lines, with its event name.
function sseData(text) {
  const frames = [];
  for (const chunk of text.split(/\r?\n\r?\n/)) {
    let event = "message";
    const data = [];
    for (const line of chunk.split(/\r?\n/)) {
      if (line.startsWith("event:")) event = line.slice(6).trim();
      else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
    }
    if (data.length) frames.push({ event, data: data.join("\n") });
  }
  return frames;
}

function foldAnthropicStream(frames) {
  const out = { blocks: [], stop: null, usage: null, model: null, error: null, events: new Map(), malformed: 0 };
  const open = [];
  for (const frame of frames) {
    const d = parseJson(frame.data);
    if (d instanceof Error || !d) {
      out.malformed++;
      continue;
    }
    out.events.set(d.type, (out.events.get(d.type) || 0) + 1);
    if (d.type === "message_start" && d.message) {
      out.model = d.message.model || null;
      out.usage = { ...(d.message.usage || {}) };
    } else if (d.type === "content_block_start") {
      open[d.index] = { ...d.content_block, partial: "" };
    } else if (d.type === "content_block_delta" && open[d.index]) {
      const block = open[d.index];
      const delta = d.delta || {};
      if (delta.type === "text_delta") block.text = (block.text || "") + delta.text;
      else if (delta.type === "thinking_delta") block.thinking = (block.thinking || "") + delta.thinking;
      else if (delta.type === "input_json_delta") block.partial += delta.partial_json;
    } else if (d.type === "content_block_stop" && open[d.index]) {
      out.blocks.push(closeBlock(open[d.index]));
      open[d.index] = null;
    } else if (d.type === "message_delta") {
      if (d.delta && d.delta.stop_reason) out.stop = d.delta.stop_reason;
      if (d.usage) out.usage = { ...(out.usage || {}), ...d.usage };
    } else if (d.type === "error") {
      out.error = d.error || d;
    }
  }
  // Blocks still open when the stream ended are shown as far as they got.
  for (const block of open) if (block) out.blocks.push(closeBlock(block));
  return out;
}

function closeBlock(block) {
  const { partial, ...rest } = block;
  if (partial) {
    const input = parseJson(partial);
    rest.input = input instanceof Error ? partial : input;
  }
  return anthropicBlock(rest);
}

function anthropicDocument(doc) {
  if (doc && doc.type === "error") return { blocks: [], stop: null, usage: null, model: null, error: doc.error || doc };
  return {
    blocks: anthropicContent(doc.content),
    stop: doc.stop_reason || null,
    usage: doc.usage || null,
    model: doc.model || null,
    error: doc.error || null,
  };
}

function foldOpenaiStream(frames) {
  const out = { blocks: [], stop: null, usage: null, model: null, error: null, events: new Map(), malformed: 0 };
  let reasoning = "";
  let content = "";
  // By index, and started once named, as the router's own decoder takes them.
  const calls = new Map();
  let last = null;
  for (const frame of frames) {
    if (frame.data.trim() === "[DONE]") {
      out.events.set("[DONE]", 1);
      continue;
    }
    const d = parseJson(frame.data);
    if (d instanceof Error || !d) {
      out.malformed++;
      continue;
    }
    out.events.set("chunk", (out.events.get("chunk") || 0) + 1);
    if (d.error) out.error = d.error;
    if (d.model) out.model = d.model;
    if (d.usage) out.usage = d.usage;
    for (const choice of d.choices || []) {
      const delta = choice.delta || {};
      reasoning += delta.reasoning_content || delta.reasoning || "";
      content += delta.content || "";
      for (const call of delta.tool_calls || []) {
        const fn = call.function || {};
        const id = typeof call.id === "string" ? call.id : null;
        const name = typeof fn.name === "string" ? fn.name : null;
        let slot = Number.isInteger(call.index) ? call.index : null;
        // Unnumbered: a delta with neither id nor name continues the latest
        // call; anything else is a new one after it.
        if (slot === null) slot = last !== null && id === null && name === null ? last : calls.size ? Math.max(...calls.keys()) + 1 : 0;
        last = slot;
        if (!calls.has(slot)) calls.set(slot, { id: null, function: { name: null, arguments: "" } });
        const entry = calls.get(slot);
        if (entry.id === null) entry.id = id;
        if (entry.function.name === null) entry.function.name = name;
        if (typeof fn.arguments === "string") entry.function.arguments += fn.arguments;
      }
      if (choice.finish_reason) out.stop = choice.finish_reason;
    }
  }
  if (reasoning) out.blocks.push({ kind: "thinking", text: reasoning });
  if (content) out.blocks.push({ kind: "text", text: content });
  for (const slot of [...calls.keys()].sort((a, b) => a - b)) out.blocks.push(openaiCall(calls.get(slot)));
  return out;
}

function openaiDocument(doc) {
  const choice = (doc.choices || [])[0];
  return {
    blocks: choice && choice.message ? openaiMessage(choice.message) : [],
    stop: choice ? choice.finish_reason || null : null,
    usage: doc.usage || null,
    model: doc.model || null,
    error: doc.error || null,
  };
}

/// Usage fields as the backend named them; nested counts are dotted.
function usageText(usage) {
  const parts = [];
  const walk = (value, path) => {
    if (value !== null && typeof value === "object") {
      for (const [key, inner] of Object.entries(value)) walk(inner, path ? `${path}.${key}` : key);
    } else if (value !== null && value !== undefined) {
      parts.push(`${path} ${typeof value === "number" ? fmt.int(value) : value}`);
    }
  };
  walk(usage, "");
  return h("span", { class: "mono" }, parts.join(", "));
}

// ---------------------------------------------------------------- meta

function metaView(exchange) {
  const m = exchange.meta;
  if (!m) return banner("meta.json does not read as JSON; Raw shows it as recorded.", "info");
  const headers = (map) => table(["Header", "Value"], Object.entries(map || {}).map(([name, value]) =>
    h("tr", null, h("td", { class: "mono nowrap" }, name), h("td", { class: "mono wrap-anywhere" }, value))),
  { empty: "None recorded." });
  const sent = table(["Header", "Value", "From"], (m.request_headers || []).map((x) =>
    h("tr", null,
      h("td", { class: "mono nowrap" }, x.name),
      h("td", { class: "mono wrap-anywhere" }, x.value),
      h("td", null, badge(x.source)))),
  { empty: "None recorded." });
  const shown = new Set(["request_headers", "dropped_headers", "response_headers"]);
  return [
    table(["Field", "Value"], Object.entries(m).filter(([key]) => !shown.has(key)).map(([key, value]) =>
      h("tr", null, h("td", { class: "mono nowrap" }, key), h("td", { class: "mono wrap-anywhere" }, typeof value === "string" ? value : JSON.stringify(value))))),
    h("h3", null, "Request headers, as the backend received them"),
    h("p", { class: "note" }, "Every header the backend saw. ",
      h("code", null, "transport"), " is what the HTTP client frames the request with; ",
      h("code", null, "credential"), " and forced ", h("code", null, "backend"),
      " values may be secrets and are never written to disk."),
    sent,
    (m.dropped_headers || []).length ? [
      h("h3", null, "Headers the backend never saw"),
      h("p", { class: "note" }, "The client sent these; the router left them out. What this backend's configuration decided comes first: ",
        h("code", null, "drop_headers"), " and ", h("code", null, "backend kind"),
        ". The rest go on every request whatever the configuration says."),
      table(["Header", "Value", "Why"], [...m.dropped_headers].sort(byDropReason).map((x) =>
        h("tr", null,
          h("td", { class: "mono nowrap" }, x.name),
          h("td", { class: "mono wrap-anywhere" }, x.value),
          h("td", null, badge(dropLabel(x.reason), CHOSEN_DROPS.includes(x.reason) ? "info" : null))))),
    ] : null,
    h("h3", null, "Response headers"),
    headers(m.response_headers),
  ];
}

/// Drops a backend's configuration decided, which is what someone reading
/// this table came for; the others happen to every request.
const CHOSEN_DROPS = ["drop_headers", "backend_kind"];

function byDropReason(a, b) {
  const rank = (x) => (CHOSEN_DROPS.includes(x.reason) ? CHOSEN_DROPS.indexOf(x.reason) : CHOSEN_DROPS.length);
  return rank(a) - rank(b) || a.name.localeCompare(b.name);
}

/// Why a header the client sent did not reach the backend. A reason this
/// console does not know yet is shown as `meta.json` names it.
function dropLabel(reason) {
  if (reason === "client_credential") return "client credential";
  if (reason === "backend_kind") return "backend kind";
  return String(reason);
}

// ---------------------------------------------------------------- helpers

/// A `<details>` whose body is built the first time it opens.
function lazyDetails(props, summary, build, open) {
  const el = h("details", props, h("summary", null, summary));
  const body = h("div", { class: "item-body" });
  el.append(body);
  let built = false;
  const fill = () => {
    if (built || !el.open) return;
    built = true;
    append(body, [build()]);
  };
  el.addEventListener("toggle", fill);
  if (open) {
    el.open = true;
    fill();
  }
  return el;
}

/// Short values on one line, longer ones indented in a block.
function jsonValue(value, needle) {
  const line = JSON.stringify(value);
  if (line === undefined) return "";
  if (line.length <= 100) return marked(line, needle);
  return h("pre", null, marked(JSON.stringify(value, null, 2), needle));
}

function firstLine(text) {
  const line = String(text || "").split("\n").find((l) => l.trim()) || "";
  return line.length > 160 ? `${line.slice(0, 160)}…` : line;
}

function byteSize(value) {
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text === undefined ? 0 : utf8.encode(text).length;
}
