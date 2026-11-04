package monitor

import (
	"context"
	"errors"
	"net/http"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
	"github.com/ItzTas/sei-monitor/internal/store"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

func (h *harness) run(t *testing.T) func() error {
	t.Helper()
	ctx, cancel := context.WithCancel(t.Context())
	done := make(chan error, 1)
	go func() { done <- h.monitor.Run(ctx) }()
	return func() error {
		cancel()
		select {
		case err := <-done:
			return err
		case <-time.After(10 * time.Second):
			t.Fatal("Run did not stop")
			return nil
		}
	}
}

func (h *harness) lastRun(t *testing.T) store.CycleRun {
	t.Helper()
	run, _, err := h.store.LastCycleRun(t.Context(), h.monitor.State().AccountID)
	if err != nil {
		t.Fatal(err)
	}
	return run
}

func TestRunBacksOffAfterFailuresAndThenKeepsTheInterval(t *testing.T) {
	h := newHarness(t, storetest.SQLiteURL(t), nil)
	populate(h.server)
	h.server.FailNext(seitest.RouteProcesses, http.StatusBadGateway, 2)
	stop := h.run(t)

	var waits []time.Duration
	for range 3 {
		wait := h.clock.nextWait(t)
		waits = append(waits, wait.duration)
		h.clock.Advance(wait.duration)
		wait.fire <- h.clock.Now()
	}
	h.clock.nextWait(t)
	if err := stop(); err != nil {
		t.Fatal(err)
	}

	if !slices.Equal(waits, []time.Duration{30 * time.Second, time.Minute, 15 * time.Minute}) {
		t.Errorf("waits = %v, want 30s, 1m, then the 15m interval", waits)
	}
	if run := h.lastRun(t); run.Error != "" || run.ProcessesSeen != 2 {
		t.Errorf("last run = %+v", run)
	}
}

func TestRejectedLoginPausesWithoutTryingAgain(t *testing.T) {
	h := newHarness(t, storetest.SQLiteURL(t), nil)
	populate(h.server)
	h.server.SetPassword("changed-by-the-user")
	stop := h.run(t)

	deadline := time.Now().Add(10 * time.Second)
	for !h.monitor.State().Paused && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	h.clock.Advance(time.Hour)
	triggerErr := h.monitor.TriggerCycle()
	if err := stop(); err != nil {
		t.Fatal(err)
	}

	state := h.monitor.State()
	if !state.Paused || !strings.Contains(state.PausedReason, "Usuário ou Senha Inválida.") {
		t.Fatalf("state = %+v", state)
	}
	if !errors.Is(triggerErr, ErrPaused) {
		t.Errorf("manual cycle while paused: error = %v, want ErrPaused", triggerErr)
	}
	if h.server.FailedLogins() != 1 {
		t.Errorf("rejected logins = %d, want 1", h.server.FailedLogins())
	}
	account, err := h.store.EnsureAccount(t.Context(), store.AccountKey{SEIBaseURL: h.server.URL(), Username: seitest.Username, OrgID: seitest.OrgID}, start)
	if err != nil || account.PausedReason != state.PausedReason {
		t.Errorf("stored pause = %q, %v", account.PausedReason, err)
	}
	events, err := h.store.RecentEvents(t.Context(), 10)
	if err != nil || len(events) != 1 || events[0].Kind != event.AccountLoginFailed || events[0].Payload.Reason != state.PausedReason {
		t.Errorf("events = %+v, %v", events, err)
	}
	if run := h.lastRun(t); !strings.Contains(run.Error, "Usuário ou Senha Inválida.") {
		t.Errorf("last run = %+v", run)
	}
}

func TestManualCycleWaitsForTheMinimumInterval(t *testing.T) {
	h := newHarness(t, storetest.SQLiteURL(t), nil)
	populate(h.server)
	stop := h.run(t)
	h.clock.nextWait(t)

	h.clock.Advance(time.Minute)
	early := h.monitor.TriggerCycle()
	h.clock.Advance(4 * time.Minute)
	accepted := h.monitor.TriggerCycle()
	h.clock.nextWait(t)
	if err := stop(); err != nil {
		t.Fatal(err)
	}

	tooSoon, ok := errors.AsType[*TooSoonError](early)
	if !ok || tooSoon.RetryAfter != 4*time.Minute {
		t.Errorf("early manual cycle: error = %v, want a TooSoonError asking for 4m", early)
	}
	if accepted != nil {
		t.Errorf("manual cycle after 5m: error = %v", accepted)
	}
	if run := h.lastRun(t); run.Trigger != store.TriggerManual || run.Error != "" {
		t.Errorf("last run = %+v", run)
	}
}

func TestRunWaitsForADisabledModule(t *testing.T) {
	h := newHarness(t, storetest.SQLiteURL(t), nil)
	populate(h.server)
	h.server.SetModuleInactive(true)
	stop := h.run(t)

	retry := h.clock.nextWait(t)
	h.server.SetModuleInactive(false)
	retry.fire <- h.clock.Now()
	h.clock.nextWait(t)
	if err := stop(); err != nil {
		t.Fatal(err)
	}

	if retry.duration != 30*time.Second {
		t.Errorf("first retry after %v, want 30s", retry.duration)
	}
	if run := h.lastRun(t); run.Error != "" || run.ProcessesSeen != 2 {
		t.Errorf("last run = %+v", run)
	}
}

func TestRunStopsOnAnUnknownAgency(t *testing.T) {
	h := newHarness(t, storetest.SQLiteURL(t), func(cfg *Config) { cfg.Target.Org = "NOPE" })

	err := h.monitor.Run(t.Context())

	if !errors.Is(err, sei.ErrOrgNotFound) {
		t.Errorf("Run = %v, want sei.ErrOrgNotFound", err)
	}
}
