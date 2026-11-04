package monitor

import (
	"log/slog"
	"net/http"
	"os"
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
	"github.com/ItzTas/sei-monitor/internal/store"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

var start = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

func TestMain(m *testing.M) {
	os.Exit(storetest.Run(m, store.Migrate))
}

type fakeClock struct {
	mu    sync.Mutex
	now   time.Time
	waits chan pendingWait
}

type pendingWait struct {
	duration time.Duration
	fire     chan time.Time
}

func newFakeClock(now time.Time) *fakeClock {
	return &fakeClock{now: now, waits: make(chan pendingWait, 16)}
}

func (c *fakeClock) Now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.now
}

func (c *fakeClock) Advance(d time.Duration) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.now = c.now.Add(d)
}

func (c *fakeClock) After(d time.Duration) <-chan time.Time {
	fire := make(chan time.Time, 1)
	c.waits <- pendingWait{duration: d, fire: fire}
	return fire
}

func (c *fakeClock) nextWait(t *testing.T) pendingWait {
	t.Helper()
	select {
	case wait := <-c.waits:
		return wait
	case <-time.After(10 * time.Second):
		t.Fatal("the monitor never started waiting")
		return pendingWait{}
	}
}

type harness struct {
	server  *seitest.Server
	store   *store.Store
	clock   *fakeClock
	monitor *Monitor
}

func newHarness(t *testing.T, databaseURL string, configure func(*Config)) *harness {
	t.Helper()
	server := seitest.New(t)
	st, err := store.Open(t.Context(), databaseURL)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := st.Close(); err != nil {
			t.Error(err)
		}
	})
	clock := newFakeClock(start)
	cfg := Config{
		Target: sei.Target{
			SEIURL: server.URL(), Org: seitest.OrgAcronym, Username: seitest.Username,
			Password: config.StaticPassword(config.NewSecret(seitest.Password)),
		},
		Options:           sei.Options{UserAgent: "sei-monitor/test", Location: server.Location()},
		PollInterval:      15 * time.Minute,
		Concurrency:       3,
		ReturnWarningDays: 2,
		Location:          server.Location(),
		Channels:          []string{"webhook"},
	}
	if configure != nil {
		configure(&cfg)
	}
	noJitter := func(time.Duration) time.Duration { return 0 }
	m := New(cfg, st, slog.New(slog.DiscardHandler), WithClock(clock.Now, clock.After), WithJitter(noJitter))
	return &harness{server: server, store: st, clock: clock, monitor: m}
}

func (h *harness) runOnce(t *testing.T) Result {
	t.Helper()
	result, err := h.monitor.RunOnce(t.Context(), store.TriggerScheduled)
	if err != nil {
		t.Fatal(err)
	}
	return result
}

func (h *harness) events(t *testing.T) []event.Event {
	t.Helper()
	events, err := h.store.RecentEvents(t.Context(), 100)
	if err != nil {
		t.Fatal(err)
	}
	slices.Reverse(events)
	return events
}

func activity(id int64, at time.Time, text string) seitest.Activity {
	return seitest.Activity{ID: id, User: seitest.Username, Unit: "SEDEMO/NUPROC", Text: text, At: at}
}

func document(id int64, label string) seitest.Document {
	return seitest.Document{ID: id, Protocol: "00" + label, Type: "Ofício", Label: label}
}

func populate(server *seitest.Server) {
	assignee := &seitest.User{ID: seitest.UserID, Acronym: seitest.Username, Name: seitest.UserName}
	server.SetProcesses(seitest.UnitNUPROC,
		seitest.Process{ID: 9001, Number: seitest.ProcessNumber(1), Type: "Contrato", Description: "Compra de cadeiras", Assignee: assignee, ReturnDate: "05/11/2026"},
		seitest.Process{ID: 9002, Number: seitest.ProcessNumber(2), Type: "Diárias"},
	)
	server.AddActivities(9001,
		activity(50001, start.Add(-72*time.Hour), "Processo gerado"),
		activity(50002, start.Add(-48*time.Hour), "Processo remetido pela unidade SEDEMO/PROTOCOLO"),
		activity(50003, start.Add(-24*time.Hour), "Processo recebido na unidade"),
	)
	server.AddActivities(9002, activity(60001, start.Add(-24*time.Hour), "Processo gerado"))
	server.AddDocuments(9001, document(80001, "Ofício 1"), document(80002, "Despacho 2"), document(80003, "Anexo 3"))
}

func TestStateKnowsTheUserAcronymBeforeTheFirstLogin(t *testing.T) {
	h := newHarness(t, storetest.SQLiteURL(t), nil)

	if self := h.monitor.State().Self; self != (Self{Acronym: seitest.Username}) {
		t.Errorf("self before any login = %+v, want the configured username", self)
	}
}

func TestFirstCycleIsASilentBaseline(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		h := newHarness(t, databaseURL, nil)
		populate(h.server)
		ctx := t.Context()
		if err := h.monitor.Prepare(ctx); err != nil {
			t.Fatal(err)
		}

		result := h.runOnce(t)

		if result != (Result{ProcessesSeen: 2, EventsCreated: 0}) {
			t.Errorf("result = %+v", result)
		}
		if state := h.monitor.State(); !slices.Equal(state.Units, []string{"SEDEMO/NUPROC"}) || state.Self != (Self{UserID: seitest.UserID, Acronym: seitest.Username}) {
			t.Errorf("state = %+v", state)
		}
		if events := h.events(t); len(events) != 0 {
			t.Errorf("baseline events = %+v", events)
		}
		processes, err := h.store.Processes(ctx, h.monitor.State().AccountID)
		if err != nil {
			t.Fatal(err)
		}
		if len(processes) != 2 || processes[0].LastActivityID != 50003 || processes[0].DocumentTotal != 3 || processes[0].AssignedUser != seitest.Username ||
			processes[0].ReturnDate != "2026-11-05" || processes[0].UnitAcronym != "SEDEMO/NUPROC" || processes[1].LastActivityID != 60001 {
			t.Errorf("stored processes = %+v", processes)
		}
		documents, err := h.store.DocumentIDs(ctx, h.monitor.State().AccountID, 9001)
		if err != nil || len(documents) != 3 {
			t.Errorf("stored documents = %v, %v", documents, err)
		}
		run, found, err := h.store.LastCycleRun(ctx, h.monitor.State().AccountID)
		if err != nil || !found || run.Error != "" || run.ProcessesSeen != 2 || run.Trigger != store.TriggerScheduled || run.FinishedAt.IsZero() {
			t.Errorf("cycle run = %+v, %v, %v", run, found, err)
		}
		account, err := h.store.EnsureAccount(ctx, store.AccountKey{SEIBaseURL: h.server.URL(), Username: seitest.Username, OrgID: seitest.OrgID}, start)
		if err != nil || account.BaselineAt.IsZero() || account.Info != (store.AccountInfo{OrgAcronym: seitest.OrgAcronym, SEIVersion: seitest.SEIVersion, WSSEIVersion: seitest.ModuleVersion}) {
			t.Errorf("account = %+v, %v", account, err)
		}
	})
}

func TestNextCycleReportsWhatChanged(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		h := newHarness(t, databaseURL, nil)
		populate(h.server)
		if err := h.monitor.Prepare(t.Context()); err != nil {
			t.Fatal(err)
		}
		h.runOnce(t)

		h.clock.Advance(15 * time.Minute)
		h.server.RemoveProcess(seitest.UnitNUPROC, 9002)
		h.server.AddProcess(seitest.UnitNUPROC, seitest.Process{ID: 9003, Number: seitest.ProcessNumber(3), Type: "Ofício"})
		h.server.AddActivities(9003, activity(70001, start, "Processo gerado"), activity(70002, start, "Processo remetido"))
		h.server.AddDocuments(9003, document(90001, "Ofício 9"))
		h.server.UpdateProcess(seitest.UnitNUPROC, 9001, func(p *seitest.Process) {
			p.Assignee = &seitest.User{ID: 202, Acronym: "beltrano.silva", Name: "Beltrano da Silva"}
		})
		newActivity := start.Add(10 * time.Minute)
		h.server.AddActivities(9001, activity(50004, newActivity, "Documento assinado"))
		h.server.AddDocuments(9001, document(80004, "Ofício 4"))

		result := h.runOnce(t)

		cycle := start.Add(15 * time.Minute).Format(time.RFC3339)
		want := []kindRef{
			{event.AssignmentChanged, "9001:" + cycle},
			{event.ReturnDueSoon, "9001:2026-11-05"},
			{event.ProcessEntered, "9003:" + cycle},
			{event.ProcessLeft, "9002:" + cycle},
			{event.ActivityAdded, "50004"},
			{event.DocumentAdded, "80004"},
		}
		events := h.events(t)
		if got := kindsAndRefs(events); !slices.Equal(got, want) {
			t.Fatalf("events = %v\nwant %v", got, want)
		}
		if result.EventsCreated != len(want) || result.ProcessesSeen != 2 {
			t.Errorf("result = %+v", result)
		}
		activityEvent := events[4]
		if !activityEvent.OccurredAt.Equal(newActivity) || activityEvent.Payload.ActivityText != "Documento assinado" || activityEvent.Payload.Unit != "SEDEMO/NUPROC" {
			t.Errorf("activity event = %+v", activityEvent)
		}
		if documentEvent := events[5]; documentEvent.Payload.DocumentName != "Ofício 4" || documentEvent.Payload.ProcessNumber != seitest.ProcessNumber(1) {
			t.Errorf("document event = %+v", documentEvent)
		}

		h.clock.Advance(15 * time.Minute)
		if again := h.runOnce(t); again.EventsCreated != 0 {
			t.Errorf("a cycle without changes created %d events", again.EventsCreated)
		}
	})
}

func TestExpiredSessionDoesNotStopTheCycle(t *testing.T) {
	h := newHarness(t, storetest.SQLiteURL(t), nil)
	populate(h.server)
	if err := h.monitor.Prepare(t.Context()); err != nil {
		t.Fatal(err)
	}
	h.runOnce(t)

	h.server.ExpireTokens()
	h.server.AddActivities(9002, activity(60002, start, "Processo recebido na unidade"))
	h.runOnce(t)

	refs := kindsAndRefs(h.events(t))
	if !slices.Contains(refs, kindRef{event.ActivityAdded, "60002"}) || h.server.Logins() != 2 {
		t.Errorf("events = %v after %d logins, want the new activity after logging in again", refs, h.server.Logins())
	}
}

func TestRefusedProcessKeepsItsStateUntilTheNextCycle(t *testing.T) {
	h := newHarness(t, storetest.SQLiteURL(t), func(cfg *Config) { cfg.Concurrency = 1 })
	populate(h.server)
	if err := h.monitor.Prepare(t.Context()); err != nil {
		t.Fatal(err)
	}
	h.runOnce(t)
	h.server.AddActivities(9001, activity(50004, start, "Documento assinado"))
	h.server.AddActivities(9002, activity(60002, start, "Processo recebido na unidade"))

	h.server.FailNext(seitest.RouteActivities, http.StatusOK, 1)
	h.clock.Advance(15 * time.Minute)
	first := h.runOnce(t)
	h.clock.Advance(15 * time.Minute)
	second := h.runOnce(t)

	refs := []string{}
	for _, ev := range h.events(t) {
		if ev.Kind == event.ActivityAdded {
			refs = append(refs, ev.Ref)
		}
	}
	if !slices.Equal(refs, []string{"60002", "50004"}) {
		t.Errorf("activity events = %v, want the refused process to catch up in the next cycle", refs)
	}
	if first.ProcessesSeen != 2 || second.EventsCreated == 0 {
		t.Errorf("results = %+v, %+v", first, second)
	}
}
