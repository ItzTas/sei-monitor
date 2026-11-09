package events

import (
	"net/http"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/danielgtaylor/huma/v2"

	"github.com/ItzTas/sei-monitor/internal/resp"
)

func Configure(api huma.API, pool *pgxpool.Pool, mon Monitor, full bool, location *time.Location) {
	RegisterRoutes(api, &Handler{service: NewService(NewPostgres(pool), mon, full, location)})
}

func RegisterRoutes(api huma.API, h *Handler) {
	huma.Register(api, huma.Operation{
		OperationID: "list-events",
		Method:      http.MethodGet,
		Path:        "/events",
		Summary:     "List the events",
		Description: "What changed in your processes, newest first. Each event carries what the matching RSS feed item says, as JSON.",
		Tags:        []string{"events"},
		Security:    resp.Bearer(),
		Errors:      []int{http.StatusBadRequest, http.StatusUnauthorized},
	}, h.List)
}
