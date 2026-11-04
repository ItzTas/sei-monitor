-- +goose Up
ALTER TABLE accounts ADD COLUMN org_acronym TEXT NOT NULL DEFAULT '';
ALTER TABLE accounts ADD COLUMN sei_version TEXT NOT NULL DEFAULT '';
ALTER TABLE accounts ADD COLUMN wssei_version TEXT NOT NULL DEFAULT '';

CREATE TABLE cycle_runs (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    "trigger" TEXT NOT NULL CHECK ("trigger" IN ('scheduled', 'manual')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    error TEXT NOT NULL DEFAULT '',
    processes_seen INTEGER NOT NULL DEFAULT 0,
    events_created INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX cycle_runs_account ON cycle_runs (account_id, id);

-- +goose Down
DROP TABLE cycle_runs;
ALTER TABLE accounts DROP COLUMN wssei_version;
ALTER TABLE accounts DROP COLUMN sei_version;
ALTER TABLE accounts DROP COLUMN org_acronym;
