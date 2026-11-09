package deliveries_test

import (
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/modules/deliveries"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

var now = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

func TestListDeliveriesFiltersAndPages(t *testing.T) {
	database := coretest.Open(t)
	cycles := monitor.NewPostgres(database.Pool)
	account, err := cycles.EnsureAccount(t.Context(), core.AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}, now)
	if err != nil {
		t.Fatal(err)
	}
	_, err = cycles.ApplyCycle(t.Context(), monitor.CycleChanges{
		AccountID: account.ID, Now: now, Channels: []string{"telegram", "webhook"},
		Events: []event.Event{
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "a1", OccurredAt: now},
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "a2", OccurredAt: now},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	outbox := notify.NewPostgres(database.Pool)
	due, err := outbox.DueDeliveries(t.Context(), "webhook", now, 1)
	if err != nil {
		t.Fatal(err)
	}
	sent := due[0].Delivery
	sent.Status, sent.Attempts, sent.SentAt = core.DeliverySent, 1, now.Add(time.Second)
	if err := outbox.UpdateDeliveries(t.Context(), []core.Delivery{sent}); err != nil {
		t.Fatal(err)
	}
	repo := deliveries.NewPostgres(database.Pool)
	list := func(filter deliveries.Filter) []core.Delivery {
		t.Helper()
		filter.Limit = 50
		if filter.AccountID == 0 {
			filter.AccountID = account.ID
		}
		found, err := repo.ListDeliveries(t.Context(), filter)
		if err != nil {
			t.Fatal(err)
		}
		return found
	}

	all := list(deliveries.Filter{})
	channels := core.Map(all, func(d core.Delivery) string { return d.Channel })
	if len(all) != 4 || !slices.Equal(channels, []string{"telegram", "webhook", "telegram", "webhook"}) || all[0].EventID <= all[2].EventID {
		t.Fatalf("deliveries = %+v, want newest event first, channels in order", all)
	}
	if first := all[0]; first.Status != core.DeliveryPending || first.Attempts != 0 || !first.NextAttemptAt.Equal(now) || !first.SentAt.IsZero() {
		t.Errorf("pending delivery = %+v", first)
	}
	if got := list(deliveries.Filter{Channel: "webhook", Status: core.DeliverySent}); len(got) != 1 || got[0] != (core.Delivery{
		EventID: sent.EventID, Channel: "webhook", Status: core.DeliverySent, Attempts: 1, NextAttemptAt: now, SentAt: now.Add(time.Second),
	}) {
		t.Errorf("sent webhook deliveries = %+v", got)
	}
	if got := list(deliveries.Filter{Status: core.DeliveryPending}); len(got) != 3 {
		t.Errorf("pending deliveries = %+v", got)
	}
	if got := list(deliveries.Filter{AccountID: account.ID + 1}); len(got) != 0 {
		t.Errorf("deliveries of another account = %+v", got)
	}
	next := list(deliveries.Filter{After: &deliveries.Key{EventID: all[0].EventID, Channel: all[0].Channel}})
	if !slices.Equal(next, all[1:]) {
		t.Errorf("deliveries after the first = %+v, want %+v", next, all[1:])
	}
}
