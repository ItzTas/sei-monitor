package deliveries

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/db"
)

type Filter struct {
	AccountID int64
	Status    string
	Channel   string
	Limit     int
	After     *Key
}

type Key struct {
	EventID int64
	Channel string
}

type Repository interface {
	ListDeliveries(ctx context.Context, filter Filter) ([]core.Delivery, error)
}

type Postgres struct {
	queries *db.Queries
}

func NewPostgres(pool *pgxpool.Pool) *Postgres {
	return &Postgres{queries: db.New(pool)}
}

func (p *Postgres) ListDeliveries(ctx context.Context, filter Filter) ([]core.Delivery, error) {
	params := db.ListDeliveriesParams{
		AccountID: filter.AccountID, Status: filter.Status, Channel: filter.Channel, RowLimit: int64(filter.Limit),
	}
	if after := filter.After; after != nil {
		params.AfterEventID, params.AfterChannel = &after.EventID, after.Channel
	}
	rows, err := p.queries.ListDeliveries(ctx, params)
	if err != nil {
		return nil, fmt.Errorf("deliveries: read the deliveries: %w", err)
	}
	return core.Map(rows, core.DeliveryFromRow), nil
}
