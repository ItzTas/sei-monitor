-- name: StartCycleRun :one
INSERT INTO cycle_runs (account_id, "trigger", started_at)
VALUES (@account_id, @trigger, @started_at)
RETURNING id;

-- name: FinishCycleRun :exec
UPDATE cycle_runs
SET finished_at = @finished_at, error = @error, processes_seen = @processes_seen, events_created = @events_created
WHERE id = @id AND account_id = @account_id;

-- name: PruneCycleRuns :exec
DELETE FROM cycle_runs AS runs
WHERE runs.account_id = @account_id AND runs.id <= (
    SELECT kept.id FROM cycle_runs AS kept
    WHERE kept.account_id = @account_id
    ORDER BY kept.id DESC
    LIMIT 1 OFFSET @kept_runs::bigint
);

-- name: GetLastCycleRun :one
SELECT * FROM cycle_runs WHERE account_id = @account_id ORDER BY id DESC LIMIT 1;
