package store

import (
	"encoding/json"
	"os"
	"slices"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

var testNow = time.Date(2026, 11, 4, 15, 4, 5, 123456789, time.UTC)

func TestMain(m *testing.M) {
	os.Exit(storetest.Run(m, Migrate))
}

func openStore(t *testing.T, databaseURL string) *Store {
	t.Helper()
	s, err := Open(t.Context(), databaseURL)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := s.Close(); err != nil {
			t.Error(err)
		}
	})
	return s
}

func newAccount(t *testing.T, s *Store) Account {
	t.Helper()
	account, err := s.EnsureAccount(t.Context(), AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}, testNow)
	if err != nil {
		t.Fatal(err)
	}
	return account
}

func micro(t time.Time) time.Time {
	return t.UTC().Truncate(time.Microsecond)
}

func TestOpeningAgainKeepsTheSchema(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		first := openStore(t, databaseURL)
		account := newAccount(t, first)

		second := openStore(t, databaseURL)
		again := newAccount(t, second)

		if again.ID != account.ID {
			t.Errorf("account id after reopening = %d, want %d", again.ID, account.ID)
		}
	})
}

func TestEnsureAccount(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		key := AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}

		first, err := s.EnsureAccount(ctx, key, testNow)
		if err != nil {
			t.Fatal(err)
		}
		again, err := s.EnsureAccount(ctx, key, testNow.Add(time.Hour))
		if err != nil {
			t.Fatal(err)
		}
		other, err := s.EnsureAccount(ctx, AccountKey{SEIBaseURL: key.SEIBaseURL, Username: "beltrano", OrgID: 63}, testNow)
		if err != nil {
			t.Fatal(err)
		}

		if first.ID == 0 || again.ID != first.ID || other.ID == first.ID {
			t.Errorf("ids = %d, %d, %d; want the same id for the same key", first.ID, again.ID, other.ID)
		}
		if !again.CreatedAt.Equal(micro(testNow)) || !again.BaselineAt.IsZero() || again.PausedReason != "" || again.Key != key {
			t.Errorf("account = %+v", again)
		}

		if err := s.SetPausedReason(ctx, first.ID, "SEI rejected the login"); err != nil {
			t.Fatal(err)
		}
		paused, err := s.EnsureAccount(ctx, key, testNow)
		if err != nil || paused.PausedReason != "SEI rejected the login" {
			t.Errorf("paused account = %+v, %v", paused, err)
		}
		if err := s.SetPausedReason(ctx, 999999, "x"); err == nil {
			t.Error("pausing an account that does not exist returned no error")
		}
	})
}

func TestApplyCycleKeepsTheSnapshot(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)
		first := Process{
			ProcedureID: 9001, Number: "00000.000001/2026-00", Type: "Contrato", Description: "Compra de cadeiras",
			UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC", AssignedUser: "fulano.tal", AssignedUserID: 101,
			ReturnDate: "2026-11-06", ReturnOverdue: true, LastActivityID: 50003, DocumentTotal: 2,
		}
		second := Process{ProcedureID: 9002, Number: "00000.000002/2026-00", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC"}

		_, err := s.ApplyCycle(ctx, CycleChanges{
			AccountID: account.ID, Now: testNow, Baseline: true,
			Processes: []Process{first, second},
			Documents: []Document{{DocumentID: 80001, ProcedureID: 9001, Protocol: "0012345", Type: "Ofício", Name: "Ofício 12 (0012345)"}},
		})
		if err != nil {
			t.Fatal(err)
		}

		stored, err := s.Processes(ctx, account.ID)
		if err != nil {
			t.Fatal(err)
		}
		first.FirstSeenAt, first.LastSeenAt = micro(testNow), micro(testNow)
		second.FirstSeenAt, second.LastSeenAt = micro(testNow), micro(testNow)
		if len(stored) != 2 || stored[0] != first || stored[1] != second {
			t.Fatalf("processes = %+v\nwant %+v\nand  %+v", stored, first, second)
		}
		documents, err := s.DocumentIDs(ctx, account.ID, 9001)
		if err != nil || len(documents) != 1 || !documents[80001] {
			t.Errorf("documents = %v, %v", documents, err)
		}
		if marked := newAccount(t, s); !marked.BaselineAt.Equal(micro(testNow)) {
			t.Errorf("baseline = %v, want %v", marked.BaselineAt, micro(testNow))
		}
	})
}

func TestProcessesLeaveAndComeBack(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)
		staying := Process{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}
		leaving := Process{ProcedureID: 9002, Number: "00000.000002/2026-00", UnitID: 1001}
		apply := func(now time.Time, processes []Process, left []int64) {
			t.Helper()
			if _, err := s.ApplyCycle(ctx, CycleChanges{AccountID: account.ID, Now: now, Processes: processes, Left: left}); err != nil {
				t.Fatal(err)
			}
		}

		apply(testNow, []Process{staying, leaving}, nil)
		staying.AssignedUser = "beltrano"
		apply(testNow.Add(15*time.Minute), []Process{staying}, []int64{9002})
		gone, err := s.Processes(ctx, account.ID)
		if err != nil {
			t.Fatal(err)
		}
		if gone[0].AssignedUser != "beltrano" || !gone[0].LastSeenAt.Equal(micro(testNow.Add(15*time.Minute))) || !gone[0].FirstSeenAt.Equal(micro(testNow)) {
			t.Errorf("updated process = %+v", gone[0])
		}
		if !gone[1].LeftAt.Equal(micro(testNow.Add(15 * time.Minute))) {
			t.Errorf("process that left = %+v", gone[1])
		}

		apply(testNow.Add(30*time.Minute), []Process{staying, leaving}, nil)
		back, err := s.Processes(ctx, account.ID)
		if err != nil {
			t.Fatal(err)
		}
		if !back[1].LeftAt.IsZero() || !back[1].FirstSeenAt.Equal(micro(testNow)) {
			t.Errorf("process that came back = %+v", back[1])
		}
	})
}

func TestEventsAreStoredOnceWithTheirDeliveries(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)
		events := []event.Event{
			{Kind: event.ProcessEntered, ProcedureID: 9001, Ref: "9001:1", Payload: event.Payload{ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC"}},
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "50003", OccurredAt: testNow.Add(-time.Hour), Payload: event.Payload{ActivityText: "Processo recebido na unidade"}},
		}
		changes := CycleChanges{
			AccountID: account.ID, Now: testNow,
			Processes: []Process{{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}},
			Events:    events,
			Channels:  []string{"telegram", "webhook"},
		}

		inserted, err := s.ApplyCycle(ctx, changes)
		if err != nil {
			t.Fatal(err)
		}
		if len(inserted) != 2 || inserted[0].ID == 0 || inserted[1].ID == inserted[0].ID || inserted[0].AccountID != account.ID {
			t.Fatalf("inserted = %+v", inserted)
		}
		if !inserted[0].OccurredAt.Equal(testNow) || !inserted[1].OccurredAt.Equal(testNow.Add(-time.Hour)) {
			t.Errorf("occurred at = %v and %v", inserted[0].OccurredAt, inserted[1].OccurredAt)
		}

		again, err := s.ApplyCycle(ctx, changes)
		if err != nil {
			t.Fatal(err)
		}
		if len(again) != 0 {
			t.Errorf("applying the same events again inserted %+v", again)
		}

		rows, err := s.db.QueryContext(ctx, `SELECT d.channel, d.status, d.attempts, e.payload FROM deliveries d JOIN events e ON e.id = d.event_id ORDER BY d.event_id, d.channel`)
		if err != nil {
			t.Fatal(err)
		}
		defer func() { _ = rows.Close() }()
		var channels []string
		for rows.Next() {
			var channel, status, payload string
			var attempts int
			if err := rows.Scan(&channel, &status, &attempts, &payload); err != nil {
				t.Fatal(err)
			}
			var decoded event.Payload
			if err := json.Unmarshal([]byte(payload), &decoded); err != nil {
				t.Fatal(err)
			}
			if status != "pending" || attempts != 0 || (decoded != events[0].Payload && decoded != events[1].Payload) {
				t.Errorf("delivery = %s %s %d %+v", channel, status, attempts, decoded)
			}
			channels = append(channels, channel)
		}
		if err := rows.Err(); err != nil {
			t.Fatal(err)
		}
		if !slices.Equal(channels, []string{"telegram", "webhook", "telegram", "webhook"}) {
			t.Errorf("deliveries = %v, want one per channel and event", channels)
		}
	})
}

func TestApplyCycleIsAllOrNothing(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)

		_, err := s.ApplyCycle(ctx, CycleChanges{
			AccountID: account.ID, Now: testNow, Baseline: true,
			Processes: []Process{{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}},
			Documents: []Document{{DocumentID: 80001, ProcedureID: 9999}},
		})
		if err == nil {
			t.Fatal("a document of an unknown process was accepted")
		}

		processes, err := s.Processes(ctx, account.ID)
		if err != nil || len(processes) != 0 {
			t.Errorf("processes after the failed cycle = %+v, %v", processes, err)
		}
		if again := newAccount(t, s); !again.BaselineAt.IsZero() {
			t.Errorf("baseline after the failed cycle = %v", again.BaselineAt)
		}
	})
}

func TestRebind(t *testing.T) {
	tests := map[string]string{
		"SELECT ?, ?":                       "SELECT $1, $2",
		"SELECT '?', ?":                     "SELECT '?', $1",
		`SELECT "a?" FROM t WHERE x = ?`:    `SELECT "a?" FROM t WHERE x = $1`,
		"SELECT 1 -- why?\nWHERE x = ?":     "SELECT 1 -- why?\nWHERE x = $1",
		"/* ? */ SELECT ?":                  "/* ? */ SELECT $1",
		"SELECT 'it''s ?', ?":               "SELECT 'it''s ?', $1",
		"SELECT $1":                         "SELECT $1",
		"INSERT INTO t VALUES (?, ?, ?, ?)": "INSERT INTO t VALUES ($1, $2, $3, $4)",
	}
	for query, want := range tests {
		if got := rebind(query); got != want {
			t.Errorf("rebind(%q) = %q, want %q", query, got, want)
		}
	}
}

func TestStoredTimesSortAsText(t *testing.T) {
	times := []time.Time{
		time.Date(2026, 11, 4, 15, 4, 5, 0, time.UTC),
		time.Date(2026, 11, 4, 15, 4, 5, 100000, time.UTC),
		time.Date(2026, 11, 4, 15, 4, 5, 120000000, time.UTC),
		time.Date(2026, 11, 4, 12, 4, 6, 0, time.FixedZone("UTC-3", -3*3600)),
	}
	var texts []string
	for _, value := range times {
		text, err := dbTime(value).Value()
		if err != nil {
			t.Fatal(err)
		}
		texts = append(texts, text.(string))
	}
	if !slices.IsSorted(texts) {
		t.Errorf("stored times do not sort like the times: %q", texts)
	}
	var back dbTime
	if err := back.Scan(texts[1]); err != nil || !time.Time(back).Equal(times[1]) {
		t.Errorf("scan(%q) = %v, %v", texts[1], time.Time(back), err)
	}
}

func TestCycleRuns(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)
		if _, found, err := s.LastCycleRun(ctx, account.ID); found || err != nil {
			t.Fatalf("last run before any cycle = %v, %v", found, err)
		}

		failed, err := s.StartCycleRun(ctx, account.ID, TriggerScheduled, testNow)
		if err != nil {
			t.Fatal(err)
		}
		if err := s.FinishCycleRun(ctx, CycleRun{ID: failed, AccountID: account.ID, FinishedAt: testNow.Add(time.Second), Error: "SEI answered with an internal error"}); err != nil {
			t.Fatal(err)
		}
		if last, _, err := s.LastCycleRun(ctx, account.ID); err != nil || last.Error != "SEI answered with an internal error" || last.Trigger != TriggerScheduled {
			t.Errorf("failed run = %+v, %v", last, err)
		}

		manual, err := s.StartCycleRun(ctx, account.ID, TriggerManual, testNow.Add(time.Minute))
		if err != nil {
			t.Fatal(err)
		}
		_, err = s.ApplyCycle(ctx, CycleChanges{
			AccountID: account.ID, RunID: manual, Now: testNow.Add(time.Minute), FinishedAt: testNow.Add(2 * time.Minute),
			Processes: []Process{{ProcedureID: 9001, Number: "00000.000001/2026-00", UnitID: 1001}},
			Events:    []event.Event{{Kind: event.ProcessEntered, ProcedureID: 9001, Ref: "9001:1"}},
		})
		if err != nil {
			t.Fatal(err)
		}

		last, found, err := s.LastCycleRun(ctx, account.ID)
		want := CycleRun{
			ID: manual, AccountID: account.ID, Trigger: TriggerManual,
			StartedAt: micro(testNow.Add(time.Minute)), FinishedAt: micro(testNow.Add(2 * time.Minute)),
			ProcessesSeen: 1, EventsCreated: 1,
		}
		if err != nil || !found || last != want {
			t.Errorf("last run = %+v, %v, %v\nwant %+v", last, found, err, want)
		}
	})
}

func TestOnlyTheLatestCycleRunsAreKept(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)
		tx, err := s.db.BeginTx(ctx, nil)
		if err != nil {
			t.Fatal(err)
		}
		var first int64
		for i := range keptRunsPerAccount + 2 {
			var id int64
			err := tx.QueryRowContext(ctx, `INSERT INTO cycle_runs (account_id, "trigger", started_at, finished_at) VALUES (?, 'scheduled', ?, ?) RETURNING id`,
				account.ID, dbTime(testNow.Add(time.Duration(i)*time.Minute)), dbTime(testNow.Add(time.Duration(i)*time.Minute))).Scan(&id)
			if err != nil {
				t.Fatal(err)
			}
			if i == 0 {
				first = id
			}
		}
		if err := tx.Commit(); err != nil {
			t.Fatal(err)
		}

		latest, err := s.StartCycleRun(ctx, account.ID, TriggerScheduled, testNow.Add(24*time.Hour))
		if err != nil {
			t.Fatal(err)
		}
		if err := s.FinishCycleRun(ctx, CycleRun{ID: latest, AccountID: account.ID, FinishedAt: testNow.Add(24 * time.Hour)}); err != nil {
			t.Fatal(err)
		}

		var count int
		var oldest int64
		if err := s.db.QueryRowContext(ctx, `SELECT COUNT(*), MIN(id) FROM cycle_runs WHERE account_id = ?`, account.ID).Scan(&count, &oldest); err != nil {
			t.Fatal(err)
		}
		if count != keptRunsPerAccount || oldest != first+3 {
			t.Errorf("runs = %d starting at %d, want %d starting at %d", count, oldest, keptRunsPerAccount, first+3)
		}
	})
}

func TestAccountInfo(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		account := newAccount(t, s)
		info := AccountInfo{OrgAcronym: "SEDEMO", SEIVersion: "5.0.5", WSSEIVersion: "3.0.4"}

		if err := s.UpdateAccountInfo(t.Context(), account.ID, info); err != nil {
			t.Fatal(err)
		}

		if again := newAccount(t, s); again.Info != info {
			t.Errorf("account info = %+v, want %+v", again.Info, info)
		}
	})
}

func TestRecentEventsComeNewestFirst(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		s := openStore(t, databaseURL)
		ctx := t.Context()
		account := newAccount(t, s)
		for i, ref := range []string{"50001", "50002", "50003"} {
			_, err := s.ApplyCycle(ctx, CycleChanges{
				AccountID: account.ID, Now: testNow.Add(time.Duration(i) * time.Minute),
				Events: []event.Event{{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: ref, OccurredAt: testNow.Add(-time.Duration(i) * time.Hour), Payload: event.Payload{ActivityText: "andamento " + ref}}},
			})
			if err != nil {
				t.Fatal(err)
			}
		}

		events, err := s.RecentEvents(ctx, 2)
		if err != nil {
			t.Fatal(err)
		}

		if len(events) != 2 || events[0].Ref != "50003" || events[1].Ref != "50002" {
			t.Fatalf("events = %+v", events)
		}
		latest := events[0]
		if latest.Kind != event.ActivityAdded || latest.AccountID != account.ID || latest.ProcedureID != 9001 ||
			!latest.OccurredAt.Equal(micro(testNow.Add(-2*time.Hour))) || !latest.DetectedAt.Equal(micro(testNow.Add(2*time.Minute))) ||
			latest.Payload.ActivityText != "andamento 50003" {
			t.Errorf("latest event = %+v", latest)
		}
	})
}
