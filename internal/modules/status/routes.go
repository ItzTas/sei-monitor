package status

import (
	"net/http"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/danielgtaylor/huma/v2"

	"github.com/ItzTas/sei-monitor/internal/resp"
)

func Configure(api huma.API, pool *pgxpool.Pool, mon Monitor, fullDetail bool) {
	RegisterRoutes(api, &Handler{service: NewService(NewPostgres(pool), mon, fullDetail)})
}

func RegisterRoutes(api huma.API, h *Handler) {
	huma.Register(api, huma.Operation{
		OperationID: "get-status",
		Method:      http.MethodGet,
		Path:        "/status",
		Summary:     "Monitoring status",
		Description: "The agency, the watched units, the SEI versions, the latest cycle, the next one and the deliveries per channel. It never includes your username.",
		Tags:        []string{"status"},
		Security:    resp.Bearer(),
		Errors:      []int{http.StatusUnauthorized},
	}, h.Get)
}
