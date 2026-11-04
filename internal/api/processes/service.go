package processes

import (
	"context"
	"errors"
	"time"

	"golang.org/x/sync/errgroup"

	"github.com/ItzTas/sei-monitor/internal/api/resp"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/store"
)

const detailEvents = 50

var ErrNotFound = errors.New("process not found")

type Repository interface {
	ListProcesses(ctx context.Context, filter store.ProcessFilter) ([]store.Process, error)
	ProcessByID(ctx context.Context, accountID, procedureID int64) (store.Process, error)
	Documents(ctx context.Context, accountID, procedureID int64) ([]store.Document, error)
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

func (s *Service) List(ctx context.Context, input ListInput) (resp.List[Process], error) {
	after, err := resp.DecodeCursor[cursor](input.Cursor)
	if err != nil {
		return resp.List[Process]{}, err
	}
	state := s.monitor.State()
	if state.AccountID == 0 || (input.AssignedToMe && state.Self == (monitor.Self{})) {
		return resp.List[Process]{Items: []Process{}}, nil
	}
	filter := store.ProcessFilter{
		AccountID:       state.AccountID,
		Unit:            input.Unit,
		ReturnScheduled: input.ReturnScheduled,
		ReturnOverdue:   input.ReturnOverdue,
		IncludeLeft:     input.IncludeLeft,
		Limit:           input.Limit + 1,
	}
	if input.AssignedToMe {
		filter.AssignedUserID, filter.AssignedAcronym = state.Self.UserID, state.Self.Acronym
	}
	if after != nil {
		filter.After = &store.ProcessKey{LastSeenAt: after.LastSeenAt, ProcedureID: after.ProcedureID}
	}
	rows, err := s.repo.ListProcesses(ctx, filter)
	if err != nil {
		return resp.List[Process]{}, err
	}
	return resp.Paginate(rows, input.Limit, s.view(state.Self), func(p store.Process) cursor {
		return cursor{LastSeenAt: p.LastSeenAt, ProcedureID: p.ProcedureID}
	})
}

func (s *Service) Detail(ctx context.Context, procedureID int64) (Detail, error) {
	state := s.monitor.State()
	if state.AccountID == 0 {
		return Detail{}, ErrNotFound
	}
	process, err := s.repo.ProcessByID(ctx, state.AccountID, procedureID)
	if errors.Is(err, store.ErrNotFound) {
		return Detail{}, ErrNotFound
	}
	if err != nil {
		return Detail{}, err
	}

	var documents []store.Document
	var events []event.Event
	g, gctx := errgroup.WithContext(ctx)
	g.Go(func() (err error) { documents, err = s.repo.Documents(gctx, state.AccountID, procedureID); return err })
	g.Go(func() (err error) {
		events, err = s.repo.ListEvents(gctx, store.EventFilter{AccountID: state.AccountID, ProcedureID: procedureID, Limit: detailEvents})
		return err
	})
	if err := g.Wait(); err != nil {
		return Detail{}, err
	}

	detail := Detail{Process: s.view(state.Self)(process), Documents: make([]Document, 0, len(documents)), Events: make([]event.View, 0, len(events))}
	for _, d := range documents {
		document := Document{DocumentID: d.DocumentID, Protocol: d.Protocol, FirstSeenAt: d.FirstSeenAt.UTC()}
		if s.full {
			document.DocumentType, document.Name = d.Type, d.Name
		}
		detail.Documents = append(detail.Documents, document)
	}
	formatter := event.Formatter{Location: s.location, Full: s.full, Self: state.Self.Acronym}
	for _, ev := range events {
		detail.Events = append(detail.Events, formatter.View(ev))
	}
	return detail, nil
}

func (s *Service) view(self monitor.Self) func(store.Process) Process {
	return func(p store.Process) Process {
		mine := p.AssignedUser != "" && monitor.IsSelf(p.AssignedUserID, p.AssignedUser, self)
		view := Process{
			ProcedureID:   p.ProcedureID,
			Number:        p.Number,
			Unit:          p.UnitAcronym,
			AssignedToMe:  mine,
			ReturnDate:    p.ReturnDate,
			ReturnOverdue: p.ReturnOverdue,
			FirstSeenAt:   p.FirstSeenAt.UTC(),
			LastSeenAt:    p.LastSeenAt.UTC(),
		}
		if !p.LeftAt.IsZero() {
			left := p.LeftAt.UTC()
			view.LeftAt = &left
		}
		if !s.full {
			return view
		}
		view.ProcessType, view.Description = p.Type, p.Description
		if !mine {
			view.AssignedUser = p.AssignedUser
		}
		return view
	}
}
