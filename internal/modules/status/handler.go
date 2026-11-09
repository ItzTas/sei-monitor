package status

import (
	"context"

	"github.com/ItzTas/sei-monitor/internal/resp"
)

type Handler struct {
	service *Service
}

func (h *Handler) Get(ctx context.Context, _ *struct{}) (*Output, error) {
	result, err := h.service.Status(ctx)
	if err != nil {
		return nil, resp.Error(err)
	}
	return &Output{Body: result}, nil
}
