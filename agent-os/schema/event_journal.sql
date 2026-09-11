-- Event Journal inception schema for events.db.
-- Extracted verbatim from specs/event-pipeline.md ("Event Journal SQLite schema", C8).
-- events.db is a downstream durable projection; kernel.db remains the single
-- authority for current execution correctness (architecture/persistence.md).
-- This file is self-contained: execute it when events.db is created. It shares
-- no tables with kernel.db (specs/kernel-store-schema.sql).

CREATE TABLE events (
  event_id TEXT PRIMARY KEY,
  stream_key TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event_version INTEGER NOT NULL,
  occurred_at_ms INTEGER NOT NULL,
  envelope BLOB NOT NULL,
  UNIQUE(stream_key, sequence)
);
CREATE INDEX idx_events_stream ON events(stream_key, sequence);
