package monitor

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/sei"
)

const firstRetryDelay = 30 * time.Second

var (
	ErrRunning  = errors.New("a cycle is already running")
	ErrPaused   = errors.New("monitoring is paused because SEI rejected the login; restart sei-monitor after fixing the configuration")
	ErrNotReady = errors.New("sei-monitor has not reached SEI yet")
)

type TooSoonError struct {
	RetryAfter time.Duration
}

func (e *TooSoonError) Error() string {
	return fmt.Sprintf("the last cycle started less than %s ago; try again in %s", core.MinPollInterval, e.RetryAfter.Round(time.Second))
}

func (m *Monitor) Run(ctx context.Context) error {
	ready, err := m.prepareUntilReady(ctx)
	if !ready {
		return err
	}
	trigger := TriggerScheduled
	failures := 0
	for {
		_, err := m.RunOnce(ctx, trigger)
		if ctx.Err() != nil {
			return nil
		}
		if errors.Is(err, sei.ErrInvalidCredentials) {
			m.pause(ctx, err)
			<-ctx.Done()
			return nil
		}
		wait := m.nextWait(err, &failures)
		m.mu.Lock()
		m.state.NextCycleAt = m.now().Add(wait)
		m.mu.Unlock()
		select {
		case <-ctx.Done():
			return nil
		case <-m.after(wait):
			trigger = TriggerScheduled
		case <-m.manual:
			trigger = TriggerManual
		}
	}
}

func (m *Monitor) TriggerCycle() error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.state.Paused {
		return ErrPaused
	}
	if m.state.AccountID == 0 {
		return ErrNotReady
	}
	if m.state.Running {
		return ErrRunning
	}
	if since := m.now().Sub(m.state.LastStartedAt); !m.state.LastStartedAt.IsZero() && since < core.MinPollInterval {
		return &TooSoonError{RetryAfter: core.MinPollInterval - since}
	}
	select {
	case m.manual <- struct{}{}:
		return nil
	default:
		return ErrRunning
	}
}

func (m *Monitor) prepareUntilReady(ctx context.Context) (bool, error) {
	failures := 0
	for {
		err := m.Prepare(ctx)
		if err == nil {
			return true, nil
		}
		if ctx.Err() != nil {
			return false, nil
		}
		if errors.Is(err, sei.ErrOrgNotFound) || errors.Is(err, sei.ErrModuleNotFound) {
			return false, err
		}
		failures++
		wait := m.backoff(failures)
		m.log.Warn("could not reach SEI; trying again", "error", err, "retry_in", wait.Round(time.Second))
		select {
		case <-ctx.Done():
			return false, nil
		case <-m.after(wait):
		}
	}
}

func (m *Monitor) nextWait(err error, failures *int) time.Duration {
	if err == nil {
		*failures = 0
		return m.cfg.PollInterval + m.jitter(m.cfg.PollInterval/10)
	}
	*failures++
	wait := m.backoff(*failures)
	m.log.Warn("cycle failed", "error", err, "retry_in", wait.Round(time.Second))
	return wait
}

func (m *Monitor) backoff(failures int) time.Duration {
	delay := firstRetryDelay
	for range min(failures-1, 16) {
		delay *= 2
	}
	delay = min(delay, max(m.cfg.PollInterval, firstRetryDelay))
	return delay + m.jitter(delay/10)
}

func (m *Monitor) pause(ctx context.Context, cause error) {
	ctx = context.WithoutCancel(ctx)
	now := m.now()
	reason, ev := loginFailure(cause, now)
	m.mu.Lock()
	m.state.Paused, m.state.PausedReason, m.state.NextCycleAt = true, reason, time.Time{}
	m.account.PausedReason = reason
	accountID := m.account.ID
	m.mu.Unlock()

	m.log.Error("SEI rejected the login; monitoring is paused until sei-monitor restarts with the configuration fixed", "reason", reason)
	if err := m.repo.SetPausedReason(ctx, accountID, reason); err != nil {
		m.log.Error("could not record the pause", "error", err)
	}
	changes := CycleChanges{AccountID: accountID, Now: now, Events: []event.Event{ev}, Channels: m.cfg.Channels}
	if _, err := m.repo.ApplyCycle(ctx, changes); err != nil {
		m.log.Error("could not record the login failure event", "error", err)
	}
}
