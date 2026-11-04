package store

import (
	"context"

	"github.com/ItzTas/sei-monitor/internal/event"
)

func (s *Store) RecentEvents(ctx context.Context, limit int) ([]event.Event, error) {
	return s.queryEvents(ctx, `SELECT id, account_id, kind, procedure_id, ref, occurred_at, detected_at, payload FROM events ORDER BY id DESC LIMIT ?`, limit)
}
