package event

import (
	"encoding/json"
	"strings"
	"testing"
	"time"
)

func sampleEvent() Event {
	return Event{
		ID: 42, Kind: ActivityAdded, ProcedureID: 9001,
		OccurredAt: time.Date(2026, 11, 4, 17, 13, 5, 0, time.FixedZone("UTC-4", -4*3600)),
		DetectedAt: time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC),
		Payload: Payload{
			ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC", ProcessType: "Contrato", Description: "Compra de cadeiras",
			ActivityText: "Processo recebido na unidade", ActivityUser: "beltrano.silva", ActivityUnit: "SEDEMO/PROTOCOLO",
		},
	}
}

func encode(t *testing.T, view View) map[string]any {
	t.Helper()
	raw, err := json.Marshal(view)
	if err != nil {
		t.Fatal(err)
	}
	var decoded map[string]any
	if err := json.Unmarshal(raw, &decoded); err != nil {
		t.Fatal(err)
	}
	return decoded
}

func TestMinimalViewLeavesTheDetailsOut(t *testing.T) {
	view := encode(t, Formatter{Location: time.UTC}.View(sampleEvent()))

	if view["id"] != float64(42) || view["kind"] != "activity_added" || view["process_number"] != "00000.000001/2026-00" || view["unit"] != "SEDEMO/NUPROC" {
		t.Errorf("view = %v", view)
	}
	if view["occurred_at"] != "2026-11-04T21:13:05Z" || view["title"] != "New activity on process 00000.000001/2026-00 (2026-11-04 21:13)" {
		t.Errorf("time and title = %v, %v", view["occurred_at"], view["title"])
	}
	if detail := view["detail"].(map[string]any); len(detail) != 0 {
		t.Errorf("minimal detail = %v, want no fields at all", detail)
	}
}

func TestFullViewShowsTheDetailsButNeverTheUserOwnAcronym(t *testing.T) {
	ev := sampleEvent()
	formatter := Formatter{Location: time.UTC, Full: true, Self: "Fulano.Tal"}

	other := encode(t, formatter.View(ev))["detail"].(map[string]any)
	ev.Payload.ActivityUser = "fulano.tal"
	own := encode(t, formatter.View(ev))["detail"].(map[string]any)

	if other["activity_text"] != "Processo recebido na unidade" || other["activity_user"] != "beltrano.silva" || other["process_type"] != "Contrato" || other["description"] != "Compra de cadeiras" {
		t.Errorf("full detail = %v", other)
	}
	if _, shown := own["activity_user"]; shown || own["activity_by_me"] != true {
		t.Errorf("detail of the user's own activity = %v", own)
	}
}

func TestAssignmentViewHidesTheUserBehindAssignedToMe(t *testing.T) {
	ev := Event{Kind: AssignmentChanged, Payload: Payload{
		ProcessNumber: "00000.000001/2026-00", AssignedUser: "fulano.tal", AssignedUserName: "Fulano de Tal",
		AssignedToSelf: true, PreviousAssignee: "beltrano.silva",
	}}

	raw, err := json.Marshal(Formatter{Full: true, Self: "fulano.tal"}.View(ev))
	if err != nil {
		t.Fatal(err)
	}

	text := string(raw)
	if strings.Contains(text, "fulano.tal") || strings.Contains(text, "Fulano de Tal") || !strings.Contains(text, `"assigned_to_me":true`) || !strings.Contains(text, `"previous_assigned_user":"beltrano.silva"`) {
		t.Errorf("view = %s", text)
	}
}
