"use strict";

// The Recordings tab. Loaded before app.js; it only defines functions, which
// use app.js's helpers when they run.

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
