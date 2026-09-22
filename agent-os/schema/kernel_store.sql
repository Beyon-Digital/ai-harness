-- Inception KernelStore schema. Executed verbatim when kernel.db is created.
-- Normative PRAGMA block (architecture/persistence.md:12-19, R6.3).
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS kernel_meta (
  key TEXT PRIMARY KEY,
  value BLOB NOT NULL
);

-- Inception schema version (design Data model, R5.2). Only this schema creates tables.
INSERT OR IGNORE INTO kernel_meta (key, value) VALUES ('schema_version', '1');

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
  -- RunState (contracts/domain/core.proto): 1=CREATED, 2=READY, 3=RUNNING, 4=WAITING_TOOL,
  -- 5=WAITING_CHILD, 6=WAITING_HUMAN, 7=SUSPENDED, 8=CANCELLING, 9=COMPLETED, 10=FAILED,
  -- 11=CANCELLED. 0=RUN_STATE_UNSPECIFIED is never persisted.
  state INTEGER NOT NULL CHECK (state BETWEEN 1 AND 11),
  -- RecoveryDisposition (contracts/domain/core.proto): 1=NORMAL, 2=NEEDS_RECONCILIATION,
  -- 3=RECOVERING, 4=BLOCKED_UNKNOWN_EFFECT, 5=BLOCKED_MISSING_RESOURCE,
  -- 6=REQUIRES_HUMAN_DECISION. 0=RECOVERY_UNSPECIFIED is never persisted.
  recovery_disposition INTEGER NOT NULL CHECK (recovery_disposition BETWEEN 1 AND 6),
  run_revision INTEGER NOT NULL DEFAULT 0,
  loop_epoch INTEGER NOT NULL DEFAULT 0,
  step_sequence INTEGER NOT NULL DEFAULT 0,
  input_event_cursor TEXT NOT NULL DEFAULT '',
  cancellation_epoch INTEGER NOT NULL DEFAULT 0,
  resolved_environment_id TEXT,
  -- Binding inputs captured at creation (immutable): exact AgentSpec ref,
  -- requested profile, and optional pre-provisioned workspace URI.
  agent_spec_id TEXT,
  agent_spec_version TEXT,
  agent_spec_digest TEXT,
  requested_profile TEXT NOT NULL DEFAULT '',
  workspace_uri TEXT,
  output_ref TEXT,
  current_turn_id TEXT,
  claim_owner TEXT,
  claim_token INTEGER,
  claim_expires_ms INTEGER,
  claim_daemon_epoch INTEGER,
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
  -- DependencyCondition (contracts/domain/entities.proto): completed_successfully=COMPLETED_SUCCESSFULLY,
  -- any_terminal=ANY_TERMINAL, completed_or_cancelled=COMPLETED_OR_CANCELLED.
  dependency_condition TEXT NOT NULL CHECK (dependency_condition IN ('completed_successfully','any_terminal','completed_or_cancelled')),
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
  -- WorkspaceAccessMode (contracts/domain/core.proto): 1=READ_ONLY, 2=EXCLUSIVE_WRITE,
  -- 3=ISOLATED_FORK, 4=SHARED_COORDINATED_WRITE. 0=WORKSPACE_MODE_UNSPECIFIED is never persisted.
  mode INTEGER NOT NULL CHECK (mode BETWEEN 1 AND 4),
  lease_epoch INTEGER NOT NULL,
  -- Workspace lease enforcement lifecycle (specs/workspace.md): active -> revoked on transfer.
  enforcement_state TEXT NOT NULL CHECK (enforcement_state IN ('active','revoked')),
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
  -- EffectClass (contracts/domain/effects.proto): 1=EFFECT_CLASS_READ_ONLY, 2=LOCAL_MUTATION,
  -- 3=EXTERNAL_MUTATION, 4=OPAQUE. 0=EFFECT_CLASS_UNSPECIFIED is never persisted.
  effect_class INTEGER NOT NULL CHECK (effect_class BETWEEN 1 AND 4),
  -- IdempotencySemantics (contracts/domain/effects.proto): 1=NATURALLY_IDEMPOTENT,
  -- 2=IDEMPOTENCY_KEY_SUPPORTED, 3=NOT_IDEMPOTENT, 4=UNKNOWN_IDEMPOTENCY.
  -- 0=IDEMPOTENCY_UNSPECIFIED is never persisted.
  idempotency_semantics INTEGER NOT NULL CHECK (idempotency_semantics BETWEEN 1 AND 4),
  -- ReconciliationSemantics (contracts/domain/effects.proto): 1=STATUS_LOOKUP, 2=RESULT_LOOKUP,
  -- 3=DETERMINISTIC_INSPECTION, 4=IMPOSSIBLE, 5=UNKNOWN_RECONCILIATION.
  -- 0=RECONCILIATION_UNSPECIFIED is never persisted.
  reconciliation_semantics INTEGER NOT NULL CHECK (reconciliation_semantics BETWEEN 1 AND 5),
  cancellation_semantics TEXT NOT NULL,
  compensation_capability TEXT,
  adapter_id TEXT NOT NULL,
  adapter_version TEXT NOT NULL,
  adapter_digest TEXT NOT NULL,
  -- EffectState (contracts/domain/effects.proto): 1=PREPARED, 2=CLAIMED, 3=DISPATCHED,
  -- 4=ACKNOWLEDGED, 5=COMMITTED, 6=EFFECT_STATE_FAILED, 7=EFFECT_STATE_CANCELLED, 8=UNKNOWN.
  -- 0=EFFECT_STATE_UNSPECIFIED is never persisted.
  state INTEGER NOT NULL CHECK (state BETWEEN 1 AND 8),
  executor_id TEXT,
  executor_fencing_token INTEGER,
  daemon_fencing_epoch INTEGER,
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
  claim_daemon_epoch INTEGER,
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
  -- Adapter trust lifecycle (specs/adapter-registry.md).
  trust_state TEXT NOT NULL CHECK (trust_state IN ('trusted','untrusted')),
  -- Conformance lifecycle (specs/adapter-registry.md, ADP-006): reports bind to exact identity.
  conformance_state TEXT NOT NULL CHECK (conformance_state IN ('untested','passed','failed')),
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY (adapter_id, version, bundle_digest)
);

CREATE TABLE IF NOT EXISTS config_generations (
  generation_id TEXT PRIMARY KEY,
  digest TEXT NOT NULL UNIQUE,
  document BLOB NOT NULL,
  -- Config generation pipeline (specs/config-engine.md): proposed -> validated -> tested.
  validation_state TEXT NOT NULL CHECK (validation_state IN ('proposed','validated','rejected')),
  test_state TEXT NOT NULL CHECK (test_state IN ('untested','passed','failed')),
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
  -- Sensitivity (contracts/events/event.proto): 1=PUBLIC, 2=INTERNAL, 3=CONFIDENTIAL, 4=SECRET.
  -- 0=SENSITIVITY_UNSPECIFIED is never persisted.
  sensitivity INTEGER NOT NULL CHECK (sensitivity BETWEEN 1 AND 4),
  -- RetentionClass (contracts/events/event.proto): 1=EPHEMERAL, 2=STANDARD, 3=AUDIT.
  -- 0=RETENTION_UNSPECIFIED is never persisted.
  retention INTEGER NOT NULL CHECK (retention BETWEEN 1 AND 3),
  payload BLOB NOT NULL,
  journal_published_at_ms INTEGER,
  live_published_at_ms INTEGER,
  UNIQUE (stream_key, sequence)
);
CREATE INDEX IF NOT EXISTS idx_outbox_unpublished
  ON outbox_events(journal_published_at_ms, stream_key, sequence);

-- ---------------------------------------------------------------------------
-- GC-3 new durable records (design.md Data model, R5.1)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS artifacts (
  artifact_id TEXT PRIMARY KEY,
  uri TEXT NOT NULL UNIQUE,
  digest TEXT NOT NULL,
  media_type TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  origin_run_id TEXT NOT NULL,
  origin_effect_id TEXT,
  -- Sensitivity (contracts/events/event.proto): 1=PUBLIC, 2=INTERNAL, 3=CONFIDENTIAL, 4=SECRET.
  sensitivity INTEGER NOT NULL CHECK (sensitivity BETWEEN 1 AND 4),
  -- RetentionClass (contracts/events/event.proto): 1=EPHEMERAL, 2=STANDARD, 3=AUDIT.
  retention INTEGER NOT NULL CHECK (retention BETWEEN 1 AND 3),
  locator TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  FOREIGN KEY (origin_run_id) REFERENCES runs(run_id),
  FOREIGN KEY (origin_effect_id) REFERENCES effects(effect_id)
);

CREATE TABLE IF NOT EXISTS workspaces (
  workspace_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  base_revision TEXT,
  parent_workspace_id TEXT,
  created_at_ms INTEGER NOT NULL,
  FOREIGN KEY (parent_workspace_id) REFERENCES workspaces(workspace_id)
);

CREATE TABLE IF NOT EXISTS loop_turns (
  turn_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  run_revision INTEGER NOT NULL,
  loop_epoch INTEGER NOT NULL,
  step_sequence INTEGER NOT NULL,
  input_event_cursor TEXT NOT NULL,
  -- Turn lifecycle (specs/runtime-manager.md): issued -> accepted; stale = fenced-out response.
  state TEXT NOT NULL CHECK (state IN ('issued','accepted','stale')),
  issued_at_ms INTEGER NOT NULL,
  UNIQUE (run_id, turn_id),
  FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

CREATE TABLE IF NOT EXISTS decisions (
  decision_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  turn_id TEXT NOT NULL,
  -- Persisted literals are the canonical LoopDecision message names
  -- (contracts/protocols/agent_loop.proto; specs/command-catalog.md:94-101).
  decision_type TEXT NOT NULL CHECK (decision_type IN ('Complete','Fail','Wait','SpawnAgent','InvokeEffect','RequestApproval')),
  decision_digest TEXT NOT NULL,
  decision_bytes BLOB NOT NULL,
  run_revision INTEGER NOT NULL,
  loop_epoch INTEGER NOT NULL,
  step_sequence INTEGER NOT NULL,
  input_event_cursor TEXT NOT NULL,
  accepted_at_ms INTEGER NOT NULL,
  UNIQUE (run_id, decision_id),
  FOREIGN KEY (run_id) REFERENCES runs(run_id),
  FOREIGN KEY (turn_id) REFERENCES loop_turns(turn_id)
);

CREATE TABLE IF NOT EXISTS adapter_instances (
  adapter_instance_id TEXT PRIMARY KEY,
  adapter_id TEXT NOT NULL,
  adapter_version TEXT NOT NULL,
  bundle_digest TEXT NOT NULL,
  daemon_instance_id TEXT NOT NULL,
  pid INTEGER,
  process_start_identity TEXT,
  -- Instance lifecycle (specs/process-supervisor.md): starting -> ready -> exited|failed.
  state TEXT NOT NULL CHECK (state IN ('starting','ready','exited','failed')),
  exit_reason TEXT,
  last_heartbeat_ms INTEGER,
  started_at_ms INTEGER NOT NULL,
  ended_at_ms INTEGER,
  FOREIGN KEY (adapter_id, adapter_version, bundle_digest)
    REFERENCES adapter_registrations(adapter_id, version, bundle_digest)
);

CREATE TABLE IF NOT EXISTS conformance_reports (
  adapter_id TEXT NOT NULL,
  adapter_version TEXT NOT NULL,
  bundle_digest TEXT NOT NULL,
  report_digest TEXT NOT NULL,
  harness_version TEXT NOT NULL,
  -- Conformance outcome (specs/adapter-registry.md, ADP-006).
  result TEXT NOT NULL CHECK (result IN ('pass','fail')),
  run_at_ms INTEGER NOT NULL,
  details BLOB,
  PRIMARY KEY (adapter_id, adapter_version, bundle_digest),
  FOREIGN KEY (adapter_id, adapter_version, bundle_digest)
    REFERENCES adapter_registrations(adapter_id, version, bundle_digest)
);

-- ---------------------------------------------------------------------------
-- Immutability triggers (design.md Data model, R5.4)
-- ---------------------------------------------------------------------------

CREATE TRIGGER IF NOT EXISTS trg_resolved_run_environments_update
BEFORE UPDATE ON resolved_run_environments
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_resolved_run_environments_delete
BEFORE DELETE ON resolved_run_environments
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_resolved_bindings_update
BEFORE UPDATE ON resolved_bindings
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_resolved_bindings_delete
BEFORE DELETE ON resolved_bindings
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_agent_specs_update
BEFORE UPDATE ON agent_specs
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_agent_specs_delete
BEFORE DELETE ON agent_specs
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_approval_requests_update
BEFORE UPDATE ON approval_requests
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_approval_requests_delete
BEFORE DELETE ON approval_requests
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_conformance_reports_update
BEFORE UPDATE ON conformance_reports
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

CREATE TRIGGER IF NOT EXISTS trg_conformance_reports_delete
BEFORE DELETE ON conformance_reports
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;

-- Outbox rows are immutable except publication metadata
-- (journal_published_at_ms, live_published_at_ms; architecture/persistence.md:37-44).
CREATE TRIGGER IF NOT EXISTS trg_outbox_events_publication_only
BEFORE UPDATE ON outbox_events
WHEN OLD.event_id IS NOT NEW.event_id
  OR OLD.event_type IS NOT NEW.event_type
  OR OLD.event_version IS NOT NEW.event_version
  OR OLD.stream_key IS NOT NEW.stream_key
  OR OLD.sequence IS NOT NEW.sequence
  OR OLD.occurred_at_ms IS NOT NEW.occurred_at_ms
  OR OLD.run_id IS NOT NEW.run_id
  OR OLD.task_id IS NOT NEW.task_id
  OR OLD.session_id IS NOT NEW.session_id
  OR OLD.effect_id IS NOT NEW.effect_id
  OR OLD.causation_id IS NOT NEW.causation_id
  OR OLD.correlation_id IS NOT NEW.correlation_id
  OR OLD.sensitivity IS NOT NEW.sensitivity
  OR OLD.retention IS NOT NEW.retention
  OR OLD.payload IS NOT NEW.payload
BEGIN
  SELECT RAISE(ABORT, 'immutable record');
END;
