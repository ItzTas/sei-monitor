package store

import (
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

func threeEventsForTwoChannels(t *testing.T, s *Store) Account {
	t.Helper()
	account := newAccount(t, s)
	applyAt(t, s, account, testNow, CycleChanges{Channels: []string{"telegram", "webhook"}, Events: []event.Event{
		activityAt("late", 9001, testNow.Add(-time.Hour)),
		activityAt("early", 9001, testNow.Add(-2*time.Hour)),
		activityAt("middle", 9002, testNow.Add(-90*time.Minute)),
	}})
	return account
}

func due(t *testing.T, s *Store, channel string, now time.Time, limit int) []DueDelivery {
	t.Helper()
	deliveries, err := s.DueDeliveries(t.Context(), channel, now, limit)
	if err != nil {
		t.Fatal(err)
	}
	return deliveries
}

func dueRefs(deliveries []DueDelivery) []string {
	result := make([]string, 0, len(deliveries))
	for _, d := range deliveries {
		result = append(result, d.Event.Ref)
	}
	return result
}

func TestDueDeliveriesComeOldestFirstPerChannel(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		threeEventsForTwoChannels(t, s)

		telegram := due(t, s, "telegram", testNow, 2)
		webhook := due(t, s, "webhook", testNow, 10)

		if got := dueRefs(telegram); !slices.Equal(got, []string{"early", "middle"}) {
			t.Errorf("telegram = %v", got)
		}
		if got := dueRefs(webhook); !slices.Equal(got, []string{"early", "middle", "late"}) {
			t.Errorf("webhook = %v", got)
		}
		first := telegram[0]
		if first.Channel != "telegram" || first.Status != DeliveryPending || first.Attempts != 0 || first.EventID != first.Event.ID ||
			first.Event.Kind != event.ActivityAdded || first.Event.ProcedureID != 9001 || !first.NextAttemptAt.Equal(micro(testNow)) {
			t.Errorf("first due delivery = %+v", first)
		}
	})
}

func TestUpdateDeliveriesRecordsEachAttempt(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		account := threeEventsForTwoChannels(t, s)
		pending := due(t, s, "telegram", testNow, 10)
		sent, retry, gaveUp := pending[0].Delivery, pending[1].Delivery, pending[2].Delivery
		sent.Status, sent.Attempts, sent.SentAt = DeliverySent, 1, testNow
		retry.Attempts, retry.NextAttemptAt, retry.LastError = 1, testNow.Add(time.Minute), "the server answered 500"
		gaveUp.Status, gaveUp.Attempts, gaveUp.LastError = DeliveryFailed, 6, "the server answered 500"

		if err := s.UpdateDeliveries(t.Context(), []Delivery{sent, retry, gaveUp}); err != nil {
			t.Fatal(err)
		}

		if got := dueRefs(due(t, s, "telegram", testNow, 10)); len(got) != 0 {
			t.Errorf("due right after the attempt = %v", got)
		}
		later := due(t, s, "telegram", testNow.Add(time.Minute), 10)
		if got := dueRefs(later); !slices.Equal(got, []string{"middle"}) || later[0].Attempts != 1 || later[0].LastError != "the server answered 500" {
			t.Errorf("due a minute later = %+v", later)
		}
		if got := dueRefs(due(t, s, "webhook", testNow, 10)); !slices.Equal(got, []string{"early", "middle", "late"}) {
			t.Errorf("the webhook deliveries changed: %v", got)
		}
		listed, err := s.ListDeliveries(t.Context(), DeliveryFilter{AccountID: account.ID, Channel: "telegram", Limit: 10})
		if err != nil {
			t.Fatal(err)
		}
		byEvent := map[int64]Delivery{}
		for _, d := range listed {
			byEvent[d.EventID] = d
		}
		if d := byEvent[sent.EventID]; d.Status != DeliverySent || d.Attempts != 1 || !d.SentAt.Equal(micro(testNow)) || d.LastError != "" {
			t.Errorf("sent delivery = %+v", d)
		}
		if d := byEvent[gaveUp.EventID]; d.Status != DeliveryFailed || d.Attempts != 6 || !d.SentAt.IsZero() {
			t.Errorf("failed delivery = %+v", d)
		}
	})
}
