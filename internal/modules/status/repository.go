package status

import (
	"context"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/db"
)

type Repository interface {
	Account(ctx context.Context, id int64) (core.Account, error)
	LastCycleRun(ctx context.Context, accountID int64) (core.CycleRun, bool, error)
	DeliveryCounts(ctx context.Context, accountID int64) (map[string]map[string]int, error)
}

type Postgres struct {
	queries *db.Queries
}

func NewPostgres(pool *pgxpool.Pool) *Postgres {
	return &Postgres{queries: db.New(pool)}
}

func (p *Postgres) Account(ctx context.Context, id int64) (core.Account, error) {
	row, err := p.queries.GetAccount(ctx, id)
	if errors.Is(err, pgx.ErrNoRows) {
		return core.Account{}, core.ErrNotFound
	}
	if err != nil {
		return core.Account{}, fmt.Errorf("status: read the account: %w", err)
	}
	return core.AccountFromRow(row), nil
}

func (p *Postgres) LastCycleRun(ctx context.Context, accountID int64) (core.CycleRun, bool, error) {
	row, err := p.queries.GetLastCycleRun(ctx, accountID)
	if errors.Is(err, pgx.ErrNoRows) {
		return core.CycleRun{}, false, nil
	}
	if err != nil {
		return core.CycleRun{}, false, fmt.Errorf("status: read the last cycle: %w", err)
	}
	return core.CycleRunFromRow(row), true, nil
}

func (p *Postgres) DeliveryCounts(ctx context.Context, accountID int64) (map[string]map[string]int, error) {
	rows, err := p.queries.CountDeliveries(ctx, accountID)
	if err != nil {
		return nil, fmt.Errorf("status: count the deliveries: %w", err)
	}
	counts := map[string]map[string]int{}
	for _, row := range rows {
		if counts[row.Channel] == nil {
			counts[row.Channel] = map[string]int{}
		}
		counts[row.Channel][row.Status] = int(row.Total)
	}
	return counts, nil
}
