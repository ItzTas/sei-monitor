package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
)

const processColumns = `procedure_id, number, process_type, description, unit_id, unit_acronym, assigned_user,
	assigned_user_id, return_date, return_overdue, last_activity_id, document_total,
	first_seen_at, last_seen_at, left_at`

type ProcessFilter struct {
	AccountID       int64
	Unit            string
	AssignedUserID  int64
	AssignedAcronym string
	ReturnScheduled bool
	ReturnOverdue   bool
	IncludeLeft     bool
	Limit           int
	After           *ProcessKey
}

type ProcessKey struct {
	LastSeenAt  time.Time
	ProcedureID int64
}

type EventFilter struct {
	AccountID   int64
	Kinds       []event.Kind
	ProcedureID int64
	Since       time.Time
	Until       time.Time
	Limit       int
	After       *EventKey
}

type EventKey struct {
	OccurredAt time.Time
	ID         int64
}

type Delivery struct {
	EventID       int64
	Channel       string
	Status        string
	Attempts      int
	NextAttemptAt time.Time
	LastError     string
	SentAt        time.Time
}

type DeliveryFilter struct {
	AccountID int64
	Status    string
	Channel   string
	Limit     int
	After     *DeliveryKey
}

type DeliveryKey struct {
	EventID int64
	Channel string
}

type conditions struct {
	clauses []string
	args    []any
}

func (c *conditions) add(clause string, args ...any) {
	c.clauses = append(c.clauses, clause)
	c.args = append(c.args, args...)
}

func (c *conditions) where() string {
	if len(c.clauses) == 0 {
		return ""
	}
	return " WHERE " + strings.Join(c.clauses, " AND ")
}

func (s *Store) ListProcesses(ctx context.Context, filter ProcessFilter) ([]Process, error) {
	var c conditions
	c.add("account_id = ?", filter.AccountID)
	if filter.Unit != "" {
		c.add("LOWER(unit_acronym) = LOWER(?)", filter.Unit)
	}
	if filter.AssignedUserID != 0 || filter.AssignedAcronym != "" {
		c.add("(assigned_user_id = ? OR (assigned_user_id = 0 AND assigned_user <> '' AND LOWER(assigned_user) = LOWER(?)))", filter.AssignedUserID, filter.AssignedAcronym)
	}
	if filter.ReturnScheduled {
		c.add("return_date <> ''")
	}
	if filter.ReturnOverdue {
		c.add("return_overdue = ?", true)
	}
	if !filter.IncludeLeft {
		c.add("left_at IS NULL")
	}
	if after := filter.After; after != nil {
		c.add("(last_seen_at < ? OR (last_seen_at = ? AND procedure_id < ?))", dbTime(after.LastSeenAt), dbTime(after.LastSeenAt), after.ProcedureID)
	}
	query := "SELECT " + processColumns + " FROM processes" + c.where() + " ORDER BY last_seen_at DESC, procedure_id DESC LIMIT ?"
	return s.queryProcesses(ctx, query, append(c.args, filter.Limit)...)
}

func (s *Store) ProcessByID(ctx context.Context, accountID, procedureID int64) (Process, error) {
	processes, err := s.queryProcesses(ctx, "SELECT "+processColumns+" FROM processes WHERE account_id = ? AND procedure_id = ?", accountID, procedureID)
	if err != nil {
		return Process{}, err
	}
	if len(processes) == 0 {
		return Process{}, ErrNotFound
	}
	return processes[0], nil
}

func (s *Store) queryProcesses(ctx context.Context, query string, args ...any) ([]Process, error) {
	rows, err := s.db.QueryContext(ctx, query, args...)
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

func (s *Store) Documents(ctx context.Context, accountID, procedureID int64) ([]Document, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT document_id, procedure_id, formatted_protocol, document_type, name, first_seen_at
		FROM documents
		WHERE account_id = ? AND procedure_id = ?
		ORDER BY document_id`, accountID, procedureID)
	if err != nil {
		return nil, fmt.Errorf("store: read the documents: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var documents []Document
	for rows.Next() {
		var d Document
		var firstSeen dbTime
		if err := rows.Scan(&d.DocumentID, &d.ProcedureID, &d.Protocol, &d.Type, &d.Name, &firstSeen); err != nil {
			return nil, fmt.Errorf("store: read a document: %w", err)
		}
		d.FirstSeenAt = time.Time(firstSeen)
		documents = append(documents, d)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: read the documents: %w", err)
	}
	return documents, nil
}

func (s *Store) ListEvents(ctx context.Context, filter EventFilter) ([]event.Event, error) {
	var c conditions
	if filter.AccountID != 0 {
		c.add("account_id = ?", filter.AccountID)
	}
	if len(filter.Kinds) > 0 {
		placeholders := strings.TrimSuffix(strings.Repeat("?, ", len(filter.Kinds)), ", ")
		kinds := make([]any, len(filter.Kinds))
		for i, kind := range filter.Kinds {
			kinds[i] = string(kind)
		}
		c.add("kind IN ("+placeholders+")", kinds...)
	}
	if filter.ProcedureID != 0 {
		c.add("procedure_id = ?", filter.ProcedureID)
	}
	if !filter.Since.IsZero() {
		c.add("occurred_at >= ?", dbTime(filter.Since))
	}
	if !filter.Until.IsZero() {
		c.add("occurred_at < ?", dbTime(filter.Until))
	}
	if after := filter.After; after != nil {
		c.add("(occurred_at < ? OR (occurred_at = ? AND id < ?))", dbTime(after.OccurredAt), dbTime(after.OccurredAt), after.ID)
	}
	query := `SELECT id, account_id, kind, procedure_id, ref, occurred_at, detected_at, payload FROM events` +
		c.where() + " ORDER BY occurred_at DESC, id DESC LIMIT ?"
	return s.queryEvents(ctx, query, append(c.args, filter.Limit)...)
}

func (s *Store) queryEvents(ctx context.Context, query string, args ...any) ([]event.Event, error) {
	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("store: read the events: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var events []event.Event
	for rows.Next() {
		ev, err := scanEvent(rows)
		if err != nil {
			return nil, err
		}
		events = append(events, ev)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: read the events: %w", err)
	}
	return events, nil
}

func scanEvent(rows *sql.Rows, extra ...any) (event.Event, error) {
	var ev event.Event
	var kind, payload string
	var occurred, detected dbTime
	columns := append([]any{&ev.ID, &ev.AccountID, &kind, &ev.ProcedureID, &ev.Ref, &occurred, &detected, &payload}, extra...)
	if err := rows.Scan(columns...); err != nil {
		return event.Event{}, fmt.Errorf("store: read an event: %w", err)
	}
	if err := json.Unmarshal([]byte(payload), &ev.Payload); err != nil {
		return event.Event{}, fmt.Errorf("store: decode the payload of event %d: %w", ev.ID, err)
	}
	ev.Kind, ev.OccurredAt, ev.DetectedAt = event.Kind(kind), time.Time(occurred), time.Time(detected)
	return ev, nil
}

func (s *Store) ListDeliveries(ctx context.Context, filter DeliveryFilter) ([]Delivery, error) {
	var c conditions
	if filter.AccountID != 0 {
		c.add("e.account_id = ?", filter.AccountID)
	}
	if filter.Status != "" {
		c.add("d.status = ?", filter.Status)
	}
	if filter.Channel != "" {
		c.add("d.channel = ?", filter.Channel)
	}
	if after := filter.After; after != nil {
		c.add("(d.event_id < ? OR (d.event_id = ? AND d.channel > ?))", after.EventID, after.EventID, after.Channel)
	}
	query := `SELECT d.event_id, d.channel, d.status, d.attempts, d.next_attempt_at, d.last_error, d.sent_at
		FROM deliveries d JOIN events e ON e.id = d.event_id` + c.where() + " ORDER BY d.event_id DESC, d.channel ASC LIMIT ?"
	return s.queryDeliveries(ctx, query, append(c.args, filter.Limit)...)
}

func (s *Store) queryDeliveries(ctx context.Context, query string, args ...any) ([]Delivery, error) {
	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("store: read the deliveries: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var deliveries []Delivery
	for rows.Next() {
		var d Delivery
		var next, sent dbTime
		if err := rows.Scan(&d.EventID, &d.Channel, &d.Status, &d.Attempts, &next, &d.LastError, &sent); err != nil {
			return nil, fmt.Errorf("store: read a delivery: %w", err)
		}
		d.NextAttemptAt, d.SentAt = time.Time(next), time.Time(sent)
		deliveries = append(deliveries, d)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: read the deliveries: %w", err)
	}
	return deliveries, nil
}

func (s *Store) DeliveryCounts(ctx context.Context, accountID int64) (map[string]map[string]int, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT d.channel, d.status, COUNT(*)
		FROM deliveries d JOIN events e ON e.id = d.event_id
		WHERE e.account_id = ?
		GROUP BY d.channel, d.status`, accountID)
	if err != nil {
		return nil, fmt.Errorf("store: count the deliveries: %w", err)
	}
	defer func() { _ = rows.Close() }()

	counts := map[string]map[string]int{}
	for rows.Next() {
		var channel, status string
		var count int
		if err := rows.Scan(&channel, &status, &count); err != nil {
			return nil, fmt.Errorf("store: count the deliveries: %w", err)
		}
		if counts[channel] == nil {
			counts[channel] = map[string]int{}
		}
		counts[channel][status] = count
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: count the deliveries: %w", err)
	}
	return counts, nil
}
