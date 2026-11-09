package events_test

import (
	"fmt"
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/modules/events"
	"github.com/ItzTas/sei-monitor/internal/monitor"
)

var now = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

type fixture struct {
	repo    *events.Postgres
	cycles  *monitor.Postgres
	account core.Account
}

func newFixture(t *testing.T) fixture {
	t.Helper()
	database := coretest.Open(t)
	cycles := monitor.NewPostgres(database.Pool)
	account, err := cycles.EnsureAccount(t.Context(), core.AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}, now)
	if err != nil {
		t.Fatal(err)
	}
	return fixture{repo: events.NewPostgres(database.Pool), cycles: cycles, account: account}
}

func (f fixture) add(t *testing.T, at time.Time, list ...event.Event) {
	t.Helper()
	if _, err := f.cycles.ApplyCycle(t.Context(), monitor.CycleChanges{AccountID: f.account.ID, Now: at, Events: list}); err != nil {
		t.Fatal(err)
	}
}

func activityAt(ref string, procedureID int64, occurred time.Time) event.Event {
	return event.Event{Kind: event.ActivityAdded, ProcedureID: procedureID, Ref: ref, OccurredAt: occurred}
}

func refs(list []event.Event) []string {
	return core.Map(list, func(ev event.Event) string { return ev.Ref })
}

func TestListEventsFilters(t *testing.T) {
	f := newFixture(t)
	f.add(t, now,
		activityAt("a1", 9001, now.Add(-3*time.Hour)),
		activityAt("a2", 9002, now.Add(-2*time.Hour)),
		event.Event{Kind: event.DocumentAdded, ProcedureID: 9001, Ref: "d1", OccurredAt: now.Add(-time.Hour)},
		event.Event{Kind: event.ProcessLeft, ProcedureID: 9002, Ref: "l1", OccurredAt: now},
	)

	tests := []struct {
		name   string
		filter events.Filter
		want   []string
	}{
		{"newest first", events.Filter{}, []string{"l1", "d1", "a2", "a1"}},
		{"some kinds", events.Filter{Kinds: []event.Kind{event.DocumentAdded, event.ProcessLeft}}, []string{"l1", "d1"}},
		{"one process", events.Filter{ProcedureID: 9001}, []string{"d1", "a1"}},
		{"time window", events.Filter{Since: now.Add(-2 * time.Hour), Until: now}, []string{"d1", "a2"}},
		{"other account", events.Filter{AccountID: f.account.ID + 1}, []string{}},
	}
	for _, tt := range tests {
		if tt.filter.AccountID == 0 {
			tt.filter.AccountID = f.account.ID
		}
		tt.filter.Limit = 50
		list, err := f.repo.ListEvents(t.Context(), tt.filter)
		if err != nil {
			t.Fatal(err)
		}
		if got := refs(list); !slices.Equal(got, tt.want) {
			t.Errorf("%s: events = %v, want %v", tt.name, got, tt.want)
		}
	}
}

func TestListEventsPagesWithoutRepeatsOrGapsWhileEventsArrive(t *testing.T) {
	f := newFixture(t)
	var initial []event.Event
	for i := range 7 {
		initial = append(initial, activityAt(fmt.Sprintf("old%d", i), 9001, now.Add(-time.Duration(i)*time.Minute)))
	}
	initial = append(initial, activityAt("tie", 9002, now.Add(-3*time.Minute)))
	f.add(t, now, initial...)

	var seen []string
	filter := events.Filter{AccountID: f.account.ID, Limit: 3}
	for page := range 10 {
		list, err := f.repo.ListEvents(t.Context(), filter)
		if err != nil {
			t.Fatal(err)
		}
		if len(list) == 0 {
			break
		}
		seen = append(seen, refs(list)...)
		last := list[len(list)-1]
		filter.After = &events.Key{OccurredAt: last.OccurredAt, ID: last.ID}
		if page == 0 {
			f.add(t, now.Add(time.Minute), activityAt("newest", 9001, now.Add(time.Minute)), activityAt("late", 9002, now.Add(-90*time.Second)))
		}
	}

	for _, ref := range refs(initial) {
		if count := countOf(seen, ref); count != 1 {
			t.Errorf("event %s seen %d times in %v", ref, count, seen)
		}
	}
	if countOf(seen, "newest") != 0 {
		t.Errorf("an event newer than the first page showed up on a later page: %v", seen)
	}
}

func countOf(values []string, value string) int {
	count := 0
	for _, v := range values {
		if v == value {
			count++
		}
	}
	return count
}
