package event

import (
	"cmp"
	"fmt"
	"strings"
	"time"
)

const (
	dateLayout     = "2006-01-02"
	dateTimeLayout = "2006-01-02 15:04"
	labelLimit     = 40
	summaryLimit   = 80
)

type Formatter struct {
	Location *time.Location
	Full     bool
	Self     string
}

func (f Formatter) Title(ev Event) string {
	p := ev.Payload
	titles := map[Kind]func() string{
		ProcessEntered: func() string {
			return fmt.Sprintf("New process in %s: %s%s", p.Unit, p.ProcessNumber, map[bool]string{true: " (assigned to you)"}[p.AssignedToSelf])
		},
		ProcessLeft:        func() string { return fmt.Sprintf("Process %s left %s", p.ProcessNumber, p.Unit) },
		ActivityAdded:      func() string { return f.activity(ev) },
		DocumentAdded:      func() string { return fmt.Sprintf("New document on process %s", p.ProcessNumber) },
		AssignmentChanged:  func() string { return f.assignment(p) },
		ReturnDueSoon:      func() string { return f.returnDue(ev) },
		ReturnOverdue:      func() string { return fmt.Sprintf("Scheduled return of process %s is overdue", p.ProcessNumber) },
		AccountLoginFailed: func() string { return "sei-monitor could not log in to SEI" },
	}
	title, ok := titles[ev.Kind]
	if !ok {
		return string(ev.Kind)
	}
	return title()
}

func (f Formatter) Details(ev Event) []string {
	p := ev.Payload
	if ev.Kind == AccountLoginFailed {
		return []string{
			"Reason: " + p.Reason,
			"Monitoring is paused until sei-monitor restarts with the configuration fixed.",
		}
	}
	lines := []string{"When: " + f.time(ev.OccurredAt)}
	if p.Unit != "" && ev.Kind != ProcessEntered && ev.Kind != ProcessLeft {
		lines = append(lines, "Unit: "+p.Unit)
	}
	if !f.Full {
		return lines
	}
	optional := []struct{ label, value string }{
		{"Type", p.ProcessType},
		{"Description", p.Description},
		{"Activity", joinNonEmpty(p.ActivityText, parenthesized(f.person(p.ActivityUser), p.ActivityUnit))},
		{"Document", p.DocumentName},
		{"Return date", f.date(p.ReturnDate)},
		{"Assigned to", f.assignee(p)},
	}
	for _, item := range optional {
		if item.value != "" {
			lines = append(lines, item.label+": "+item.value)
		}
	}
	return lines
}

func (f Formatter) IsSelf(acronym string) bool {
	return acronym != "" && f.Self != "" && strings.EqualFold(acronym, f.Self)
}

func (f Formatter) person(acronym string) string {
	if f.IsSelf(acronym) {
		return "you"
	}
	return acronym
}

func (f Formatter) assignedToSelf(p Payload) bool {
	return p.AssignedToSelf || f.IsSelf(p.AssignedUser)
}

func (f Formatter) assignee(p Payload) string {
	if f.assignedToSelf(p) {
		return "you"
	}
	return joinNonEmpty(p.AssignedUser, parenthesized(p.AssignedUserName))
}

func (f Formatter) assignment(p Payload) string {
	if f.assignedToSelf(p) {
		return fmt.Sprintf("Process %s was assigned to you", p.ProcessNumber)
	}
	if p.AssignedUser == "" {
		return fmt.Sprintf("Process %s is no longer assigned to anyone", p.ProcessNumber)
	}
	if f.Full {
		return fmt.Sprintf("Process %s was assigned to %s", p.ProcessNumber, p.AssignedUser)
	}
	return fmt.Sprintf("Process %s was assigned to someone else", p.ProcessNumber)
}

func (f Formatter) activity(ev Event) string {
	p := ev.Payload
	if !f.Full || strings.TrimSpace(p.ActivityText) == "" {
		return fmt.Sprintf("New activity on process %s (%s)", p.ProcessNumber, f.time(ev.OccurredAt))
	}
	label := shorten(cmp.Or(strings.TrimSpace(p.Description), p.ProcessType), labelLimit)
	return joinNonEmpty(p.ProcessNumber, parenthesized(label)) + ": " + shorten(p.ActivityText, summaryLimit)
}

func (f Formatter) returnDue(ev Event) string {
	date, err := time.Parse(dateLayout, ev.Payload.ReturnDate)
	if err != nil {
		return fmt.Sprintf("Scheduled return of process %s is due soon", ev.Payload.ProcessNumber)
	}
	year, month, day := ev.DetectedAt.In(f.location()).Date()
	days := int(date.Sub(time.Date(year, month, day, 0, 0, 0, 0, time.UTC)).Hours() / 24)
	when := map[int]string{0: "today", 1: "tomorrow"}[days]
	if when == "" {
		when = fmt.Sprintf("in %d days", days)
	}
	return fmt.Sprintf("Scheduled return of process %s is due %s (%s)", ev.Payload.ProcessNumber, when, ev.Payload.ReturnDate)
}

func (f Formatter) time(t time.Time) string {
	if t.IsZero() {
		return "unknown time"
	}
	return t.In(f.location()).Format(dateTimeLayout)
}

func (f Formatter) date(text string) string {
	if _, err := time.Parse(dateLayout, text); err != nil {
		return ""
	}
	return text
}

func (f Formatter) location() *time.Location {
	if f.Location == nil {
		return time.UTC
	}
	return f.Location
}

func shorten(text string, limit int) string {
	runes := []rune(strings.Join(strings.Fields(text), " "))
	if len(runes) <= limit {
		return string(runes)
	}
	return strings.TrimSpace(string(runes[:limit-1])) + "…"
}

func parenthesized(parts ...string) string {
	kept := nonEmpty(parts)
	if len(kept) == 0 {
		return ""
	}
	return "(" + strings.Join(kept, ", ") + ")"
}

func joinNonEmpty(parts ...string) string {
	return strings.Join(nonEmpty(parts), " ")
}

func nonEmpty(parts []string) []string {
	kept := make([]string, 0, len(parts))
	for _, part := range parts {
		if part = strings.TrimSpace(part); part != "" {
			kept = append(kept, part)
		}
	}
	return kept
}
