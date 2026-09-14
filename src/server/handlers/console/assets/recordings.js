"use strict";

// The Recordings tab: the body log listed, and one exchange read either as
// sections or as its files exactly as recorded. Loaded before app.js; it only
// defines functions, which use app.js's helpers when they run.

/// Displays at most this much of a recorded file as raw text.
const SHOWN_BYTES = 2 * 1024 * 1024;
/// Messages after the last prompt up to this size start unfolded.
const OPEN_BYTES = 4 * 1024;

const PARTS = [["request", "Request"], ["response", "Response"], ["meta", "Meta"]];
const VIEWS = [["sections", "Sections"], ["raw", "Raw"]];
const REMINDER = /<system-reminder>([\s\S]*?)<\/system-reminder>/g;
const utf8 = new TextEncoder();

// ---------------------------------------------------------------- list

function recordings(view, opened) {
  const filter = h("input", { type: "text", placeholder: "Filter by model, backend, id, status or outcome", value: state.recordingsFilter });
  const list = h("div");
  const inspector = h("div");
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
      table(["Time", "Request", "Model", "Status", "Outcome", ["Size", "num"], ""], entries.map((e) => {
        const remove = h("button", { type: "button", class: "small danger" }, "Delete");
        remove.addEventListener("click", async (event) => {
          event.stopPropagation();
          if (!confirm(`Delete recording ${e.name}?`)) return;
          remove.disabled = true;
          try {
            await api(`/api/recordings/${encodeURIComponent(e.name)}`, { method: "DELETE" });
            if (opened === e.name) go("recordings");
            else await load();
          } catch (error) {
            if (!(error instanceof SignedOut)) replace(inspector, banner(error.message));
          }
        });
        return h("tr", { class: `clickable ${e.name === opened ? "selected" : ""}`, onclick: () => go("recordings", e.name) },
          h("td", { class: "nowrap" }, fmt.time(e.at)),
          h("td", { class: "mono" }, e.request_id, h("div", { class: "sub" }, e.path || "")),
          h("td", { class: "mono wrap-anywhere" }, e.model || "–", h("div", { class: "sub" }, e.backend || "")),
          h("td", null, statusBadge(e.status)),
          h("td", { class: "wrap-anywhere" }, e.outcome || h("span", { class: "muted" }, "in progress")),
          h("td", { class: "num" }, fmt.bytes(e.bytes)),
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
      inspect(inspector, opened, entry ? entry.files : null);
    }
  };

  filter.addEventListener("input", () => { state.recordingsFilter = filter.value; draw(); });
  refresh.addEventListener("click", () => guarded(list, load));
  removeAll.addEventListener("click", async () => {
    if (!confirm("Delete every recording? This cannot be undone.")) return;
    removeAll.disabled = true;
    try {
      const result = await api("/api/recordings", { method: "DELETE" });
      replace(inspector, banner(`Deleted ${result.removed} recording(s).`, "info"));
      await load();
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
}

// ---------------------------------------------------------------- inspector

/// `files` are the entry's files as listed; without them every known name is
/// tried.
async function inspect(target, name, files) {
  await guarded(target, async () => {
    const names = files || ["meta.json", "request.json", "response.json", "response.sse", "response.bin"];
    const responseName = names.find((file) => file.startsWith("response."));
    const [meta, request, response] = await Promise.all([
      recordedFile(name, "meta.json"),
      recordedFile(name, "request.json"),
      responseName ? recordedFile(name, responseName) : null,
    ]);
    const exchange = {
      name,
      files: { meta, request, response },
      meta: meta ? parseJson(meta.text) : null,
      parsed: {},
    };
    if (exchange.meta instanceof Error) exchange.meta = null;
    exchange.dialect = exchange.meta && String(exchange.meta.path || "").startsWith("/v1/chat/completions") ? "openai" : "anthropic";

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
    const close = h("button", { type: "button", class: "small", onclick: () => go("recordings") }, "Close");
    const title = exchange.meta ? exchange.meta.request_id : name;
    replace(target, panel(`Recording ${title}`, close,
      summaryFacts(exchange),
      h("div", { class: "controls" }, partSwitch, viewSwitch),
      body));
    draw();
    target.scrollIntoView({ block: "nearest" });
  });
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
  const response = exchange.files.response ? parsedOnce(exchange, "response", () => foldResponse(exchange.files.response, exchange.dialect)) : null;
  const usage = response && !(response instanceof Error) && response.usage ? usageText(response.usage) : null;
  return h("dl", { class: "facts" },
    fact("Request", `${m.method} ${m.path}${m.stream ? " (stream)" : ""}, ${fmt.time(m.received_at)}`),
    fact("Route", [h("span", { class: "mono" }, route.join(" ")), ` on ${m.backend}`]),
    fact("Result", [statusBadge(m.status), " ", outcomeBadge(m), m.outcome && m.outcome !== "complete" ? ` ${m.outcome}` : "",
      m.attempts > 1 ? `, ${m.attempts} attempts` : "",
      m.latency_ms !== undefined ? `, headers ${fmt.ms(m.latency_ms)}` : "",
      m.duration_ms !== undefined ? `, total ${fmt.ms(m.duration_ms)}` : ""]),
    fact("Size", `request ${fmt.bytes(exchange.files.request ? byteSize(exchange.files.request.text) : null)}, response ${fmt.bytes(m.response_bytes)}`),
    usage ? fact("Tokens", usage) : null);
}

function rawView(exchange, file) {
  if (!file) return missingFile(exchange);
  const cut = file.text.length > SHOWN_BYTES;
  const shown = cut ? file.text.slice(0, SHOWN_BYTES) : file.text;
  return [
    h("p", { class: "note" }, h("code", null, file.name), `, ${fmt.bytes(byteSize(file.text))}, exactly as recorded.`),
    cut ? banner(`Showing the first ${fmt.bytes(SHOWN_BYTES)} of ${fmt.bytes(file.text.length)}.`, "info") : null,
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
    { id: "prompt", label: "Last prompt", count: prompt < 0 ? "none" : `#${doc.messages[prompt].index}`, items: prompt < 0 ? [] : doc.messages.slice(prompt), render: (ctx) => promptSection(doc, prompt, ctx) },
    { id: "messages", label: "Messages", count: doc.messages.length, items: doc.messages, render: (ctx) => messagesSection(doc, matching(doc.messages, ctx.needle), ctx) },
    { id: "system", label: "System", count: doc.system.length, items: doc.system, render: (ctx) => systemSection(doc, matching(doc.system, ctx.needle), ctx) },
    { id: "tools", label: "Tools", count: doc.tools.length, items: doc.tools, render: (ctx) => toolsSection(doc, matching(doc.tools, ctx.needle), ctx) },
    { id: "params", label: "Parameters", count: doc.params.length, items: doc.params, render: (ctx) => paramsSection(matching(doc.params, ctx.needle), ctx) },
  ];
  const total = sections.slice(1).reduce((all, s) => all + sum(s.items), 0) || 1;
  const find = h("input", { type: "text", placeholder: "Find in the request: prompts, reminders, tool calls and results, system text, tools", "aria-label": "Find in the request", value: state.requestFind });
  const found = h("span", { class: "muted" });
  const nav = h("nav", { class: "section-nav", "aria-label": "Request sections" });
  const content = h("div", { class: "section-body" });
  const links = toolLinks(doc);
  const show = (id) => {
    state.requestSection = id;
    const needle = state.requestFind.trim().toLowerCase();
    const ctx = { ...links, needle, reveal };
    replace(found, needle ? `${sections.slice(1).reduce((n, s) => n + matching(s.items, needle).length, 0)} matching item(s)` : "");
    replace(nav, sections.map((s) => {
      const fill = h("span");
      fill.style.width = `${Math.min(100, (sum(s.items) / total) * 100)}%`;
      const hits = needle ? matching(s.items, needle).length : null;
      return h("button", { type: "button", class: hits === 0 ? "empty" : null, "aria-current": s.id === id ? "true" : null, onclick: () => show(s.id) },
        h("span", { class: "label" }, s.label),
        h("span", { class: "count" }, hits === null ? String(s.count) : `${hits} found`),
        h("span", { class: "size" }, fmt.bytes(sum(s.items))),
        h("span", { class: "bar" }, fill));
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
    .map(([key, value]) => indexed({ key, value }, { [key]: value }));
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
      messages: messages.slice(lead).map((m, i) => indexed({ index: lead + i, role: m.role, blocks: openaiMessage(m) }, m)),
    };
  }
  const system = typeof body.system === "string" ? [{ type: "text", text: body.system }] : Array.isArray(body.system) ? body.system : [];
  return {
    params,
    system: system.map((b, i) => indexed({ label: `system[${i}]`, blocks: [anthropicBlock(b)] }, b)),
    tools: tools.map((t) => indexed({ name: t.name || t.type || "?", description: t.description || "", schema: t.input_schema, extra: omit(t, ["name", "description", "input_schema"]) }, t)),
    messages: messages.map((m, i) => indexed({ index: i, role: m.role, blocks: anthropicContent(m.content) }, m)),
  };
}

function indexed(item, source) {
  item.bytes = byteSize(source);
  item.find = findText(source);
  return item;
}

/// Every key and string or scalar value under `value`, lowercased; encoded
/// payloads (`data`, `signature`) are left out.
function findText(value) {
  const parts = [];
  const walk = (v) => {
    if (typeof v === "string") parts.push(v);
    else if (typeof v === "number" || typeof v === "boolean") parts.push(String(v));
    else if (Array.isArray(v)) v.forEach(walk);
    else if (v && typeof v === "object") {
      for (const [key, inner] of Object.entries(v)) {
        parts.push(key);
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
  return content.map((part) => {
    if (part && part.type === "text") return { kind: "text", text: String(part.text ?? "") };
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

/// The position in `messages` of the last user message with text of its own,
/// beyond system reminders and tool results; -1 when there is none.
function lastPrompt(messages) {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role !== "user") continue;
    if (messages[i].blocks.some((b) => b.kind === "text" && b.text.replace(REMINDER, "").trim())) return i;
  }
  return -1;
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

function promptSection(doc, position, ctx) {
  if (position < 0) return h("p", { class: "note" }, "No user message carries text of its own.");
  const m = doc.messages[position];
  const after = doc.messages.slice(position + 1);
  const calls = after.reduce((n, next) => n + next.blocks.filter((b) => b.kind === "tool_use").length, 0);
  const reminders = m.blocks.reduce((n, b) => n + (b.kind === "text" ? (b.text.match(REMINDER) || []).length : 0), 0);
  const own = m.blocks
    .filter((b) => b.kind === "text")
    .map((b) => b.text.replace(REMINDER, "").trim())
    .filter(Boolean)
    .join("\n\n");
  const told = [`Message #${m.index} of ${doc.messages.length}`];
  if (reminders) told.push(`sent with ${reminders} system reminder(s)`);
  told.push(after.length ? `followed by ${after.length} message(s) with ${calls} tool call(s)` : "the last message");
  const opens = (item, fallback) => (ctx.needle ? item.find.includes(ctx.needle) : fallback);
  return [
    h("div", { class: "prompt text" }, marked(own, ctx.needle)),
    h("p", { class: "note" }, `${told.join(", ")}.`),
    messageItem(m, ctx, opens(m, false)),
    after.map((next) => messageItem(next, ctx, opens(next, next.bytes <= OPEN_BYTES))),
  ];
}

function messagesSection(doc, shown, ctx) {
  return [
    ctx.needle ? h("p", { class: "note" }, `${shown.length} of ${doc.messages.length} message(s) contain the text.`) : null,
    shown.map((m) => messageItem(m, ctx, Boolean(ctx.needle))),
  ];
}

function systemSection(doc, shown, ctx) {
  if (!doc.system.length) return h("p", { class: "note" }, "The request has no system prompt.");
  if (!shown.length) return h("p", { class: "note" }, "No system block contains the text.");
  return shown.map((s) => {
    const text = s.blocks.filter((b) => b.kind === "text").map((b) => b.text).join("\n");
    return lazyDetails({ class: "item" },
      [h("span", { class: "mono muted" }, s.label), s.blocks.some((b) => b.cache) ? badge("cache", "info") : null,
        h("span", { class: "preview" }, marked(firstLine(text), ctx.needle)), h("span", { class: "size" }, fmt.bytes(s.bytes))],
      () => s.blocks.map((b) => blockView(b, ctx)), Boolean(ctx.needle));
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
  return ordered.map((group) =>
    lazyDetails({ class: "item group" },
      [h("strong", null, group.label), h("span", { class: "kinds" }, `${group.tools.length} tool(s)`),
        h("span", { class: "preview" }, group.tools.map((t) => shortToolName(t, group.prefix)).join(", ")),
        h("span", { class: "size" }, fmt.bytes(group.tools.reduce((n, t) => n + t.bytes, 0)))],
      () => group.tools.map((tool) => toolItem(tool, group.prefix, ctx)), open));
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

function toolItem(tool, prefix, ctx) {
  return lazyDetails({ class: "item" },
    [h("strong", { class: "mono", title: tool.name }, marked(shortToolName(tool, prefix), ctx.needle)),
      h("span", { class: "preview" }, marked(firstLine(tool.description), ctx.needle)),
      h("span", { class: "size" }, fmt.bytes(tool.bytes))],
    () => [
      tool.description ? h("div", { class: "text" }, marked(tool.description, ctx.needle)) : null,
      tool.schema !== undefined ? [h("h4", null, "Input schema"), h("pre", null, marked(JSON.stringify(tool.schema, null, 2), ctx.needle))] : null,
      tool.extra ? [h("h4", null, "Other fields"), h("pre", null, marked(JSON.stringify(tool.extra, null, 2), ctx.needle))] : null,
    ], Boolean(ctx.needle));
}

function paramsSection(shown, ctx) {
  return table(["Field", "Value"], shown.map((p) =>
    h("tr", null,
      h("td", { class: "mono nowrap" }, marked(p.key, ctx.needle)),
      h("td", { class: "mono wrap-anywhere" }, jsonValue(p.value, ctx.needle)))),
  { empty: ctx.needle ? "No field contains the text." : "The request has no other fields." });
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
    counts.set(label, (counts.get(label) || 0) + 1);
  }
  return [...counts].map(([label, n]) => (n > 1 ? `${label} ×${n}` : label)).join(" · ");
}

function messagePreview(blocks) {
  for (const b of blocks) {
    if (b.kind === "text") {
      const own = b.text.replace(REMINDER, "").trim();
      if (own) return firstLine(own);
    }
  }
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
      return h("div", { class: "block" }, cache, textView(b.text, needle));
    case "thinking":
      return lazyDetails({ class: "item thinking" },
        [badge("thinking"), cache, h("span", { class: "preview" }, marked(firstLine(b.text), needle)), h("span", { class: "size" }, fmt.bytes(byteSize(b.text)))],
        () => h("div", { class: "text" }, marked(b.text, needle)), contains(b.text, needle));
    case "redacted_thinking":
      return h("div", { class: "block" }, badge("redacted thinking"), cache);
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
  for (let i = lower.indexOf(needle); i >= 0; i = lower.indexOf(needle, at)) {
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
  const calls = [];
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
        const slot = call.index ?? calls.length;
        calls[slot] = calls[slot] || { id: null, function: { name: "", arguments: "" } };
        if (call.id) calls[slot].id = call.id;
        if (call.function && call.function.name) calls[slot].function.name += call.function.name;
        if (call.function && call.function.arguments) calls[slot].function.arguments += call.function.arguments;
      }
      if (choice.finish_reason) out.stop = choice.finish_reason;
    }
  }
  if (reasoning) out.blocks.push({ kind: "thinking", text: reasoning });
  if (content) out.blocks.push({ kind: "text", text: content });
  for (const call of calls) if (call) out.blocks.push(openaiCall(call));
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
  const shown = new Set(["request_headers", "response_headers"]);
  return [
    table(["Field", "Value"], Object.entries(m).filter(([key]) => !shown.has(key)).map(([key, value]) =>
      h("tr", null, h("td", { class: "mono nowrap" }, key), h("td", { class: "mono wrap-anywhere" }, typeof value === "string" ? value : JSON.stringify(value))))),
    h("h3", null, "Request headers, as sent upstream"),
    headers(m.request_headers),
    h("h3", null, "Response headers"),
    headers(m.response_headers),
  ];
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
