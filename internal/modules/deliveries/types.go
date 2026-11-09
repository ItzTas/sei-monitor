package deliveries

import (
	"time"

	"github.com/ItzTas/sei-monitor/internal/resp"
)

type ListInput struct {
	Status  string `query:"status" enum:"pending,sent,failed" doc:"Only deliveries in this status."`
	Channel string `query:"channel" enum:"email,telegram,ntfy,webhook,amqp,whatsapp" doc:"Only deliveries through this channel."`
	resp.PageInput
}

type ListOutput struct {
	Body resp.List[Delivery]
}

type Delivery struct {
	EventID       int64      `json:"event_id"`
	Channel       string     `json:"channel"`
	Status        string     `json:"status" enum:"pending,sent,failed"`
	Attempts      int        `json:"attempts"`
	NextAttemptAt time.Time  `json:"next_attempt_at"`
	LastError     string     `json:"last_error,omitempty" doc:"Why the last attempt failed, without secrets."`
	SentAt        *time.Time `json:"sent_at,omitempty"`
}

type cursor struct {
	EventID int64  `json:"e"`
	Channel string `json:"c"`
}
