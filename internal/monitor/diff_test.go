package monitor

import (
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/store"
)

var (
	diffNow  = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)
	nuproc   = sei.Unit{ID: 1001, Acronym: "SEDEMO/NUPROC"}
	self     = Self{UserID: 101, Acronym: "fulano.tal"}
	fulano   = &sei.User{ID: 101, Acronym: "fulano.tal", Name: "Fulano de Tal"}
	beltrano = &sei.User{ID: 202, Acronym: "beltrano.silva", Name: "Beltrano da Silva"}
)

func rules(baseline bool) Rules {
	location, _ := time.LoadLocation("America/Boa_Vista")
	return Rules{Now: diffNow, Location: location, ReturnWarningDays: 2, Self: self, Baseline: baseline}
}

func observed(id int64, change func(*sei.Process)) Observation {
	process := sei.Process{ProcedureID: id, Number: "00000.00000" + string(rune('0'+id%10)) + "/2026-00", Type: "Contrato"}
	if change != nil {
		change(&process)
	}
	return Observation{Process: process, Unit: nuproc}
}

func stored(id int64, change func(*store.Process)) store.Process {
	process := store.Process{ProcedureID: id, Number: "00000.00000" + string(rune('0'+id%10)) + "/2026-00", UnitID: nuproc.ID, UnitAcronym: nuproc.Acronym, LastActivityID: 500}
	if change != nil {
		change(&process)
	}
	return process
}

func date(text string) time.Time {
	parsed, _ := time.Parse("2006-01-02", text)
	return parsed
}

type kindRef struct {
	kind event.Kind
	ref  string
}

func kindsAndRefs(events []event.Event) []kindRef {
	result := make([]kindRef, 0, len(events))
	for _, ev := range events {
		result = append(result, kindRef{ev.Kind, ev.Ref})
	}
	return result
}

func TestDiffProcesses(t *testing.T) {
	cycle := "2026-11-04T18:00:00Z"
	tests := []struct {
		name     string
		previous []store.Process
		current  []Observation
		baseline bool
		want     []kindRef
		left     []int64
	}{
		{
			name:     "first cycle is a silent baseline",
			current:  []Observation{observed(1, nil), observed(2, func(p *sei.Process) { p.ReturnDate = date("2026-11-05") })},
			baseline: true,
		},
		{
			name:     "a new process only enters",
			previous: []store.Process{stored(1, nil)},
			current:  []Observation{observed(1, nil), observed(2, func(p *sei.Process) { p.ReturnDate, p.Assignee = date("2026-11-05"), beltrano })},
			want:     []kindRef{{event.ProcessEntered, "2:" + cycle}},
		},
		{
			name:     "a missing process leaves",
			previous: []store.Process{stored(1, nil), stored(2, nil)},
			current:  []Observation{observed(1, nil)},
			want:     []kindRef{{event.ProcessLeft, "2:" + cycle}},
			left:     []int64{2},
		},
		{
			name:     "a process that left and came back enters again",
			previous: []store.Process{stored(2, func(p *store.Process) { p.LeftAt = diffNow.Add(-time.Hour) })},
			current:  []Observation{observed(2, nil)},
			want:     []kindRef{{event.ProcessEntered, "2:" + cycle}},
		},
		{
			name:     "a process that already left does not leave twice",
			previous: []store.Process{stored(2, func(p *store.Process) { p.LeftAt = diffNow.Add(-time.Hour) })},
		},
		{
			name:     "assignment to the user",
			previous: []store.Process{stored(1, nil)},
			current:  []Observation{observed(1, func(p *sei.Process) { p.Assignee = fulano })},
			want:     []kindRef{{event.AssignmentChanged, "1:" + cycle}},
		},
		{
			name:     "assignment removed",
			previous: []store.Process{stored(1, func(p *store.Process) { p.AssignedUser, p.AssignedUserID = "beltrano.silva", 202 })},
			current:  []Observation{observed(1, nil)},
			want:     []kindRef{{event.AssignmentChanged, "1:" + cycle}},
		},
		{
			name:     "same assignee written with other letters is no change",
			previous: []store.Process{stored(1, func(p *store.Process) { p.AssignedUser, p.AssignedUserID = "Beltrano.Silva", 202 })},
			current:  []Observation{observed(1, func(p *sei.Process) { p.Assignee = beltrano })},
		},
		{
			name:     "return due within the warning window",
			previous: []store.Process{stored(1, nil)},
			current:  []Observation{observed(1, func(p *sei.Process) { p.ReturnDate = date("2026-11-06") })},
			want:     []kindRef{{event.ReturnDueSoon, "1:2026-11-06"}},
		},
		{
			name:     "return beyond the warning window",
			previous: []store.Process{stored(1, nil)},
			current:  []Observation{observed(1, func(p *sei.Process) { p.ReturnDate = date("2026-11-07") })},
		},
		{
			name:     "return that just became overdue",
			previous: []store.Process{stored(1, func(p *store.Process) { p.ReturnDate = "2026-11-03" })},
			current:  []Observation{observed(1, func(p *sei.Process) { p.ReturnDate, p.ReturnOverdue = date("2026-11-03"), true })},
			want:     []kindRef{{event.ReturnOverdue, "1:2026-11-03"}},
		},
		{
			name:     "return still overdue",
			previous: []store.Process{stored(1, func(p *store.Process) { p.ReturnDate, p.ReturnOverdue = "2026-11-03", true })},
			current:  []Observation{observed(1, func(p *sei.Process) { p.ReturnDate, p.ReturnOverdue = date("2026-11-03"), true })},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			changes := DiffProcesses(tt.previous, tt.current, rules(tt.baseline))

			if got := kindsAndRefs(changes.Events); !slices.Equal(got, tt.want) {
				t.Errorf("events = %v, want %v", got, tt.want)
			}
			var left []int64
			for _, process := range changes.Left {
				left = append(left, process.ProcedureID)
			}
			if !slices.Equal(left, tt.left) {
				t.Errorf("left = %v, want %v", left, tt.left)
			}
			if len(changes.Present) != len(tt.current) {
				t.Errorf("present = %d, want %d", len(changes.Present), len(tt.current))
			}
		})
	}
}

func TestDiffPayloads(t *testing.T) {
	previous := []store.Process{stored(1, func(p *store.Process) { p.AssignedUser, p.AssignedUserID = "beltrano.silva", 202 })}
	current := []Observation{
		observed(1, func(p *sei.Process) { p.Assignee = fulano }),
		observed(2, func(p *sei.Process) { p.Assignee, p.Description = fulano, "Compra de cadeiras" }),
	}

	events := DiffProcesses(previous, current, rules(false)).Events

	assignment, entered := events[0].Payload, events[1].Payload
	if !assignment.AssignedToSelf || assignment.AssignedUser != "fulano.tal" || assignment.PreviousAssignee != "beltrano.silva" || assignment.Unit != "SEDEMO/NUPROC" {
		t.Errorf("assignment payload = %+v", assignment)
	}
	if !entered.AssignedToSelf || entered.Description != "Compra de cadeiras" || entered.ProcessType != "Contrato" {
		t.Errorf("entered payload = %+v", entered)
	}
}

func TestDuplicateProcessesAcrossUnitsCountOnce(t *testing.T) {
	gab := sei.Unit{ID: 1002, Acronym: "SEDEMO/GAB"}
	current := []Observation{observed(1, nil), {Process: observed(1, nil).Process, Unit: gab}}

	changes := DiffProcesses(nil, current, rules(true))

	if len(changes.Present) != 1 || changes.Present[0].Unit != nuproc {
		t.Errorf("present = %+v, want the process once, in the first unit", changes.Present)
	}
}

func TestFreshActivities(t *testing.T) {
	page := []sei.Activity{{ID: 507}, {ID: 506}, {ID: 505}, {ID: 503}}

	fresh, reached := FreshActivities(505, page)
	if !reached || len(fresh) != 2 || fresh[0].ID != 507 || fresh[1].ID != 506 {
		t.Errorf("fresh = %+v, reached %v", fresh, reached)
	}
	fresh, reached = FreshActivities(400, page)
	if reached || len(fresh) != 4 {
		t.Errorf("all fresh = %+v, reached %v", fresh, reached)
	}
}

func TestFreshDocuments(t *testing.T) {
	documents := []sei.Document{{ID: 801}, {ID: 802}, {ID: 803}}

	fresh := FreshDocuments(map[int64]bool{801: true, 803: true}, documents)

	if len(fresh) != 1 || fresh[0].ID != 802 {
		t.Errorf("fresh = %+v", fresh)
	}
}
