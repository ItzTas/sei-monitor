package monitor

import (
	"context"
	"crypto/rand"
	"errors"
	"log/slog"
	"math/big"
	"slices"
	"strings"
	"sync"
	"time"

	"golang.org/x/sync/errgroup"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/store"
)

const (
	activityPageSize = 20
	documentPageSize = 50
	maxActivityPages = 10
	maxDocumentPages = 200
	maxErrorLength   = 500
)

type Config struct {
	Target            sei.Target
	Options           sei.Options
	Units             []string
	AllUnits          bool
	PollInterval      time.Duration
	Concurrency       int
	ReturnWarningDays int
	Location          *time.Location
	Channels          []string
}

type Result struct {
	ProcessesSeen int
	EventsCreated int
}

type State struct {
	AccountID     int64
	Units         []string
	Self          Self
	Running       bool
	Paused        bool
	PausedReason  string
	NextCycleAt   time.Time
	LastStartedAt time.Time
}

type Option func(*Monitor)

func WithClock(now func() time.Time, after func(time.Duration) <-chan time.Time) Option {
	return func(m *Monitor) {
		m.now, m.after = now, after
	}
}

func WithJitter(jitter func(time.Duration) time.Duration) Option {
	return func(m *Monitor) {
		m.jitter = jitter
	}
}

type Monitor struct {
	cfg    Config
	store  *store.Store
	log    *slog.Logger
	now    func() time.Time
	after  func(time.Duration) <-chan time.Time
	jitter func(time.Duration) time.Duration
	manual chan struct{}

	mu      sync.Mutex
	conn    sei.Connection
	account store.Account
	logins  uint64
	state   State
}

type detail struct {
	skipped        bool
	lastActivityID int64
	documentTotal  int
	activities     []sei.Activity
	documents      []sei.Document
	newDocuments   []sei.Document
}

func New(cfg Config, st *store.Store, log *slog.Logger, options ...Option) *Monitor {
	m := &Monitor{
		cfg:    cfg,
		store:  st,
		log:    log,
		now:    time.Now,
		after:  time.After,
		jitter: randomJitter,
		manual: make(chan struct{}, 1),
	}
	for _, option := range options {
		option(m)
	}
	m.cfg.Concurrency = max(m.cfg.Concurrency, 1)
	return m
}

func randomJitter(limit time.Duration) time.Duration {
	if limit <= 0 {
		return 0
	}
	n, err := rand.Int(rand.Reader, big.NewInt(int64(limit)))
	if err != nil {
		return 0
	}
	return time.Duration(n.Int64())
}

func (m *Monitor) State() State {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.state
}

func (m *Monitor) Prepare(ctx context.Context) error {
	conn, err := sei.Connect(ctx, m.cfg.Target, m.cfg.Options)
	if err != nil {
		return err
	}
	key := store.AccountKey{SEIBaseURL: m.cfg.Target.SEIURL, Username: m.cfg.Target.Username, OrgID: conn.Org.ID}
	account, err := m.store.EnsureAccount(ctx, key, m.now())
	if err != nil {
		return err
	}
	if acronym := m.orgAcronym(ctx, conn); acronym != "" && acronym != account.Info.OrgAcronym {
		account.Info.OrgAcronym = acronym
		if err := m.store.UpdateAccountInfo(ctx, account.ID, account.Info); err != nil {
			return err
		}
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	m.conn, m.account = conn, account
	m.state.AccountID, m.state.PausedReason = account.ID, account.PausedReason
	return nil
}

func (m *Monitor) orgAcronym(ctx context.Context, conn sei.Connection) string {
	if conn.Org.Acronym != "" {
		return conn.Org.Acronym
	}
	orgs, err := sei.ListOrgs(ctx, m.cfg.Target.SEIURL, m.cfg.Options)
	if err != nil {
		m.log.Debug("could not read the agency acronym from the login page", "error", err)
		return ""
	}
	for _, org := range orgs {
		if org.ID == conn.Org.ID {
			return org.Acronym
		}
	}
	return ""
}

func (m *Monitor) RunOnce(ctx context.Context, trigger string) (Result, error) {
	m.mu.Lock()
	if m.state.Running {
		m.mu.Unlock()
		return Result{}, ErrRunning
	}
	if m.state.AccountID == 0 {
		m.mu.Unlock()
		return Result{}, ErrNotReady
	}
	started := m.now()
	m.state.Running, m.state.LastStartedAt = true, started
	conn, account := m.conn, m.account
	m.mu.Unlock()
	defer func() {
		m.mu.Lock()
		m.state.Running = false
		m.mu.Unlock()
	}()

	runID, err := m.store.StartCycleRun(ctx, account.ID, trigger, started)
	if err != nil {
		return Result{}, err
	}
	result, err := m.cycle(ctx, conn, account, runID, started)
	if err == nil {
		return result, nil
	}
	finish := store.CycleRun{ID: runID, AccountID: account.ID, FinishedAt: m.now(), Error: describe(err)}
	return Result{}, errors.Join(err, m.store.FinishCycleRun(context.WithoutCancel(ctx), finish))
}

func (m *Monitor) cycle(ctx context.Context, conn sei.Connection, account store.Account, runID int64, started time.Time) (Result, error) {
	session, err := conn.Client.Session(ctx)
	if err != nil {
		return Result{}, err
	}
	if err := m.refreshVersions(ctx, conn, account); err != nil {
		return Result{}, err
	}
	units, err := session.SelectUnits(m.cfg.Units, m.cfg.AllUnits)
	if err != nil {
		return Result{}, err
	}
	m.rememberSession(session, units)
	observations, err := m.observe(ctx, conn.Client, units)
	if err != nil {
		return Result{}, err
	}
	previous, err := m.store.Processes(ctx, account.ID)
	if err != nil {
		return Result{}, err
	}
	baseline := account.BaselineAt.IsZero()
	rules := Rules{
		Now:               started,
		Location:          m.cfg.Location,
		ReturnWarningDays: m.cfg.ReturnWarningDays,
		Self:              Self{UserID: session.UserID, Acronym: session.UserAcronym},
		Baseline:          baseline,
	}
	diff := DiffProcesses(previous, observations, rules)
	details, err := m.fetchDetails(ctx, conn.Client, account.ID, diff.Present, baseline)
	if err != nil {
		return Result{}, err
	}

	changes := buildChanges(account.ID, runID, started, baseline, diff, details)
	changes.FinishedAt, changes.Channels = m.now(), m.cfg.Channels
	inserted, err := m.store.ApplyCycle(ctx, changes)
	if err != nil {
		return Result{}, err
	}
	m.afterSuccess(ctx, account, baseline, started)
	result := Result{ProcessesSeen: len(diff.Present), EventsCreated: len(inserted)}
	m.log.Info("cycle finished", "processes", result.ProcessesSeen, "events", result.EventsCreated, "baseline", baseline, "duration", m.now().Sub(started).Round(time.Millisecond))
	return result, nil
}

func (m *Monitor) rememberSession(session sei.Session, units []sei.Unit) {
	acronyms := make([]string, 0, len(units))
	for _, unit := range units {
		acronyms = append(acronyms, unit.Acronym)
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	m.state.Units, m.state.Self = acronyms, Self{UserID: session.UserID, Acronym: session.UserAcronym}
}

func (m *Monitor) afterSuccess(ctx context.Context, account store.Account, baseline bool, started time.Time) {
	m.mu.Lock()
	if baseline {
		m.account.BaselineAt = started
	}
	paused := m.account.PausedReason != ""
	m.account.PausedReason, m.state.PausedReason = "", ""
	m.mu.Unlock()
	if !paused {
		return
	}
	if err := m.store.SetPausedReason(ctx, account.ID, ""); err != nil {
		m.log.Warn("could not clear the pause reason", "error", err)
	}
}

func (m *Monitor) refreshVersions(ctx context.Context, conn sei.Connection, account store.Account) error {
	logins := conn.Client.Logins()
	m.mu.Lock()
	seen := m.logins
	m.mu.Unlock()
	if logins == seen {
		return nil
	}
	version, err := conn.Client.Version(ctx)
	if err != nil {
		return err
	}
	info := store.AccountInfo{OrgAcronym: account.Info.OrgAcronym, SEIVersion: version.SEI, WSSEIVersion: version.Module}
	if err := m.store.UpdateAccountInfo(ctx, account.ID, info); err != nil {
		return err
	}
	m.mu.Lock()
	m.logins, m.account.Info = logins, info
	m.mu.Unlock()
	return nil
}

func (m *Monitor) observe(ctx context.Context, client *sei.Client, units []sei.Unit) ([]Observation, error) {
	lists := make([][]sei.Process, len(units))
	g, gctx := errgroup.WithContext(ctx)
	g.SetLimit(m.cfg.Concurrency)
	for i, unit := range units {
		g.Go(func() (err error) {
			lists[i], err = client.Processes(gctx, unit.ID)
			return err
		})
	}
	if err := g.Wait(); err != nil {
		return nil, err
	}
	var observations []Observation
	for i, unit := range units {
		for _, process := range lists[i] {
			observations = append(observations, Observation{Process: process, Unit: unit})
		}
	}
	return observations, nil
}

func (m *Monitor) fetchDetails(ctx context.Context, client *sei.Client, accountID int64, present []Presence, baseline bool) ([]detail, error) {
	details := make([]detail, len(present))
	g, gctx := errgroup.WithContext(ctx)
	g.SetLimit(m.cfg.Concurrency)
	for i, presence := range present {
		g.Go(func() error {
			d, err := m.fetchDetail(gctx, client, accountID, presence, baseline)
			if errors.Is(err, sei.ErrRefused) {
				m.log.Warn("SEI refused the details of a process; it keeps its previous state in this cycle", "process", presence.Process.Number, "error", err)
				details[i] = detail{skipped: true}
				return nil
			}
			details[i] = d
			return err
		})
	}
	return details, g.Wait()
}

func (m *Monitor) fetchDetail(ctx context.Context, client *sei.Client, accountID int64, p Presence, baseline bool) (detail, error) {
	quiet := p.quiet(baseline)
	var lastSeen int64
	var savedTotal int
	if p.Previous != nil {
		lastSeen, savedTotal = p.Previous.LastActivityID, p.Previous.DocumentTotal
	}
	activities, lastID, err := readActivities(ctx, client, p, lastSeen, quiet)
	if err != nil {
		return detail{}, err
	}
	d := detail{lastActivityID: lastID, documentTotal: savedTotal}
	if !quiet {
		d.activities = activities
	}
	if !quiet && len(activities) == 0 {
		return d, nil
	}

	known := map[int64]bool{}
	if p.Previous != nil {
		if known, err = m.store.DocumentIDs(ctx, accountID, p.Process.ProcedureID); err != nil {
			return detail{}, err
		}
	}
	documents, total, err := readDocuments(ctx, client, p, savedTotal)
	if err != nil {
		return detail{}, err
	}
	d.documentTotal = total
	d.documents = FreshDocuments(known, documents)
	if !quiet {
		d.newDocuments = d.documents
	}
	return d, nil
}

func readActivities(ctx context.Context, client *sei.Client, p Presence, lastSeen int64, quiet bool) ([]sei.Activity, int64, error) {
	var collected []sei.Activity
	for page := range maxActivityPages {
		result, err := client.ActivityPage(ctx, p.Unit.ID, p.Process.ProcedureID, page, activityPageSize)
		if err != nil {
			return nil, 0, err
		}
		if quiet {
			return nil, max(lastSeen, highestActivity(result.Items)), nil
		}
		fresh, reached := FreshActivities(lastSeen, result.Items)
		collected = append(collected, fresh...)
		if reached || len(result.Items) < activityPageSize {
			break
		}
	}
	return collected, max(lastSeen, highestActivity(collected)), nil
}

func readDocuments(ctx context.Context, client *sei.Client, p Presence, savedTotal int) ([]sei.Document, int, error) {
	start := 0
	if savedTotal > 0 {
		start = (savedTotal - 1) / documentPageSize
	}
	var documents []sei.Document
	total := 0
	for page := start; page < start+maxDocumentPages; page++ {
		result, err := client.DocumentPage(ctx, p.Unit.ID, p.Process.ProcedureID, page, documentPageSize)
		if err != nil {
			return nil, 0, err
		}
		if page == start && start > 0 && len(result.Items) == 0 {
			return readDocuments(ctx, client, p, result.Total)
		}
		documents = append(documents, result.Items...)
		total = max(result.Total, page*documentPageSize+len(result.Items))
		if len(result.Items) < documentPageSize || (page+1)*documentPageSize >= result.Total {
			break
		}
	}
	return documents, total, nil
}

func highestActivity(activities []sei.Activity) int64 {
	var highest int64
	for _, activity := range activities {
		highest = max(highest, activity.ID)
	}
	return highest
}

func buildChanges(accountID, runID int64, now time.Time, baseline bool, diff ProcessChanges, details []detail) store.CycleChanges {
	changes := store.CycleChanges{AccountID: accountID, RunID: runID, Now: now, Baseline: baseline, Events: diff.Events}
	for _, process := range diff.Left {
		changes.Left = append(changes.Left, process.ProcedureID)
	}
	for i, p := range diff.Present {
		d := details[i]
		if d.skipped && p.Previous != nil {
			d.lastActivityID, d.documentTotal = p.Previous.LastActivityID, p.Previous.DocumentTotal
		}
		changes.Processes = append(changes.Processes, processRow(p, d))
		for _, document := range d.documents {
			changes.Documents = append(changes.Documents, store.Document{
				DocumentID: document.ID, ProcedureID: p.Process.ProcedureID,
				Protocol: document.Protocol, Type: document.Type, Name: document.Name,
			})
		}
		for _, activity := range slices.Backward(d.activities) {
			changes.Events = append(changes.Events, activityEvent(p, activity))
		}
		for _, document := range d.newDocuments {
			changes.Events = append(changes.Events, documentEvent(p, document))
		}
	}
	return changes
}

func processRow(p Presence, d detail) store.Process {
	row := store.Process{
		ProcedureID:    p.Process.ProcedureID,
		Number:         p.Process.Number,
		Type:           p.Process.Type,
		Description:    p.Process.Description,
		UnitID:         p.Unit.ID,
		UnitAcronym:    p.Unit.Acronym,
		ReturnDate:     formatDate(p.Process.ReturnDate),
		ReturnOverdue:  p.Process.ReturnOverdue,
		LastActivityID: d.lastActivityID,
		DocumentTotal:  d.documentTotal,
	}
	if assignee := p.Process.Assignee; assignee != nil {
		row.AssignedUser, row.AssignedUserID = assignee.Acronym, assignee.ID
	}
	if p.Previous != nil {
		row.FirstSeenAt = p.Previous.FirstSeenAt
	}
	return row
}

func describe(err error) string {
	text := strings.TrimSpace(err.Error())
	if len(text) > maxErrorLength {
		text = strings.ToValidUTF8(text[:maxErrorLength], "")
	}
	return text
}

func loginFailure(err error, now time.Time) (string, event.Event) {
	reason := "SEI rejected the login"
	if apiErr, ok := errors.AsType[*sei.APIError](err); ok && apiErr.Message != "" {
		reason += " (" + apiErr.Message + ")"
	}
	ev := event.Event{Kind: event.AccountLoginFailed, Ref: "login:" + now.UTC().Format(time.RFC3339), Payload: event.Payload{Reason: reason}}
	return reason, ev
}
