const state = {
  activeSessionId: null,
  activeTurnId: null,
  source: null,
  textNodes: new Map(),
};

const els = {
  health: document.querySelector("#health-pill"),
  model: document.querySelector("#model-pill"),
  sessions: document.querySelector("#session-list"),
  title: document.querySelector("#active-session-title"),
  prompt: document.querySelector("#prompt-input"),
  run: document.querySelector("#run-turn"),
  newSession: document.querySelector("#new-session"),
  timeline: document.querySelector("#timeline"),
  clear: document.querySelector("#clear-events"),
  turnState: document.querySelector("#turn-state"),
  detailSession: document.querySelector("#detail-session"),
  detailTurn: document.querySelector("#detail-turn"),
  detailStatus: document.querySelector("#detail-status"),
  detailTrace: document.querySelector("#detail-trace"),
  finalAnswer: document.querySelector("#final-answer"),
};

const eventNames = [
  "turn/started",
  "item/step/started",
  "item/step/completed",
  "item/agentMessage/started",
  "item/agentMessage/delta",
  "item/agentMessage/completed",
  "item/toolCall/started",
  "item/toolCall/completed",
  "runtime/warning",
  "item/patch/detected",
  "item/question/requested",
  "turn/error",
  "turn/completed",
  "turn/failed",
];

async function boot() {
  await refreshHealth();
  await refreshModels();
  await refreshSessions();
  els.newSession.addEventListener("click", createSession);
  els.run.addEventListener("click", runTurn);
  els.clear.addEventListener("click", clearTimeline);
}

async function refreshHealth() {
  try {
    const data = await getJSON("/api/health");
    els.health.textContent = data.ok ? "online" : "offline";
    els.health.className = data.ok ? "pill ok" : "pill bad";
  } catch {
    els.health.textContent = "offline";
    els.health.className = "pill bad";
  }
}

async function refreshModels() {
  try {
    const data = await getJSON("/api/models");
    const model = data.models?.[0];
    els.model.textContent = model ? model.id : "no model";
  } catch {
    els.model.textContent = "model unavailable";
  }
}

async function refreshSessions() {
  const data = await getJSON("/api/sessions");
  els.sessions.replaceChildren();
  if (!data.sessions?.length) {
    const empty = document.createElement("div");
    empty.className = "session-meta";
    empty.textContent = "还没有会话";
    els.sessions.append(empty);
    return;
  }
  data.sessions.forEach((session) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `session-item ${session.id === state.activeSessionId ? "active" : ""}`;
    button.innerHTML = `
      <div class="session-id">${escapeHTML(session.id)}</div>
      <div class="session-meta">${session.message_count || 0} messages · ${escapeHTML(session.status || "idle")}</div>
    `;
    button.addEventListener("click", () => selectSession(session.id));
    els.sessions.append(button);
  });
}

async function createSession() {
  const data = await postJSON("/api/sessions", {});
  selectSession(data.session.id);
  await refreshSessions();
}

async function selectSession(sessionId) {
  state.activeSessionId = sessionId;
  const data = await getJSON(`/api/sessions/${encodeURIComponent(sessionId)}`);
  els.title.textContent = shortId(sessionId);
  els.detailSession.textContent = sessionId;
  els.detailStatus.textContent = data.session.status || "idle";
  await refreshSessions();
}

async function runTurn() {
  const input = els.prompt.value.trim();
  if (!input) return;
  if (!state.activeSessionId) {
    const data = await postJSON("/api/sessions", {});
    state.activeSessionId = data.session.id;
    els.detailSession.textContent = state.activeSessionId;
  }

  clearTimeline();
  els.run.disabled = true;
  els.turnState.textContent = "starting";
  els.finalAnswer.textContent = "-";

  try {
    const data = await postJSON(`/api/sessions/${encodeURIComponent(state.activeSessionId)}/turns`, {
      input,
    });
    state.activeTurnId = data.turn.id;
    els.detailTurn.textContent = state.activeTurnId;
    streamTurn(state.activeTurnId);
  } catch (error) {
    els.run.disabled = false;
    els.turnState.textContent = "failed";
    addEventRow("turn/failed", error.message, "error");
  }
}

function streamTurn(turnId) {
  if (state.source) state.source.close();
  const source = new EventSource(`/api/turns/${encodeURIComponent(turnId)}/events`);
  state.source = source;
  eventNames.forEach((name) => {
    source.addEventListener(name, (message) => handleAppEvent(JSON.parse(message.data)));
  });
  source.onerror = () => {
    if (els.detailStatus.textContent !== "completed" && els.detailStatus.textContent !== "failed") {
      els.turnState.textContent = "connection retrying";
    }
  };
}

function handleAppEvent(appEvent) {
  const { method, params } = appEvent;
  els.turnState.textContent = method;
  els.detailStatus.textContent = method.includes("failed") ? "failed" : method.includes("completed") ? "completed" : "running";

  if (method === "item/agentMessage/delta") {
    renderTextDelta(params.event);
    return;
  }
  if (method === "turn/completed" || method === "turn/failed") {
    els.run.disabled = false;
    els.turnState.textContent = method === "turn/completed" ? "completed" : "failed";
    els.finalAnswer.textContent = params.final_answer || "-";
    els.detailTrace.textContent = traceLabel(params.trace);
    if (state.source) state.source.close();
  }

  const severity = method.includes("warning") ? "warning" : method.includes("failed") || method.includes("error") ? "error" : "";
  addEventRow(method, summarizeParams(params), severity, classForMethod(method));
}

function renderTextDelta(event) {
  const id = event.id || "assistant";
  let node = state.textNodes.get(id);
  if (!node) {
    node = addEventRow("item/agentMessage/delta", "", "", "agent-message");
    state.textNodes.set(id, node.querySelector(".event-body"));
  }
  node.textContent += event.text || "";
  els.timeline.scrollTop = els.timeline.scrollHeight;
}

function addEventRow(method, body, severity = "", extraClass = "") {
  removeEmptyState();
  const row = document.createElement("article");
  row.className = ["event-row", severity, extraClass].filter(Boolean).join(" ");
  const time = new Date().toLocaleTimeString();
  row.innerHTML = `
    <div class="event-head">
      <span class="event-method">${escapeHTML(method)}</span>
      <span>${time}</span>
    </div>
    <div class="event-body">${escapeHTML(body)}</div>
  `;
  els.timeline.append(row);
  els.timeline.scrollTop = els.timeline.scrollHeight;
  return row;
}

function classForMethod(method) {
  if (method.includes("toolCall")) return "tool-event";
  if (method.includes("agentMessage")) return "agent-message";
  return "";
}

function summarizeParams(params) {
  const event = params.event || {};
  if (event.type === "tool-call") {
    return `${event.name} ${JSON.stringify(event.input || {})}`;
  }
  if (event.type === "tool-result") {
    return event.error ? `${event.call_id}\nERROR: ${event.error}` : `${event.call_id}\n${event.output || ""}`;
  }
  if (event.type === "runtime-warning") {
    return `${event.code}\n${event.message}`;
  }
  if (event.type === "step-start") {
    return `snapshot: ${event.snapshot_id || "-"}`;
  }
  if (event.type === "step-finish") {
    return `finish: ${event.finish_reason || "-"}\ntokens: ${JSON.stringify(event.tokens || {})}\ncost: ${event.cost ?? 0}`;
  }
  if (event.type === "patch") {
    return `files: ${(event.files || []).length}\nhash: ${event.hash || "-"}`;
  }
  if (event.type === "error") {
    return event.error || "Unknown error";
  }
  return JSON.stringify(params, null, 2);
}

function clearTimeline() {
  state.textNodes.clear();
  els.timeline.innerHTML = `
    <div class="empty-state">
      <div class="empty-title">等待任务开始</div>
      <div class="empty-copy">模型输出、工具调用、runtime warning 和最终结果会流式出现在这里。</div>
    </div>
  `;
}

function removeEmptyState() {
  const empty = els.timeline.querySelector(".empty-state");
  if (empty) empty.remove();
}

async function getJSON(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

async function postJSON(url, body) {
  const response = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text);
  }
  return response.json();
}

function shortId(id) {
  return id ? `${id.slice(0, 18)}…` : "-";
}

function traceLabel(trace) {
  if (!trace) return "-";
  return trace.trace_id || trace.run_id || trace.trace_path || "-";
}

function escapeHTML(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

boot().catch((error) => {
  els.health.textContent = "offline";
  els.health.className = "pill bad";
  addEventRow("app/error", error.message, "error");
});
