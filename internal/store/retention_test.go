package store

import (
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

func TestPruneBeforeRemovesOnlyWhatIsOlderThanTheCutoff(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)
		staying := Process{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}
		leftEarly := Process{ProcedureID: 9002, Number: "00000.000002/2026-00", UnitID: 1001}
		leftLate := Process{ProcedureID: 9003, Number: "00000.000003/2026-00", UnitID: 1001}
		early, late, cutoff := testNow, testNow.AddDate(0, 0, 40), testNow.AddDate(0, 0, 30)

		cycles := []CycleChanges{
			{
				Now:       early,
				Processes: []Process{staying, leftEarly, leftLate},
				Documents: []Document{{DocumentID: 80001, ProcedureID: 9001}, {DocumentID: 80002, ProcedureID: 9002}},
				Events:    []event.Event{{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "50001"}},
				Channels:  []string{"webhook"},
			},
			{Now: early.Add(time.Hour), Processes: []Process{staying, leftLate}, Left: []int64{9002}},
			{
				Now:       late,
				Processes: []Process{staying},
				Left:      []int64{9003},
				Events:    []event.Event{{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "50002"}},
				Channels:  []string{"webhook"},
			},
		}
		for _, changes := range cycles {
			changes.AccountID = account.ID
			if _, err := s.ApplyCycle(ctx, changes); err != nil {
				t.Fatal(err)
			}
		}

		pruned, err := s.PruneBefore(ctx, account.ID, cutoff)
		if err != nil {
			t.Fatal(err)
		}

		if pruned != (Pruned{Events: 1, Processes: 1}) {
			t.Errorf("pruned = %+v, want the early event and the process that left early", pruned)
		}
		events, err := s.RecentEvents(ctx, 10)
		if err != nil {
			t.Fatal(err)
		}
		if len(events) != 1 || events[0].Ref != "50002" {
			t.Errorf("events left = %+v, want only the one detected after the cutoff", events)
		}
		processes, err := s.Processes(ctx, account.ID)
		if err != nil {
			t.Fatal(err)
		}
		ids := []int64{}
		for _, p := range processes {
			ids = append(ids, p.ProcedureID)
		}
		if !slices.Equal(ids, []int64{9001, 9003}) {
			t.Errorf("processes left = %v, want the one still present and the one that left after the cutoff", ids)
		}
		documents, err := s.DocumentIDs(ctx, account.ID, 9001)
		if err != nil || !documents[80001] {
			t.Errorf("documents of the present process = %v, %v, want them kept", documents, err)
		}
		var deliveries, orphanDocuments int
		if err := s.db.QueryRowContext(ctx, `SELECT COUNT(*) FROM deliveries`).Scan(&deliveries); err != nil {
			t.Fatal(err)
		}
		if err := s.db.QueryRowContext(ctx, `SELECT COUNT(*) FROM documents WHERE procedure_id = 9002`).Scan(&orphanDocuments); err != nil {
			t.Fatal(err)
		}
		if deliveries != 1 || orphanDocuments != 0 {
			t.Errorf("deliveries = %d, documents of the pruned process = %d, want 1 and 0", deliveries, orphanDocuments)
		}
	})
}
