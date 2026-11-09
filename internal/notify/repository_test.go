package notify_test

import (
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
)

func outboxWithThreeEvents(t *testing.T) outbox {
	t.Helper()
	o := newOutbox(t)
	o.add(t, t0,
		activity("late", t0.Add(-time.Hour)),
		activity("early", t0.Add(-2*time.Hour)),
		activity("middle", t0.Add(-90*time.Minute)),
	)
	return o
}

func due(t *testing.T, o outbox, channel string, now time.Time, limit int) []core.DueDelivery {
	t.Helper()
	deliveries, err := o.DueDeliveries(t.Context(), channel, now, limit)
	if err != nil {
		t.Fatal(err)
	}
	return deliveries
}

func dueRefs(deliveries []core.DueDelivery) []string {
	return core.Map(deliveries, func(d core.DueDelivery) string { return d.Event.Ref })
}

func TestDueDeliveriesComeOldestFirstPerChannel(t *testing.T) {
	o := outboxWithThreeEvents(t)

	telegram := due(t, o, "telegram", t0, 2)
	webhook := due(t, o, "webhook", t0, 10)
	early := due(t, o, "webhook", t0.Add(-time.Second), 10)

	if got := dueRefs(telegram); !slices.Equal(got, []string{"early", "middle"}) {
		t.Errorf("telegram = %v", got)
	}
	if got := dueRefs(webhook); !slices.Equal(got, []string{"early", "middle", "late"}) {
		t.Errorf("webhook = %v", got)
	}
	if len(early) != 0 {
		t.Errorf("due before the first attempt time = %v", dueRefs(early))
	}
	first := telegram[0]
	if first.Channel != "telegram" || first.Status != core.DeliveryPending || first.Attempts != 0 || first.EventID != first.Event.ID ||
		first.Event.Kind != event.ActivityAdded || first.Event.AccountID != o.account.ID || !first.NextAttemptAt.Equal(t0) ||
		first.Event.Payload.ActivityUser != "beltrano.silva" {
		t.Errorf("first due delivery = %+v", first)
	}
}

func TestUpdateDeliveriesRecordsEachAttempt(t *testing.T) {
	o := outboxWithThreeEvents(t)
	pending := due(t, o, "telegram", t0, 10)
	sent, retry, gaveUp := pending[0].Delivery, pending[1].Delivery, pending[2].Delivery
	sent.Status, sent.Attempts, sent.SentAt = core.DeliverySent, 1, t0
	retry.Attempts, retry.NextAttemptAt, retry.LastError = 1, t0.Add(time.Minute), "the server answered 500"
	gaveUp.Status, gaveUp.Attempts, gaveUp.LastError = core.DeliveryFailed, 6, "the server answered 500"

	if err := o.UpdateDeliveries(t.Context(), []core.Delivery{sent, retry, gaveUp}); err != nil {
		t.Fatal(err)
	}

	if got := dueRefs(due(t, o, "telegram", t0, 10)); len(got) != 0 {
		t.Errorf("due right after the attempts = %v", got)
	}
	later := due(t, o, "telegram", t0.Add(time.Minute), 10)
	if got := dueRefs(later); !slices.Equal(got, []string{"middle"}) || later[0].Attempts != 1 || later[0].LastError != "the server answered 500" {
		t.Errorf("due a minute later = %+v", later)
	}
	if got := dueRefs(due(t, o, "webhook", t0, 10)); !slices.Equal(got, []string{"early", "middle", "late"}) {
		t.Errorf("webhook deliveries = %v, want them untouched", got)
	}
	byEvent := map[int64]core.Delivery{}
	for _, d := range o.deliveries(t, "telegram") {
		byEvent[d.EventID] = d
	}
	if d := byEvent[sent.EventID]; d.Status != core.DeliverySent || d.Attempts != 1 || !d.SentAt.Equal(t0) || d.LastError != "" {
		t.Errorf("sent delivery = %+v", d)
	}
	if d := byEvent[gaveUp.EventID]; d.Status != core.DeliveryFailed || d.Attempts != 6 || !d.SentAt.IsZero() || d.LastError != "the server answered 500" {
		t.Errorf("failed delivery = %+v", d)
	}
}
