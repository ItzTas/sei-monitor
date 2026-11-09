-- name: InsertDelivery :exec
INSERT INTO deliveries (event_id, channel, status, attempts, next_attempt_at)
VALUES (@event_id, @channel, 'pending', 0, @next_attempt_at);

-- name: ListDueDeliveries :many
SELECT sqlc.embed(e), d.channel, d.status, d.attempts, d.next_attempt_at, d.last_error
FROM deliveries d JOIN events e ON e.id = d.event_id
WHERE d.channel = @channel AND d.status = 'pending' AND d.next_attempt_at <= @due_at
ORDER BY e.occurred_at, e.id
LIMIT @row_limit::bigint;

-- name: UpdateDelivery :exec
UPDATE deliveries
SET status = @status, attempts = @attempts, next_attempt_at = @next_attempt_at, last_error = @last_error, sent_at = @sent_at
WHERE event_id = @event_id AND channel = @channel;

-- name: ListDeliveries :many
SELECT d.* FROM deliveries d JOIN events e ON e.id = d.event_id
WHERE (@account_id::bigint = 0 OR e.account_id = @account_id)
  AND (@status::text = '' OR d.status = @status)
  AND (@channel::text = '' OR d.channel = @channel)
  AND (sqlc.narg(after_event_id)::bigint IS NULL
       OR d.event_id < sqlc.narg(after_event_id)
       OR (d.event_id = sqlc.narg(after_event_id) AND d.channel > @after_channel::text))
ORDER BY d.event_id DESC, d.channel ASC
LIMIT @row_limit::bigint;

-- name: CountDeliveries :many
SELECT d.channel, d.status, COUNT(*) AS total
FROM deliveries d JOIN events e ON e.id = d.event_id
WHERE e.account_id = @account_id
GROUP BY d.channel, d.status;
