package event

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/goldentest"
)

func TestGoldenVectors(t *testing.T) {
	goldentest.Golden(t, "event_text.json", eventTextVectors(t))
	goldentest.Golden(t, "shorten.json", shortenVectors())
}

const (
	goldenNumber = "00000.000001/2026-00"
	goldenUnit   = "SEDEMO/NUPROC"
)

type goldenEvent struct {
	ID          int64   `json:"id"`
	Kind        Kind    `json:"kind"`
	ProcedureID int64   `json:"procedure_id"`
	Ref         string  `json:"ref"`
	OccurredAt  string  `json:"occurred_at"`
	DetectedAt  string  `json:"detected_at"`
	Payload     Payload `json:"payload"`
}

type eventTextCase struct {
	Name    string      `json:"name"`
	Event   goldenEvent `json:"event"`
	Zone    string      `json:"zone"`
	Self    string      `json:"self"`
	Full    bool        `json:"full"`
	Title   string      `json:"title"`
	Details []string    `json:"details"`
	View    View        `json:"view"`
	Hidden  []string    `json:"hidden"`
}

type eventTextInput struct {
	name   string
	zone   string
	self   string
	event  Event
	hidden []string
}

func stamp(t time.Time) string {
	return t.UTC().Format(time.RFC3339Nano)
}

func eventTextVectors(t *testing.T) []eventTextCase {
	t.Helper()
	var cases []eventTextCase
	for _, input := range eventTextInputs() {
		location, err := time.LoadLocation(input.zone)
		if err != nil {
			t.Fatalf("load zone %s: %v", input.zone, err)
		}
		for _, full := range []bool{false, true} {
			cases = append(cases, eventTextEntry(t, input, location, full))
		}
	}
	return cases
}

func eventTextEntry(t *testing.T, input eventTextInput, location *time.Location, full bool) eventTextCase {
	t.Helper()
	formatter := Formatter{Location: location, Full: full, Self: input.self}
	ev := input.event
	level := map[bool]string{false: "minimal", true: "full"}[full]
	entry := eventTextCase{
		Name: input.name + " (" + level + ")",
		Event: goldenEvent{
			ID: ev.ID, Kind: ev.Kind, ProcedureID: ev.ProcedureID, Ref: ev.Ref,
			OccurredAt: stamp(ev.OccurredAt), DetectedAt: stamp(ev.DetectedAt), Payload: ev.Payload,
		},
		Zone:    input.zone,
		Self:    input.self,
		Full:    full,
		Title:   formatter.Title(ev),
		Details: formatter.Details(ev),
		View:    formatter.View(ev),
		Hidden:  append([]string{}, input.hidden...),
	}
	raw, err := json.Marshal(entry.View)
	if err != nil {
		t.Fatal(err)
	}
	shown := entry.Title + "\n" + strings.Join(entry.Details, "\n") + "\n" + string(raw)
	for _, hidden := range entry.Hidden {
		if strings.Contains(shown, hidden) {
			t.Errorf("%s: the text shows %q:\n%s", entry.Name, hidden, shown)
		}
	}
	return entry
}

func everything() Payload {
	return Payload{
		ProcessNumber: goldenNumber, Unit: goldenUnit, ProcessType: "Contrato", Description: "Compra de cadeiras",
		ActivityText: "Processo recebido na unidade", ActivityUser: "beltrano.silva", ActivityUnit: "SEDEMO/PROTOCOLO",
		DocumentName: "Ofício 12 (0012345)", DocumentType: "Ofício", DocumentProtocol: "0012345",
		AssignedUser: "beltrano.silva", AssignedUserName: "Beltrano da Silva", PreviousAssignee: "sicrano.souza",
		ReturnDate: "2026-11-06", Reason: "SEI rejected the login (Usuário ou Senha Inválida.)",
	}
}

func basic(change func(*Payload)) Payload {
	p := Payload{ProcessNumber: goldenNumber, Unit: goldenUnit}
	if change != nil {
		change(&p)
	}
	return p
}

func eventTextInputs() []eventTextInput {
	detected := time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)
	occurred := time.Date(2026, 11, 4, 17, 13, 5, 0, time.UTC)
	process := func(kind Kind, ref string, payload Payload) Event {
		return Event{ID: 42, Kind: kind, ProcedureID: 9001, Ref: ref, OccurredAt: detected, DetectedAt: detected, Payload: payload}
	}
	cycleRef := "9001:2026-11-04T18:00:00Z"
	inputs := []eventTextInput{
		{name: "process_entered", zone: "America/Boa_Vista", self: "fulano.tal", event: process(ProcessEntered, cycleRef, basic(nil))},
		{name: "process_entered with every payload field", zone: "America/Sao_Paulo", self: "fulano.tal", event: process(ProcessEntered, cycleRef, everything())},
		{
			name: "process_entered assigned to the user, stored acronym in other letters", zone: "America/Boa_Vista", self: "FULANO.TAL",
			event: process(ProcessEntered, cycleRef, basic(func(p *Payload) {
				p.ProcessType, p.AssignedUser, p.AssignedUserName, p.AssignedToSelf = "Contrato", "Fulano.Tal", "Fulano de Tal", true
			})),
			hidden: []string{"Fulano.Tal", "Fulano de Tal"},
		},
		{
			name: "process_entered whose assignee is the user without assigned_to_self", zone: "America/Boa_Vista", self: "fulano.tal",
			event: process(ProcessEntered, cycleRef, basic(func(p *Payload) {
				p.AssignedUser, p.AssignedUserName = "Fulano.Tal", "Fulano de Tal"
			})),
			hidden: []string{"Fulano.Tal", "Fulano de Tal"},
		},
		{
			name: "process_entered assigned to the user with an empty self", zone: "UTC", self: "",
			event: process(ProcessEntered, cycleRef, basic(func(p *Payload) {
				p.AssignedUser, p.AssignedUserName, p.AssignedToSelf = "fulano.tal", "Fulano de Tal", true
			})),
			hidden: []string{"fulano.tal", "Fulano de Tal"},
		},
		{name: "process_left with every payload field", zone: "UTC", self: "fulano.tal", event: process(ProcessLeft, cycleRef, everything())},
		{name: "process_left", zone: "America/Boa_Vista", self: "fulano.tal", event: process(ProcessLeft, cycleRef, basic(nil))},
		{name: "document_added with every payload field", zone: "America/Boa_Vista", self: "fulano.tal", event: process(DocumentAdded, "80001", everything())},
		{name: "document_added", zone: "America/Sao_Paulo", self: "fulano.tal", event: process(DocumentAdded, "80001", basic(nil))},
		{name: "return_overdue with every payload field", zone: "America/Boa_Vista", self: "fulano.tal", event: process(ReturnOverdue, "9001:2026-11-06", everything())},
		{name: "return_overdue without a date", zone: "America/Boa_Vista", self: "fulano.tal", event: process(ReturnOverdue, cycleRef, basic(nil))},
	}
	inputs = append(inputs, activityInputs(process, occurred)...)
	inputs = append(inputs, assignmentInputs(process, cycleRef)...)
	inputs = append(inputs, returnDueInputs(process)...)
	return append(inputs, loginFailureInputs(detected)...)
}

func activityInputs(process func(Kind, string, Payload) Event, occurred time.Time) []eventTextInput {
	activity := func(change func(*Payload)) Event {
		ev := process(ActivityAdded, "50001", basic(change))
		ev.OccurredAt = occurred
		return ev
	}
	described := func(text string) func(*Payload) {
		return func(p *Payload) {
			p.ProcessType, p.Description, p.ActivityText = "Contrato", "Compra de cadeiras", text
			p.ActivityUser, p.ActivityUnit = "beltrano.silva", "SEDEMO/PROTOCOLO"
		}
	}
	unknownTime := activity(described("Processo recebido na unidade"))
	unknownTime.OccurredAt = time.Time{}
	everyField := process(ActivityAdded, "50001", everything())
	everyField.OccurredAt = occurred
	return []eventTextInput{
		{name: "activity_added in the SEI time zone", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(nil)},
		{name: "activity_added with every payload field", zone: "America/Sao_Paulo", self: "fulano.tal", event: everyField},
		{name: "activity_added with a description", zone: "America/Sao_Paulo", self: "fulano.tal", event: activity(described("Processo recebido na unidade"))},
		{name: "activity_added in UTC", zone: "UTC", self: "fulano.tal", event: activity(described("Processo recebido na unidade"))},
		{name: "activity_added without a description uses the type", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			p.ProcessType, p.ActivityText, p.ActivityUnit = "Contrato", "Processo recebido na unidade", "SEDEMO/PROTOCOLO"
		})},
		{name: "activity_added with a description of only spaces uses the type", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			p.ProcessType, p.Description, p.ActivityText = "Contrato", " \t\u00a0\n ", "Processo recebido na unidade"
		})},
		{name: "activity_added without a description or a type", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			p.ActivityText = "Processo recebido na unidade"
		})},
		{name: "activity_added with an empty text", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(described(""))},
		{name: "activity_added with a text of only spaces", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(described(" \n\t\u3000 "))},
		{name: "activity_added with long texts", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			p.Description = "Aquisição de cadeiras ergonômicas para o setor de protocolo"
			p.ActivityText = "Processo remetido pela unidade SEDEMO/NUPROC\n  para a unidade SEDEMO/PROTOCOLO com retorno programado"
		})},
		{name: "activity_added with texts that are long in bytes but not in runes", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			p.Description, p.ActivityText = strings.Repeat("ã", 40), strings.Repeat("çã", 40)
		})},
		{name: "activity_added with texts one rune over the limits", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			p.Description, p.ActivityText = strings.Repeat("ã", 41), "é"+strings.Repeat("çã", 40)
		})},
		{name: "activity_added with texts one rune under the limits", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			p.Description, p.ActivityText = strings.Repeat("ã", 39), strings.Repeat("çã", 39)+"ç"
		})},
		{name: "activity_added with spaces at the cut and tabs and line breaks", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			p.ProcessType = "Solicitação\tde   diárias e passagens para\n\nservidor em viagem"
			p.ActivityText = "Despacho\tnº 12 assinado\r\n por fulano.tal na unidade SEDEMO/NUPROC, com ciência de todos os interessados"
		})},
		{name: "activity_added at an unknown time", zone: "America/Boa_Vista", self: "fulano.tal", event: unknownTime},
		{
			name: "activity_added by the user, stored acronym in other letters", zone: "America/Boa_Vista", self: "fulano.tal",
			event: activity(func(p *Payload) {
				described("Processo recebido na unidade")(p)
				p.ActivityUser = "FULANO.TAL"
			}),
			hidden: []string{"FULANO.TAL"},
		},
		{name: "activity_added by the user with an empty self", zone: "America/Boa_Vista", self: "", event: activity(func(p *Payload) {
			described("Processo recebido na unidade")(p)
			p.ActivityUser = "FULANO.TAL"
		})},
		{name: "activity_added by someone whose acronym contains the user's", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			described("Processo recebido na unidade")(p)
			p.ActivityUser = "fulano.tal2"
		})},
		{name: "activity_added without a user", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			described("Processo recebido na unidade")(p)
			p.ActivityUser = ""
		})},
		{name: "activity_added without a user or a unit", zone: "America/Boa_Vista", self: "fulano.tal", event: activity(func(p *Payload) {
			described("Processo recebido na unidade")(p)
			p.ActivityUser, p.ActivityUnit = "", ""
		})},
	}
}

func assignmentInputs(process func(Kind, string, Payload) Event, cycleRef string) []eventTextInput {
	assignment := func(change func(*Payload)) Event {
		return process(AssignmentChanged, cycleRef, basic(change))
	}
	return []eventTextInput{
		{name: "assignment_changed with every payload field", zone: "America/Boa_Vista", self: "fulano.tal", event: process(AssignmentChanged, cycleRef, everything())},
		{
			name: "assignment_changed to the user, stored acronym in other letters", zone: "America/Boa_Vista", self: "FULANO.TAL",
			event: assignment(func(p *Payload) {
				p.AssignedUser, p.AssignedUserName, p.PreviousAssignee = "Fulano.Tal", "Fulano de Tal", "beltrano.silva"
			}),
			hidden: []string{"Fulano.Tal", "Fulano de Tal"},
		},
		{name: "assignment_changed with assigned_to_self and another assignee", zone: "America/Boa_Vista", self: "fulano.tal", event: assignment(func(p *Payload) {
			p.AssignedUser, p.AssignedUserName, p.AssignedToSelf, p.PreviousAssignee = "beltrano.silva", "Beltrano da Silva", true, "sicrano.souza"
		})},
		{name: "assignment_changed to someone else", zone: "America/Boa_Vista", self: "fulano.tal", event: assignment(func(p *Payload) {
			p.AssignedUser, p.AssignedUserName, p.PreviousAssignee = "beltrano.silva", "Beltrano da Silva", "sicrano.souza"
		})},
		{name: "assignment_changed to nobody", zone: "America/Boa_Vista", self: "fulano.tal", event: assignment(func(p *Payload) {
			p.PreviousAssignee = "beltrano.silva"
		})},
		{
			name: "assignment_changed from the user to someone else", zone: "America/Boa_Vista", self: "fulano.tal",
			event: assignment(func(p *Payload) {
				p.AssignedUser, p.AssignedUserName, p.PreviousAssignee = "beltrano.silva", "Beltrano da Silva", "fulano.TAL"
			}),
			hidden: []string{"fulano.TAL"},
		},
		{
			name: "assignment_changed from the user to nobody", zone: "America/Boa_Vista", self: "fulano.tal",
			event:  assignment(func(p *Payload) { p.PreviousAssignee = "FULANO.TAL" }),
			hidden: []string{"FULANO.TAL"},
		},
		{name: "assignment_changed to the user with an empty self", zone: "America/Boa_Vista", self: "", event: assignment(func(p *Payload) {
			p.AssignedUser, p.AssignedUserName, p.PreviousAssignee = "Fulano.Tal", "Fulano de Tal", "beltrano.silva"
		})},
		{name: "assignment_changed to someone whose acronym contains the user's", zone: "America/Boa_Vista", self: "fulano.tal", event: assignment(func(p *Payload) {
			p.AssignedUser, p.AssignedUserName, p.PreviousAssignee = "fulano.tal2", "Fulano Segundo", "beltrano.silva"
		})},
	}
}

func returnDueInputs(process func(Kind, string, Payload) Event) []eventTextInput {
	due := func(detected time.Time, date string) Event {
		ev := process(ReturnDueSoon, "9001:"+date, basic(func(p *Payload) { p.ReturnDate = date }))
		ev.DetectedAt, ev.OccurredAt = detected, detected
		return ev
	}
	beforeMidnight := time.Date(2026, 11, 5, 3, 59, 0, 0, time.UTC)
	midnight := time.Date(2026, 11, 5, 4, 0, 0, 0, time.UTC)
	saoPauloBeforeMidnight := time.Date(2026, 11, 5, 2, 59, 59, 0, time.UTC)
	everyField := process(ReturnDueSoon, "9001:2026-11-06", everything())
	return []eventTextInput{
		{name: "return_due_soon at 23:59 local, date the day before", zone: "America/Boa_Vista", self: "fulano.tal", event: due(beforeMidnight, "2026-11-03")},
		{name: "return_due_soon at 23:59 local, date today", zone: "America/Boa_Vista", self: "fulano.tal", event: due(beforeMidnight, "2026-11-04")},
		{name: "return_due_soon at 23:59 local, date tomorrow", zone: "America/Boa_Vista", self: "fulano.tal", event: due(beforeMidnight, "2026-11-05")},
		{name: "return_due_soon at 23:59 local, date in 2 days", zone: "America/Boa_Vista", self: "fulano.tal", event: due(beforeMidnight, "2026-11-06")},
		{name: "return_due_soon at 00:00 local, date the day before", zone: "America/Boa_Vista", self: "fulano.tal", event: due(midnight, "2026-11-04")},
		{name: "return_due_soon at 00:00 local, date today", zone: "America/Boa_Vista", self: "fulano.tal", event: due(midnight, "2026-11-05")},
		{name: "return_due_soon at 00:00 local, date tomorrow", zone: "America/Boa_Vista", self: "fulano.tal", event: due(midnight, "2026-11-06")},
		{name: "return_due_soon at 03:59 UTC, date today", zone: "UTC", self: "fulano.tal", event: due(beforeMidnight, "2026-11-05")},
		{name: "return_due_soon at 23:59:59 in Sao Paulo, date today", zone: "America/Sao_Paulo", self: "fulano.tal", event: due(saoPauloBeforeMidnight, "2026-11-04")},
		{name: "return_due_soon a year ahead", zone: "America/Boa_Vista", self: "fulano.tal", event: due(midnight, "2027-11-05")},
		{name: "return_due_soon without a date", zone: "America/Boa_Vista", self: "fulano.tal", event: due(midnight, "")},
		{name: "return_due_soon with a date in another layout", zone: "America/Boa_Vista", self: "fulano.tal", event: due(midnight, "05/11/2026")},
		{name: "return_due_soon with every payload field", zone: "America/Boa_Vista", self: "fulano.tal", event: everyField},
	}
}

func loginFailureInputs(detected time.Time) []eventTextInput {
	failure := func(reason string) Event {
		return Event{
			ID: 7, Kind: AccountLoginFailed, Ref: "login:2026-11-04T18:00:00Z",
			OccurredAt: detected, DetectedAt: detected, Payload: Payload{Reason: reason},
		}
	}
	return []eventTextInput{
		{name: "account_login_failed with the SEI message", zone: "America/Boa_Vista", self: "fulano.tal", event: failure("SEI rejected the login (Usuário ou Senha Inválida.)")},
		{name: "account_login_failed without a message", zone: "UTC", self: "", event: failure("SEI rejected the login")},
		{name: "account_login_failed without a reason", zone: "UTC", self: "fulano.tal", event: failure("")},
	}
}

type shortenCase struct {
	Text   string `json:"text"`
	Limit  int    `json:"limit"`
	Output string `json:"output"`
}

func shortenVectors() []shortenCase {
	inputs := []struct {
		text  string
		limit int
	}{
		{"", 5},
		{"abcde", 5},
		{"abcdef", 5},
		{"abcd", 5},
		{"ãçéõú", 5},
		{"ãçéõúê", 5},
		{"ãçéõ", 5},
		{"ab cd", 5},
		{"abcd efgh", 6},
		{"abcd efgh", 5},
		{"a  b\t\tc\n\nd", 7},
		{"a  b\t\tc\n\nd e", 7},
		{"  abcde  ", 5},
		{"   ", 5},
		{" \t\n\r\v\f", 5},
		{"\u00a0\u3000\u2028\u0085", 5},
		{"a\u00a0b\u3000c\u2028d\u0085e", 9},
		{"a\u00a0b\u3000c\u2028d\u0085e f", 9},
		{"a\u200bb\u200bc", 5},
		{"a\u200bb\u200bc\u200b", 5},
		{"😀😀😀😀😀", 5},
		{"😀😀😀😀😀😀", 5},
		{"e\u0301e\u0301e\u0301", 5},
		{"e\u0301e\u0301e\u0301", 6},
		{"abc", 1},
		{"a", 1},
		{"ab", 2},
		{strings.Repeat("ã", 40), 40},
		{strings.Repeat("ã", 41), 40},
		{strings.Repeat("çã", 40), 80},
		{"é" + strings.Repeat("çã", 40), 80},
		{"Aquisição de cadeiras ergonômicas para o setor de protocolo", 40},
		{"Processo remetido pela unidade SEDEMO/NUPROC\n  para a unidade SEDEMO/PROTOCOLO com retorno programado", 80},
	}
	cases := make([]shortenCase, len(inputs))
	for i, input := range inputs {
		cases[i] = shortenCase{Text: input.text, Limit: input.limit, Output: shorten(input.text, input.limit)}
	}
	return cases
}
