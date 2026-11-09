package status

import (
	"context"
	"time"

	"golang.org/x/sync/errgroup"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/monitor"
)

type Monitor interface {
	State() monitor.State
}

type Service struct {
	repo       Repository
	monitor    Monitor
	fullDetail bool
}

func NewService(repo Repository, mon Monitor, fullDetail bool) *Service {
	return &Service{repo: repo, monitor: mon, fullDetail: fullDetail}
}

func (s *Service) Status(ctx context.Context) (Status, error) {
	state := s.monitor.State()
	result := Status{
		Units:        append([]string{}, state.Units...),
		PausedReason: state.PausedReason,
		DetailLevel:  map[bool]string{true: "full", false: "minimal"}[s.fullDetail],
		NextCycleAt:  optionalTime(state.NextCycleAt),
		CycleRunning: state.Running,
		Deliveries:   map[string]DeliveryCounts{},
	}
	if state.AccountID == 0 {
		return result, nil
	}

	var (
		account core.Account
		run     core.CycleRun
		found   bool
		counts  map[string]map[string]int
	)
	g, gctx := errgroup.WithContext(ctx)
	g.Go(func() (err error) { account, err = s.repo.Account(gctx, state.AccountID); return err })
	g.Go(func() (err error) { run, found, err = s.repo.LastCycleRun(gctx, state.AccountID); return err })
	g.Go(func() (err error) { counts, err = s.repo.DeliveryCounts(gctx, state.AccountID); return err })
	if err := g.Wait(); err != nil {
		return Status{}, err
	}

	result.OrgAcronym, result.SEIVersion, result.WSSEIVersion = account.Info.OrgAcronym, account.Info.SEIVersion, account.Info.WSSEIVersion
	if result.PausedReason == "" {
		result.PausedReason = account.PausedReason
	}
	if found {
		result.LastCycle = &CycleRun{
			StartedAt: run.StartedAt.UTC(), FinishedAt: optionalTime(run.FinishedAt), Trigger: run.Trigger,
			Error: run.Error, ProcessesSeen: run.ProcessesSeen, EventsCreated: run.EventsCreated,
		}
	}
	for channel, statuses := range counts {
		result.Deliveries[channel] = DeliveryCounts{Pending: statuses["pending"], Sent: statuses["sent"], Failed: statuses["failed"]}
	}
	return result, nil
}

func optionalTime(t time.Time) *time.Time {
	if t.IsZero() {
		return nil
	}
	utc := t.UTC()
	return &utc
}
