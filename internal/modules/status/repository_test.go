package status_test

import (
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/modules/status"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

var now = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

func TestStatusRepositoryReadsTheAccountTheLastRunAndTheDeliveryCounts(t *testing.T) {
	database := coretest.Open(t)
	cycles := monitor.NewPostgres(database.Pool)
	repo := status.NewPostgres(database.Pool)
	account, err := cycles.EnsureAccount(t.Context(), core.AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}, now)
	if err != nil {
		t.Fatal(err)
	}
	if _, found, err := repo.LastCycleRun(t.Context(), account.ID); found || err != nil {
		t.Errorf("last run before any cycle = %v, %v", found, err)
	}
	info := core.AccountInfo{OrgAcronym: "SEDEMO", SEIVersion: "5.0.5", WSSEIVersion: "3.0.4"}
	if err := cycles.UpdateAccountInfo(t.Context(), account.ID, info); err != nil {
		t.Fatal(err)
	}
	run, err := cycles.StartCycleRun(t.Context(), account.ID, monitor.TriggerManual, now)
	if err != nil {
		t.Fatal(err)
	}
	_, err = cycles.ApplyCycle(t.Context(), monitor.CycleChanges{
		AccountID: account.ID, RunID: run, Now: now, FinishedAt: now.Add(time.Minute), Channels: []string{"telegram", "webhook"},
		Events: []event.Event{
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "a1", OccurredAt: now},
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "a2", OccurredAt: now},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	outbox := notify.NewPostgres(database.Pool)
	due, err := outbox.DueDeliveries(t.Context(), "telegram", now, 1)
	if err != nil {
		t.Fatal(err)
	}
	failed := due[0].Delivery
	failed.Status, failed.Attempts = core.DeliveryFailed, 6
	if err := outbox.UpdateDeliveries(t.Context(), []core.Delivery{failed}); err != nil {
		t.Fatal(err)
	}

	found, err := repo.Account(t.Context(), account.ID)
	if err != nil || found.Info != info || found.Key != account.Key {
		t.Errorf("account = %+v, %v", found, err)
	}
	if _, err := repo.Account(t.Context(), account.ID+1000); !errors.Is(err, core.ErrNotFound) {
		t.Errorf("unknown account: error = %v, want core.ErrNotFound", err)
	}
	last, ok, err := repo.LastCycleRun(t.Context(), account.ID)
	if err != nil || !ok || last.ID != run || last.Trigger != monitor.TriggerManual || last.EventsCreated != 2 || !last.FinishedAt.Equal(now.Add(time.Minute)) {
		t.Errorf("last run = %+v, %v, %v", last, ok, err)
	}
	counts, err := repo.DeliveryCounts(t.Context(), account.ID)
	want := map[string]map[string]int{"telegram": {"failed": 1, "pending": 1}, "webhook": {"pending": 2}}
	if err != nil || fmt.Sprint(counts) != fmt.Sprint(want) {
		t.Errorf("counts = %v, %v, want %v", counts, err, want)
	}
	if other, err := repo.DeliveryCounts(t.Context(), account.ID+1); err != nil || len(other) != 0 {
		t.Errorf("counts of another account = %v, %v", other, err)
	}
}
