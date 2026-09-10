PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS kernel_meta (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS daemon_fence (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  instance_id TEXT NOT NULL,
  fencing_epoch INTEGER NOT NULL CHECK (fencing_epoch >= 1),
  lease_expires_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_specs (
  agent_spec_id TEXT NOT NULL,
  version TEXT NOT NULL,
  digest TEXT NOT NULL,
  body BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY (agent_spec_id, version),
  UNIQUE (digest)
);

CREATE TABLE IF NOT EXISTS sessions (
  session_id TEXT PRIMARY KEY,
  principal_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  metadata BLOB
);

CREATE TABLE IF NOT EXISTS tasks (
  task_id TEXT PRIMARY KEY,
  session_id TEXT,
  created_by_actor_id TEXT NOT NULL,
  task_kind TEXT NOT NULL,
  payload BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL,
  FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);

CREATE TABLE IF NOT EXISTS resolved_run_environments (
  environment_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL UNIQUE,
  agent_spec_id TEXT NOT NULL,
  agent_spec_version TEXT NOT NULL,
  agent_spec_digest TEXT NOT NULL,
  agent_loop_id TEXT NOT NULL,
  agent_loop_version TEXT NOT NULL,
  agent_loop_digest TEXT NOT NULL,
  config_generation_id TEXT NOT NULL,
  workspace_uri TEXT,
  workspace_base_revision TEXT,
  workspace_mode INTEGER NOT NULL,
  model_provider TEXT,
  model_id TEXT,
  model_parameters BLOB,
  kernel_version TEXT NOT NULL,
  protocol_versions BLOB NOT NULL,
  capability_grant_ids BLOB NOT NULL,
  approval_request_ids BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runs (
  run_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  session_id TEXT,
  parent_run_id TEXT,
  state INTEGER NOT NULL,
  recovery_disposition INTEGER NOT NULL,
  run_revision INTEGER NOT NULL DEFAULT 0,
  loop_epoch INTEGER NOT NULL DEFAULT 0,
  step_sequence INTEGER NOT NULL DEFAULT 0,
  input_event_cursor TEXT NOT NULL DEFAULT '',
  cancellation_epoch INTEGER NOT NULL DEFAULT 0,
  resolved_environment_id TEXT,
  claim_owner TEXT,
  claim_token INTEGER,
  claim_expires_ms INTEGER,
  terminal_reason TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  FOREIGN KEY (task_id) REFERENCES tasks(task_id),
  FOREIGN KEY (parent_run_id) REFERENCES runs(run_id),
  FOREIGN KEY (resolved_environment_id) REFERENCES resolved_run_environments(environment_id)
);
CREATE INDEX IF NOT EXISTS idx_runs_task ON runs(task_id);
CREATE INDEX IF NOT EXISTS idx_runs_parent ON runs(parent_run_id);
CREATE INDEX IF NOT EXISTS idx_runs_state ON runs(state);

CREATE TABLE IF NOT EXISTS resolved_bindings (
  environment_id TEXT NOT NULL,
  port_id TEXT NOT NULL,
  adapter_id TEXT NOT NULL,
  adapter_version TEXT NOT NULL,
  adapter_digest TEXT NOT NULL,
  capabilities BLOB NOT NULL,
  PRIMARY KEY (environment_id, port_id),
  FOREIGN KEY (environment_id) REFERENCES resolved_run_environments(environment_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS run_graph_heads (
  task_id TEXT PRIMARY KEY,
  graph_revision INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY (task_id) REFERENCES tasks(task_id)
);

CREATE TABLE IF NOT EXISTS run_dependencies (
  dependency_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  source_run_id TEXT NOT NULL,
  target_run_id TEXT NOT NULL,
  dependency_condition TEXT NOT NULL,
  created_graph_revision INTEGER NOT NULL,
  created_at_ms INTEGER NOT NULL,
  UNIQUE (source_run_id, target_run_id),
  FOREIGN KEY (task_id) REFERENCES tasks(task_id),
  FOREIGN KEY (source_run_id) REFERENCES runs(run_id),
  FOREIGN KEY (target_run_id) REFERENCES runs(run_id)
);
CREATE INDEX IF NOT EXISTS idx_run_dependencies_target ON run_dependencies(target_run_id);
CREATE INDEX IF NOT EXISTS idx_run_dependencies_source ON run_dependencies(source_run_id);

CREATE TABLE IF NOT EXISTS workspace_leases (
  lease_id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL,
  owner_run_id TEXT NOT NULL,
  mode INTEGER NOT NULL,
  lease_epoch INTEGER NOT NULL,
  enforcement_state TEXT NOT NULL,
  delegated_from BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  FOREIGN KEY (owner_run_id) REFERENCES runs(run_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_workspace_exclusive_lease
  ON workspace_leases(workspace_id)
  WHERE mode = 2 AND enforcement_state = 'active';

CREATE TABLE IF NOT EXISTS effects (
  effect_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  step_sequence INTEGER NOT NULL,
  decision_id TEXT NOT NULL,
  operation TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  request_payload BLOB NOT NULL,
  effect_class INTEGER NOT NULL,
  idempotency_semantics INTEGER NOT NULL,
  reconciliation_semantics INTEGER NOT NULL,
  cancellation_semantics TEXT NOT NULL,
  compensation_capability TEXT,
  adapter_id TEXT NOT NULL,
  adapter_version TEXT NOT NULL,
  adapter_digest TEXT NOT NULL,
  state INTEGER NOT NULL,
  executor_id TEXT,
  executor_fencing_token INTEGER,
  lease_expires_ms INTEGER,
  provider_operation_ref TEXT,
  result_ref TEXT,
  error_code TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  UNIQUE (run_id, decision_id, operation, request_hash),
  FOREIGN KEY (run_id) REFERENCES runs(run_id)
);
CREATE INDEX IF NOT EXISTS idx_effects_state ON effects(state);
CREATE INDEX IF NOT EXISTS idx_effects_run ON effects(run_id);

CREATE TABLE IF NOT EXISTS resource_reservations (
  reservation_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('reserved','allocated','released','expired','unknown')),
  amount INTEGER NOT NULL,
  unit TEXT NOT NULL,
  fencing_token INTEGER NOT NULL,
  parent_reservation_id TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  FOREIGN KEY (run_id) REFERENCES runs(run_id),
  FOREIGN KEY (parent_reservation_id) REFERENCES resource_reservations(reservation_id)
);
CREATE INDEX IF NOT EXISTS idx_reservations_run ON resource_reservations(run_id);

CREATE TABLE IF NOT EXISTS timers (
  timer_id TEXT PRIMARY KEY,
  run_id TEXT,
  timer_kind TEXT NOT NULL,
  payload BLOB NOT NULL,
  due_at_ms INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('scheduled','claimed','fired','cancelled')),
  version INTEGER NOT NULL DEFAULT 0,
  claim_owner TEXT,
  claim_fencing_token INTEGER,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  FOREIGN KEY (run_id) REFERENCES runs(run_id)
);
CREATE INDEX IF NOT EXISTS idx_timers_due ON timers(state, due_at_ms);

CREATE TABLE IF NOT EXISTS capability_grants (
  grant_id TEXT PRIMARY KEY,
  principal_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  run_id TEXT,
  capability_id TEXT NOT NULL,
  scope BLOB NOT NULL,
  delegated_from_grant_id TEXT,
  expires_at_ms INTEGER,
  revoked_at_ms INTEGER,
  created_at_ms INTEGER NOT NULL,
  FOREIGN KEY (run_id) REFERENCES runs(run_id),
  FOREIGN KEY (delegated_from_grant_id) REFERENCES capability_grants(grant_id)
);

CREATE TABLE IF NOT EXISTS delegation_hops (
  chain_id TEXT NOT NULL,
  hop_index INTEGER NOT NULL,
  principal_or_actor_id TEXT NOT NULL,
  run_id TEXT,
  capability_grant_ids BLOB NOT NULL,
  PRIMARY KEY (chain_id, hop_index),
  FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

CREATE TABLE IF NOT EXISTS approval_requests (
  request_id TEXT PRIMARY KEY,
  request_digest TEXT NOT NULL,
  principal_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  run_id TEXT,
  operation TEXT NOT NULL,
  target_resource TEXT,
  capability_ids BLOB NOT NULL,
  extension_bundle_digest TEXT,
  config_generation_digest TEXT,
  expires_at_ms INTEGER NOT NULL,
  nonce TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending','approved','denied','expired')),
  created_at_ms INTEGER NOT NULL,
  resolved_at_ms INTEGER,
  UNIQUE (request_digest, nonce)
);

CREATE TABLE IF NOT EXISTS approval_responses (
  request_id TEXT PRIMARY KEY,
  request_digest TEXT NOT NULL,
  decision TEXT NOT NULL CHECK (decision IN ('approve','deny')),
  device_id TEXT NOT NULL,
  responder_principal_id TEXT NOT NULL,
  responded_at_ms INTEGER NOT NULL,
  FOREIGN KEY (request_id) REFERENCES approval_requests(request_id)
);

CREATE TABLE IF NOT EXISTS adapter_registrations (
  adapter_id TEXT NOT NULL,
  version TEXT NOT NULL,
  bundle_digest TEXT NOT NULL,
  manifest_digest TEXT NOT NULL,
  runtime_type TEXT NOT NULL,
  implemented_ports BLOB NOT NULL,
  capabilities BLOB NOT NULL,
  trust_state TEXT NOT NULL,
  conformance_state TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY (adapter_id, version, bundle_digest)
);

CREATE TABLE IF NOT EXISTS config_generations (
  generation_id TEXT PRIMARY KEY,
  digest TEXT NOT NULL UNIQUE,
  document BLOB NOT NULL,
  validation_state TEXT NOT NULL,
  test_state TEXT NOT NULL,
  created_by_actor_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS active_config_generation (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  generation_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  activated_at_ms INTEGER NOT NULL,
  FOREIGN KEY (generation_id) REFERENCES config_generations(generation_id)
);

CREATE TABLE IF NOT EXISTS idempotency_records (
  principal_id TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  request_digest TEXT NOT NULL,
  command_id TEXT NOT NULL,
  outcome_code TEXT NOT NULL,
  outcome_payload BLOB NOT NULL,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY (principal_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS event_stream_heads (
  stream_key TEXT PRIMARY KEY,
  last_sequence INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS outbox_events (
  event_id TEXT PRIMARY KEY,
  event_type TEXT NOT NULL,
  event_version INTEGER NOT NULL,
  stream_key TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  occurred_at_ms INTEGER NOT NULL,
  run_id TEXT,
  task_id TEXT,
  session_id TEXT,
  effect_id TEXT,
  causation_id TEXT,
  correlation_id TEXT,
  sensitivity INTEGER NOT NULL,
  retention INTEGER NOT NULL,
  payload BLOB NOT NULL,
  journal_published_at_ms INTEGER,
  live_published_at_ms INTEGER,
  UNIQUE (stream_key, sequence)
);
CREATE INDEX IF NOT EXISTS idx_outbox_unpublished
  ON outbox_events(journal_published_at_ms, stream_key, sequence);
