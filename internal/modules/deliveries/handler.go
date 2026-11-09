package deliveries

import (
	"context"

	"github.com/ItzTas/sei-monitor/internal/resp"
)

type Handler struct {
	service *Service
}

func (h *Handler) List(ctx context.Context, input *ListInput) (*ListOutput, error) {
	list, err := h.service.List(ctx, *input)
	if err != nil {
		return nil, resp.Error(err)
	}
	return &ListOutput{Body: list}, nil
}
