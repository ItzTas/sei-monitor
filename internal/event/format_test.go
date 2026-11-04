package event

import (
	"slices"
	"strings"
	"testing"
	"time"
)

func boaVista(t *testing.T) *time.Location {
	t.Helper()
	location, err := time.LoadLocation("America/Boa_Vista")
	if err != nil {
		t.Fatal(err)
	}
	return location
}

func TestTitles(t *testing.T) {
	detected := time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)
	process := Payload{ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC"}
	with := func(change func(*Payload)) Payload {
		p := process
		change(&p)
		return p
	}
	tests := []struct {
		name    string
		event   Event
		minimal string
		full    string
	}{
		{"entered", Event{Kind: ProcessEntered, Payload: process}, "New process in SEDEMO/NUPROC: 00000.000001/2026-00", ""},
		{"entered and assigned to the user", Event{Kind: ProcessEntered, Payload: with(func(p *Payload) { p.AssignedToSelf = true })}, "New process in SEDEMO/NUPROC: 00000.000001/2026-00 (assigned to you)", ""},
		{"left", Event{Kind: ProcessLeft, Payload: process}, "Process 00000.000001/2026-00 left SEDEMO/NUPROC", ""},
		{"activity in the SEI time zone", Event{Kind: ActivityAdded, OccurredAt: time.Date(2026, 11, 4, 17, 13, 5, 0, time.UTC), Payload: process}, "New activity on process 00000.000001/2026-00 (2026-11-04 13:13)", ""},
		{"document", Event{Kind: DocumentAdded, Payload: process}, "New document on process 00000.000001/2026-00", ""},
		{"assigned to the user", Event{Kind: AssignmentChanged, Payload: with(func(p *Payload) { p.AssignedUser, p.AssignedToSelf = "fulano.tal", true })}, "Process 00000.000001/2026-00 was assigned to you", ""},
		{"assigned to someone else", Event{Kind: AssignmentChanged, Payload: with(func(p *Payload) { p.AssignedUser = "beltrano.silva" })}, "Process 00000.000001/2026-00 was assigned to someone else", "Process 00000.000001/2026-00 was assigned to beltrano.silva"},
		{"assignment removed", Event{Kind: AssignmentChanged, Payload: process}, "Process 00000.000001/2026-00 is no longer assigned to anyone", ""},
		{"return in two days", Event{Kind: ReturnDueSoon, DetectedAt: detected, Payload: with(func(p *Payload) { p.ReturnDate = "2026-11-06" })}, "Scheduled return of process 00000.000001/2026-00 is due in 2 days (2026-11-06)", ""},
		{"return tomorrow", Event{Kind: ReturnDueSoon, DetectedAt: detected, Payload: with(func(p *Payload) { p.ReturnDate = "2026-11-05" })}, "Scheduled return of process 00000.000001/2026-00 is due tomorrow (2026-11-05)", ""},
		{"return today", Event{Kind: ReturnDueSoon, DetectedAt: detected, Payload: with(func(p *Payload) { p.ReturnDate = "2026-11-04" })}, "Scheduled return of process 00000.000001/2026-00 is due today (2026-11-04)", ""},
		{"overdue return", Event{Kind: ReturnOverdue, Payload: process}, "Scheduled return of process 00000.000001/2026-00 is overdue", ""},
		{"login failure", Event{Kind: AccountLoginFailed, Payload: Payload{Reason: "SEI rejected the login"}}, "sei-monitor could not log in to SEI", ""},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			minimal := Formatter{Location: boaVista(t)}.Title(tt.event)
			full := Formatter{Location: boaVista(t), Full: true}.Title(tt.event)
			wantFull := tt.full
			if wantFull == "" {
				wantFull = tt.minimal
			}
			if minimal != tt.minimal || full != wantFull {
				t.Errorf("titles = %q / %q\nwant     %q / %q", minimal, full, tt.minimal, wantFull)
			}
		})
	}
}

func TestDetailsFollowTheDetailLevel(t *testing.T) {
	ev := Event{
		Kind:       ActivityAdded,
		OccurredAt: time.Date(2026, 11, 4, 17, 13, 5, 0, time.UTC),
		Payload: Payload{
			ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC", ProcessType: "Contrato",
			Description: "Compra de cadeiras", ActivityText: "Processo recebido na unidade",
			ActivityUser: "beltrano.silva", ActivityUnit: "SEDEMO/PROTOCOLO",
		},
	}

	minimal := Formatter{Location: boaVista(t)}.Details(ev)
	full := Formatter{Location: boaVista(t), Full: true}.Details(ev)

	if !slices.Equal(minimal, []string{"When: 2026-11-04 13:13", "Unit: SEDEMO/NUPROC"}) {
		t.Errorf("minimal details = %q", minimal)
	}
	wantFull := []string{
		"When: 2026-11-04 13:13", "Unit: SEDEMO/NUPROC", "Type: Contrato", "Description: Compra de cadeiras",
		"Activity: Processo recebido na unidade (beltrano.silva, SEDEMO/PROTOCOLO)",
	}
	if !slices.Equal(full, wantFull) {
		t.Errorf("full details = %q\nwant %q", full, wantFull)
	}
}

func TestMinimalDetailsHideOtherPeopleAndContent(t *testing.T) {
	ev := Event{
		Kind: AssignmentChanged,
		Payload: Payload{
			ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC", ProcessType: "Contrato",
			Description: "Compra de cadeiras", AssignedUser: "beltrano.silva", AssignedUserName: "Beltrano da Silva",
			DocumentName: "Ofício 12 (0012345)",
		},
	}
	formatter := Formatter{Location: time.UTC}

	text := formatter.Title(ev) + "\n" + strings.Join(formatter.Details(ev), "\n")

	for _, hidden := range []string{"beltrano", "Beltrano", "Contrato", "cadeiras", "Ofício"} {
		if strings.Contains(text, hidden) {
			t.Errorf("minimal text shows %q:\n%s", hidden, text)
		}
	}
	full := Formatter{Location: time.UTC, Full: true}.Details(ev)
	if !slices.Contains(full, "Assigned to: beltrano.silva (Beltrano da Silva)") || !slices.Contains(full, "Document: Ofício 12 (0012345)") {
		t.Errorf("full details = %q", full)
	}
}

func TestFullTextCallsTheUserYou(t *testing.T) {
	formatter := Formatter{Location: time.UTC, Full: true, Self: "12345678901"}
	activity := Event{Kind: ActivityAdded, Payload: Payload{
		ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC",
		ActivityText: "Processo recebido na unidade", ActivityUser: "12345678901", ActivityUnit: "SEDEMO/NUPROC",
	}}
	assignment := Event{Kind: AssignmentChanged, Payload: Payload{
		ProcessNumber: "00000.000001/2026-00", AssignedUser: "12345678901", AssignedUserName: "Fulano de Tal",
	}}

	activityDetails := formatter.Details(activity)
	assignmentTitle, assignmentDetails := formatter.Title(assignment), formatter.Details(assignment)

	if !slices.Contains(activityDetails, "Activity: Processo recebido na unidade (you, SEDEMO/NUPROC)") {
		t.Errorf("activity details = %q", activityDetails)
	}
	if assignmentTitle != "Process 00000.000001/2026-00 was assigned to you" || !slices.Contains(assignmentDetails, "Assigned to: you") {
		t.Errorf("assignment = %q %q", assignmentTitle, assignmentDetails)
	}
	text := strings.Join(slices.Concat(activityDetails, assignmentDetails), "\n")
	if strings.Contains(text, "12345678901") || strings.Contains(text, "Fulano") {
		t.Errorf("the text shows the user:\n%s", text)
	}
}

func TestLoginFailureDetails(t *testing.T) {
	details := Formatter{}.Details(Event{Kind: AccountLoginFailed, Payload: Payload{Reason: "SEI rejected the login (Usuário ou Senha Inválida.)"}})

	want := []string{
		"Reason: SEI rejected the login (Usuário ou Senha Inválida.)",
		"Monitoring is paused until sei-monitor restarts with the configuration fixed.",
	}
	if !slices.Equal(details, want) {
		t.Errorf("details = %q", details)
	}
}
