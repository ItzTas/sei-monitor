package processes_test

import (
	"errors"
	"fmt"
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/modules/processes"
	"github.com/ItzTas/sei-monitor/internal/monitor"
)

var now = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

type fixture struct {
	repo    *processes.Postgres
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
	return fixture{repo: processes.NewPostgres(database.Pool), cycles: cycles, account: account}
}

func (f fixture) apply(t *testing.T, at time.Time, changes monitor.CycleChanges) {
	t.Helper()
	changes.AccountID, changes.Now = f.account.ID, at
	if _, err := f.cycles.ApplyCycle(t.Context(), changes); err != nil {
		t.Fatal(err)
	}
}

func procedureIDs(list []core.Process) []int64 {
	return core.Map(list, func(p core.Process) int64 { return p.ProcedureID })
}

func TestListProcessesFilters(t *testing.T) {
	f := newFixture(t)
	f.apply(t, now.Add(-time.Hour), monitor.CycleChanges{Processes: []core.Process{
		{ProcedureID: 9003, Number: "3", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC"},
	}})
	f.apply(t, now, monitor.CycleChanges{
		Processes: []core.Process{
			{ProcedureID: 9001, Number: "1", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC", AssignedUser: "fulano.tal", AssignedUserID: 101, ReturnDate: "2026-11-06"},
			{ProcedureID: 9004, Number: "4", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC", AssignedUser: "FULANO.TAL"},
		},
		Left: []int64{9003},
	})
	f.apply(t, now.Add(time.Minute), monitor.CycleChanges{Processes: []core.Process{
		{ProcedureID: 9002, Number: "2", UnitID: 1002, UnitAcronym: "SEDEMO/GAB", AssignedUser: "beltrano", AssignedUserID: 202, ReturnDate: "2026-11-01", ReturnOverdue: true},
	}})

	tests := []struct {
		name   string
		filter processes.Filter
		want   []int64
	}{
		{"present processes, latest first", processes.Filter{}, []int64{9002, 9004, 9001}},
		{"unit, ignoring case", processes.Filter{Unit: "sedemo/gab"}, []int64{9002}},
		{"assigned to the user by id or acronym", processes.Filter{AssignedUserID: 101, AssignedAcronym: "fulano.tal"}, []int64{9004, 9001}},
		{"scheduled return", processes.Filter{ReturnScheduled: true}, []int64{9002, 9001}},
		{"overdue return", processes.Filter{ReturnOverdue: true}, []int64{9002}},
		{"including the ones that left", processes.Filter{IncludeLeft: true}, []int64{9002, 9004, 9001, 9003}},
		{"other account", processes.Filter{AccountID: f.account.ID + 1}, []int64{}},
	}
	for _, tt := range tests {
		if tt.filter.AccountID == 0 {
			tt.filter.AccountID = f.account.ID
		}
		tt.filter.Limit = 50
		list, err := f.repo.ListProcesses(t.Context(), tt.filter)
		if err != nil {
			t.Fatal(err)
		}
		if got := procedureIDs(list); !slices.Equal(got, tt.want) {
			t.Errorf("%s: processes = %v, want %v", tt.name, got, tt.want)
		}
	}
}

func TestListProcessesPagesWithTheKey(t *testing.T) {
	f := newFixture(t)
	var present []core.Process
	for id := int64(9001); id <= 9005; id++ {
		present = append(present, core.Process{ProcedureID: id, Number: fmt.Sprint(id), UnitID: 1001})
	}
	f.apply(t, now, monitor.CycleChanges{Processes: present})

	var seen []int64
	filter := processes.Filter{AccountID: f.account.ID, Limit: 2}
	for range 5 {
		page, err := f.repo.ListProcesses(t.Context(), filter)
		if err != nil {
			t.Fatal(err)
		}
		if len(page) == 0 {
			break
		}
		seen = append(seen, procedureIDs(page)...)
		last := page[len(page)-1]
		filter.After = &processes.Key{LastSeenAt: last.LastSeenAt, ProcedureID: last.ProcedureID}
	}

	if !slices.Equal(seen, []int64{9005, 9004, 9003, 9002, 9001}) {
		t.Errorf("pages = %v", seen)
	}
}

func TestProcessByIDDocumentsAndEvents(t *testing.T) {
	f := newFixture(t)
	f.apply(t, now, monitor.CycleChanges{
		Processes: []core.Process{
			{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001},
			{ProcedureID: 9002, Number: "00000.000002/2026-00", UnitID: 1001},
		},
		Documents: []core.Document{
			{DocumentID: 80002, ProcedureID: 9001, Protocol: "0012346", Type: "Despacho", Name: "Despacho 2"},
			{DocumentID: 80001, ProcedureID: 9001, Protocol: "0012345", Type: "Ofício", Name: "Ofício 1"},
			{DocumentID: 80003, ProcedureID: 9002, Protocol: "0012347"},
		},
		Events: []event.Event{
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "a1", OccurredAt: now.Add(-3 * time.Hour)},
			{Kind: event.ActivityAdded, ProcedureID: 9002, Ref: "a2", OccurredAt: now.Add(-2 * time.Hour)},
			{Kind: event.DocumentAdded, ProcedureID: 9001, Ref: "d1", OccurredAt: now.Add(-time.Hour)},
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "a3", OccurredAt: now},
		},
	})

	process, err := f.repo.ProcessByID(t.Context(), f.account.ID, 9001)
	if err != nil || process.Number != "00000.000001/2026-00" || !process.FirstSeenAt.Equal(now) {
		t.Errorf("process = %+v, %v", process, err)
	}
	for _, missing := range [][2]int64{{f.account.ID, 9999}, {f.account.ID + 1, 9001}} {
		if _, err := f.repo.ProcessByID(t.Context(), missing[0], missing[1]); !errors.Is(err, processes.ErrNotFound) {
			t.Errorf("ProcessByID(%v) error = %v, want ErrNotFound", missing, err)
		}
	}
	documents, err := f.repo.Documents(t.Context(), f.account.ID, 9001)
	want := []core.Document{
		{DocumentID: 80001, ProcedureID: 9001, Protocol: "0012345", Type: "Ofício", Name: "Ofício 1", FirstSeenAt: now},
		{DocumentID: 80002, ProcedureID: 9001, Protocol: "0012346", Type: "Despacho", Name: "Despacho 2", FirstSeenAt: now},
	}
	if err != nil || !slices.Equal(documents, want) {
		t.Errorf("documents = %+v, %v", documents, err)
	}
	events, err := f.repo.ProcessEvents(t.Context(), f.account.ID, 9001, 2)
	refs := core.Map(events, func(ev event.Event) string { return ev.Ref })
	if err != nil || !slices.Equal(refs, []string{"a3", "d1"}) {
		t.Errorf("process events = %v, %v, want the two newest of 9001", refs, err)
	}
}
