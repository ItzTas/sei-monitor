package monitor

import (
	"errors"
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/event"
)

var repoNow = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

var repoKey = core.AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}

type repoFixture struct {
	repo     *Postgres
	database *core.Database
	account  core.Account
}

func newRepoFixture(t *testing.T) repoFixture {
	t.Helper()
	database := coretest.Open(t)
	repo := NewPostgres(database.Pool)
	account, err := repo.EnsureAccount(t.Context(), repoKey, repoNow)
	if err != nil {
		t.Fatal(err)
	}
	return repoFixture{repo: repo, database: database, account: account}
}

func (f repoFixture) apply(t *testing.T, changes CycleChanges) []event.Event {
	t.Helper()
	changes.AccountID = f.account.ID
	inserted, err := f.repo.ApplyCycle(t.Context(), changes)
	if err != nil {
		t.Fatal(err)
	}
	return inserted
}

func (f repoFixture) count(t *testing.T, query string, args ...any) int {
	t.Helper()
	var count int
	if err := f.database.Pool.QueryRow(t.Context(), query, args...).Scan(&count); err != nil {
		t.Fatal(err)
	}
	return count
}

func (f repoFixture) reloadAccount(t *testing.T) core.Account {
	t.Helper()
	account, err := f.repo.Account(t.Context(), f.account.ID)
	if err != nil {
		t.Fatal(err)
	}
	return account
}

func TestEnsureAccountReturnsTheSameAccountPerKey(t *testing.T) {
	f := newRepoFixture(t)

	again, err := f.repo.EnsureAccount(t.Context(), repoKey, repoNow.Add(time.Hour))
	if err != nil {
		t.Fatal(err)
	}
	other, err := f.repo.EnsureAccount(t.Context(), core.AccountKey{SEIBaseURL: repoKey.SEIBaseURL, Username: repoKey.Username, OrgID: 64}, repoNow)
	if err != nil {
		t.Fatal(err)
	}

	if again != f.account || !again.CreatedAt.Equal(repoNow) || again.Key != repoKey || !again.BaselineAt.IsZero() || again.PausedReason != "" {
		t.Errorf("account again = %+v, want %+v", again, f.account)
	}
	if other.ID == f.account.ID {
		t.Errorf("another agency got the same account %d", other.ID)
	}
}

func TestAccountByIDAndItsDetails(t *testing.T) {
	f := newRepoFixture(t)
	info := core.AccountInfo{OrgAcronym: "SEDEMO", SEIVersion: "5.0.5", WSSEIVersion: "3.0.4"}

	if err := f.repo.UpdateAccountInfo(t.Context(), f.account.ID, info); err != nil {
		t.Fatal(err)
	}
	if err := f.repo.SetPausedReason(t.Context(), f.account.ID, "SEI refused the login"); err != nil {
		t.Fatal(err)
	}

	want := f.account
	want.Info, want.PausedReason = info, "SEI refused the login"
	if got := f.reloadAccount(t); got != want {
		t.Errorf("account = %+v, want %+v", got, want)
	}
	if _, err := f.repo.Account(t.Context(), f.account.ID+1000); !errors.Is(err, core.ErrNotFound) {
		t.Errorf("unknown account: error = %v, want core.ErrNotFound", err)
	}
	if err := f.repo.SetPausedReason(t.Context(), f.account.ID+1000, "x"); !errors.Is(err, ErrAccountNotFound) {
		t.Errorf("pausing an unknown account: error = %v, want ErrAccountNotFound", err)
	}
}

func TestApplyCycleKeepsTheSnapshot(t *testing.T) {
	f := newRepoFixture(t)
	first := core.Process{
		ProcedureID: 9001, Number: "00000.000001/2026-00", Type: "Contrato", Description: "Compra de cadeiras",
		UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC", AssignedUser: "fulano.tal", AssignedUserID: 101,
		ReturnDate: "2026-11-06", ReturnOverdue: true, LastActivityID: 50003, DocumentTotal: 2,
	}
	second := core.Process{ProcedureID: 9002, Number: "00000.000002/2026-00", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC"}
	document := core.Document{DocumentID: 80001, ProcedureID: 9001, Protocol: "0012345", Type: "Ofício", Name: "Ofício 12 (0012345)"}

	f.apply(t, CycleChanges{Now: repoNow, Baseline: true, Processes: []core.Process{first, second}, Documents: []core.Document{document}})
	renamed := document
	renamed.Name = "Ofício 12 renomeado"
	f.apply(t, CycleChanges{Now: repoNow.Add(time.Hour), Baseline: true, Processes: []core.Process{first, second}, Documents: []core.Document{renamed}})

	stored, err := f.repo.Processes(t.Context(), f.account.ID)
	if err != nil {
		t.Fatal(err)
	}
	first.FirstSeenAt, first.LastSeenAt = repoNow, repoNow.Add(time.Hour)
	second.FirstSeenAt, second.LastSeenAt = repoNow, repoNow.Add(time.Hour)
	if !slices.Equal(stored, []core.Process{first, second}) {
		t.Errorf("processes = %+v\nwant %+v", stored, []core.Process{first, second})
	}
	documents, err := f.repo.DocumentIDs(t.Context(), f.account.ID, 9001)
	if err != nil || len(documents) != 1 || !documents[80001] {
		t.Errorf("document ids = %v, %v", documents, err)
	}
	if names := f.count(t, `SELECT COUNT(*) FROM documents WHERE name = 'Ofício 12 (0012345)'`); names != 1 {
		t.Errorf("documents keeping the first name = %d, want 1", names)
	}
	if got := f.reloadAccount(t).BaselineAt; !got.Equal(repoNow) {
		t.Errorf("baseline = %v, want the first cycle at %v", got, repoNow)
	}
}

func TestProcessesLeaveAndComeBack(t *testing.T) {
	f := newRepoFixture(t)
	staying := core.Process{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}
	leaving := core.Process{ProcedureID: 9002, Number: "00000.000002/2026-00", UnitID: 1001}
	processes := func() []core.Process {
		t.Helper()
		stored, err := f.repo.Processes(t.Context(), f.account.ID)
		if err != nil {
			t.Fatal(err)
		}
		return stored
	}

	f.apply(t, CycleChanges{Now: repoNow, Processes: []core.Process{staying, leaving}})
	f.apply(t, CycleChanges{Now: repoNow.Add(15 * time.Minute), Processes: []core.Process{staying}, Left: []int64{9002}})
	f.apply(t, CycleChanges{Now: repoNow.Add(20 * time.Minute), Processes: []core.Process{staying}, Left: []int64{9002}})

	gone := processes()
	if !gone[1].LeftAt.Equal(repoNow.Add(15*time.Minute)) || !gone[0].LeftAt.IsZero() {
		t.Errorf("after leaving = %+v", gone)
	}

	f.apply(t, CycleChanges{Now: repoNow.Add(30 * time.Minute), Processes: []core.Process{staying, leaving}})

	back := processes()[1]
	if !back.LeftAt.IsZero() || !back.FirstSeenAt.Equal(repoNow) || !back.LastSeenAt.Equal(repoNow.Add(30*time.Minute)) {
		t.Errorf("process that came back = %+v", back)
	}
}

func TestEventsAreStoredOnceWithAPendingDeliveryPerChannel(t *testing.T) {
	f := newRepoFixture(t)
	changes := CycleChanges{
		Now: repoNow,
		Events: []event.Event{
			{Kind: event.ProcessEntered, ProcedureID: 9001, Ref: "9001:1", Payload: event.Payload{ProcessNumber: "00000.000001/2026-00"}},
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "50003", OccurredAt: repoNow.Add(-time.Hour), Payload: event.Payload{ActivityText: "Processo recebido"}},
		},
		Channels: []string{"telegram", "webhook"},
	}

	inserted := f.apply(t, changes)
	changes.Events = append(changes.Events, event.Event{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "50004"})
	again := f.apply(t, changes)

	if len(inserted) != 2 || inserted[0].ID == 0 || inserted[0].ID == inserted[1].ID || inserted[0].AccountID != f.account.ID {
		t.Fatalf("inserted = %+v", inserted)
	}
	if !inserted[0].OccurredAt.Equal(repoNow) || !inserted[0].DetectedAt.Equal(repoNow) || !inserted[1].OccurredAt.Equal(repoNow.Add(-time.Hour)) {
		t.Errorf("times = %+v", inserted)
	}
	if len(again) != 1 || again[0].Ref != "50004" {
		t.Errorf("second cycle inserted %+v, want only the new event", again)
	}
	if events := f.count(t, `SELECT COUNT(*) FROM events`); events != 3 {
		t.Errorf("events = %d, want 3", events)
	}
	perEvent := f.count(t, `
		SELECT COUNT(*) FROM (
			SELECT event_id FROM deliveries
			WHERE status = 'pending' AND attempts = 0 AND next_attempt_at = $1
			GROUP BY event_id HAVING array_agg(channel ORDER BY channel) = ARRAY['telegram', 'webhook']
		) AS queued`, repoNow)
	if total := f.count(t, `SELECT COUNT(*) FROM deliveries`); total != 6 || perEvent != 3 {
		t.Errorf("deliveries = %d, events with one pending delivery per channel = %d, want 6 and 3", total, perEvent)
	}
	stored := recentEvents(t, f.database.Pool)
	if stored[1].Payload.ActivityText != "Processo recebido" {
		t.Errorf("stored payload = %+v", stored[1].Payload)
	}
}

func TestApplyCycleIsAllOrNothing(t *testing.T) {
	f := newRepoFixture(t)
	run, err := f.repo.StartCycleRun(t.Context(), f.account.ID, TriggerScheduled, repoNow)
	if err != nil {
		t.Fatal(err)
	}

	_, err = f.repo.ApplyCycle(t.Context(), CycleChanges{
		AccountID: f.account.ID, RunID: run, Now: repoNow, FinishedAt: repoNow.Add(time.Minute), Baseline: true,
		Processes: []core.Process{{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}},
		Documents: []core.Document{{DocumentID: 80001, ProcedureID: 9999}},
		Events:    []event.Event{{Kind: event.ProcessEntered, ProcedureID: 9001, Ref: "9001:1"}},
		Channels:  []string{"webhook"},
	})

	if err == nil {
		t.Fatal("a document of an unknown process was accepted")
	}
	if rows := f.count(t, `SELECT (SELECT COUNT(*) FROM processes) + (SELECT COUNT(*) FROM events) + (SELECT COUNT(*) FROM deliveries)`); rows != 0 {
		t.Errorf("rows left by the failed cycle = %d", rows)
	}
	if got := f.reloadAccount(t).BaselineAt; !got.IsZero() {
		t.Errorf("baseline after the failed cycle = %v", got)
	}
	if last, _, err := f.repo.LastCycleRun(t.Context(), f.account.ID); err != nil || !last.FinishedAt.IsZero() {
		t.Errorf("run after the failed cycle = %+v, %v, want it unfinished", last, err)
	}
}

func TestCycleRuns(t *testing.T) {
	f := newRepoFixture(t)
	if _, found, err := f.repo.LastCycleRun(t.Context(), f.account.ID); found || err != nil {
		t.Fatalf("last run before any cycle = %v, %v", found, err)
	}

	failed, err := f.repo.StartCycleRun(t.Context(), f.account.ID, TriggerScheduled, repoNow)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.repo.FinishCycleRun(t.Context(), core.CycleRun{ID: failed, AccountID: f.account.ID, FinishedAt: repoNow.Add(time.Second), Error: "SEI answered 500"}); err != nil {
		t.Fatal(err)
	}
	if last, _, err := f.repo.LastCycleRun(t.Context(), f.account.ID); err != nil || last.ID != failed || last.Error != "SEI answered 500" {
		t.Errorf("failed run = %+v, %v", last, err)
	}

	manual, err := f.repo.StartCycleRun(t.Context(), f.account.ID, TriggerManual, repoNow.Add(time.Minute))
	if err != nil {
		t.Fatal(err)
	}
	f.apply(t, CycleChanges{
		RunID: manual, Now: repoNow.Add(time.Minute), FinishedAt: repoNow.Add(2 * time.Minute),
		Processes: []core.Process{{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}},
		Events:    []event.Event{{Kind: event.ProcessEntered, ProcedureID: 9001, Ref: "9001:1"}},
	})

	last, found, err := f.repo.LastCycleRun(t.Context(), f.account.ID)
	want := core.CycleRun{
		ID: manual, AccountID: f.account.ID, Trigger: TriggerManual,
		StartedAt: repoNow.Add(time.Minute), FinishedAt: repoNow.Add(2 * time.Minute),
		ProcessesSeen: 1, EventsCreated: 1,
	}
	if err != nil || !found || last != want {
		t.Errorf("last run = %+v, %v, %v\nwant %+v", last, found, err, want)
	}
}

func TestOnlyTheLatestCycleRunsAreKept(t *testing.T) {
	f := newRepoFixture(t)
	var first int64
	err := f.database.Pool.QueryRow(t.Context(), `
		INSERT INTO cycle_runs (account_id, "trigger", started_at, finished_at)
		SELECT $1, 'scheduled', $2::timestamptz + make_interval(mins => n), $2::timestamptz + make_interval(mins => n)
		FROM generate_series(0, $3::int - 1) AS n
		RETURNING id`, f.account.ID, repoNow, keptRunsPerAccount+2).Scan(&first)
	if err != nil {
		t.Fatal(err)
	}

	latest, err := f.repo.StartCycleRun(t.Context(), f.account.ID, TriggerScheduled, repoNow.Add(24*time.Hour))
	if err != nil {
		t.Fatal(err)
	}
	if err := f.repo.FinishCycleRun(t.Context(), core.CycleRun{ID: latest, AccountID: f.account.ID, FinishedAt: repoNow.Add(24 * time.Hour)}); err != nil {
		t.Fatal(err)
	}

	count := f.count(t, `SELECT COUNT(*) FROM cycle_runs WHERE account_id = $1`, f.account.ID)
	kept := f.count(t, `SELECT COUNT(*) FROM cycle_runs WHERE account_id = $1 AND id > $2`, f.account.ID, latest-keptRunsPerAccount)
	if count != keptRunsPerAccount || kept != keptRunsPerAccount {
		t.Errorf("runs = %d, newest %d kept = %d, want %d", count, keptRunsPerAccount, kept, keptRunsPerAccount)
	}
}

func TestPruneBeforeRemovesOnlyWhatIsOlderThanTheCutoff(t *testing.T) {
	f := newRepoFixture(t)
	staying := core.Process{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}
	leftEarly := core.Process{ProcedureID: 9002, Number: "00000.000002/2026-00", UnitID: 1001}
	leftLate := core.Process{ProcedureID: 9003, Number: "00000.000003/2026-00", UnitID: 1001}
	early, late, cutoff := repoNow, repoNow.AddDate(0, 0, 40), repoNow.AddDate(0, 0, 30)
	f.apply(t, CycleChanges{
		Now:       early,
		Processes: []core.Process{staying, leftEarly, leftLate},
		Documents: []core.Document{{DocumentID: 80001, ProcedureID: 9001}, {DocumentID: 80002, ProcedureID: 9002}},
		Events:    []event.Event{{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "50001"}},
		Channels:  []string{"webhook"},
	})
	f.apply(t, CycleChanges{Now: early.Add(time.Hour), Processes: []core.Process{staying, leftLate}, Left: []int64{9002}})
	f.apply(t, CycleChanges{
		Now:       late,
		Processes: []core.Process{staying},
		Left:      []int64{9003},
		Events:    []event.Event{{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "50002"}},
		Channels:  []string{"webhook"},
	})

	pruned, err := f.repo.PruneBefore(t.Context(), f.account.ID, cutoff)
	if err != nil {
		t.Fatal(err)
	}

	if pruned != (Pruned{Events: 1, Processes: 1}) {
		t.Errorf("pruned = %+v, want the early event and the process that left early", pruned)
	}
	if events := recentEvents(t, f.database.Pool); len(events) != 1 || events[0].Ref != "50002" {
		t.Errorf("events left = %+v", events)
	}
	stored, err := f.repo.Processes(t.Context(), f.account.ID)
	if err != nil {
		t.Fatal(err)
	}
	ids := core.Map(stored, func(p core.Process) int64 { return p.ProcedureID })
	if !slices.Equal(ids, []int64{9001, 9003}) {
		t.Errorf("processes left = %v, want 9001 and 9003", ids)
	}
	if documents, err := f.repo.DocumentIDs(t.Context(), f.account.ID, 9001); err != nil || !documents[80001] {
		t.Errorf("documents of the present process = %v, %v", documents, err)
	}
	deliveries := f.count(t, `SELECT COUNT(*) FROM deliveries`)
	orphans := f.count(t, `SELECT COUNT(*) FROM documents WHERE procedure_id = 9002`)
	if deliveries != 1 || orphans != 0 {
		t.Errorf("deliveries = %d, documents of the pruned process = %d, want 1 and 0", deliveries, orphans)
	}
}
