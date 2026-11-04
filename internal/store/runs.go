package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"time"
)

const (
	TriggerScheduled = "scheduled"
	TriggerManual    = "manual"

	keptRunsPerAccount = 1000
)

type CycleRun struct {
	ID            int64
	AccountID     int64
	Trigger       string
	StartedAt     time.Time
	FinishedAt    time.Time
	Error         string
	ProcessesSeen int
	EventsCreated int
}

func (s *Store) StartCycleRun(ctx context.Context, accountID int64, trigger string, startedAt time.Time) (int64, error) {
	var id int64
	err := s.db.QueryRowContext(ctx, `INSERT INTO cycle_runs (account_id, "trigger", started_at) VALUES (?, ?, ?) RETURNING id`,
		accountID, trigger, dbTime(startedAt)).Scan(&id)
	if err != nil {
		return 0, fmt.Errorf("store: record the cycle start: %w", err)
	}
	return id, nil
}

func (s *Store) FinishCycleRun(ctx context.Context, run CycleRun) error {
	return s.inTx(ctx, func(tx *sql.Tx) error {
		return finishRun(ctx, tx, run)
	})
}

func (s *Store) LastCycleRun(ctx context.Context, accountID int64) (CycleRun, bool, error) {
	run := CycleRun{AccountID: accountID}
	var started, finished dbTime
	err := s.db.QueryRowContext(ctx, `
		SELECT id, "trigger", started_at, finished_at, error, processes_seen, events_created
		FROM cycle_runs
		WHERE account_id = ?
		ORDER BY id DESC
		LIMIT 1`, accountID).Scan(&run.ID, &run.Trigger, &started, &finished, &run.Error, &run.ProcessesSeen, &run.EventsCreated)
	if errors.Is(err, sql.ErrNoRows) {
		return CycleRun{}, false, nil
	}
	if err != nil {
		return CycleRun{}, false, fmt.Errorf("store: read the last cycle: %w", err)
	}
	run.StartedAt, run.FinishedAt = time.Time(started), time.Time(finished)
	return run, true, nil
}

func finishRun(ctx context.Context, tx *sql.Tx, run CycleRun) error {
	_, err := tx.ExecContext(ctx, `
		UPDATE cycle_runs SET finished_at = ?, error = ?, processes_seen = ?, events_created = ?
		WHERE id = ? AND account_id = ?`,
		dbTime(run.FinishedAt), run.Error, run.ProcessesSeen, run.EventsCreated, run.ID, run.AccountID)
	if err != nil {
		return fmt.Errorf("store: record the cycle end: %w", err)
	}
	_, err = tx.ExecContext(ctx, `
		DELETE FROM cycle_runs
		WHERE account_id = ? AND id <= (
			SELECT id FROM cycle_runs WHERE account_id = ? ORDER BY id DESC LIMIT 1 OFFSET ?
		)`, run.AccountID, run.AccountID, keptRunsPerAccount)
	if err != nil {
		return fmt.Errorf("store: prune old cycles: %w", err)
	}
	return nil
}
