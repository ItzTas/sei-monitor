package cycle

import (
	"context"
	"errors"
	"math"
	"net/http"
	"strconv"

	"github.com/danielgtaylor/huma/v2"

	"github.com/ItzTas/sei-monitor/internal/api/resp"
	"github.com/ItzTas/sei-monitor/internal/monitor"
)

type Handler struct {
	monitor Monitor
}

func (h *Handler) Trigger(_ context.Context, _ *struct{}) (*Output, error) {
	err := h.monitor.TriggerCycle()
	if err == nil {
		return &Output{Body: Scheduled{Status: "scheduled"}}, nil
	}
	if tooSoon, ok := errors.AsType[*monitor.TooSoonError](err); ok {
		seconds := strconv.Itoa(int(math.Ceil(tooSoon.RetryAfter.Seconds())))
		return nil, huma.ErrorWithHeaders(huma.NewError(http.StatusTooManyRequests, tooSoon.Error()), http.Header{"Retry-After": {seconds}})
	}
	for _, conflict := range []error{monitor.ErrRunning, monitor.ErrPaused, monitor.ErrNotReady} {
		if errors.Is(err, conflict) {
			return nil, huma.Error409Conflict(err.Error())
		}
	}
	return nil, resp.Error(err)
}
