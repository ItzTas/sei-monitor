package store

import (
	"errors"
	"fmt"
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

func applyAt(t *testing.T, s *Store, account Account, now time.Time, changes CycleChanges) {
	t.Helper()
	changes.AccountID, changes.Now = account.ID, now
	if _, err := s.ApplyCycle(t.Context(), changes); err != nil {
		t.Fatal(err)
	}
}

func procedureIDs(processes []Process) []int64 {
	ids := make([]int64, 0, len(processes))
	for _, p := range processes {
		ids = append(ids, p.ProcedureID)
	}
	return ids
}

func TestListProcessesFilters(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		account := newAccount(t, s)
		applyAt(t, s, account, testNow.Add(-time.Hour), CycleChanges{Processes: []Process{
			{ProcedureID: 9003, Number: "3", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC"},
		}})
		applyAt(t, s, account, testNow, CycleChanges{
			Processes: []Process{
				{ProcedureID: 9001, Number: "1", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC", AssignedUser: "fulano.tal", AssignedUserID: 101, ReturnDate: "2026-11-06"},
				{ProcedureID: 9004, Number: "4", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC", AssignedUser: "FULANO.TAL"},
			},
			Left: []int64{9003},
		})
		applyAt(t, s, account, testNow.Add(time.Minute), CycleChanges{Processes: []Process{
			{ProcedureID: 9002, Number: "2", UnitID: 1002, UnitAcronym: "SEDEMO/GAB", AssignedUser: "beltrano", AssignedUserID: 202, ReturnDate: "2026-11-01", ReturnOverdue: true},
		}})

		tests := []struct {
			name   string
			filter ProcessFilter
			want   []int64
		}{
			{"present processes, latest first", ProcessFilter{}, []int64{9002, 9004, 9001}},
			{"unit, ignoring case", ProcessFilter{Unit: "sedemo/gab"}, []int64{9002}},
			{"assigned to the user by id or acronym", ProcessFilter{AssignedUserID: 101, AssignedAcronym: "fulano.tal"}, []int64{9004, 9001}},
			{"scheduled return", ProcessFilter{ReturnScheduled: true}, []int64{9002, 9001}},
			{"overdue return", ProcessFilter{ReturnOverdue: true}, []int64{9002}},
			{"including the ones that left", ProcessFilter{IncludeLeft: true}, []int64{9002, 9004, 9001, 9003}},
		}
		for _, tt := range tests {
			tt.filter.AccountID, tt.filter.Limit = account.ID, 50
			processes, err := s.ListProcesses(t.Context(), tt.filter)
			if err != nil {
				t.Fatal(err)
			}
			if got := procedureIDs(processes); !slices.Equal(got, tt.want) {
				t.Errorf("%s: processes = %v, want %v", tt.name, got, tt.want)
			}
		}
	})
}

func TestListProcessesPagesWithTheKey(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		account := newAccount(t, s)
		var processes []Process
		for id := int64(9001); id <= 9005; id++ {
			processes = append(processes, Process{ProcedureID: id, Number: fmt.Sprint(id), UnitID: 1001})
		}
		applyAt(t, s, account, testNow, CycleChanges{Processes: processes})

		var seen []int64
		filter := ProcessFilter{AccountID: account.ID, Limit: 2}
		for range 5 {
			page, err := s.ListProcesses(t.Context(), filter)
			if err != nil {
				t.Fatal(err)
			}
			if len(page) == 0 {
				break
			}
			seen = append(seen, procedureIDs(page)...)
			last := page[len(page)-1]
			filter.After = &ProcessKey{LastSeenAt: last.LastSeenAt, ProcedureID: last.ProcedureID}
		}

		if !slices.Equal(seen, []int64{9005, 9004, 9003, 9002, 9001}) {
			t.Errorf("pages = %v", seen)
		}
	})
}

func TestProcessByIDAndDocuments(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		account := newAccount(t, s)
		applyAt(t, s, account, testNow, CycleChanges{
			Processes: []Process{{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}},
			Documents: []Document{
				{DocumentID: 80002, ProcedureID: 9001, Protocol: "0012346", Type: "Despacho", Name: "Despacho 2"},
				{DocumentID: 80001, ProcedureID: 9001, Protocol: "0012345", Type: "Ofício", Name: "Ofício 1"},
			},
		})

		process, err := s.ProcessByID(t.Context(), account.ID, 9001)
		if err != nil || process.Number != "00000.000001/2026-00" {
			t.Errorf("process = %+v, %v", process, err)
		}
		for _, missing := range [][2]int64{{account.ID, 9999}, {account.ID + 1, 9001}} {
			if _, err := s.ProcessByID(t.Context(), missing[0], missing[1]); !errors.Is(err, ErrNotFound) {
				t.Errorf("ProcessByID(%v) error = %v, want ErrNotFound", missing, err)
			}
		}
		documents, err := s.Documents(t.Context(), account.ID, 9001)
		want := []Document{
			{DocumentID: 80001, ProcedureID: 9001, Protocol: "0012345", Type: "Ofício", Name: "Ofício 1", FirstSeenAt: micro(testNow)},
			{DocumentID: 80002, ProcedureID: 9001, Protocol: "0012346", Type: "Despacho", Name: "Despacho 2", FirstSeenAt: micro(testNow)},
		}
		if err != nil || !slices.Equal(documents, want) {
			t.Errorf("documents = %+v, %v", documents, err)
		}
	})
}

func activityAt(ref string, procedureID int64, occurred time.Time) event.Event {
	return event.Event{Kind: event.ActivityAdded, ProcedureID: procedureID, Ref: ref, OccurredAt: occurred}
}

func refs(events []event.Event) []string {
	result := make([]string, 0, len(events))
	for _, ev := range events {
		result = append(result, ev.Ref)
	}
	return result
}

func TestListEventsFilters(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		account := newAccount(t, s)
		applyAt(t, s, account, testNow, CycleChanges{Events: []event.Event{
			activityAt("a1", 9001, testNow.Add(-3*time.Hour)),
			activityAt("a2", 9002, testNow.Add(-2*time.Hour)),
			{Kind: event.DocumentAdded, ProcedureID: 9001, Ref: "d1", OccurredAt: testNow.Add(-time.Hour)},
			{Kind: event.ProcessLeft, ProcedureID: 9002, Ref: "l1", OccurredAt: testNow},
		}})

		tests := []struct {
			name   string
			filter EventFilter
			want   []string
		}{
			{"newest first", EventFilter{}, []string{"l1", "d1", "a2", "a1"}},
			{"some kinds", EventFilter{Kinds: []event.Kind{event.DocumentAdded, event.ProcessLeft}}, []string{"l1", "d1"}},
			{"one process", EventFilter{ProcedureID: 9001}, []string{"d1", "a1"}},
			{"time window", EventFilter{Since: testNow.Add(-2 * time.Hour), Until: testNow}, []string{"d1", "a2"}},
			{"other account", EventFilter{AccountID: account.ID + 1}, []string{}},
		}
		for _, tt := range tests {
			if tt.filter.AccountID == 0 {
				tt.filter.AccountID = account.ID
			}
			tt.filter.Limit = 50
			events, err := s.ListEvents(t.Context(), tt.filter)
			if err != nil {
				t.Fatal(err)
			}
			if got := refs(events); !slices.Equal(got, tt.want) {
				t.Errorf("%s: events = %v, want %v", tt.name, got, tt.want)
			}
		}
	})
}

func TestListEventsPagesWithoutRepeatsOrGapsWhileEventsArrive(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		account := newAccount(t, s)
		var initial []event.Event
		for i := range 7 {
			initial = append(initial, activityAt(fmt.Sprintf("old%d", i), 9001, testNow.Add(-time.Duration(i)*time.Minute)))
		}
		initial = append(initial, activityAt("tie", 9002, testNow.Add(-3*time.Minute)))
		applyAt(t, s, account, testNow, CycleChanges{Events: initial})

		var seen []string
		filter := EventFilter{AccountID: account.ID, Limit: 3}
		for page := range 10 {
			events, err := s.ListEvents(t.Context(), filter)
			if err != nil {
				t.Fatal(err)
			}
			if len(events) == 0 {
				break
			}
			seen = append(seen, refs(events)...)
			last := events[len(events)-1]
			filter.After = &EventKey{OccurredAt: last.OccurredAt, ID: last.ID}
			if page == 0 {
				applyAt(t, s, account, testNow.Add(time.Minute), CycleChanges{Events: []event.Event{
					activityAt("newest", 9001, testNow.Add(time.Minute)),
					activityAt("late", 9002, testNow.Add(-90*time.Second)),
				}})
			}
		}

		for _, ref := range refs(initial) {
			if count := countOf(seen, ref); count != 1 {
				t.Errorf("event %s seen %d times in %v", ref, count, seen)
			}
		}
		if countOf(seen, "newest") != 0 {
			t.Errorf("an event newer than the first page showed up later: %v", seen)
		}
	})
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

func TestListDeliveriesAndCounts(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)
		applyAt(t, s, account, testNow, CycleChanges{
			Events:   []event.Event{activityAt("a1", 9001, testNow), activityAt("a2", 9001, testNow)},
			Channels: []string{"telegram", "webhook"},
		})

		all, err := s.ListDeliveries(ctx, DeliveryFilter{AccountID: account.ID, Limit: 50})
		if err != nil {
			t.Fatal(err)
		}
		if len(all) != 4 || all[0].Channel != "telegram" || all[1].Channel != "webhook" || all[0].EventID <= all[2].EventID {
			t.Fatalf("deliveries = %+v", all)
		}
		if first := all[0]; first.Status != "pending" || first.Attempts != 0 || !first.NextAttemptAt.Equal(micro(testNow)) || !first.SentAt.IsZero() {
			t.Errorf("delivery = %+v", first)
		}

		webhook, err := s.ListDeliveries(ctx, DeliveryFilter{AccountID: account.ID, Channel: "webhook", Status: "pending", Limit: 50})
		if err != nil || len(webhook) != 2 {
			t.Errorf("webhook deliveries = %+v, %v", webhook, err)
		}
		next, err := s.ListDeliveries(ctx, DeliveryFilter{AccountID: account.ID, Limit: 50, After: &DeliveryKey{EventID: all[0].EventID, Channel: all[0].Channel}})
		if err != nil || len(next) != 3 || next[0] != all[1] {
			t.Errorf("deliveries after the first = %+v, %v", next, err)
		}

		counts, err := s.DeliveryCounts(ctx, account.ID)
		want := map[string]map[string]int{"telegram": {"pending": 2}, "webhook": {"pending": 2}}
		if err != nil || fmt.Sprint(counts) != fmt.Sprint(want) {
			t.Errorf("counts = %v, %v", counts, err)
		}
	})
}
