package store

import (
	"context"
	"database/sql"
	"fmt"
	"time"
)

type Pruned struct {
	Events    int64
	Processes int64
}

// PruneBefore deletes the events detected before cutoff, with their deliveries, and the processes
// that left the units before cutoff, with their documents. A process still in the units keeps its
// documents, since they tell the next cycle which documents are new.
func (s *Store) PruneBefore(ctx context.Context, accountID int64, cutoff time.Time) (Pruned, error) {
	var pruned Pruned
	err := s.inTx(ctx, func(tx *sql.Tx) error {
		var err error
		pruned.Events, err = deleteRows(ctx, tx, `DELETE FROM events WHERE account_id = ? AND detected_at < ?`, accountID, dbTime(cutoff))
		if err != nil {
			return fmt.Errorf("store: prune old events: %w", err)
		}
		pruned.Processes, err = deleteRows(ctx, tx, `DELETE FROM processes WHERE account_id = ? AND left_at < ?`, accountID, dbTime(cutoff))
		if err != nil {
			return fmt.Errorf("store: prune processes that left: %w", err)
		}
		return nil
	})
	if err != nil {
		return Pruned{}, err
	}
	return pruned, nil
}

func deleteRows(ctx context.Context, tx *sql.Tx, query string, args ...any) (int64, error) {
	result, err := tx.ExecContext(ctx, query, args...)
	if err != nil {
		return 0, err
	}
	return result.RowsAffected()
}
