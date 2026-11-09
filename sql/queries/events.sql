-- name: InsertEvent :one
INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
VALUES (@account_id, @kind, @procedure_id, @ref, @occurred_at, @detected_at, @payload)
ON CONFLICT (account_id, kind, ref) DO NOTHING
RETURNING id;

-- name: ListRecentEvents :many
SELECT * FROM events ORDER BY id DESC LIMIT @row_limit::bigint;

-- name: ListEvents :many
SELECT * FROM events
WHERE (@account_id::bigint = 0 OR account_id = @account_id)
  AND (COALESCE(cardinality(@kinds::text[]), 0) = 0 OR kind = ANY(@kinds::text[]))
  AND (@procedure_id::bigint = 0 OR procedure_id = @procedure_id)
  AND (sqlc.narg(since)::timestamptz IS NULL OR occurred_at >= sqlc.narg(since))
  AND (sqlc.narg(until)::timestamptz IS NULL OR occurred_at < sqlc.narg(until))
  AND (sqlc.narg(after_occurred_at)::timestamptz IS NULL
       OR occurred_at < sqlc.narg(after_occurred_at)
       OR (occurred_at = sqlc.narg(after_occurred_at) AND id < @after_id))
ORDER BY occurred_at DESC, id DESC
LIMIT @row_limit::bigint;

-- name: PruneEventsBefore :execrows
DELETE FROM events WHERE account_id = @account_id AND detected_at < @cutoff;
