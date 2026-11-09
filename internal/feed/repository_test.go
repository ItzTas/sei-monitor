package feed

import (
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/monitor"
)

func TestRecentEventsComeNewestFirst(t *testing.T) {
	database := coretest.Open(t)
	cycles := monitor.NewPostgres(database.Pool)
	start := time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)
	account, err := cycles.EnsureAccount(t.Context(), core.AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}, start)
	if err != nil {
		t.Fatal(err)
	}
	for i, ref := range []string{"50001", "50002", "50003"} {
		_, err := cycles.ApplyCycle(t.Context(), monitor.CycleChanges{
			AccountID: account.ID, Now: start.Add(time.Duration(i) * time.Minute),
			Events: []event.Event{{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: ref, OccurredAt: start.Add(-time.Duration(i) * time.Hour), Payload: event.Payload{ActivityText: "andamento " + ref}}},
		})
		if err != nil {
			t.Fatal(err)
		}
	}

	events, err := NewPostgres(database.Pool).RecentEvents(t.Context(), 2)
	if err != nil {
		t.Fatal(err)
	}

	if len(events) != 2 || events[0].Ref != "50003" || events[1].Ref != "50002" {
		t.Fatalf("events = %+v", events)
	}
	latest := events[0]
	if latest.Kind != event.ActivityAdded || latest.AccountID != account.ID || latest.ProcedureID != 9001 ||
		!latest.OccurredAt.Equal(start.Add(-2*time.Hour)) || !latest.DetectedAt.Equal(start.Add(2*time.Minute)) ||
		latest.Payload.ActivityText != "andamento 50003" {
		t.Errorf("latest event = %+v", latest)
	}
}
