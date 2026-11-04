package store

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
)

func (s *Store) RecentEvents(ctx context.Context, limit int) ([]event.Event, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT id, account_id, kind, procedure_id, ref, occurred_at, detected_at, payload
		FROM events
		ORDER BY id DESC
		LIMIT ?`, limit)
	if err != nil {
		return nil, fmt.Errorf("store: read the events: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var events []event.Event
	for rows.Next() {
		var ev event.Event
		var kind, payload string
		var occurred, detected dbTime
		if err := rows.Scan(&ev.ID, &ev.AccountID, &kind, &ev.ProcedureID, &ev.Ref, &occurred, &detected, &payload); err != nil {
			return nil, fmt.Errorf("store: read an event: %w", err)
		}
		if err := json.Unmarshal([]byte(payload), &ev.Payload); err != nil {
			return nil, fmt.Errorf("store: decode the payload of event %d: %w", ev.ID, err)
		}
		ev.Kind, ev.OccurredAt, ev.DetectedAt = event.Kind(kind), time.Time(occurred), time.Time(detected)
		events = append(events, ev)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: read the events: %w", err)
	}
	return events, nil
}
