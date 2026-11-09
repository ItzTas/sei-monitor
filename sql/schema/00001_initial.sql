-- +goose Up
CREATE TABLE accounts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    sei_base_url TEXT NOT NULL,
    username TEXT NOT NULL,
    org_id BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    baseline_at TIMESTAMPTZ,
    paused_reason TEXT NOT NULL DEFAULT '',
    UNIQUE (sei_base_url, username, org_id)
);

CREATE TABLE processes (
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    procedure_id BIGINT NOT NULL,
    number TEXT NOT NULL,
    process_type TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    unit_id BIGINT NOT NULL,
    unit_acronym TEXT NOT NULL DEFAULT '',
    assigned_user TEXT NOT NULL DEFAULT '',
    assigned_user_id BIGINT NOT NULL DEFAULT 0,
    return_date TEXT NOT NULL DEFAULT '',
    return_overdue BOOLEAN NOT NULL DEFAULT FALSE,
    last_activity_id BIGINT NOT NULL DEFAULT 0,
    document_total BIGINT NOT NULL DEFAULT 0,
    first_seen_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    left_at TIMESTAMPTZ,
    PRIMARY KEY (account_id, procedure_id)
);

CREATE TABLE documents (
    account_id BIGINT NOT NULL,
    document_id BIGINT NOT NULL,
    procedure_id BIGINT NOT NULL,
    formatted_protocol TEXT NOT NULL DEFAULT '',
    document_type TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    first_seen_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (account_id, document_id),
    FOREIGN KEY (account_id, procedure_id) REFERENCES processes (account_id, procedure_id) ON DELETE CASCADE
);

CREATE INDEX documents_procedure ON documents (account_id, procedure_id);

CREATE TABLE events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    procedure_id BIGINT NOT NULL DEFAULT 0,
    ref TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    detected_at TIMESTAMPTZ NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE (account_id, kind, ref)
);

CREATE TABLE deliveries (
    event_id BIGINT NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    channel TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'failed')),
    attempts BIGINT NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL,
    last_error TEXT NOT NULL DEFAULT '',
    sent_at TIMESTAMPTZ,
    PRIMARY KEY (event_id, channel)
);

CREATE INDEX deliveries_due ON deliveries (channel, status, next_attempt_at);

-- +goose Down
DROP TABLE deliveries;
DROP TABLE events;
DROP TABLE documents;
DROP TABLE processes;
DROP TABLE accounts;
