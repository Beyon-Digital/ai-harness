/* Typed client for the agentgw JSON API (same routes the daemon exposes). */

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    headers: init?.body ? { "content-type": "application/json" } : undefined,
    ...init,
  });
  if (!res.ok) {
    let detail = res.statusText;
    try {
      const body = await res.json();
      detail = body.message || body.error || detail;
    } catch {
      /* keep statusText */
    }
    throw new ApiError(res.status, detail);
  }
  return res.json() as Promise<T>;
}

/* ---- types ---- */

export interface Health {
  status: string;
  daemon_instance_id: string;
  daemon_fencing_epoch: number;
  active_config_generation_id: string;
  outbox_unpublished_count: number;
}

export interface Session {
  session_id: string;
  metadata?: string;
}

export interface Spec {
  agent_spec_id: string;
  version: string;
  digest: string;
  display_name?: string;
  profile?: string;
}

export interface Run {
  run_id: string;
  task_id: string;
  session_id: string;
  state: number;
  state_name: string;
  loop_epoch: number;
  step_sequence: number;
  run_revision: number;
  output_ref: string;
  parent_run_id: string;
  resolved_environment_id: string;
}

export interface Decision {
  sequence: number;
  step_sequence: number;
  kind: string;
  detail: Record<string, string>;
}

export interface Environment {
  environment: {
    environment_id: string;
    agent_loop_id: string;
    agent_loop_version: string;
    config_generation_id: string;
    model_provider: string;
    model_id: string;
    workspace_uri: string;
    kernel_version: string;
    protocol_versions: number[];
  };
  bindings: { port_id: string; adapter_id: string; adapter_version: string }[];
}

export interface Adapter {
  adapter?: { id: string; version: string };
  manifest_digest: string;
  runtime_type: string;
  trust_state: string;
  conformance_state: string;
  ports: string[];
}

export interface Generation {
  generation_id: string;
  digest: string;
  validation_state: string;
  test_state: string;
  created_by: string;
  created_at_ms: number;
  active: boolean;
}

export interface Profile {
  name: string;
  bindings: Record<string, string>;
}

export interface Approval {
  request_id: string;
  request_digest: string;
  principal_id?: string;
  actor_id?: string;
  run_id?: string;
  operation?: string;
  target_resource?: string;
  state?: string;
  expires_at_ms?: number;
  created_at_ms?: number;
}

export interface Index {
  sessions: Session[];
  specs: Spec[];
  runs: Run[];
}

/* ---- calls ---- */

export const getHealth = () => api<Health>("/api/health");
export const getIndex = () => api<Index>("/api/index");
export const getAdapters = () =>
  api<{ adapters: Adapter[] }>("/api/adapters").then((r) => r.adapters);
export const getRun = (id: string) => api<Run>(`/api/runs/${id}`);
export const getRuns = () => api<{ runs: Run[] }>("/api/runs").then((r) => r.runs);
export const getDecisions = (id: string) =>
  api<{ decisions: Decision[] }>(`/api/runs/${id}/decisions`).then(
    (r) => r.decisions,
  );
export const getEnvironment = (id: string) =>
  api<Environment>(`/api/runs/${id}/environment`);
export const getGenerations = () =>
  api<{ generations: Generation[] }>("/api/config/generations").then(
    (r) => r.generations,
  );
export const getConfig = () =>
  api<{ generation_id: string; digest: string; document: string }>(
    "/api/config",
  );
export const getProfiles = () =>
  api<{ profiles: Profile[] }>("/api/profiles").then((r) => r.profiles);
export const getMetrics = () =>
  api<Record<string, unknown>>("/api/metrics");

export function flattenMetrics(
  obj: Record<string, unknown>,
  prefix = "",
): { name: string; value: string }[] {
  const out: { name: string; value: string }[] = [];
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === "object" && !Array.isArray(v))
      out.push(...flattenMetrics(v as Record<string, unknown>, key));
    else if (Array.isArray(v)) {
      // `*_by_state` rows arrive as [{state, count}]; render compactly.
      const parts = v.map((row) => {
        const r = row as { state?: unknown; count?: unknown };
        return r && typeof r === "object" && "count" in r
          ? `${r.state}: ${r.count}`
          : JSON.stringify(row);
      });
      out.push({ name: key, value: parts.length ? parts.join("  ") : "0" });
    } else out.push({ name: key, value: v === null ? "—" : String(v) });
  }
  return out;
}
export const getApprovals = () =>
  api<{ approvals: Approval[] }>("/api/approvals").then((r) => r.approvals);
export const getProposals = () =>
  api<{ proposals: ConfigProposal[] }>("/api/config/proposals").then(
    (r) => r.proposals,
  );

export interface ConfigProposal {
  proposal_id: string;
  path: string;
  created_at_ms: number;
  valid: boolean;
  error?: string;
}

export function createSession(sessionId: string, metadata = "") {
  return api<{ session_id: string }>("/api/sessions", {
    method: "POST",
    body: JSON.stringify({ session_id: sessionId, metadata }),
  });
}

export function putSpec(
  agentSpecId: string,
  version: string,
  body: string,
) {
  return api<{ agent_spec_id: string; digest: string }>("/api/specs", {
    method: "POST",
    body: JSON.stringify({
      agent_spec_id: agentSpecId,
      version,
      body,
    }),
  });
}

export interface CreateRunArgs {
  sessionId: string;
  specId?: string;
  specVersion?: string;
  specDigest?: string;
  profile?: string;
  taskPayload: string;
  taskId?: string;
  runId?: string;
}

export function createRun(a: CreateRunArgs) {
  return api<{ run_id: string; task_id: string }>("/api/runs", {
    method: "POST",
    body: JSON.stringify({
      session_id: a.sessionId,
      task_id: a.taskId || "",
      run_id: a.runId || "",
      agent_spec_id: a.specId || "",
      spec_version: a.specVersion || "",
      spec_digest: a.specDigest || "",
      task_payload: a.taskPayload,
      requested_profile: a.profile || "",
    }),
  });
}

export function cancelRun(runId: string, reason: string) {
  return api(`/api/runs/${runId}/cancel`, {
    method: "POST",
    body: JSON.stringify({ reason }),
  });
}

export function respondApproval(
  requestId: string,
  requestDigest: string,
  decision: "approve" | "deny",
) {
  return api(`/api/approvals/respond`, {
    method: "POST",
    body: JSON.stringify({
      request_id: requestId,
      request_digest: requestDigest,
      decision,
    }),
  });
}

export function proposeConfig(document: string) {
  return api<ConfigProposal>("/api/config/proposals", {
    method: "POST",
    body: JSON.stringify({ document }),
  });
}

/* SSE event stream — returns an unsubscribe fn. */
export function subscribeEvents(
  streamKey: string,
  onEvent: (eventType: string, data: unknown) => void,
): () => void {
  const es = new EventSource(
    `/api/events/subscribe?stream_key=${encodeURIComponent(streamKey)}`,
  );
  es.onmessage = (m) => {
    try {
      const parsed = JSON.parse(m.data);
      onEvent(parsed.event_type || parsed.type || m.type, parsed);
    } catch {
      onEvent(m.type, m.data);
    }
  };
  return () => es.close();
}

/* uuidv7 (time-ordered) — kernel requires v7 entity ids. */
export function uuidv7(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const ms = Date.now();
  bytes[0] = (ms / 2 ** 40) & 0xff;
  bytes[1] = (ms / 2 ** 32) & 0xff;
  bytes[2] = (ms / 2 ** 24) & 0xff;
  bytes[3] = (ms / 2 ** 16) & 0xff;
  bytes[4] = (ms / 2 ** 8) & 0xff;
  bytes[5] = ms & 0xff;
  bytes[6] = (bytes[6] & 0x0f) | 0x70;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function decodeDataUri(uri: string): string | null {
  const m = /^data:[^;,]*;base64,(.*)$/.exec(uri || "");
  if (!m) return null;
  try {
    const binary = atob(m[1]);
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    return new TextDecoder().decode(bytes);
  } catch {
    return null;
  }
}
