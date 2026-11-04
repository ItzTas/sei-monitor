package events

import (
	"context"
	"time"

	"github.com/ItzTas/sei-monitor/internal/api/resp"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/store"
)

type Repository interface {
	ListEvents(ctx context.Context, filter store.EventFilter) ([]event.Event, error)
}

type Monitor interface {
	State() monitor.State
}

type Service struct {
	repo     Repository
	monitor  Monitor
	full     bool
	location *time.Location
}

func NewService(repo Repository, mon Monitor, full bool, location *time.Location) *Service {
	return &Service{repo: repo, monitor: mon, full: full, location: location}
}

func (s *Service) List(ctx context.Context, input ListInput) (resp.List[event.View], error) {
	after, err := resp.DecodeCursor[cursor](input.Cursor)
	if err != nil {
		return resp.List[event.View]{}, err
	}
	state := s.monitor.State()
	if state.AccountID == 0 {
		return resp.List[event.View]{Items: []event.View{}}, nil
	}
	filter := store.EventFilter{
		AccountID:   state.AccountID,
		ProcedureID: input.ProcedureID,
		Since:       input.Since,
		Until:       input.Until,
		Limit:       input.Limit + 1,
	}
	for _, kind := range input.Kinds {
		filter.Kinds = append(filter.Kinds, event.Kind(kind))
	}
	if after != nil {
		filter.After = &store.EventKey{OccurredAt: after.OccurredAt, ID: after.ID}
	}
	rows, err := s.repo.ListEvents(ctx, filter)
	if err != nil {
		return resp.List[event.View]{}, err
	}
	viewer := event.Viewer{Formatter: event.Formatter{Location: s.location, Full: s.full}, Self: state.Self.Acronym}
	return resp.Paginate(rows, input.Limit, viewer.View, func(ev event.Event) cursor {
		return cursor{OccurredAt: ev.OccurredAt, ID: ev.ID}
	})
}
