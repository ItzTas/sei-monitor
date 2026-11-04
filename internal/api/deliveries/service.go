package deliveries

import (
	"context"

	"github.com/ItzTas/sei-monitor/internal/api/resp"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/store"
)

type Repository interface {
	ListDeliveries(ctx context.Context, filter store.DeliveryFilter) ([]store.Delivery, error)
}

type Monitor interface {
	State() monitor.State
}

type Service struct {
	repo    Repository
	monitor Monitor
}

func NewService(repo Repository, mon Monitor) *Service {
	return &Service{repo: repo, monitor: mon}
}

func (s *Service) List(ctx context.Context, input ListInput) (resp.List[Delivery], error) {
	after, err := resp.DecodeCursor[cursor](input.Cursor)
	if err != nil {
		return resp.List[Delivery]{}, err
	}
	accountID := s.monitor.State().AccountID
	if accountID == 0 {
		return resp.List[Delivery]{Items: []Delivery{}}, nil
	}
	filter := store.DeliveryFilter{AccountID: accountID, Status: input.Status, Channel: input.Channel, Limit: input.Limit + 1}
	if after != nil {
		filter.After = &store.DeliveryKey{EventID: after.EventID, Channel: after.Channel}
	}
	rows, err := s.repo.ListDeliveries(ctx, filter)
	if err != nil {
		return resp.List[Delivery]{}, err
	}
	return resp.Paginate(rows, input.Limit, view, func(d store.Delivery) cursor {
		return cursor{EventID: d.EventID, Channel: d.Channel}
	})
}

func view(d store.Delivery) Delivery {
	delivery := Delivery{
		EventID: d.EventID, Channel: d.Channel, Status: d.Status, Attempts: d.Attempts,
		NextAttemptAt: d.NextAttemptAt.UTC(), LastError: d.LastError,
	}
	if !d.SentAt.IsZero() {
		sent := d.SentAt.UTC()
		delivery.SentAt = &sent
	}
	return delivery
}
