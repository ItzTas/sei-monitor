package notify_test

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/notify"
)

var t0 = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

func activity(number string, at time.Time) event.Event {
	return event.Event{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: number, OccurredAt: at, DetectedAt: t0, Payload: event.Payload{
		ProcessNumber: number, Unit: "SEDEMO/NUPROC",
		ActivityText: "Processo recebido na unidade", ActivityUser: "beltrano.silva", ActivityUnit: "SEDEMO/PROTOCOLO",
	}}
}

func TestMinimalMessageListsOneTitlePerLine(t *testing.T) {
	events := []event.Event{
		activity("00000.000001/2026-00", t0.Add(-time.Hour)),
		{Kind: event.DocumentAdded, Payload: event.Payload{ProcessNumber: "00000.000002/2026-00", DocumentName: "Ofício 1 (0012345)"}},
	}

	msg := notify.Compose(events, event.Formatter{Location: time.UTC})

	want := "New activity on process 00000.000001/2026-00 (2026-11-04 17:00)\nNew document on process 00000.000002/2026-00"
	if msg.Subject != "2 updates on your processes" || msg.Body != want || len(msg.Events) != 2 || msg.Urgent || msg.Test {
		t.Errorf("message = %+v", msg)
	}
	if msg.Events[1].Detail.DocumentName != "" {
		t.Errorf("the minimal event view shows the document name: %+v", msg.Events[1])
	}
}

func TestFullMessageIndentsTheDetailsUnderEachTitle(t *testing.T) {
	events := []event.Event{activity("00000.000001/2026-00", t0), activity("00000.000002/2026-00", t0)}

	msg := notify.Compose(events, event.Formatter{Location: time.UTC, Full: true, Self: "fulano.tal"})

	first := "00000.000001/2026-00: Processo recebido na unidade\n" +
		"  When: 2026-11-04 18:00\n  Unit: SEDEMO/NUPROC\n  Activity: Processo recebido na unidade (beltrano.silva, SEDEMO/PROTOCOLO)"
	if !strings.HasPrefix(msg.Body, first+"\n\n00000.000002/2026-00: Processo recebido na unidade") {
		t.Errorf("body = %q", msg.Body)
	}
	if msg.Events[0].Detail.ActivityText != "Processo recebido na unidade" {
		t.Errorf("the full event view misses the activity text: %+v", msg.Events[0])
	}
}

func TestLongMessageListsTenAndCountsTheRest(t *testing.T) {
	var events []event.Event
	for i := range 12 {
		events = append(events, activity(fmt.Sprintf("00000.%06d/2026-00", i+1), t0))
	}

	msg := notify.Compose(events, event.Formatter{Location: time.UTC})

	lines := strings.Split(msg.Body, "\n")
	if msg.Subject != "12 updates on your processes" || len(msg.Events) != 12 || len(lines) != 12 ||
		lines[9] != "New activity on process 00000.000010/2026-00 (2026-11-04 18:00)" || lines[10] != "" || lines[11] != "and 2 more" {
		t.Errorf("subject %q, %d events, body:\n%s", msg.Subject, len(msg.Events), msg.Body)
	}
}

func TestUrgentKinds(t *testing.T) {
	tests := map[event.Kind]bool{
		event.ActivityAdded:      false,
		event.ReturnDueSoon:      false,
		event.ReturnOverdue:      true,
		event.AccountLoginFailed: true,
	}
	for kind, want := range tests {
		events := []event.Event{activity("00000.000001/2026-00", t0), {Kind: kind}}
		if got := notify.Compose(events, event.Formatter{}).Urgent; got != want {
			t.Errorf("urgent with %s = %v, want %v", kind, got, want)
		}
	}
}

func TestLoginFailureExplainsItselfAtTheMinimalLevel(t *testing.T) {
	ev := event.Event{Kind: event.AccountLoginFailed, Payload: event.Payload{Reason: "SEI rejected the login (Usuário ou Senha Inválida.)"}}

	msg := notify.Compose([]event.Event{ev}, event.Formatter{})

	want := "sei-monitor could not log in to SEI\n" +
		"  Reason: SEI rejected the login (Usuário ou Senha Inválida.)\n" +
		"  Monitoring is paused until sei-monitor restarts with the configuration fixed."
	if msg.Body != want {
		t.Errorf("body = %q", msg.Body)
	}
}
