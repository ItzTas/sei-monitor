package processes

import (
	"time"

	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/resp"
)

type ListInput struct {
	Unit            string `query:"unit" doc:"Only processes in this watched unit (acronym, any case)."`
	AssignedToMe    bool   `query:"assigned_to_me" doc:"Only processes assigned to you."`
	ReturnScheduled bool   `query:"return_scheduled" doc:"Only processes with a scheduled return."`
	ReturnOverdue   bool   `query:"return_overdue" doc:"Only processes whose scheduled return is overdue."`
	IncludeLeft     bool   `query:"include_left" doc:"Also list the processes that left the watched units."`
	resp.PageInput
}

type ListOutput struct {
	Body resp.List[Process]
}

type DetailInput struct {
	ProcedureID int64 `path:"procedure_id" doc:"SEI procedure id of the process."`
}

type DetailOutput struct {
	Body Detail
}

type Process struct {
	ProcedureID   int64      `json:"procedure_id" doc:"SEI procedure id."`
	Number        string     `json:"number" doc:"Formatted process number."`
	Unit          string     `json:"unit" doc:"Acronym of the watched unit the process is in."`
	ProcessType   string     `json:"process_type,omitempty" doc:"Only at the full detail level."`
	Description   string     `json:"description,omitempty" doc:"Only at the full detail level."`
	AssignedToMe  bool       `json:"assigned_to_me"`
	AssignedUser  string     `json:"assigned_user,omitempty" doc:"Acronym of the assignee, only at the full detail level and never when it is you."`
	ReturnDate    string     `json:"return_date,omitempty" format:"date" doc:"Scheduled return date."`
	ReturnOverdue bool       `json:"return_overdue"`
	FirstSeenAt   time.Time  `json:"first_seen_at"`
	LastSeenAt    time.Time  `json:"last_seen_at"`
	LeftAt        *time.Time `json:"left_at,omitempty" doc:"When the process left the watched units."`
}

type Document struct {
	DocumentID   int64     `json:"document_id"`
	Protocol     string    `json:"protocol" doc:"Formatted SEI number of the document."`
	DocumentType string    `json:"document_type,omitempty" doc:"Only at the full detail level."`
	Name         string    `json:"name,omitempty" doc:"Only at the full detail level."`
	FirstSeenAt  time.Time `json:"first_seen_at"`
}

type Detail struct {
	Process
	Documents []Document   `json:"documents" doc:"Documents sei-monitor has seen in the process."`
	Events    []event.View `json:"events" doc:"The latest 50 events of the process, newest first."`
}

type cursor struct {
	LastSeenAt  time.Time `json:"t"`
	ProcedureID int64     `json:"i"`
}
