package processes

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/db"
	"github.com/ItzTas/sei-monitor/internal/event"
)

var ErrNotFound = errors.New("process not found")

type Filter struct {
	AccountID       int64
	Unit            string
	AssignedUserID  int64
	AssignedAcronym string
	ReturnScheduled bool
	ReturnOverdue   bool
	IncludeLeft     bool
	Limit           int
	After           *Key
}

type Key struct {
	LastSeenAt  time.Time
	ProcedureID int64
}

type Repository interface {
	ListProcesses(ctx context.Context, filter Filter) ([]core.Process, error)
	ProcessByID(ctx context.Context, accountID, procedureID int64) (core.Process, error)
	Documents(ctx context.Context, accountID, procedureID int64) ([]core.Document, error)
	ProcessEvents(ctx context.Context, accountID, procedureID int64, limit int) ([]event.Event, error)
}

type Postgres struct {
	queries *db.Queries
}

func NewPostgres(pool *pgxpool.Pool) *Postgres {
	return &Postgres{queries: db.New(pool)}
}

func (p *Postgres) ListProcesses(ctx context.Context, filter Filter) ([]core.Process, error) {
	params := db.ListProcessesParams{
		AccountID:           filter.AccountID,
		OnlyAssigned:        filter.AssignedUserID != 0 || filter.AssignedAcronym != "",
		AssignedUserID:      filter.AssignedUserID,
		AssignedAcronym:     filter.AssignedAcronym,
		OnlyReturnScheduled: filter.ReturnScheduled,
		OnlyReturnOverdue:   filter.ReturnOverdue,
		IncludeLeft:         filter.IncludeLeft,
		RowLimit:            int64(filter.Limit),
	}
	if filter.Unit != "" {
		params.Unit = &filter.Unit
	}
	if after := filter.After; after != nil {
		params.AfterLastSeenAt, params.AfterProcedureID = &after.LastSeenAt, after.ProcedureID
	}
	rows, err := p.queries.ListProcesses(ctx, params)
	if err != nil {
		return nil, fmt.Errorf("processes: read the processes: %w", err)
	}
	return core.Map(rows, core.ProcessFromRow), nil
}

func (p *Postgres) ProcessByID(ctx context.Context, accountID, procedureID int64) (core.Process, error) {
	row, err := p.queries.GetProcess(ctx, db.GetProcessParams{AccountID: accountID, ProcedureID: procedureID})
	if errors.Is(err, pgx.ErrNoRows) {
		return core.Process{}, ErrNotFound
	}
	if err != nil {
		return core.Process{}, fmt.Errorf("processes: read the process: %w", err)
	}
	return core.ProcessFromRow(row), nil
}

func (p *Postgres) Documents(ctx context.Context, accountID, procedureID int64) ([]core.Document, error) {
	rows, err := p.queries.ListDocuments(ctx, db.ListDocumentsParams{AccountID: accountID, ProcedureID: procedureID})
	if err != nil {
		return nil, fmt.Errorf("processes: read the documents: %w", err)
	}
	return core.Map(rows, core.DocumentFromRow), nil
}

func (p *Postgres) ProcessEvents(ctx context.Context, accountID, procedureID int64, limit int) ([]event.Event, error) {
	rows, err := p.queries.ListEvents(ctx, db.ListEventsParams{
		AccountID: accountID, Kinds: []string{}, ProcedureID: procedureID, RowLimit: int64(limit),
	})
	if err != nil {
		return nil, fmt.Errorf("processes: read the events: %w", err)
	}
	return core.EventsFromRows(rows)
}
