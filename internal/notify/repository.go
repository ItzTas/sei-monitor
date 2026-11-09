package notify

import (
	"context"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/db"
)

type Postgres struct {
	pool    *pgxpool.Pool
	queries *db.Queries
}

func NewPostgres(pool *pgxpool.Pool) *Postgres {
	return &Postgres{pool: pool, queries: db.New(pool)}
}

func (p *Postgres) DueDeliveries(ctx context.Context, channel string, now time.Time, limit int) ([]core.DueDelivery, error) {
	rows, err := p.queries.ListDueDeliveries(ctx, db.ListDueDeliveriesParams{Channel: channel, DueAt: now, RowLimit: int64(limit)})
	if err != nil {
		return nil, fmt.Errorf("notify: read the due deliveries: %w", err)
	}
	due := make([]core.DueDelivery, 0, len(rows))
	for _, row := range rows {
		ev, err := core.EventFromRow(row.Event)
		if err != nil {
			return nil, fmt.Errorf("notify: %w", err)
		}
		due = append(due, core.DueDelivery{
			Event:   ev,
			EventID: ev.ID, Channel: row.Channel, Status: row.Status, Attempts: int(row.Attempts),
			NextAttemptAt: row.NextAttemptAt.UTC(), LastError: row.LastError,
		})
	}
	return due, nil
}

func (p *Postgres) UpdateDeliveries(ctx context.Context, deliveries []core.Delivery) error {
	return core.InTx(ctx, p.pool, func(tx pgx.Tx) error {
		queries := p.queries.WithTx(tx)
		for _, d := range deliveries {
			err := queries.UpdateDelivery(ctx, db.UpdateDeliveryParams{
				Status: d.Status, Attempts: int64(d.Attempts), NextAttemptAt: d.NextAttemptAt, LastError: d.LastError,
				SentAt: core.OptionalTime(d.SentAt), EventID: d.EventID, Channel: d.Channel,
			})
			if err != nil {
				return fmt.Errorf("notify: update the %s delivery of event %d: %w", d.Channel, d.EventID, err)
			}
		}
		return nil
	})
}
