package deliveries

import (
	"net/http"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/danielgtaylor/huma/v2"

	"github.com/ItzTas/sei-monitor/internal/resp"
)

func Configure(api huma.API, pool *pgxpool.Pool, mon Monitor) {
	RegisterRoutes(api, &Handler{service: NewService(NewPostgres(pool), mon)})
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
