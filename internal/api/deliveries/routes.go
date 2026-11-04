package deliveries

import (
	"net/http"

	"github.com/danielgtaylor/huma/v2"

	"github.com/ItzTas/sei-monitor/internal/api/resp"
)

func Configure(api huma.API, repo Repository, mon Monitor) {
	RegisterRoutes(api, &Handler{service: NewService(repo, mon)})
}

func RegisterRoutes(api huma.API, h *Handler) {
	huma.Register(api, huma.Operation{
		OperationID: "list-deliveries",
		Method:      http.MethodGet,
		Path:        "/deliveries",
		Summary:     "List the deliveries",
		Description: "One delivery per event and channel, newest event first.",
		Tags:        []string{"deliveries"},
		Security:    resp.Bearer(),
		Errors:      []int{http.StatusBadRequest, http.StatusUnauthorized},
	}, h.List)
}
