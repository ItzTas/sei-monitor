package store

import (
	"context"
	"database/sql"
	"fmt"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
)

const (
	DeliveryPending = "pending"
	DeliverySent    = "sent"
	DeliveryFailed  = "failed"
)

type DueDelivery struct {
	Delivery
	Event event.Event
}

func (s *Store) DueDeliveries(ctx context.Context, channel string, now time.Time, limit int) ([]DueDelivery, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT e.id, e.account_id, e.kind, e.procedure_id, e.ref, e.occurred_at, e.detected_at, e.payload,
		       d.channel, d.status, d.attempts, d.next_attempt_at, d.last_error
		FROM deliveries d JOIN events e ON e.id = d.event_id
		WHERE d.channel = ? AND d.status = 'pending' AND d.next_attempt_at <= ?
		ORDER BY e.occurred_at, e.id
		LIMIT ?`, channel, dbTime(now), limit)
	if err != nil {
		return nil, fmt.Errorf("store: read the due deliveries: %w", err)
	}
	defer func() { _ = rows.Close() }()

	var due []DueDelivery
	for rows.Next() {
		var d DueDelivery
		var next dbTime
		ev, err := scanEvent(rows, &d.Channel, &d.Status, &d.Attempts, &next, &d.LastError)
		if err != nil {
			return nil, err
		}
		d.Event, d.EventID, d.NextAttemptAt = ev, ev.ID, time.Time(next)
		due = append(due, d)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: read the due deliveries: %w", err)
	}
	return due, nil
}

func (s *Store) UpdateDeliveries(ctx context.Context, deliveries []Delivery) error {
	return s.inTx(ctx, func(tx *sql.Tx) error {
		stmt, err := tx.PrepareContext(ctx, `
			UPDATE deliveries
			SET status = ?, attempts = ?, next_attempt_at = ?, last_error = ?, sent_at = ?
			WHERE event_id = ? AND channel = ?`)
		if err != nil {
			return fmt.Errorf("store: prepare the delivery update: %w", err)
		}
		defer func() { _ = stmt.Close() }()
		for _, d := range deliveries {
			_, err := stmt.ExecContext(ctx, d.Status, d.Attempts, dbTime(d.NextAttemptAt), d.LastError, dbTime(d.SentAt), d.EventID, d.Channel)
			if err != nil {
				return fmt.Errorf("store: update the %s delivery of event %d: %w", d.Channel, d.EventID, err)
			}
		}
		return nil
	})
}
