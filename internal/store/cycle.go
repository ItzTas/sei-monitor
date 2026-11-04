package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
)

type Process struct {
	ProcedureID    int64
	Number         string
	Type           string
	Description    string
	UnitID         int64
	UnitAcronym    string
	AssignedUser   string
	AssignedUserID int64
	ReturnDate     string
	ReturnOverdue  bool
	LastActivityID int64
	DocumentTotal  int
	FirstSeenAt    time.Time
	LastSeenAt     time.Time
	LeftAt         time.Time
}

type Document struct {
	DocumentID  int64
	ProcedureID int64
	Protocol    string
	Type        string
	Name        string
}

type CycleChanges struct {
	AccountID  int64
	RunID      int64
	Now        time.Time
	FinishedAt time.Time
	Baseline   bool
	Processes  []Process
	Left       []int64
	Documents  []Document
	Events     []event.Event
	Channels   []string
}

func (s *Store) Processes(ctx context.Context, accountID int64) ([]Process, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT procedure_id, number, process_type, description, unit_id, unit_acronym, assigned_user,
		       assigned_user_id, return_date, return_overdue, last_activity_id, document_total,
		       first_seen_at, last_seen_at, left_at
		FROM processes
		WHERE account_id = ?
		ORDER BY procedure_id`, accountID)
	if err != nil {
		return nil, fmt.Errorf("store: read the processes: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var processes []Process
	for rows.Next() {
		var p Process
		var firstSeen, lastSeen, left dbTime
		err := rows.Scan(&p.ProcedureID, &p.Number, &p.Type, &p.Description, &p.UnitID, &p.UnitAcronym, &p.AssignedUser,
			&p.AssignedUserID, &p.ReturnDate, &p.ReturnOverdue, &p.LastActivityID, &p.DocumentTotal,
			&firstSeen, &lastSeen, &left)
		if err != nil {
			return nil, fmt.Errorf("store: read a process: %w", err)
		}
		p.FirstSeenAt, p.LastSeenAt, p.LeftAt = time.Time(firstSeen), time.Time(lastSeen), time.Time(left)
		processes = append(processes, p)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: read the processes: %w", err)
	}
	return processes, nil
}

func (s *Store) DocumentIDs(ctx context.Context, accountID, procedureID int64) (map[int64]bool, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT document_id FROM documents WHERE account_id = ? AND procedure_id = ?`, accountID, procedureID)
	if err != nil {
		return nil, fmt.Errorf("store: read the documents: %w", err)
	}
	defer func() { _ = rows.Close() }()

	ids := map[int64]bool{}
	for rows.Next() {
		var id int64
		if err := rows.Scan(&id); err != nil {
			return nil, fmt.Errorf("store: read a document: %w", err)
		}
		ids[id] = true
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: read the documents: %w", err)
	}
	return ids, nil
}

func (s *Store) ApplyCycle(ctx context.Context, changes CycleChanges) ([]event.Event, error) {
	var inserted []event.Event
	err := s.inTx(ctx, func(tx *sql.Tx) error {
		steps := []func(context.Context, *sql.Tx, CycleChanges) error{upsertProcesses, markLeft, insertDocuments, markBaseline}
		for _, step := range steps {
			if err := step(ctx, tx, changes); err != nil {
				return err
			}
		}
		events, err := insertEvents(ctx, tx, changes)
		if err != nil {
			return err
		}
		inserted = events
		if changes.RunID == 0 {
			return nil
		}
		return finishRun(ctx, tx, CycleRun{
			ID: changes.RunID, AccountID: changes.AccountID, FinishedAt: changes.FinishedAt,
			ProcessesSeen: len(changes.Processes), EventsCreated: len(events),
		})
	})
	if err != nil {
		return nil, err
	}
	return inserted, nil
}

func upsertProcesses(ctx context.Context, tx *sql.Tx, changes CycleChanges) error {
	if len(changes.Processes) == 0 {
		return nil
	}
	stmt, err := tx.PrepareContext(ctx, `
		INSERT INTO processes (account_id, procedure_id, number, process_type, description, unit_id, unit_acronym,
		                       assigned_user, assigned_user_id, return_date, return_overdue, last_activity_id,
		                       document_total, first_seen_at, last_seen_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT (account_id, procedure_id) DO UPDATE SET
			number = excluded.number,
			process_type = excluded.process_type,
			description = excluded.description,
			unit_id = excluded.unit_id,
			unit_acronym = excluded.unit_acronym,
			assigned_user = excluded.assigned_user,
			assigned_user_id = excluded.assigned_user_id,
			return_date = excluded.return_date,
			return_overdue = excluded.return_overdue,
			last_activity_id = excluded.last_activity_id,
			document_total = excluded.document_total,
			last_seen_at = excluded.last_seen_at,
			left_at = NULL`)
	if err != nil {
		return fmt.Errorf("store: prepare the process update: %w", err)
	}
	defer func() { _ = stmt.Close() }()
	for _, p := range changes.Processes {
		firstSeen := p.FirstSeenAt
		if firstSeen.IsZero() {
			firstSeen = changes.Now
		}
		_, err := stmt.ExecContext(ctx, changes.AccountID, p.ProcedureID, p.Number, p.Type, p.Description, p.UnitID, p.UnitAcronym,
			p.AssignedUser, p.AssignedUserID, p.ReturnDate, p.ReturnOverdue, p.LastActivityID, p.DocumentTotal,
			dbTime(firstSeen), dbTime(changes.Now))
		if err != nil {
			return fmt.Errorf("store: save process %d: %w", p.ProcedureID, err)
		}
	}
	return nil
}

func markLeft(ctx context.Context, tx *sql.Tx, changes CycleChanges) error {
	for _, procedureID := range changes.Left {
		_, err := tx.ExecContext(ctx, `UPDATE processes SET left_at = ? WHERE account_id = ? AND procedure_id = ? AND left_at IS NULL`,
			dbTime(changes.Now), changes.AccountID, procedureID)
		if err != nil {
			return fmt.Errorf("store: mark process %d as gone: %w", procedureID, err)
		}
	}
	return nil
}

func insertDocuments(ctx context.Context, tx *sql.Tx, changes CycleChanges) error {
	if len(changes.Documents) == 0 {
		return nil
	}
	stmt, err := tx.PrepareContext(ctx, `
		INSERT INTO documents (account_id, document_id, procedure_id, formatted_protocol, document_type, name, first_seen_at)
		VALUES (?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT (account_id, document_id) DO NOTHING`)
	if err != nil {
		return fmt.Errorf("store: prepare the document insert: %w", err)
	}
	defer func() { _ = stmt.Close() }()
	for _, d := range changes.Documents {
		_, err := stmt.ExecContext(ctx, changes.AccountID, d.DocumentID, d.ProcedureID, d.Protocol, d.Type, d.Name, dbTime(changes.Now))
		if err != nil {
			return fmt.Errorf("store: save document %d: %w", d.DocumentID, err)
		}
	}
	return nil
}

func markBaseline(ctx context.Context, tx *sql.Tx, changes CycleChanges) error {
	if !changes.Baseline {
		return nil
	}
	_, err := tx.ExecContext(ctx, `UPDATE accounts SET baseline_at = ? WHERE id = ? AND baseline_at IS NULL`, dbTime(changes.Now), changes.AccountID)
	if err != nil {
		return fmt.Errorf("store: mark the baseline: %w", err)
	}
	return nil
}

func insertEvents(ctx context.Context, tx *sql.Tx, changes CycleChanges) ([]event.Event, error) {
	if len(changes.Events) == 0 {
		return nil, nil
	}
	eventStmt, err := tx.PrepareContext(ctx, `
		INSERT INTO events (account_id, kind, procedure_id, ref, occurred_at, detected_at, payload)
		VALUES (?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT (account_id, kind, ref) DO NOTHING
		RETURNING id`)
	if err != nil {
		return nil, fmt.Errorf("store: prepare the event insert: %w", err)
	}
	defer func() { _ = eventStmt.Close() }()
	deliveryStmt, err := tx.PrepareContext(ctx, `
		INSERT INTO deliveries (event_id, channel, status, attempts, next_attempt_at)
		VALUES (?, ?, 'pending', 0, ?)`)
	if err != nil {
		return nil, fmt.Errorf("store: prepare the delivery insert: %w", err)
	}
	defer func() { _ = deliveryStmt.Close() }()

	var inserted []event.Event
	for _, ev := range changes.Events {
		ev.AccountID = changes.AccountID
		if ev.DetectedAt.IsZero() {
			ev.DetectedAt = changes.Now
		}
		if ev.OccurredAt.IsZero() {
			ev.OccurredAt = ev.DetectedAt
		}
		payload, err := json.Marshal(ev.Payload)
		if err != nil {
			return nil, fmt.Errorf("store: encode the event payload: %w", err)
		}
		err = eventStmt.QueryRowContext(ctx, ev.AccountID, string(ev.Kind), ev.ProcedureID, ev.Ref,
			dbTime(ev.OccurredAt), dbTime(ev.DetectedAt), string(payload)).Scan(&ev.ID)
		if errors.Is(err, sql.ErrNoRows) {
			continue
		}
		if err != nil {
			return nil, fmt.Errorf("store: save a %s event: %w", ev.Kind, err)
		}
		for _, channel := range changes.Channels {
			if _, err := deliveryStmt.ExecContext(ctx, ev.ID, channel, dbTime(changes.Now)); err != nil {
				return nil, fmt.Errorf("store: queue the %s delivery: %w", channel, err)
			}
		}
		inserted = append(inserted, ev)
	}
	return inserted, nil
}
