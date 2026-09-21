# Task Report: API-003

- **Spec**: event API (specs/events.md)
- **Task**: API-003 — durable ReadStream + gap-free Subscribe
- **Status**: DONE
- **Commits**: see git log for `devin/1789944697-support-wave`
- **Branch**: devin/1789944697-support-wave

## What was implemented

`EventApiService` (`event_service.rs`) implements `MvpEventApi` over
`EventJournalPort` + `LiveBus`:

- `ReadStream` reads directly from the journal (honors `retention_gap`).
- `Subscribe` attaches the broadcast receiver BEFORE replaying the
  journal, so events appended mid-replay are buffered; the handoff
  dedupes by `sequence <= last_sequence`, guaranteeing no gap and no
  duplicates between durable replay and live tail.
- On `LiveItem::Lagged` the subscription terminates with a `LagNotice`
  carrying `resume_sequence = last delivered sequence` — a cursor the
  journal can always serve, so a client never gets a durable cursor the
  journal cannot serve.
- `project()` strips `payload` for `SensitivityClass::Secret` events so
  secret material never leaves the API.

## Evidence

`cargo test -p control-api --test events_api` (3 tests):
- `read_stream_reads_journal_history` — journal pages served directly.
- `subscribe_replays_then_lives_without_gap` — durable event replayed,
  then live event follows in the same stream, ordered.
- `subscribe_lag_terminates_with_resume_cursor` — replay backlog (300)
  parks the forwarder mid-replay; a live burst overflows the cap-1 bus;
  the terminal LagNotice carries the last delivered sequence.
