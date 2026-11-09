package monitor

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/db"
	"github.com/ItzTas/sei-monitor/internal/event"
)

var ErrAccountNotFound = errors.New("monitor: account not found")

type Repository interface {
	EnsureAccount(ctx context.Context, key core.AccountKey, now time.Time) (core.Account, error)
	Account(ctx context.Context, id int64) (core.Account, error)
	SetPausedReason(ctx context.Context, accountID int64, reason string) error
	UpdateAccountInfo(ctx context.Context, accountID int64, info core.AccountInfo) error
	Processes(ctx context.Context, accountID int64) ([]core.Process, error)
	DocumentIDs(ctx context.Context, accountID, procedureID int64) (map[int64]bool, error)
	ApplyCycle(ctx context.Context, changes CycleChanges) ([]event.Event, error)
	StartCycleRun(ctx context.Context, accountID int64, trigger string, startedAt time.Time) (int64, error)
	FinishCycleRun(ctx context.Context, run core.CycleRun) error
	LastCycleRun(ctx context.Context, accountID int64) (core.CycleRun, bool, error)
	PruneBefore(ctx context.Context, accountID int64, cutoff time.Time) (Pruned, error)
}

type Postgres struct {
	pool    *pgxpool.Pool
	queries *db.Queries
}

func NewPostgres(pool *pgxpool.Pool) *Postgres {
	return &Postgres{pool: pool, queries: db.New(pool)}
}

func (p *Postgres) EnsureAccount(ctx context.Context, key core.AccountKey, now time.Time) (core.Account, error) {
	row, err := p.queries.EnsureAccount(ctx, db.EnsureAccountParams{
		SeiBaseUrl: key.SEIBaseURL, Username: key.Username, OrgID: key.OrgID, CreatedAt: now,
	})
	if err != nil {
		return core.Account{}, fmt.Errorf("monitor: save the account: %w", err)
	}
	return core.AccountFromRow(row), nil
}

func (p *Postgres) Account(ctx context.Context, id int64) (core.Account, error) {
	row, err := p.queries.GetAccount(ctx, id)
	if errors.Is(err, pgx.ErrNoRows) {
		return core.Account{}, core.ErrNotFound
	}
	if err != nil {
		return core.Account{}, fmt.Errorf("monitor: read the account: %w", err)
	}
	return core.AccountFromRow(row), nil
}

func (p *Postgres) SetPausedReason(ctx context.Context, accountID int64, reason string) error {
	rows, err := p.queries.SetPausedReason(ctx, db.SetPausedReasonParams{PausedReason: reason, ID: accountID})
	if err != nil {
		return fmt.Errorf("monitor: update the account: %w", err)
	}
	if rows == 0 {
		return ErrAccountNotFound
	}
	return nil
}

func (p *Postgres) UpdateAccountInfo(ctx context.Context, accountID int64, info core.AccountInfo) error {
	err := p.queries.UpdateAccountInfo(ctx, db.UpdateAccountInfoParams{
		OrgAcronym: info.OrgAcronym, SeiVersion: info.SEIVersion, WsseiVersion: info.WSSEIVersion, ID: accountID,
	})
	if err != nil {
		return fmt.Errorf("monitor: update the account details: %w", err)
	}
	return nil
}

func (p *Postgres) Processes(ctx context.Context, accountID int64) ([]core.Process, error) {
	rows, err := p.queries.ListAccountProcesses(ctx, accountID)
	if err != nil {
		return nil, fmt.Errorf("monitor: read the processes: %w", err)
	}
	return core.Map(rows, core.ProcessFromRow), nil
}

func (p *Postgres) DocumentIDs(ctx context.Context, accountID, procedureID int64) (map[int64]bool, error) {
	rows, err := p.queries.ListDocumentIDs(ctx, db.ListDocumentIDsParams{AccountID: accountID, ProcedureID: procedureID})
	if err != nil {
		return nil, fmt.Errorf("monitor: read the documents: %w", err)
	}
	ids := make(map[int64]bool, len(rows))
	for _, id := range rows {
		ids[id] = true
	}
	return ids, nil
}

func (p *Postgres) ApplyCycle(ctx context.Context, changes CycleChanges) ([]event.Event, error) {
	var inserted []event.Event
	err := core.InTx(ctx, p.pool, func(tx pgx.Tx) error {
		queries := p.queries.WithTx(tx)
		steps := []func(context.Context, *db.Queries, CycleChanges) error{upsertProcesses, markLeft, insertDocuments, markBaseline}
		for _, step := range steps {
			if err := step(ctx, queries, changes); err != nil {
				return err
			}
		}
		events, err := insertEvents(ctx, queries, changes)
		if err != nil {
			return err
		}
		inserted = events
		if changes.RunID == 0 {
			return nil
		}
		return finishRun(ctx, queries, core.CycleRun{
			ID: changes.RunID, AccountID: changes.AccountID, FinishedAt: changes.FinishedAt,
			ProcessesSeen: len(changes.Processes), EventsCreated: len(events),
		})
	})
	if err != nil {
		return nil, err
	}
	return inserted, nil
}

func upsertProcesses(ctx context.Context, queries *db.Queries, changes CycleChanges) error {
	for _, process := range changes.Processes {
		firstSeen := process.FirstSeenAt
		if firstSeen.IsZero() {
			firstSeen = changes.Now
		}
		err := queries.UpsertProcess(ctx, db.UpsertProcessParams{
			AccountID: changes.AccountID, ProcedureID: process.ProcedureID, Number: process.Number,
			ProcessType: process.Type, Description: process.Description, UnitID: process.UnitID,
			UnitAcronym: process.UnitAcronym, AssignedUser: process.AssignedUser, AssignedUserID: process.AssignedUserID,
			ReturnDate: process.ReturnDate, ReturnOverdue: process.ReturnOverdue, LastActivityID: process.LastActivityID,
			DocumentTotal: int64(process.DocumentTotal), FirstSeenAt: firstSeen, LastSeenAt: changes.Now,
		})
		if err != nil {
			return fmt.Errorf("monitor: save process %d: %w", process.ProcedureID, err)
		}
	}
	return nil
}

func markLeft(ctx context.Context, queries *db.Queries, changes CycleChanges) error {
	for _, procedureID := range changes.Left {
		err := queries.MarkProcessLeft(ctx, db.MarkProcessLeftParams{
			LeftAt: core.OptionalTime(changes.Now), AccountID: changes.AccountID, ProcedureID: procedureID,
		})
		if err != nil {
			return fmt.Errorf("monitor: mark process %d as gone: %w", procedureID, err)
		}
	}
	return nil
}

func insertDocuments(ctx context.Context, queries *db.Queries, changes CycleChanges) error {
	for _, document := range changes.Documents {
		err := queries.InsertDocument(ctx, db.InsertDocumentParams{
			AccountID: changes.AccountID, DocumentID: document.DocumentID, ProcedureID: document.ProcedureID,
			FormattedProtocol: document.Protocol, DocumentType: document.Type, Name: document.Name, FirstSeenAt: changes.Now,
		})
		if err != nil {
			return fmt.Errorf("monitor: save document %d: %w", document.DocumentID, err)
		}
	}
	return nil
}

func markBaseline(ctx context.Context, queries *db.Queries, changes CycleChanges) error {
	if !changes.Baseline {
		return nil
	}
	err := queries.MarkBaseline(ctx, db.MarkBaselineParams{BaselineAt: core.OptionalTime(changes.Now), ID: changes.AccountID})
	if err != nil {
		return fmt.Errorf("monitor: mark the baseline: %w", err)
	}
	return nil
}

func insertEvents(ctx context.Context, queries *db.Queries, changes CycleChanges) ([]event.Event, error) {
	var inserted []event.Event
	for _, ev := range changes.Events {
		saved, err := insertEvent(ctx, queries, changes, ev)
		if errors.Is(err, pgx.ErrNoRows) {
			continue
		}
		if err != nil {
			return nil, err
		}
		inserted = append(inserted, saved)
	}
	return inserted, nil
}

func insertEvent(ctx context.Context, queries *db.Queries, changes CycleChanges, ev event.Event) (event.Event, error) {
	ev.AccountID = changes.AccountID
	if ev.DetectedAt.IsZero() {
		ev.DetectedAt = changes.Now
	}
	if ev.OccurredAt.IsZero() {
		ev.OccurredAt = ev.DetectedAt
	}
	payload, err := json.Marshal(ev.Payload)
	if err != nil {
		return event.Event{}, fmt.Errorf("monitor: encode the event payload: %w", err)
	}
	ev.ID, err = queries.InsertEvent(ctx, db.InsertEventParams{
		AccountID: ev.AccountID, Kind: string(ev.Kind), ProcedureID: ev.ProcedureID, Ref: ev.Ref,
		OccurredAt: ev.OccurredAt, DetectedAt: ev.DetectedAt, Payload: string(payload),
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return event.Event{}, err
	}
	if err != nil {
		return event.Event{}, fmt.Errorf("monitor: save a %s event: %w", ev.Kind, err)
	}
	for _, channel := range changes.Channels {
		err := queries.InsertDelivery(ctx, db.InsertDeliveryParams{EventID: ev.ID, Channel: channel, NextAttemptAt: changes.Now})
		if err != nil {
			return event.Event{}, fmt.Errorf("monitor: queue the %s delivery: %w", channel, err)
		}
	}
	return ev, nil
}

func (p *Postgres) StartCycleRun(ctx context.Context, accountID int64, trigger string, startedAt time.Time) (int64, error) {
	id, err := p.queries.StartCycleRun(ctx, db.StartCycleRunParams{AccountID: accountID, Trigger: trigger, StartedAt: startedAt})
	if err != nil {
		return 0, fmt.Errorf("monitor: record the cycle start: %w", err)
	}
	return id, nil
}

func (p *Postgres) FinishCycleRun(ctx context.Context, run core.CycleRun) error {
	return core.InTx(ctx, p.pool, func(tx pgx.Tx) error {
		return finishRun(ctx, p.queries.WithTx(tx), run)
	})
}

func finishRun(ctx context.Context, queries *db.Queries, run core.CycleRun) error {
	err := queries.FinishCycleRun(ctx, db.FinishCycleRunParams{
		FinishedAt: core.OptionalTime(run.FinishedAt), Error: run.Error,
		ProcessesSeen: int64(run.ProcessesSeen), EventsCreated: int64(run.EventsCreated),
		ID: run.ID, AccountID: run.AccountID,
	})
	if err != nil {
		return fmt.Errorf("monitor: record the cycle end: %w", err)
	}
	if err := queries.PruneCycleRuns(ctx, db.PruneCycleRunsParams{AccountID: run.AccountID, KeptRuns: keptRunsPerAccount}); err != nil {
		return fmt.Errorf("monitor: prune old cycles: %w", err)
	}
	return nil
}

func (p *Postgres) LastCycleRun(ctx context.Context, accountID int64) (core.CycleRun, bool, error) {
	row, err := p.queries.GetLastCycleRun(ctx, accountID)
	if errors.Is(err, pgx.ErrNoRows) {
		return core.CycleRun{}, false, nil
	}
	if err != nil {
		return core.CycleRun{}, false, fmt.Errorf("monitor: read the last cycle: %w", err)
	}
	return core.CycleRunFromRow(row), true, nil
}

func (p *Postgres) PruneBefore(ctx context.Context, accountID int64, cutoff time.Time) (Pruned, error) {
	var pruned Pruned
	err := core.InTx(ctx, p.pool, func(tx pgx.Tx) error {
		queries := p.queries.WithTx(tx)
		var err error
		pruned.Events, err = queries.PruneEventsBefore(ctx, db.PruneEventsBeforeParams{AccountID: accountID, Cutoff: cutoff})
		if err != nil {
			return fmt.Errorf("monitor: prune old events: %w", err)
		}
		pruned.Processes, err = queries.PruneProcessesLeftBefore(ctx, db.PruneProcessesLeftBeforeParams{AccountID: accountID, Cutoff: &cutoff})
		if err != nil {
			return fmt.Errorf("monitor: prune processes that left: %w", err)
		}
		return nil
	})
	if err != nil {
		return Pruned{}, err
	}
	return pruned, nil
}
