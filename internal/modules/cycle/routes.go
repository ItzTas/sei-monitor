package cycle

import (
	"net/http"

	"github.com/danielgtaylor/huma/v2"

	"github.com/ItzTas/sei-monitor/internal/resp"
)

func Configure(api huma.API, mon Monitor) {
	RegisterRoutes(api, &Handler{monitor: mon})
}

func RegisterRoutes(api huma.API, h *Handler) {
	huma.Register(api, huma.Operation{
		OperationID:   "trigger-cycle",
		Method:        http.MethodPost,
		Path:          "/cycle",
		Summary:       "Start a cycle now",
		Description:   "Schedules an immediate cycle through the same loop as the scheduled ones and answers without waiting for it. Refused while a cycle runs, while monitoring is paused, and within 5 minutes of the last cycle start.",
		Tags:          []string{"cycle"},
		DefaultStatus: http.StatusAccepted,
		Security:      resp.Bearer(),
		Errors:        []int{http.StatusUnauthorized, http.StatusConflict, http.StatusTooManyRequests},
	}, h.Trigger)
}
