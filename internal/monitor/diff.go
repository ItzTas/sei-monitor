package monitor

import (
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/sei"
)

const storedDateLayout = "2006-01-02"

type Observation struct {
	Process sei.Process
	Unit    sei.Unit
}

type Self struct {
	UserID  int64
	Acronym string
}

type Rules struct {
	Now               time.Time
	Location          *time.Location
	ReturnWarningDays int
	Self              Self
	Baseline          bool
}

type Presence struct {
	Observation
	Previous *core.Process
	Entered  bool
}

func (p Presence) quiet(baseline bool) bool {
	return baseline || p.Entered || p.Previous.LastActivityID == 0
}

type ProcessChanges struct {
	Present []Presence
	Left    []core.Process
	Events  []event.Event
}

func DiffProcesses(previous []core.Process, current []Observation, rules Rules) ProcessChanges {
	known := make(map[int64]*core.Process, len(previous))
	for i := range previous {
		known[previous[i].ProcedureID] = &previous[i]
	}
	var changes ProcessChanges
	seen := map[int64]bool{}
	for _, observation := range current {
		id := observation.Process.ProcedureID
		if seen[id] {
			continue
		}
		seen[id] = true
		previous := known[id]
		presence := Presence{Observation: observation, Previous: previous, Entered: previous == nil || !previous.LeftAt.IsZero()}
		changes.Present = append(changes.Present, presence)
		if !rules.Baseline {
			changes.Events = append(changes.Events, processEvents(presence, rules)...)
		}
	}
	for _, process := range previous {
		if seen[process.ProcedureID] || !process.LeftAt.IsZero() {
			continue
		}
		changes.Left = append(changes.Left, process)
		if !rules.Baseline {
			changes.Events = append(changes.Events, leftEvent(process, rules))
		}
	}
	return changes
}

func FreshActivities(lastSeenID int64, newestFirst []sei.Activity) ([]sei.Activity, bool) {
	var fresh []sei.Activity
	for _, activity := range newestFirst {
		if activity.ID <= lastSeenID {
			return fresh, true
		}
		fresh = append(fresh, activity)
	}
	return fresh, false
}

func FreshDocuments(known map[int64]bool, documents []sei.Document) []sei.Document {
	var fresh []sei.Document
	for _, document := range documents {
		if !known[document.ID] {
			fresh = append(fresh, document)
		}
	}
	return fresh
}

func processEvents(p Presence, rules Rules) []event.Event {
	if p.Entered {
		return []event.Event{enteredEvent(p, rules)}
	}
	checks := []func(Presence, Rules) (event.Event, bool){assignmentEvent, overdueEvent, dueSoonEvent}
	var events []event.Event
	for _, check := range checks {
		if ev, ok := check(p, rules); ok {
			events = append(events, ev)
		}
	}
	return events
}

func enteredEvent(p Presence, rules Rules) event.Event {
	payload := basePayload(p)
	if assignee := p.Process.Assignee; assignee != nil {
		payload.AssignedUser, payload.AssignedUserName = assignee.Acronym, assignee.Name
		payload.AssignedToSelf = IsSelf(assignee.ID, assignee.Acronym, rules.Self)
	}
	return newEvent(event.ProcessEntered, p.Process.ProcedureID, cycleRef(p.Process.ProcedureID, rules.Now), payload)
}

func leftEvent(process core.Process, rules Rules) event.Event {
	payload := event.Payload{ProcessNumber: process.Number, Unit: process.UnitAcronym, ProcessType: process.Type, Description: process.Description}
	return newEvent(event.ProcessLeft, process.ProcedureID, cycleRef(process.ProcedureID, rules.Now), payload)
}

func assignmentEvent(p Presence, rules Rules) (event.Event, bool) {
	var id int64
	var acronym, name string
	if assignee := p.Process.Assignee; assignee != nil {
		id, acronym, name = assignee.ID, assignee.Acronym, assignee.Name
	}
	if sameAssignee(p.Previous.AssignedUserID, p.Previous.AssignedUser, id, acronym) {
		return event.Event{}, false
	}
	payload := basePayload(p)
	payload.AssignedUser, payload.AssignedUserName, payload.PreviousAssignee = acronym, name, p.Previous.AssignedUser
	payload.AssignedToSelf = acronym != "" && IsSelf(id, acronym, rules.Self)
	return newEvent(event.AssignmentChanged, p.Process.ProcedureID, cycleRef(p.Process.ProcedureID, rules.Now), payload), true
}

func overdueEvent(p Presence, rules Rules) (event.Event, bool) {
	if p.Previous.ReturnOverdue || !p.Process.ReturnOverdue {
		return event.Event{}, false
	}
	payload := basePayload(p)
	payload.ReturnDate = formatDate(p.Process.ReturnDate)
	ref := cycleRef(p.Process.ProcedureID, rules.Now)
	if payload.ReturnDate != "" {
		ref = fmt.Sprintf("%d:%s", p.Process.ProcedureID, payload.ReturnDate)
	}
	return newEvent(event.ReturnOverdue, p.Process.ProcedureID, ref, payload), true
}

func dueSoonEvent(p Presence, rules Rules) (event.Event, bool) {
	date := p.Process.ReturnDate
	if date.IsZero() || p.Process.ReturnOverdue {
		return event.Event{}, false
	}
	days := daysUntil(rules.Now, date, rules.Location)
	if days < 0 || days > rules.ReturnWarningDays {
		return event.Event{}, false
	}
	payload := basePayload(p)
	payload.ReturnDate = formatDate(date)
	return newEvent(event.ReturnDueSoon, p.Process.ProcedureID, fmt.Sprintf("%d:%s", p.Process.ProcedureID, payload.ReturnDate), payload), true
}

func activityEvent(p Presence, activity sei.Activity) event.Event {
	payload := basePayload(p)
	payload.ActivityText, payload.ActivityUser, payload.ActivityUnit = activity.Text, activity.User, activity.Unit
	ev := newEvent(event.ActivityAdded, p.Process.ProcedureID, strconv.FormatInt(activity.ID, 10), payload)
	ev.OccurredAt = activity.At
	return ev
}

func documentEvent(p Presence, document sei.Document) event.Event {
	payload := basePayload(p)
	payload.DocumentName, payload.DocumentType, payload.DocumentProtocol = document.Name, document.Type, document.Protocol
	return newEvent(event.DocumentAdded, p.Process.ProcedureID, strconv.FormatInt(document.ID, 10), payload)
}

func newEvent(kind event.Kind, procedureID int64, ref string, payload event.Payload) event.Event {
	return event.Event{Kind: kind, ProcedureID: procedureID, Ref: ref, Payload: payload}
}

func basePayload(p Presence) event.Payload {
	return event.Payload{
		ProcessNumber: p.Process.Number,
		Unit:          p.Unit.Acronym,
		ProcessType:   p.Process.Type,
		Description:   p.Process.Description,
	}
}

func cycleRef(procedureID int64, now time.Time) string {
	return fmt.Sprintf("%d:%s", procedureID, now.UTC().Format(time.RFC3339))
}

func sameAssignee(previousID int64, previousAcronym string, currentID int64, currentAcronym string) bool {
	if previousID != 0 && currentID != 0 {
		return previousID == currentID
	}
	return strings.EqualFold(previousAcronym, currentAcronym)
}

func IsSelf(id int64, acronym string, self Self) bool {
	if id != 0 && self.UserID != 0 {
		return id == self.UserID
	}
	return acronym != "" && strings.EqualFold(acronym, self.Acronym)
}

func daysUntil(now, date time.Time, location *time.Location) int {
	if location == nil {
		location = time.UTC
	}
	year, month, day := now.In(location).Date()
	today := time.Date(year, month, day, 0, 0, 0, 0, time.UTC)
	return int(date.Sub(today).Hours() / 24)
}

func formatDate(date time.Time) string {
	if date.IsZero() {
		return ""
	}
	return date.Format(storedDateLayout)
}
