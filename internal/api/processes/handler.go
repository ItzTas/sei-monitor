package processes

import (
	"context"
	"errors"

	"github.com/danielgtaylor/huma/v2"

	"github.com/ItzTas/sei-monitor/internal/api/resp"
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

func (h *Handler) Get(ctx context.Context, input *DetailInput) (*DetailOutput, error) {
	detail, err := h.service.Detail(ctx, input.ProcedureID)
	if errors.Is(err, ErrNotFound) {
		return nil, huma.Error404NotFound("process not found")
	}
	if err != nil {
		return nil, resp.Error(err)
	}
	return &DetailOutput{Body: detail}, nil
}
