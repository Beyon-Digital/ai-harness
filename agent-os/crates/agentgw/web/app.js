/* Agent OS control-surface dashboard. Talks to agentgw's JSON REST + SSE API. */

const $ = (sel) => document.querySelector(sel);
const state = { eventSource: null, pollTimer: null };

async function api(path, opts = {}) {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json" },
    ...opts,
  });
  const text = await res.text();
  let body = {};
  try { body = text ? JSON.parse(text) : {}; } catch { body = { error: text }; }
  if (!res.ok) throw new Error(body.error || res.statusText);
  return body;
}

function toast(msg, isErr = false) {
  const el = $("#toast");
  el.textContent = msg;
  el.className = "toast" + (isErr ? " toast-err" : "");
  setTimeout(() => el.classList.add("hidden"), 4000);
}

function fmtMs(ms) {
  if (!ms) return "—";
  return new Date(Number(ms)).toLocaleTimeString();
}

function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

/* ---- tabs ---- */

document.querySelectorAll(".tab").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((b) => b.classList.remove("active"));
    document.querySelectorAll(".panel").forEach((p) => p.classList.remove("active"));
    btn.classList.add("active");
    $("#tab-" + btn.dataset.tab).classList.add("active");
    refreshTab(btn.dataset.tab);
  });
});

function refreshTab(tab) {
  if (tab === "runs") loadIndex();
  if (tab === "sessions") loadIndex();
  if (tab === "approvals") loadApprovals();
  if (tab === "adapters") loadAdapters();
  if (tab === "config") { loadConfig(); loadGenerations(); }
  if (tab === "system") loadSystem();
}

/* ---- health ---- */

async function loadHealth() {
  const el = $("#health");
  try {
    const h = await api("/api/health");
    el.textContent = `${h.status} · epoch ${h.daemon_fencing_epoch} · outbox ${h.outbox_unpublished_count}`;
    el.className = "badge " + (["ok", "healthy", "running"].includes(h.status) ? "badge-ok" : "badge-warn");
  } catch (e) {
    el.textContent = "daemon unreachable";
    el.className = "badge badge-down";
  }
}

/* ---- index-driven lists ---- */

async function loadIndex() {
  let idx;
  try { idx = await api("/api/index"); } catch (e) { return; }

  const sel = document.querySelector('select[name="spec_pick"]');
  const cur = sel.value;
  sel.innerHTML = '<option value="">— pick a submitted spec —</option>' +
    (idx.specs || []).map((s, i) =>
      `<option value="${i}">${esc(s.agent_spec_id)}@${esc(s.version)}</option>`).join("");
  sel.value = cur;
  sel.onchange = () => {
    const s = (idx.specs || [])[Number(sel.value)];
    if (!s) return;
    const f = document.querySelector("#form-create-run");
    f.agent_spec_id.value = s.agent_spec_id;
    f.spec_version.value = s.version;
    f.spec_digest.value = s.digest;
  };

  $("#sessions-table tbody").innerHTML = (idx.sessions || [])
    .map((s) => `<tr><td><code>${esc(s)}</code></td></tr>`).join("") ||
    '<tr><td><em>none recorded yet</em></td></tr>';

  $("#tasks-table tbody").innerHTML = (idx.tasks || [])
    .map((t) => `<tr><td><code>${esc(t.task_id)}</code></td><td><code>${esc(t.session_id)}</code></td></tr>`)
    .join("") || '<tr><td colspan="2"><em>none recorded yet</em></td></tr>';

  const runs = idx.runs || [];
  const rows = await Promise.all(runs.map(async (r) => {
    try {
      const run = await api("/api/runs/" + encodeURIComponent(r.run_id));
      return `<tr data-run="${esc(r.run_id)}">
        <td><code class="link" title="task ${esc(r.task_id)}">${esc(r.run_id)}</code></td>
        <td><span class="state state-${esc(run.state_name)}">${esc(run.state_name)}</span></td>
        <td>${run.run_revision}</td>
        <td><button class="mini" data-cancel="${esc(r.run_id)}">cancel</button></td>
      </tr>`;
    } catch {
      return `<tr><td><code>${esc(r.run_id)}</code></td><td colspan="3"><em>unknown</em></td></tr>`;
    }
  }));
  $("#runs-table tbody").innerHTML = rows.join("") ||
    '<tr><td colspan="4"><em>no runs recorded — create one or restart the gateway</em></td></tr>';

  if (state.selectedRun && runs.some((r) => r.run_id === state.selectedRun)) {
    try {
      renderRunDetail(await api("/api/runs/" + encodeURIComponent(state.selectedRun)));
    } catch { /* keep stale detail on transient errors */ }
  }
}

// Delegated clicks on the stable tbody — re-rendering rows mid-poll
// doesn't eat the click.
document.querySelector("#runs-table tbody").addEventListener("click", async (ev) => {
  const cancelBtn = ev.target.closest("[data-cancel]");
  if (cancelBtn) {
    ev.stopPropagation();
    try {
      await api(`/api/runs/${encodeURIComponent(cancelBtn.dataset.cancel)}/cancel`,
        { method: "POST", body: JSON.stringify({ reason: "cancelled via gui" }) });
      toast("cancel submitted");
      loadIndex();
    } catch (e) { toast(e.message, true); }
    return;
  }
  const link = ev.target.closest("code.link");
  if (link) selectRun(link.textContent.trim());
});

function decodeDataUri(ref) {
  try {
    const bytes = Uint8Array.from(
      atob(ref.slice("data:text/plain;base64,".length)),
      (c) => c.charCodeAt(0),
    );
    return new TextDecoder("utf-8").decode(bytes);
  } catch {
    return null;
  }
}

function renderRunDetail(r) {
  // Skip the rebuild when nothing changed — re-rendering collapses
  // expanded decision payloads on every poll. Decisions are refreshed
  // regardless: the journal trails the run row, so the decision list can
  // arrive a poll tick after the run reaches its terminal state.
  const fp = [r.state, r.run_revision, r.step_sequence, r.output_ref].join("|");
  if (state._runFp === fp) {
    loadDecisions(r.run_id);
    return;
  }
  state._runFp = fp;
  state._decFp = null;
  const box = $("#run-detail");
  const decoded = r.output_ref?.startsWith("data:text/plain;base64,")
    ? decodeDataUri(r.output_ref)
    : null;
  const out = decoded !== null
      ? `<div class="kv"><b>output</b><div class="answer">${esc(decoded)}</div></div>`
      : `<div class="kv"><b>output</b><code>${esc(r.output_ref || "—")}</code></div>`;
  box.innerHTML = `
    <div class="kv"><b>run</b><code>${esc(r.run_id)}</code></div>
    <div class="kv"><b>task</b><code>${esc(r.task_id)}</code></div>
    <div class="kv"><b>session</b><code>${esc(r.session_id)}</code></div>
    <div class="kv"><b>state</b><span class="state state-${esc(r.state_name)}">${esc(r.state_name)} (${r.state})</span></div>
    <div class="kv"><b>revision</b>${r.run_revision} · <b>epoch</b> ${r.loop_epoch} · <b>step</b> ${r.step_sequence}</div>
    ${out}
    <div class="kv"><b>decisions</b><div id="run-decisions"><em>loading…</em></div></div>
    <div class="kv"><b>environment</b><div id="run-env"><em>loading…</em></div></div>`;
  loadDecisions(r.run_id);
  loadEnvironment(r.run_id);
}

/* Resolved run environment is frozen at run start — load once per run. */
async function loadEnvironment(runId) {
  if (state._envRun === runId) return;
  const el = $("#run-env");
  if (!el) return;
  try {
    const { environment: env, bindings } = await api(
      "/api/runs/" + encodeURIComponent(runId) + "/environment");
    if (state.selectedRun !== runId) return;
    state._envRun = runId;
    const rows = [
      ["profile env", env.environment_id],
      ["loop adapter", `${env.agent_loop_id}@${env.agent_loop_version}`],
      ["generation", env.config_generation_id],
      ["model", [env.model_provider, env.model_id].filter(Boolean).join(" / ") || "—"],
      ["workspace", env.workspace_uri || "—"],
      ["kernel", `${env.kernel_version} · proto ${JSON.stringify(env.protocol_versions)}`],
    ].map(([k, v]) => `<div class="env-row"><b>${esc(k)}</b><code>${esc(v)}</code></div>`).join("");
    const binds = (bindings || []).map((b) =>
      `<div class="env-row"><b>${esc(b.port_id)}</b><code>${esc(b.adapter_id)}@${esc(b.adapter_version)}</code></div>`).join("");
    el.innerHTML = `<div class="env-box">${rows}${binds ? `<h4>bindings</h4>${binds}` : ""}</div>`;
  } catch (e) {
    // A 404 means the run has no frozen environment — cache it so we stop
    // repolling; any other failure retries on the next render.
    if (/no resolved environment/i.test(e.message)) state._envRun = runId;
    el.innerHTML = `<em>${esc(e.message)}</em>`;
  }
}

async function loadDecisions(runId) {
  const el = $("#run-decisions");
  if (!el) return;
  try {
    const { decisions } = await api(
      "/api/runs/" + encodeURIComponent(runId) + "/decisions");
    if (state.selectedRun !== runId) return;
    // Rewrite only on change so expanded payloads survive the poll.
    const decFp = JSON.stringify(decisions || []);
    if (state._decFp === decFp) return;
    state._decFp = decFp;
    el.innerHTML = (decisions || []).map((d) => {
      const det = d.kind === "invoke_effect"
        ? `<code>${esc(d.detail.operation)}</code>` +
          (d.detail.payload
            ? ` <details><summary>payload</summary><pre class="dec-payload">${esc(d.detail.payload)}</pre></details>`
            : "")
        : Object.entries(d.detail || {})
            .map(([k, v]) => `${esc(k)}=${esc(v)}`).join(" ");
      return `<div class="dec">step ${d.step_sequence} · <span class="badge">${esc(d.kind)}</span> ${det}</div>`;
    }).join("") || "<em>none yet</em>";
  } catch (e) {
    el.innerHTML = `<em>${esc(e.message)}</em>`;
  }
}

async function selectRun(runId) {
  state.selectedRun = runId;
  state._runFp = null;
  state._decFp = null;
  state._envRun = null;
  try {
    renderRunDetail(await api("/api/runs/" + encodeURIComponent(runId)));
    $("#stream-key").value = "run/" + runId;
    startStream();
    loadApprovals(runId);
  } catch (e) {
    $("#run-detail").innerHTML = `<em>${esc(e.message)}</em>`;
  }
}

/* ---- events ---- */

function startStream() {
  stopStream();
  const key = $("#stream-key").value.trim();
  if (!key) return;
  const src = new EventSource("/api/events/subscribe?stream_key=" + encodeURIComponent(key));
  state.eventSource = src;
  $("#btn-stream-stop").disabled = false;
  src.onmessage = (ev) => {
    const log = $("#events-log");
    let line = ev.data;
    try {
      const e = JSON.parse(ev.data);
      line = e.lag
        ? `‼ lag: resubscribe from seq ${e.resume_sequence}`
        : `#${e.sequence} ${e.event_type}${e.payload_b64 ? " " + e.payload_b64.slice(0, 120) : ""}`;
    } catch { }
    log.textContent += line + "\n";
    log.scrollTop = log.scrollHeight;
  };
  src.onerror = () => {
    $("#events-log").textContent += "— stream error/disconnected —\n";
    stopStream();
  };
}

function stopStream() {
  if (state.eventSource) { state.eventSource.close(); state.eventSource = null; }
  $("#btn-stream-stop").disabled = true;
}

/* ---- forms ---- */

$("#form-create-session").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const f = new FormData(ev.target);
  try {
    await api("/api/sessions", {
      method: "POST",
      body: JSON.stringify({ session_id: f.get("session_id"), metadata: f.get("metadata") }),
    });
    toast("session created");
    loadIndex();
  } catch (e) { toast(e.message, true); }
});

$("#form-put-spec").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const f = new FormData(ev.target);
  try {
    await api("/api/specs", {
      method: "POST",
      body: JSON.stringify({
        agent_spec_id: f.get("agent_spec_id"),
        version: f.get("version"),
        body: f.get("body"),
      }),
    });
    toast("spec revision submitted");
  } catch (e) { toast(e.message, true); }
});

$("#form-create-run").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const f = new FormData(ev.target);
  const caps = (f.get("capabilities") || "").split(",").map((s) => s.trim()).filter(Boolean);
  try {
    const r = await api("/api/runs", {
      method: "POST",
      body: JSON.stringify({
        session_id: f.get("session_id"),
        task_id: f.get("task_id"),
        run_id: f.get("run_id"),
        agent_spec_id: f.get("agent_spec_id"),
        spec_version: f.get("spec_version"),
        spec_digest: f.get("spec_digest"),
        task_payload: f.get("task_payload"),
        parent_run_id: f.get("parent_run_id"),
        requested_profile: f.get("requested_profile"),
        requested_capabilities: caps,
      }),
    });
    toast("run submitted: " + (r.command_id || "ok"));
    loadIndex();
    if (r.run_id) selectRun(r.run_id);
  } catch (e) { toast(e.message, true); }
});

/* ---- approvals ---- */

async function loadApprovals(runId) {
  const q = runId ?? $("#approvals-run-id").value.trim();
  let approvals = [];
  if (q) {
    try { approvals = (await api("/api/approvals?run_id=" + encodeURIComponent(q))).approvals; }
    catch (e) { toast(e.message, true); }
  } else {
    // no list-all RPC — gather for every known run
    try {
      const idx = await api("/api/index");
      for (const r of idx.runs || []) {
        try {
          const a = await api("/api/approvals?run_id=" + encodeURIComponent(r.run_id));
          approvals.push(...a.approvals);
        } catch { }
      }
    } catch { }
  }
  const pending = approvals.filter((a) => a.state === "pending" || a.state === "Pending");
  $("#pending-count").textContent = pending.length ? String(pending.length) : "";
  $("#approvals-table tbody").innerHTML = approvals.map((a) => `<tr>
    <td><code title="${esc(a.request_digest)}">${esc(a.request_id.slice(0, 13))}…</code></td>
    <td><code>${esc(a.run_id)}</code></td>
    <td>${esc(a.operation)}</td>
    <td>${esc(a.state)}</td>
    <td>${(a.state === "pending" || a.state === "Pending") ? `
      <button class="mini ok" data-appr="${esc(a.request_id)}" data-digest="${esc(a.request_digest)}" data-dec="approve">approve</button>
      <button class="mini bad" data-appr="${esc(a.request_id)}" data-digest="${esc(a.request_digest)}" data-dec="deny">deny</button>` : ""}</td>
  </tr>`).join("") || '<tr><td colspan="5"><em>no approval requests</em></td></tr>';

}

document.querySelector("#approvals-table tbody").addEventListener("click", async (ev) => {
  const el = ev.target.closest("[data-appr]");
  if (!el) return;
  try {
    await api("/api/approvals/respond", {
      method: "POST",
      body: JSON.stringify({
        request_id: el.dataset.appr,
        request_digest: el.dataset.digest,
        decision: el.dataset.dec,
        device_id: crypto.randomUUID(),
      }),
    });
    toast(el.dataset.dec + " recorded");
    loadApprovals();
  } catch (e) { toast(e.message, true); }
});

/* ---- adapters / config ---- */

async function loadAdapters() {
  try {
    const { adapters } = await api("/api/adapters");
    $("#adapters-table tbody").innerHTML = adapters.map((a) => `<tr>
      <td><code>${esc(a.adapter?.id || "?")}</code></td>
      <td>${esc(a.adapter?.version || "")}</td>
      <td>${esc(a.trust_state)} · ${esc(a.conformance_state)}</td>
      <td>${(a.ports || []).map((p) => `<code>${esc(p)}</code>`).join(" ")}</td>
    </tr>`).join("") || '<tr><td colspan="4"><em>none registered</em></td></tr>';
  } catch (e) { toast(e.message, true); }
}

async function loadConfig() {
  try {
    const c = await api("/api/config");
    $("#config-view").textContent =
      `generation ${c.generation_id}\ndigest ${c.digest}\nvalidation ${c.validation_state} · tested ${c.test_state} · rev ${c.active_revision}\n\n${c.document}`;
  } catch (e) { $("#config-view").textContent = e.message; }
}

async function loadGenerations() {
  try {
    const { generations } = await api("/api/config/generations");
    $("#generations-table tbody").innerHTML = (generations || []).map((g) => `<tr>
      <td><code title="${esc(g.digest)}">${esc(g.generation_id)}</code></td>
      <td>${esc(g.validation_state)}</td>
      <td>${esc(g.test_state)}</td>
      <td>${fmtMs(g.created_at_ms)}</td>
      <td>${g.active ? '<span class="badge badge-ok">active</span>' : ""}</td>
    </tr>`).join("") || '<tr><td colspan="5"><em>no generations</em></td></tr>';
  } catch (e) {
    $("#generations-table tbody").innerHTML =
      `<tr><td colspan="5"><em>${esc(e.message)}</em></td></tr>`;
  }
}

/* ---- system / metrics ---- */

const RUN_STATE_NAMES = {
  1: "created", 2: "ready", 3: "running", 4: "waiting_tool",
  5: "waiting_child", 6: "waiting_human", 7: "suspended", 8: "cancelling",
  9: "completed", 10: "failed", 11: "cancelled",
};
const EFFECT_STATE_NAMES = {
  1: "prepared", 2: "claimed", 3: "dispatched", 4: "acknowledged",
  5: "committed", 6: "failed", 7: "cancelled", 8: "unknown",
};

function metricRows(list, names) {
  return (list || []).map((r) => {
    const s = r.state ?? r.kind ?? "—";
    const label = names?.[s] || s;
    return `<tr><td>${esc(label)}</td><td class="num">${r.count}</td></tr>`;
  }).join("");
}

async function loadSystem() {
  const el = $("#metrics-view");
  try {
    const m = await api("/api/metrics");
    const counter = (k) => `<div class="kv"><b>${esc(k)}</b>${m[k] ?? "—"}</div>`;
    el.innerHTML = `
      <div class="columns">
        <div class="col">
          <h3>Runs</h3>
          <table class="mini-table"><tbody>${metricRows(m.runs_by_state, RUN_STATE_NAMES)}</tbody></table>
          <h3>Effects</h3>
          <table class="mini-table"><tbody>${metricRows(m.effects_by_state, EFFECT_STATE_NAMES)}</tbody></table>
          <h3>Approvals</h3>
          <table class="mini-table"><tbody>${metricRows(m.approvals_by_state)}</tbody></table>
          <h3>Timers</h3>
          <table class="mini-table"><tbody>${metricRows(m.timers_by_state)}</tbody></table>
        </div>
        <div class="col">
          <h3>Totals</h3>
          ${counter("decisions_total")}
          ${counter("loop_turns_total")}
          ${counter("adapters_registered")}
          ${counter("adapter_instances_total")}
          ${counter("conformance_reports_total")}
          ${counter("reservations_total")}
          ${counter("outbox_pending")}
          ${counter("journal_events_total")}
          ${counter("journal_max_sequence")}
          <div class="kv"><b>active generation</b><code>${esc(m.active_generation || "—")}</code></div>
          <h3>Generations</h3>
          <table class="mini-table"><tbody>${metricRows(m.generations_by_state)}</tbody></table>
        </div>
      </div>` +
      (m.kernel_db_error || m.events_db_error
        ? `<p class="err">${esc(m.kernel_db_error || m.events_db_error)}</p>`
        : "");
  } catch (e) {
    el.innerHTML = `<em>${esc(e.message)}</em>`;
  }
}

/* ---- misc wiring ---- */

$("#btn-stream").addEventListener("click", startStream);
$("#btn-stream-stop").addEventListener("click", stopStream);
$("#btn-approvals").addEventListener("click", () => loadApprovals());
$("#btn-adapters").addEventListener("click", loadAdapters);
$("#btn-config").addEventListener("click", () => { loadConfig(); loadGenerations(); });
$("#btn-metrics").addEventListener("click", loadSystem);
$("#btn-graph").addEventListener("click", async () => {
  const id = $("#graph-task-id").value.trim();
  if (!id) return;
  try {
    const g = await api("/api/tasks/" + encodeURIComponent(id) + "/graph");
    $("#graph-view").textContent = JSON.stringify(g, null, 2);
  } catch (e) { $("#graph-view").textContent = e.message; }
});

loadHealth();
loadIndex();
state.pollTimer = setInterval(() => { loadHealth(); loadIndex(); }, 3000);
