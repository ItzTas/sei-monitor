package store

import (
	"context"
	"errors"
	"fmt"
	"time"
)

type AccountKey struct {
	SEIBaseURL string
	Username   string
	OrgID      int64
}

type Account struct {
	ID           int64
	Key          AccountKey
	CreatedAt    time.Time
	BaselineAt   time.Time
	PausedReason string
	Info         AccountInfo
}

type AccountInfo struct {
	OrgAcronym   string
	SEIVersion   string
	WSSEIVersion string
}

func (s *Store) EnsureAccount(ctx context.Context, key AccountKey, now time.Time) (Account, error) {
	account := Account{Key: key}
	var createdAt, baselineAt dbTime
	err := s.db.QueryRowContext(ctx, `
		INSERT INTO accounts (sei_base_url, username, org_id, created_at)
		VALUES (?, ?, ?, ?)
		ON CONFLICT (sei_base_url, username, org_id) DO UPDATE SET username = excluded.username
		RETURNING id, created_at, baseline_at, paused_reason, org_acronym, sei_version, wssei_version`,
		key.SEIBaseURL, key.Username, key.OrgID, dbTime(now),
	).Scan(&account.ID, &createdAt, &baselineAt, &account.PausedReason, &account.Info.OrgAcronym, &account.Info.SEIVersion, &account.Info.WSSEIVersion)
	if err != nil {
		return Account{}, fmt.Errorf("store: save the account: %w", err)
	}
	account.CreatedAt, account.BaselineAt = time.Time(createdAt), time.Time(baselineAt)
	return account, nil
}

func (s *Store) SetPausedReason(ctx context.Context, accountID int64, reason string) error {
	result, err := s.db.ExecContext(ctx, `UPDATE accounts SET paused_reason = ? WHERE id = ?`, reason, accountID)
	if err != nil {
		return fmt.Errorf("store: update the account: %w", err)
	}
	if rows, err := result.RowsAffected(); err == nil && rows == 0 {
		return errors.New("store: account not found")
	}
	return nil
}

func (s *Store) UpdateAccountInfo(ctx context.Context, accountID int64, info AccountInfo) error {
	_, err := s.db.ExecContext(ctx, `UPDATE accounts SET org_acronym = ?, sei_version = ?, wssei_version = ? WHERE id = ?`,
		info.OrgAcronym, info.SEIVersion, info.WSSEIVersion, accountID)
	if err != nil {
		return fmt.Errorf("store: update the account details: %w", err)
	}
	return nil
}
