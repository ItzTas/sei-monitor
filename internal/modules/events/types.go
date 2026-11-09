package events

import (
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/resp"
)

type ListInput struct {
	Kinds       []string  `query:"kind,explode" enum:"process_entered,process_left,activity_added,document_added,assignment_changed,return_due_soon,return_overdue,account_login_failed" doc:"Only these kinds; repeat the parameter for more than one."`
	ProcedureID int64     `query:"procedure_id" doc:"Only events of this process."`
	Since       time.Time `query:"since" doc:"Only events that happened at this time or later (RFC 3339)."`
	Until       time.Time `query:"until" doc:"Only events that happened before this time (RFC 3339)."`
	resp.PageInput
}

type ListOutput struct {
	Body resp.List[event.View]
}

type cursor struct {
	OccurredAt time.Time `json:"t"`
	ID         int64     `json:"i"`
}
