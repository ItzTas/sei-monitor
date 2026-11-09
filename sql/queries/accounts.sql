-- name: EnsureAccount :one
INSERT INTO accounts (sei_base_url, username, org_id, created_at)
VALUES (@sei_base_url, @username, @org_id, @created_at)
ON CONFLICT (sei_base_url, username, org_id) DO UPDATE SET username = excluded.username
RETURNING *;

-- name: GetAccount :one
SELECT * FROM accounts WHERE id = @id;

-- name: SetPausedReason :execrows
UPDATE accounts SET paused_reason = @paused_reason WHERE id = @id;

-- name: UpdateAccountInfo :exec
UPDATE accounts
SET org_acronym = @org_acronym, sei_version = @sei_version, wssei_version = @wssei_version
WHERE id = @id;

-- name: MarkBaseline :exec
UPDATE accounts SET baseline_at = @baseline_at WHERE id = @id AND baseline_at IS NULL;
