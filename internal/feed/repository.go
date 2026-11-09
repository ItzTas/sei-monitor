package feed

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/db"
	"github.com/ItzTas/sei-monitor/internal/event"
)

type Postgres struct {
	queries *db.Queries
}

func NewPostgres(pool *pgxpool.Pool) *Postgres {
	return &Postgres{queries: db.New(pool)}
}

func (p *Postgres) RecentEvents(ctx context.Context, limit int) ([]event.Event, error) {
	rows, err := p.queries.ListRecentEvents(ctx, int64(limit))
	if err != nil {
		return nil, fmt.Errorf("feed: read the events: %w", err)
	}
	return core.EventsFromRows(rows)
}
