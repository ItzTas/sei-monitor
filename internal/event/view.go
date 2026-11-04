package event

import "time"

type View struct {
	ID            int64      `json:"id" doc:"Event id."`
	Kind          Kind       `json:"kind" enum:"process_entered,process_left,activity_added,document_added,assignment_changed,return_due_soon,return_overdue,account_login_failed" doc:"What happened."`
	ProcedureID   int64      `json:"procedure_id,omitempty" doc:"SEI procedure id of the process, absent for account events."`
	ProcessNumber string     `json:"process_number,omitempty" doc:"Formatted process number."`
	Unit          string     `json:"unit,omitempty" doc:"Acronym of the watched unit."`
	Title         string     `json:"title" doc:"One-line description, the same as the feed item title."`
	OccurredAt    time.Time  `json:"occurred_at" doc:"When it happened in SEI, or when sei-monitor noticed it."`
	DetectedAt    time.Time  `json:"detected_at" doc:"When sei-monitor noticed it."`
	Detail        ViewDetail `json:"detail" doc:"Fields beyond the minimal level are absent unless the detail level is full."`
}

type ViewDetail struct {
	ProcessType          string `json:"process_type,omitempty"`
	Description          string `json:"description,omitempty"`
	ActivityText         string `json:"activity_text,omitempty"`
	ActivityUser         string `json:"activity_user,omitempty" doc:"Acronym of who recorded the activity, absent when it was you."`
	ActivityUnit         string `json:"activity_unit,omitempty"`
	ActivityByMe         bool   `json:"activity_by_me,omitempty"`
	DocumentName         string `json:"document_name,omitempty"`
	DocumentType         string `json:"document_type,omitempty"`
	DocumentProtocol     string `json:"document_protocol,omitempty"`
	AssignedToMe         bool   `json:"assigned_to_me,omitempty"`
	AssignedUser         string `json:"assigned_user,omitempty"`
	AssignedUserName     string `json:"assigned_user_name,omitempty"`
	PreviousAssignedUser string `json:"previous_assigned_user,omitempty"`
	ReturnDate           string `json:"return_date,omitempty" format:"date"`
	Reason               string `json:"reason,omitempty"`
}

func (f Formatter) View(ev Event) View {
	p := ev.Payload
	view := View{
		ID:            ev.ID,
		Kind:          ev.Kind,
		ProcedureID:   ev.ProcedureID,
		ProcessNumber: p.ProcessNumber,
		Unit:          p.Unit,
		Title:         f.Title(ev),
		OccurredAt:    ev.OccurredAt.UTC(),
		DetectedAt:    ev.DetectedAt.UTC(),
		Detail: ViewDetail{
			DocumentProtocol: p.DocumentProtocol,
			AssignedToMe:     f.assignedToSelf(p),
			ActivityByMe:     f.IsSelf(p.ActivityUser),
			ReturnDate:       p.ReturnDate,
			Reason:           p.Reason,
		},
	}
	if !f.Full {
		return view
	}
	detail := &view.Detail
	detail.ProcessType, detail.Description = p.ProcessType, p.Description
	detail.ActivityText, detail.ActivityUser, detail.ActivityUnit = p.ActivityText, f.other(p.ActivityUser), p.ActivityUnit
	detail.DocumentName, detail.DocumentType = p.DocumentName, p.DocumentType
	detail.PreviousAssignedUser = f.other(p.PreviousAssignee)
	if !view.Detail.AssignedToMe {
		detail.AssignedUser, detail.AssignedUserName = f.other(p.AssignedUser), p.AssignedUserName
	}
	return view
}

func (f Formatter) other(acronym string) string {
	if f.IsSelf(acronym) {
		return ""
	}
	return acronym
}
