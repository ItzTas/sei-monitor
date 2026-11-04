package processes

import (
	"net/http"
	"time"

	"github.com/danielgtaylor/huma/v2"

	"github.com/ItzTas/sei-monitor/internal/api/resp"
)

func Configure(api huma.API, repo Repository, mon Monitor, full bool, location *time.Location) {
	RegisterRoutes(api, &Handler{service: NewService(repo, mon, full, location)})
}

func RegisterRoutes(api huma.API, h *Handler) {
	huma.Register(api, huma.Operation{
		OperationID: "list-processes",
		Method:      http.MethodGet,
		Path:        "/processes",
		Summary:     "List the processes",
		Description: "Processes in the watched units now, most recently seen first.",
		Tags:        []string{"processes"},
		Security:    resp.Bearer(),
		Errors:      []int{http.StatusBadRequest, http.StatusUnauthorized},
	}, h.List)
	huma.Register(api, huma.Operation{
		OperationID: "get-process",
		Method:      http.MethodGet,
		Path:        "/processes/{procedure_id}",
		Summary:     "Get a process",
		Description: "One process with the documents sei-monitor has seen and its latest 50 events.",
		Tags:        []string{"processes"},
		Security:    resp.Bearer(),
		Errors:      []int{http.StatusUnauthorized, http.StatusNotFound},
	}, h.Get)
}
