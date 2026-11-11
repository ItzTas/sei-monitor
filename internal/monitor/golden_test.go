package monitor

import (
	"errors"
	"fmt"
	"log/slog"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/goldentest"
	"github.com/ItzTas/sei-monitor/internal/goldentest/seisetup"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

func TestGoldenVectors(t *testing.T) {
	if !goldentest.Pinned(t) {
		return
	}
	goldentest.Golden(t, "days_until.json", daysUntilVectors(t))
	goldentest.Golden(t, "diff.json", diffVectors(t))
	goldentest.Golden(t, "monitor_text.json", monitorTextVectors(t))
	t.Run("cycle", func(t *testing.T) {
		goldentest.Golden(t, "cycle.json", cycleVectors(t))
	})
}

func stamp(t time.Time) string {
	return t.UTC().Format(time.RFC3339Nano)
}

func zone(t *testing.T, name string) *time.Location {
	t.Helper()
	location, err := time.LoadLocation(name)
	if err != nil {
		t.Fatalf("load zone %s: %v", name, err)
	}
	return location
}

type daysUntilCase struct {
	Now  string `json:"now"`
	Date string `json:"date"`
	Zone string `json:"zone"`
	Days int    `json:"days"`
}

func daysUntilVectors(t *testing.T) []daysUntilCase {
	t.Helper()
	type moment struct {
		zone        string
		local       [6]int
		nanoseconds int
	}
	type group struct {
		moments []moment
		dates   []string
	}
	everyday := []string{"2025-11-04", "2026-11-03", "2026-11-04", "2026-11-05", "2026-11-06", "2026-12-31", "2027-01-01", "2099-11-04"}
	groups := []group{
		{
			moments: []moment{
				{"America/Boa_Vista", [6]int{2026, 11, 4, 23, 59, 59}, 0},
				{"America/Boa_Vista", [6]int{2026, 11, 4, 23, 59, 59}, 999999999},
				{"America/Boa_Vista", [6]int{2026, 11, 5, 0, 0, 0}, 0},
				{"America/Sao_Paulo", [6]int{2026, 11, 4, 23, 59, 59}, 0},
				{"America/Sao_Paulo", [6]int{2026, 11, 5, 0, 0, 0}, 0},
				{"UTC", [6]int{2026, 11, 4, 23, 59, 59}, 0},
				{"UTC", [6]int{2026, 11, 5, 0, 0, 0}, 0},
			},
			dates: everyday,
		},
		{
			moments: []moment{
				{"America/Sao_Paulo", [6]int{2018, 11, 3, 23, 59, 59}, 0},
				{"America/Sao_Paulo", [6]int{2018, 11, 4, 1, 0, 0}, 0},
				{"America/Sao_Paulo", [6]int{2018, 11, 4, 23, 59, 59}, 0},
				{"America/Boa_Vista", [6]int{2018, 11, 4, 0, 0, 0}, 0},
			},
			dates: []string{"2018-11-03", "2018-11-04", "2018-11-05"},
		},
		{
			moments: []moment{
				{"America/Sao_Paulo", [6]int{2019, 2, 16, 23, 30, 0}, 0},
			},
			dates: []string{"2019-02-16", "2019-02-17"},
		},
	}
	var cases []daysUntilCase
	for _, g := range groups {
		for _, m := range g.moments {
			location := zone(t, m.zone)
			now := time.Date(m.local[0], time.Month(m.local[1]), m.local[2], m.local[3], m.local[4], m.local[5], m.nanoseconds, location)
			for _, text := range g.dates {
				cases = append(cases, daysUntilCase{Now: stamp(now), Date: text, Zone: m.zone, Days: daysUntil(now, date(text), location)})
			}
		}
	}
	overlap := time.Date(2019, 2, 17, 2, 30, 0, 0, time.UTC)
	for _, text := range []string{"2019-02-16", "2019-02-17"} {
		cases = append(cases, daysUntilCase{Now: stamp(overlap), Date: text, Zone: "America/Sao_Paulo", Days: daysUntil(overlap, date(text), zone(t, "America/Sao_Paulo"))})
	}
	return cases
}

type goldenStored struct {
	ProcedureID    int64   `json:"procedure_id"`
	Number         string  `json:"number"`
	Type           string  `json:"type"`
	Description    string  `json:"description"`
	UnitID         int64   `json:"unit_id"`
	UnitAcronym    string  `json:"unit_acronym"`
	AssignedUser   string  `json:"assigned_user"`
	AssignedUserID int64   `json:"assigned_user_id"`
	ReturnDate     string  `json:"return_date"`
	ReturnOverdue  bool    `json:"return_overdue"`
	LastActivityID int64   `json:"last_activity_id"`
	DocumentTotal  int     `json:"document_total"`
	FirstSeenAt    string  `json:"first_seen_at"`
	LastSeenAt     string  `json:"last_seen_at"`
	LeftAt         *string `json:"left_at"`
}

type goldenUser struct {
	ID      int64  `json:"id"`
	Acronym string `json:"acronym"`
	Name    string `json:"name"`
}

type goldenProcess struct {
	ProcedureID   int64       `json:"procedure_id"`
	Number        string      `json:"number"`
	Type          string      `json:"type"`
	Description   string      `json:"description"`
	Assignee      *goldenUser `json:"assignee"`
	ReturnDate    string      `json:"return_date"`
	ReturnOverdue bool        `json:"return_overdue"`
	Origin        string      `json:"origin"`
}

type goldenUnit struct {
	ID      int64  `json:"id"`
	Acronym string `json:"acronym"`
}

type goldenObservation struct {
	Process goldenProcess `json:"process"`
	Unit    goldenUnit    `json:"unit"`
}

type goldenSelf struct {
	UserID  int64  `json:"user_id"`
	Acronym string `json:"acronym"`
}

type goldenRules struct {
	Now               string     `json:"now"`
	Zone              string     `json:"zone"`
	ReturnWarningDays int        `json:"return_warning_days"`
	Self              goldenSelf `json:"self"`
	Baseline          bool       `json:"baseline"`
}

type goldenPresence struct {
	ProcedureID int64 `json:"procedure_id"`
	Entered     bool  `json:"entered"`
	Quiet       bool  `json:"quiet"`
}

type goldenDiffEvent struct {
	Kind        event.Kind    `json:"kind"`
	ProcedureID int64         `json:"procedure_id"`
	Ref         string        `json:"ref"`
	OccurredAt  string        `json:"occurred_at"`
	Payload     event.Payload `json:"payload"`
}

type goldenDiffResult struct {
	Present []goldenPresence  `json:"present"`
	Left    []int64           `json:"left"`
	Events  []goldenDiffEvent `json:"events"`
}

type diffCase struct {
	Name     string              `json:"name"`
	Previous []goldenStored      `json:"previous"`
	Current  []goldenObservation `json:"current"`
	Rules    goldenRules         `json:"rules"`
	Result   goldenDiffResult    `json:"result"`
}

type diffInput struct {
	name     string
	previous []core.Process
	current  []Observation
	now      time.Time
	zone     string
	warning  int
	self     Self
	baseline bool
}

func storedRecord(process core.Process) goldenStored {
	record := goldenStored{
		ProcedureID: process.ProcedureID, Number: process.Number, Type: process.Type, Description: process.Description,
		UnitID: process.UnitID, UnitAcronym: process.UnitAcronym, AssignedUser: process.AssignedUser, AssignedUserID: process.AssignedUserID,
		ReturnDate: process.ReturnDate, ReturnOverdue: process.ReturnOverdue, LastActivityID: process.LastActivityID,
		DocumentTotal: process.DocumentTotal, FirstSeenAt: stamp(process.FirstSeenAt), LastSeenAt: stamp(process.LastSeenAt),
	}
	if !process.LeftAt.IsZero() {
		left := stamp(process.LeftAt)
		record.LeftAt = &left
	}
	return record
}

func processRecord(process sei.Process) goldenProcess {
	record := goldenProcess{
		ProcedureID: process.ProcedureID, Number: process.Number, Type: process.Type, Description: process.Description,
		ReturnOverdue: process.ReturnOverdue, Origin: process.Origin,
	}
	if process.Assignee != nil {
		record.Assignee = &goldenUser{ID: process.Assignee.ID, Acronym: process.Assignee.Acronym, Name: process.Assignee.Name}
	}
	if !process.ReturnDate.IsZero() {
		record.ReturnDate = process.ReturnDate.Format(time.DateOnly)
	}
	return record
}

func diffEventRecord(ev event.Event) goldenDiffEvent {
	return goldenDiffEvent{Kind: ev.Kind, ProcedureID: ev.ProcedureID, Ref: ev.Ref, OccurredAt: stamp(ev.OccurredAt), Payload: ev.Payload}
}

func diffVectors(t *testing.T) []diffCase {
	t.Helper()
	inputs := diffInputs()
	cases := make([]diffCase, len(inputs))
	for i, input := range inputs {
		cases[i] = diffEntry(t, input)
	}
	return cases
}

func diffEntry(t *testing.T, input diffInput) diffCase {
	t.Helper()
	rules := Rules{Now: input.now, Location: zone(t, input.zone), ReturnWarningDays: input.warning, Self: input.self, Baseline: input.baseline}
	entry := diffCase{
		Name:     input.name,
		Previous: make([]goldenStored, 0, len(input.previous)),
		Current:  make([]goldenObservation, 0, len(input.current)),
		Rules: goldenRules{
			Now: stamp(input.now), Zone: input.zone, ReturnWarningDays: input.warning,
			Self: goldenSelf{UserID: input.self.UserID, Acronym: input.self.Acronym}, Baseline: input.baseline,
		},
		Result: goldenDiffResult{Present: []goldenPresence{}, Left: []int64{}, Events: []goldenDiffEvent{}},
	}
	for _, process := range input.previous {
		entry.Previous = append(entry.Previous, storedRecord(process))
	}
	for _, observation := range input.current {
		entry.Current = append(entry.Current, goldenObservation{
			Process: processRecord(observation.Process),
			Unit:    goldenUnit{ID: observation.Unit.ID, Acronym: observation.Unit.Acronym},
		})
	}
	changes := DiffProcesses(input.previous, input.current, rules)
	for _, presence := range changes.Present {
		entry.Result.Present = append(entry.Result.Present, goldenPresence{
			ProcedureID: presence.Process.ProcedureID, Entered: presence.Entered, Quiet: presence.quiet(input.baseline),
		})
	}
	for _, process := range changes.Left {
		entry.Result.Left = append(entry.Result.Left, process.ProcedureID)
	}
	for _, ev := range changes.Events {
		entry.Result.Events = append(entry.Result.Events, diffEventRecord(ev))
	}
	return entry
}

func seen(id int64, change func(*core.Process)) core.Process {
	return stored(id, func(p *core.Process) {
		p.Type = "Contrato"
		p.FirstSeenAt, p.LastSeenAt = diffNow.Add(-48*time.Hour), diffNow.Add(-15*time.Minute)
		if change != nil {
			change(p)
		}
	})
}

func assigned(user *sei.User) func(*sei.Process) {
	return func(p *sei.Process) { p.Assignee = user }
}

func returning(text string, overdue bool) func(*sei.Process) {
	return func(p *sei.Process) { p.ReturnDate, p.ReturnOverdue = date(text), overdue }
}

func described(description string) func(*core.Process) {
	return func(p *core.Process) { p.Description = description }
}

func storedAssignee(acronym string, id int64) func(*core.Process) {
	return func(p *core.Process) { p.AssignedUser, p.AssignedUserID = acronym, id }
}

func diffInputs() []diffInput {
	boaVista := "America/Boa_Vista"
	standard := func(name string, previous []core.Process, current []Observation, baseline bool) diffInput {
		return diffInput{name: name, previous: previous, current: current, now: diffNow, zone: boaVista, warning: 2, self: self, baseline: baseline}
	}
	gab := sei.Unit{ID: 1002, Acronym: "SEDEMO/GAB"}
	inGab := func(observation Observation) Observation {
		observation.Unit = gab
		return observation
	}
	inputs := []diffInput{
		standard("first cycle is a silent baseline", nil, []Observation{observed(1, nil), observed(2, returning("2026-11-05", false))}, true),
		standard("a new process only enters", []core.Process{seen(1, nil)}, []Observation{observed(1, nil), observed(2, func(p *sei.Process) { p.ReturnDate, p.Assignee = date("2026-11-05"), beltrano })}, false),
		standard("a missing process leaves", []core.Process{seen(1, nil), seen(2, described("Compra de mesas"))}, []Observation{observed(1, nil)}, false),
		standard("a process that left and came back enters again", []core.Process{seen(2, func(p *core.Process) { p.LeftAt = diffNow.Add(-time.Hour) })}, []Observation{observed(2, nil)}, false),
		standard("a process that already left does not leave twice", []core.Process{seen(2, func(p *core.Process) { p.LeftAt = diffNow.Add(-time.Hour) })}, nil, false),
		standard("assignment to the user", []core.Process{seen(1, nil)}, []Observation{observed(1, assigned(fulano))}, false),
		standard("assignment removed", []core.Process{seen(1, storedAssignee("beltrano.silva", 202))}, []Observation{observed(1, nil)}, false),
		standard("same assignee written with other letters is no change", []core.Process{seen(1, storedAssignee("Beltrano.Silva", 202))}, []Observation{observed(1, assigned(beltrano))}, false),
		standard("return due within the warning window", []core.Process{seen(1, nil)}, []Observation{observed(1, returning("2026-11-06", false))}, false),
		standard("return beyond the warning window", []core.Process{seen(1, nil)}, []Observation{observed(1, returning("2026-11-07", false))}, false),
		standard("return that just became overdue", []core.Process{seen(1, func(p *core.Process) { p.ReturnDate = "2026-11-03" })}, []Observation{observed(1, returning("2026-11-03", true))}, false),
		standard("return still overdue", []core.Process{seen(1, func(p *core.Process) { p.ReturnDate, p.ReturnOverdue = "2026-11-03", true })}, []Observation{observed(1, returning("2026-11-03", true))}, false),
		standard("payloads of an assignment and an entry", []core.Process{seen(1, storedAssignee("beltrano.silva", 202))}, []Observation{
			observed(1, assigned(fulano)),
			observed(2, func(p *sei.Process) { p.Assignee, p.Description = fulano, "Compra de cadeiras" }),
		}, false),
		standard("a process in two units counts once, in the first unit", nil, []Observation{observed(1, nil), inGab(observed(1, nil))}, true),
		standard("a process entering in two units enters once", []core.Process{seen(2, nil)}, []Observation{observed(2, nil), observed(1, nil), inGab(observed(1, assigned(beltrano))), inGab(observed(2, nil))}, false),
		standard("empty previous and current state", nil, nil, false),
		standard("empty current state makes every process leave", []core.Process{seen(1, nil), seen(2, nil)}, nil, false),
		standard("previous order decides the order of the left events", []core.Process{seen(3, nil), seen(1, nil), seen(2, nil)}, nil, false),
		standard("assignee is the user by id with another acronym", []core.Process{seen(1, nil)}, []Observation{observed(1, assigned(&sei.User{ID: 101, Acronym: "outro.nome", Name: "Fulano de Tal"}))}, false),
		standard("assignee is the user by acronym in other letters when SEI sends no id", []core.Process{seen(1, nil)}, []Observation{observed(1, assigned(&sei.User{Acronym: "FULANO.TAL", Name: "Fulano de Tal"}))}, false),
		standard("assignee with the user's acronym and another id is not the user", []core.Process{seen(1, nil)}, []Observation{observed(1, assigned(&sei.User{ID: 303, Acronym: "fulano.tal", Name: "Fulano Homonimo"}))}, false),
		standard("assignee with the user's id and no acronym is not flagged on assignment", []core.Process{seen(1, storedAssignee("beltrano.silva", 202))}, []Observation{observed(1, assigned(&sei.User{ID: 101}))}, false),
		standard("entered process with the user's id and no acronym is flagged", nil, []Observation{observed(1, assigned(&sei.User{ID: 101}))}, false),
		standard("assignee without id or acronym replaces the previous one", []core.Process{seen(1, storedAssignee("beltrano.silva", 202))}, []Observation{observed(1, assigned(&sei.User{}))}, false),
		standard("assignee without id or acronym after nobody is no change", []core.Process{seen(1, nil)}, []Observation{observed(1, assigned(&sei.User{}))}, false),
		standard("previous assignee without id is compared by acronym", []core.Process{seen(1, storedAssignee("Beltrano.Silva", 0))}, []Observation{observed(1, assigned(beltrano))}, false),
		standard("previous assignee without id and another acronym", []core.Process{seen(1, storedAssignee("sicrano.souza", 0))}, []Observation{observed(1, assigned(beltrano))}, false),
		standard("same id with another acronym is no change", []core.Process{seen(1, storedAssignee("beltrano.antigo", 202))}, []Observation{observed(1, assigned(beltrano))}, false),
		standard("different ids with the same acronym is a change", []core.Process{seen(1, storedAssignee("beltrano.silva", 303))}, []Observation{observed(1, assigned(beltrano))}, false),
		standard("assignment from the user to someone else", []core.Process{seen(1, storedAssignee("fulano.tal", 101))}, []Observation{observed(1, assigned(beltrano))}, false),
		standard("overdue without a date uses the cycle ref", []core.Process{seen(1, nil)}, []Observation{observed(1, func(p *sei.Process) { p.ReturnOverdue = true })}, false),
		standard("return due today", []core.Process{seen(1, nil)}, []Observation{observed(1, returning("2026-11-04", false))}, false),
		standard("return date in the past without the overdue flag", []core.Process{seen(1, nil)}, []Observation{observed(1, returning("2026-11-03", false))}, false),
		standard("overdue and inside the warning window on the same process", []core.Process{seen(1, nil)}, []Observation{observed(1, returning("2026-11-05", true))}, false),
		standard("still overdue and inside the warning window", []core.Process{seen(1, func(p *core.Process) { p.ReturnDate, p.ReturnOverdue = "2026-11-05", true })}, []Observation{observed(1, returning("2026-11-05", true))}, false),
		standard("overdue flag cleared inside the warning window", []core.Process{seen(1, func(p *core.Process) { p.ReturnDate, p.ReturnOverdue = "2026-11-05", true })}, []Observation{observed(1, returning("2026-11-05", false))}, false),
		standard("process_entered suppresses the other checks", nil, []Observation{observed(1, func(p *sei.Process) {
			p.Assignee, p.ReturnDate, p.ReturnOverdue = fulano, date("2026-11-05"), true
		})}, false),
		standard("process_entered after leaving suppresses the other checks", []core.Process{seen(1, func(p *core.Process) {
			p.LeftAt, p.AssignedUser, p.AssignedUserID = diffNow.Add(-time.Hour), "beltrano.silva", 202
		})}, []Observation{observed(1, func(p *sei.Process) { p.Assignee, p.ReturnDate, p.ReturnOverdue = fulano, date("2026-11-04"), true })}, false),
		standard("baseline with changes creates no events", []core.Process{seen(1, storedAssignee("beltrano.silva", 202)), seen(2, nil)}, []Observation{
			observed(1, func(p *sei.Process) { p.Assignee, p.ReturnDate = fulano, date("2026-11-05") }),
			observed(3, nil),
		}, true),
		standard("a process without a seen activity is quiet", []core.Process{seen(1, func(p *core.Process) { p.LastActivityID = 0 })}, []Observation{observed(1, nil)}, false),
		standard("every kind at once", []core.Process{
			seen(1, storedAssignee("beltrano.silva", 202)),
			seen(2, described("Compra de mesas")),
			seen(3, func(p *core.Process) { p.ReturnDate = "2026-11-03" }),
			seen(4, nil),
			seen(5, func(p *core.Process) { p.LeftAt = diffNow.Add(-time.Hour) }),
			seen(6, func(p *core.Process) { p.LeftAt = diffNow.Add(-2 * time.Hour) }),
			seen(7, func(p *core.Process) { p.LastActivityID = 0 }),
			seen(8, described("Reforma do auditório")),
		}, []Observation{
			observed(9, assigned(beltrano)),
			observed(1, func(p *sei.Process) { p.Assignee, p.ReturnDate = fulano, date("2026-11-06") }),
			observed(3, returning("2026-11-03", true)),
			observed(4, returning("2026-11-05", false)),
			observed(5, nil),
			observed(7, nil),
			inGab(observed(10, returning("2026-11-04", true))),
			inGab(observed(9, nil)),
		}, false),
	}
	westOfUTC := time.Date(2026, 11, 5, 3, 30, 0, 0, time.UTC)
	window := []core.Process{seen(1, nil), seen(2, nil), seen(3, nil), seen(4, nil)}
	windowDates := []Observation{
		observed(1, returning("2026-11-03", false)),
		observed(2, returning("2026-11-04", false)),
		observed(3, returning("2026-11-06", false)),
		observed(4, returning("2026-11-07", false)),
	}
	withNanoseconds := time.Date(2026, 11, 4, 14, 0, 0, 999999999, time.FixedZone("UTC-4", -4*3600))
	withSeconds := time.Date(2026, 11, 4, 14, 7, 9, 123456789, time.FixedZone("UTC-3", -3*3600))
	refsPrevious := []core.Process{seen(1, nil), seen(2, nil), seen(3, storedAssignee("beltrano.silva", 202))}
	refsCurrent := []Observation{
		observed(4, nil),
		observed(1, func(p *sei.Process) { p.ReturnOverdue = true }),
		observed(3, assigned(fulano)),
	}
	return append(inputs,
		diffInput{name: "warning window boundaries at 23:30 in Boa Vista", previous: window, current: windowDates, now: westOfUTC, zone: boaVista, warning: 2, self: self},
		diffInput{name: "warning window boundaries at the same instant in UTC", previous: window, current: windowDates, now: westOfUTC, zone: "UTC", warning: 2, self: self},
		diffInput{name: "warning window of zero days", previous: window, current: windowDates, now: westOfUTC, zone: boaVista, warning: 0, self: self},
		diffInput{name: "now with nanoseconds, given at -04:00", previous: refsPrevious, current: refsCurrent, now: withNanoseconds, zone: boaVista, warning: 2, self: self},
		diffInput{name: "now with seconds and nanoseconds, given at -03:00", previous: refsPrevious, current: refsCurrent, now: withSeconds, zone: "America/Sao_Paulo", warning: 2, self: self},
		diffInput{name: "the user matched by acronym when the session has no id", previous: []core.Process{seen(1, nil)}, current: []Observation{
			observed(1, assigned(&sei.User{ID: 303, Acronym: "Fulano.Tal", Name: "Fulano de Tal"})),
		}, now: diffNow, zone: boaVista, warning: 2, self: Self{Acronym: "fulano.tal"}},
		diffInput{name: "nobody is the user before the first login", previous: []core.Process{seen(1, nil)}, current: []Observation{
			observed(1, assigned(fulano)), observed(2, assigned(&sei.User{Acronym: "fulano.tal"})),
		}, now: diffNow, zone: boaVista, warning: 2, self: Self{}},
		standard("assignment change and a newly overdue return on the same process", []core.Process{seen(1, func(p *core.Process) {
			p.AssignedUser, p.AssignedUserID, p.ReturnDate = "beltrano.silva", 202, "2026-11-03"
		})}, []Observation{observed(1, func(p *sei.Process) { p.Assignee, p.ReturnDate, p.ReturnOverdue = fulano, date("2026-11-03"), true })}, false),
	)
}

type goldenWait struct {
	PollIntervalNS int64 `json:"poll_interval_ns"`
	Failures       int   `json:"failures"`
	JitterLimitNS  int64 `json:"jitter_limit_ns"`
	WaitNS         int64 `json:"wait_ns"`
}

type goldenTooSoon struct {
	RetryAfterNS int64  `json:"retry_after_ns"`
	Text         string `json:"text"`
}

type goldenDescribe struct {
	Input  string `json:"input"`
	Output string `json:"output"`
}

type goldenAPIError struct {
	Route   string `json:"route"`
	Status  int    `json:"status"`
	Message string `json:"message"`
}

type goldenStoredEvent struct {
	Kind        event.Kind    `json:"kind"`
	ProcedureID int64         `json:"procedure_id"`
	Ref         string        `json:"ref"`
	OccurredAt  string        `json:"occurred_at"`
	DetectedAt  string        `json:"detected_at"`
	Payload     event.Payload `json:"payload"`
}

type goldenErrorLink struct {
	Depth    int     `json:"depth"`
	Type     string  `json:"type"`
	Message  string  `json:"message"`
	Sentinel *string `json:"sentinel"`
}

type goldenLoginFailure struct {
	Name     string            `json:"name"`
	Now      string            `json:"now"`
	APIError *goldenAPIError   `json:"api_error"`
	Error    string            `json:"error"`
	Chain    []goldenErrorLink `json:"chain"`
	Reason   string            `json:"reason"`
	Event    goldenStoredEvent `json:"event"`
}

type goldenSuccessWait struct {
	PollIntervalNS int64 `json:"poll_interval_ns"`
	FailuresBefore int   `json:"failures_before"`
	FailuresAfter  int   `json:"failures_after"`
	JitterLimitNS  int64 `json:"jitter_limit_ns"`
	JitterNS       int64 `json:"jitter_ns"`
	WaitNS         int64 `json:"wait_ns"`
}

type goldenErrors struct {
	Running  string `json:"running"`
	Paused   string `json:"paused"`
	NotReady string `json:"not_ready"`
}

type monitorText struct {
	Backoff      []goldenWait         `json:"backoff"`
	SuccessWait  []goldenSuccessWait  `json:"success_wait"`
	TooSoon      []goldenTooSoon      `json:"too_soon"`
	Describe     []goldenDescribe     `json:"describe"`
	LoginFailure []goldenLoginFailure `json:"login_failure"`
	Errors       goldenErrors         `json:"errors"`
}

func monitorTextVectors(t *testing.T) monitorText {
	t.Helper()
	return monitorText{
		Backoff:      backoffVectors(),
		SuccessWait:  successWaitVectors(),
		TooSoon:      tooSoonVectors(),
		Describe:     describeVectors(),
		LoginFailure: loginFailureVectors(t),
		Errors:       goldenErrors{Running: ErrRunning.Error(), Paused: ErrPaused.Error(), NotReady: ErrNotReady.Error()},
	}
}

func backoffVectors() []goldenWait {
	intervals := []struct {
		poll     time.Duration
		failures []int
	}{
		{5 * time.Minute, []int{1, 2, 3, 4, 5, 6, 7}},
		{15 * time.Minute, []int{1, 2, 3, 4, 5, 6, 7, 8, 20}},
		{24 * time.Hour, []int{1, 10, 11, 12, 13}},
		{1000 * time.Hour, []int{1, 16, 17, 18, 30}},
		{15 * time.Minute, []int{0}},
	}
	var cases []goldenWait
	for _, interval := range intervals {
		for _, failures := range interval.failures {
			var limit time.Duration
			jitter := func(l time.Duration) time.Duration {
				limit = l
				return 0
			}
			m := New(Config{PollInterval: interval.poll}, nil, slog.New(slog.DiscardHandler), WithJitter(jitter))
			wait := m.backoff(failures)
			cases = append(cases, goldenWait{PollIntervalNS: int64(interval.poll), Failures: failures, JitterLimitNS: int64(limit), WaitNS: int64(wait)})
		}
	}
	return cases
}

func successWaitVectors() []goldenSuccessWait {
	inputs := []struct {
		poll     time.Duration
		failures int
	}{
		{5 * time.Minute, 0},
		{15 * time.Minute, 3},
		{24 * time.Hour, 17},
	}
	cases := make([]goldenSuccessWait, len(inputs))
	for i, input := range inputs {
		var limit time.Duration
		jitter := func(l time.Duration) time.Duration {
			limit = l
			return l / 2
		}
		m := New(Config{PollInterval: input.poll}, nil, slog.New(slog.DiscardHandler), WithJitter(jitter))
		failures := input.failures
		wait := m.nextWait(nil, &failures)
		cases[i] = goldenSuccessWait{
			PollIntervalNS: int64(input.poll), FailuresBefore: input.failures, FailuresAfter: failures,
			JitterLimitNS: int64(limit), JitterNS: int64(limit / 2), WaitNS: int64(wait),
		}
	}
	return cases
}

func tooSoonVectors() []goldenTooSoon {
	durations := []time.Duration{
		4 * time.Minute, 4*time.Minute + 59*time.Second + 500*time.Millisecond, 4*time.Minute + 59*time.Second + 499*time.Millisecond,
		5 * time.Minute, time.Nanosecond, 499 * time.Millisecond, 500 * time.Millisecond, 1500 * time.Millisecond, 2500 * time.Millisecond,
		59*time.Second + 500*time.Millisecond, 90 * time.Second, 6 * time.Minute, 2 * time.Hour,
	}
	cases := make([]goldenTooSoon, len(durations))
	for i, d := range durations {
		cases[i] = goldenTooSoon{RetryAfterNS: int64(d), Text: (&TooSoonError{RetryAfter: d}).Error()}
	}
	return cases
}

func describeVectors() []goldenDescribe {
	inputs := []string{
		"",
		"   ",
		"  sei: /processo/listar answered HTTP 502  \n",
		"sei: /processo/listar answered HTTP 502\nmonitor: record the cycle end: connection refused",
		strings.Repeat("a", 499),
		strings.Repeat("a", 500),
		strings.Repeat("a", 501),
		"  " + strings.Repeat("a", 500) + "  ",
		strings.Repeat("a", 499) + "é",
		strings.Repeat("a", 498) + "é",
		strings.Repeat("a", 498) + "😀",
		strings.Repeat("a", 497) + "😀",
		strings.Repeat("a", 496) + "😀",
		strings.Repeat("a", 499) + " b",
		strings.Repeat("ção ", 200),
	}
	cases := make([]goldenDescribe, len(inputs))
	for i, input := range inputs {
		cases[i] = goldenDescribe{Input: input, Output: describe(errors.New(input))}
	}
	return cases
}

func loginFailureVectors(t *testing.T) []goldenLoginFailure {
	t.Helper()
	now := time.Date(2026, 11, 4, 14, 7, 9, 500000000, zone(t, "America/Boa_Vista"))
	rejected := &sei.APIError{Route: "/autenticar", Status: 200, Message: "Usuário ou Senha Inválida."}
	spaced := &sei.APIError{Route: "/autenticar", Status: 200, Message: " Usuário & Senha <inválidos> "}
	silent := &sei.APIError{Route: "/autenticar", Status: 401}
	fromClient := rejectedByTheFake(t)
	inputs := []struct {
		name     string
		apiError *sei.APIError
		err      error
	}{
		{"SEI message", rejected, rejected},
		{"SEI message with spaces and markup characters", spaced, spaced},
		{"no SEI message", silent, silent},
		{"wrapped SEI message", rejected, fmt.Errorf("sei: log in: %w", rejected)},
		{"joined with another error", rejected, errors.Join(rejected, errors.New("monitor: record the cycle end: connection refused"))},
		{"not an SEI answer", nil, sei.ErrInvalidCredentials},
		{"SEI message joined with a nil cycle-end error, as RunOnce returns it", rejected, errors.Join(rejected, nil)},
		{"rejected by the fake SEI through the client", apiErrorIn(t, fromClient), fromClient},
	}
	cases := make([]goldenLoginFailure, len(inputs))
	for i, input := range inputs {
		reason, ev := loginFailure(input.err, now)
		entry := goldenLoginFailure{
			Name: input.name, Now: stamp(now), Error: input.err.Error(), Chain: errorChain(input.err, 0), Reason: reason,
			Event: goldenStoredEvent{
				Kind: ev.Kind, ProcedureID: ev.ProcedureID, Ref: ev.Ref,
				OccurredAt: stamp(ev.OccurredAt), DetectedAt: stamp(ev.DetectedAt), Payload: ev.Payload,
			},
		}
		if input.apiError != nil {
			entry.APIError = &goldenAPIError{Route: input.apiError.Route, Status: input.apiError.Status, Message: input.apiError.Message}
		}
		cases[i] = entry
	}
	return cases
}

type goldenCycleResult struct {
	ProcessesSeen int `json:"processes_seen"`
	EventsCreated int `json:"events_created"`
}

type goldenCycle struct {
	At     string              `json:"at"`
	Setup  []seisetup.Call     `json:"setup"`
	Result goldenCycleResult   `json:"result"`
	Events []goldenStoredEvent `json:"events"`
}

type goldenCycleConfig struct {
	AllUnits          bool   `json:"all_units"`
	Concurrency       int    `json:"concurrency"`
	ReturnWarningDays int    `json:"return_warning_days"`
	Zone              string `json:"zone"`
	PrepareAt         string `json:"prepare_at"`
}

type cycleScenario struct {
	Name   string            `json:"name"`
	Config goldenCycleConfig `json:"config"`
	Cycles []goldenCycle     `json:"cycles"`
}

type cycleFile struct {
	Defaults  seisetup.Defaults `json:"defaults"`
	Scenarios []cycleScenario   `json:"scenarios"`
}

func cycleVectors(t *testing.T) cycleFile {
	t.Helper()
	h := newHarness(t, func(cfg *Config) { cfg.AllUnits = true })
	defaults, err := seisetup.ReadDefaults(t.Context(), h.server)
	if err != nil {
		t.Fatal(err)
	}
	scenario := cycleScenario{
		Name: "every kind at once",
		Config: goldenCycleConfig{
			AllUnits: true, Concurrency: 3, ReturnWarningDays: 2, Zone: seitest.TimeZone,
			PrepareAt: stamp(start.Add(7*time.Second + 250*time.Millisecond)),
		},
		Cycles: cycleInputs(),
	}
	advanceTo(t, h.clock, scenario.Config.PrepareAt)
	if err := h.monitor.Prepare(t.Context()); err != nil {
		t.Fatal(err)
	}
	recorded := 0
	for i := range scenario.Cycles {
		cycle := &scenario.Cycles[i]
		advanceTo(t, h.clock, cycle.At)
		if err := seisetup.Apply(h.server, cycle.Setup); err != nil {
			t.Fatal(err)
		}
		cycle.Result = goldenCycleResult(h.runOnce(t))
		events := recentEvents(t, h.pool)
		cycle.Events = []goldenStoredEvent{}
		for _, ev := range events[recorded:] {
			cycle.Events = append(cycle.Events, goldenStoredEvent{
				Kind: ev.Kind, ProcedureID: ev.ProcedureID, Ref: ev.Ref,
				OccurredAt: stamp(ev.OccurredAt), DetectedAt: stamp(ev.DetectedAt), Payload: ev.Payload,
			})
		}
		recorded = len(events)
	}
	return cycleFile{Defaults: defaults, Scenarios: []cycleScenario{scenario}}
}

func advanceTo(t *testing.T, clock *fakeClock, text string) {
	t.Helper()
	at, err := time.Parse(time.RFC3339Nano, text)
	if err != nil {
		t.Fatal(err)
	}
	if at.Before(clock.Now()) {
		t.Fatalf("cycle time %s is before the clock %s", text, clock.Now())
	}
	clock.Advance(at.Sub(clock.Now()))
}

func cycleInputs() []goldenCycle {
	beltranoUser := &seisetup.User{ID: 202, Acronym: "beltrano.silva", Name: "Beltrano da Silva"}
	fulanoUser := &seisetup.User{ID: seitest.UserID, Acronym: seitest.Username, Name: seitest.UserName}
	process := func(id int64, n int, kind string, change func(*seisetup.Process)) seisetup.Process {
		p := seisetup.Process{ID: id, Number: seitest.ProcessNumber(n), Type: kind}
		if change != nil {
			change(&p)
		}
		return p
	}
	act := func(id int64, user, unit string, offset time.Duration, text string) seisetup.Activity {
		return seisetup.Activity{ID: id, User: user, Unit: unit, Text: text, At: seisetup.At(start.Add(offset))}
	}
	doc := func(id int64, protocol, kind, number, label string) seisetup.Document {
		return seisetup.Document{ID: id, Protocol: protocol, Type: kind, Number: number, Label: label}
	}
	cycleAt := func(minutes int) string {
		return stamp(start.Add(time.Duration(minutes)*time.Minute + 7*time.Second + 250*time.Millisecond))
	}
	nuproc, gab := int64(seitest.UnitNUPROC), int64(seitest.UnitGAB)
	baseline := []seisetup.Process{
		process(9001, 1, "Contrato", func(p *seisetup.Process) { p.Description, p.Assignee = "Compra de cadeiras", beltranoUser }),
		process(9002, 2, "Diárias", func(p *seisetup.Process) { p.Description = "Diárias para o seminário regional" }),
		process(9003, 3, "Ofício", func(p *seisetup.Process) { p.ReturnDate = "06/11/2026" }),
		process(9004, 4, "Contrato", func(p *seisetup.Process) { p.ReturnDate = "03/11/2026" }),
		process(9005, 5, "Memorando", nil),
		process(9006, 6, "Despacho", nil),
		process(9007, 7, "Licitação", nil),
	}
	changed := []seisetup.Process{
		process(9001, 1, "Contrato", func(p *seisetup.Process) { p.Description, p.Assignee = "Compra de cadeiras", fulanoUser }),
		baseline[2],
		process(9004, 4, "Contrato", func(p *seisetup.Process) { p.ReturnDate, p.ReturnOverdue = "03/11/2026", true }),
		baseline[4],
		baseline[5],
		process(9009, 9, "Ofício", func(p *seisetup.Process) { p.Assignee, p.Generated = fulanoUser, true }),
	}
	gabBaseline := []seisetup.Process{baseline[0], process(9008, 8, "Requerimento", nil)}
	gabChanged := []seisetup.Process{
		changed[0], gabBaseline[1],
		process(9010, 10, "Contrato", func(p *seisetup.Process) { p.ReturnDate, p.ReturnOverdue = "01/11/2026", true }),
	}
	back := append(append([]seisetup.Process{}, changed...), baseline[6])
	return []goldenCycle{
		{
			At: cycleAt(0),
			Setup: []seisetup.Call{
				seisetup.SetProcesses(nuproc, baseline...),
				seisetup.SetProcesses(gab, gabBaseline...),
				seisetup.AddActivities(9001,
					act(50001, "beltrano.silva", "SEDEMO/PROTOCOLO", -72*time.Hour, "Processo gerado"),
					act(50002, "beltrano.silva", "SEDEMO/PROTOCOLO", -48*time.Hour, "Processo remetido pela unidade SEDEMO/PROTOCOLO"),
					act(50003, seitest.Username, "SEDEMO/NUPROC", -24*time.Hour, "Processo recebido na unidade"),
				),
				seisetup.AddActivities(9002, act(60001, seitest.Username, "SEDEMO/NUPROC", -24*time.Hour, "Processo gerado")),
				seisetup.AddActivities(9003, act(61001, seitest.Username, "SEDEMO/NUPROC", -24*time.Hour, "Processo gerado")),
				seisetup.AddActivities(9004, act(62001, seitest.Username, "SEDEMO/NUPROC", -24*time.Hour, "Processo gerado")),
				seisetup.AddActivities(9006, act(71001, seitest.Username, "SEDEMO/NUPROC", -24*time.Hour, "Processo gerado")),
				seisetup.AddActivities(9007, act(72001, seitest.Username, "SEDEMO/NUPROC", -24*time.Hour, "Processo gerado")),
				seisetup.AddActivities(9008, act(85005, "beltrano.silva", "SEDEMO/GAB", -24*time.Hour, "Processo gerado")),
				seisetup.AddDocuments(9001, doc(80001, "0080001", "Ofício", "1", "Ofício 1 (0080001)"), doc(80002, "0080002", "Despacho", "2", "Despacho 2 (0080002)")),
			},
		},
		{
			At: cycleAt(15),
			Setup: []seisetup.Call{
				seisetup.SetProcesses(nuproc, changed...),
				seisetup.SetProcesses(gab, gabChanged...),
				seisetup.AddActivities(9001,
					act(50010, "beltrano.silva", "SEDEMO/PROTOCOLO", 5*time.Minute+3*time.Second, "Documento Ofício 3 assinado"),
					act(50005, seitest.Username, "SEDEMO/NUPROC", 6*time.Minute+4*time.Second, "Processo recebido na unidade"),
				),
				seisetup.AddActivities(9005, act(70001, seitest.Username, "SEDEMO/NUPROC", 7*time.Minute, "Processo recebido na unidade")),
				seisetup.AddActivities(9008,
					act(85010, "beltrano.silva", "SEDEMO/GAB", 8*time.Minute, "Processo remetido pela unidade SEDEMO/GAB"),
					act(85001, "beltrano.silva", "SEDEMO/GAB", 9*time.Minute, "Processo recebido na unidade"),
				),
				seisetup.AddActivities(9009, act(90001, seitest.Username, "SEDEMO/NUPROC", 10*time.Minute, "Processo gerado")),
				seisetup.AddDocuments(9001, doc(80003, "0080003", "Ofício", "3", "Ofício 3 (0080003)")),
				seisetup.AddDocuments(9005, doc(81001, "0081001", "Memorando", "1", "Memorando 1 (0081001)")),
				seisetup.AddDocuments(9006, doc(82001, "0082001", "Despacho", "", "Despacho (0082001)")),
				seisetup.AddDocuments(9009, doc(91001, "0091001", "Ofício", "9", "Ofício 9 (0091001)")),
			},
		},
		{
			At: cycleAt(30),
			Setup: []seisetup.Call{
				seisetup.SetProcesses(nuproc, back...),
				seisetup.SetProcesses(gab, gabChanged...),
				seisetup.AddActivities(9005, act(70002, seitest.Username, "SEDEMO/NUPROC", 20*time.Minute, "Processo remetido pela unidade SEDEMO/NUPROC")),
				seisetup.AddActivities(9006, act(71002, "", "SEDEMO/NUPROC", 21*time.Minute, "Processo recebido na unidade")),
				seisetup.AddActivities(9008, act(85020, "beltrano.silva", "SEDEMO/GAB", 22*time.Minute, "Processo sobrestado")),
				seisetup.AddActivities(9009, act(90002, "beltrano.silva", "SEDEMO/PROTOCOLO", 23*time.Minute+30*time.Second, "Documento Ofício 10 assinado")),
				seisetup.AddDocuments(9009, doc(91002, "0091002", "Ofício", "10", "Ofício 10 (0091002)")),
			},
		},
		{
			At: cycleAt(45),
			Setup: []seisetup.Call{
				seisetup.SetProcesses(nuproc, back...),
				seisetup.SetProcesses(gab, gabChanged...),
			},
		},
		{
			At: cycleAt(60),
			Setup: []seisetup.Call{
				seisetup.AddActivities(9001, act(50020, seitest.Username, "SEDEMO/NUPROC", 50*time.Minute, "Documentos Ofício 4 e Despacho 5 incluídos")),
				seisetup.AddDocuments(9001, doc(80004, "0080004", "Ofício", "4", "Ofício 4 (0080004)"), doc(80005, "0080005", "Despacho", "5", "Despacho 5 (0080005)")),
			},
		},
	}
}

func rejectedByTheFake(t *testing.T) error {
	t.Helper()
	server := seitest.New(t)
	creds := sei.Credentials{Username: seitest.Username, Password: core.StaticPassword(core.NewSecret("wrong")), OrgID: seitest.OrgID}
	client := sei.New(server.APIURL(), creds, sei.Options{UserAgent: "sei-monitor/test", Location: server.Location()})
	_, err := client.Login(t.Context())
	if err == nil {
		t.Fatal("the fake accepted a wrong password")
	}
	return err
}

func apiErrorIn(t *testing.T, err error) *sei.APIError {
	t.Helper()
	apiErr, ok := errors.AsType[*sei.APIError](err)
	if !ok {
		t.Fatalf("%v is not an SEI answer", err)
	}
	return apiErr
}

func errorChain(err error, depth int) []goldenErrorLink {
	link := goldenErrorLink{Depth: depth, Type: errorType(err), Message: err.Error(), Sentinel: sentinelOf(err)}
	links := []goldenErrorLink{link}
	if joined, ok := err.(interface{ Unwrap() []error }); ok {
		for _, child := range joined.Unwrap() {
			links = append(links, errorChain(child, depth+1)...)
		}
		return links
	}
	if inner := errors.Unwrap(err); inner != nil {
		links = append(links, errorChain(inner, depth+1)...)
	}
	return links
}

func errorType(err error) string {
	if _, ok := err.(interface{ Unwrap() []error }); ok {
		return "joined"
	}
	if errors.Unwrap(err) != nil {
		return "wrapped"
	}
	if _, ok := errors.AsType[*sei.APIError](err); ok {
		return "api_error"
	}
	return "plain"
}

func sentinelOf(err error) *string {
	sentinels := []struct {
		err  error
		name string
	}{
		{sei.ErrInvalidCredentials, "invalid_credentials"},
		{sei.ErrUnauthorized, "unauthorized"},
		{sei.ErrModuleNotFound, "module_not_found"},
		{sei.ErrModuleUnavailable, "module_unavailable"},
		{sei.ErrServer, "server"},
		{sei.ErrRedirect, "redirect"},
		{sei.ErrUnexpectedResponse, "unexpected_response"},
		{sei.ErrOrgNotFound, "org_not_found"},
		{sei.ErrRefused, "refused"},
	}
	for _, sentinel := range sentinels {
		if errors.Is(err, sentinel.err) {
			return &sentinel.name
		}
	}
	return nil
}
