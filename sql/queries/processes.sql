-- name: ListAccountProcesses :many
SELECT * FROM processes WHERE account_id = @account_id ORDER BY procedure_id;

-- name: GetProcess :one
SELECT * FROM processes WHERE account_id = @account_id AND procedure_id = @procedure_id;

-- name: ListProcesses :many
SELECT * FROM processes
WHERE account_id = @account_id
  AND (sqlc.narg(unit)::text IS NULL OR LOWER(unit_acronym) = LOWER(sqlc.narg(unit)))
  AND (NOT @only_assigned::boolean
       OR assigned_user_id = @assigned_user_id
       OR (assigned_user_id = 0 AND assigned_user <> '' AND LOWER(assigned_user) = LOWER(@assigned_acronym::text)))
  AND (NOT @only_return_scheduled::boolean OR return_date <> '')
  AND (NOT @only_return_overdue::boolean OR return_overdue)
  AND (@include_left::boolean OR left_at IS NULL)
  AND (sqlc.narg(after_last_seen_at)::timestamptz IS NULL
       OR last_seen_at < sqlc.narg(after_last_seen_at)
       OR (last_seen_at = sqlc.narg(after_last_seen_at) AND procedure_id < @after_procedure_id))
ORDER BY last_seen_at DESC, procedure_id DESC
LIMIT @row_limit::bigint;

-- name: UpsertProcess :exec
INSERT INTO processes (account_id, procedure_id, number, process_type, description, unit_id, unit_acronym,
                       assigned_user, assigned_user_id, return_date, return_overdue, last_activity_id,
                       document_total, first_seen_at, last_seen_at)
VALUES (@account_id, @procedure_id, @number, @process_type, @description, @unit_id, @unit_acronym,
        @assigned_user, @assigned_user_id, @return_date, @return_overdue, @last_activity_id,
        @document_total, @first_seen_at, @last_seen_at)
ON CONFLICT (account_id, procedure_id) DO UPDATE SET
    number = excluded.number,
    process_type = excluded.process_type,
    description = excluded.description,
    unit_id = excluded.unit_id,
    unit_acronym = excluded.unit_acronym,
    assigned_user = excluded.assigned_user,
    assigned_user_id = excluded.assigned_user_id,
    return_date = excluded.return_date,
    return_overdue = excluded.return_overdue,
    last_activity_id = excluded.last_activity_id,
    document_total = excluded.document_total,
    last_seen_at = excluded.last_seen_at,
    left_at = NULL;

-- name: MarkProcessLeft :exec
UPDATE processes SET left_at = @left_at
WHERE account_id = @account_id AND procedure_id = @procedure_id AND left_at IS NULL;

-- name: PruneProcessesLeftBefore :execrows
DELETE FROM processes WHERE account_id = @account_id AND left_at < @cutoff;
