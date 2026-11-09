package events

import (
	"context"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/db"
	"github.com/ItzTas/sei-monitor/internal/event"
)

type Filter struct {
	AccountID   int64
	Kinds       []event.Kind
	ProcedureID int64
	Since       time.Time
	Until       time.Time
	Limit       int
	After       *Key
}

type Key struct {
	OccurredAt time.Time
	ID         int64
}

type Repository interface {
	ListEvents(ctx context.Context, filter Filter) ([]event.Event, error)
}

type Postgres struct {
	queries *db.Queries
}

func NewPostgres(pool *pgxpool.Pool) *Postgres {
	return &Postgres{queries: db.New(pool)}
}

func (p *Postgres) ListEvents(ctx context.Context, filter Filter) ([]event.Event, error) {
	params := db.ListEventsParams{
		AccountID:   filter.AccountID,
		Kinds:       make([]string, 0, len(filter.Kinds)),
		ProcedureID: filter.ProcedureID,
		Since:       core.OptionalTime(filter.Since),
		Until:       core.OptionalTime(filter.Until),
		RowLimit:    int64(filter.Limit),
	}
	for _, kind := range filter.Kinds {
		params.Kinds = append(params.Kinds, string(kind))
	}
	if after := filter.After; after != nil {
		params.AfterOccurredAt, params.AfterID = &after.OccurredAt, after.ID
	}
	rows, err := p.queries.ListEvents(ctx, params)
	if err != nil {
		return nil, fmt.Errorf("events: read the events: %w", err)
	}
	return core.EventsFromRows(rows)
}
