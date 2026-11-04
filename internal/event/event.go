package event

import "time"

type Kind string

const (
	ProcessEntered     Kind = "process_entered"
	ProcessLeft        Kind = "process_left"
	ActivityAdded      Kind = "activity_added"
	DocumentAdded      Kind = "document_added"
	AssignmentChanged  Kind = "assignment_changed"
	ReturnDueSoon      Kind = "return_due_soon"
	ReturnOverdue      Kind = "return_overdue"
	AccountLoginFailed Kind = "account_login_failed"
)

type Event struct {
	ID          int64
	AccountID   int64
	Kind        Kind
	ProcedureID int64
	Ref         string
	OccurredAt  time.Time
	DetectedAt  time.Time
	Payload     Payload
}

type Payload struct {
	ProcessNumber    string `json:"process_number,omitempty"`
	Unit             string `json:"unit,omitempty"`
	ProcessType      string `json:"process_type,omitempty"`
	Description      string `json:"description,omitempty"`
	ActivityText     string `json:"activity_text,omitempty"`
	ActivityUser     string `json:"activity_user,omitempty"`
	ActivityUnit     string `json:"activity_unit,omitempty"`
	DocumentName     string `json:"document_name,omitempty"`
	DocumentType     string `json:"document_type,omitempty"`
	DocumentProtocol string `json:"document_protocol,omitempty"`
	AssignedUser     string `json:"assigned_user,omitempty"`
	AssignedUserName string `json:"assigned_user_name,omitempty"`
	PreviousAssignee string `json:"previous_assigned_user,omitempty"`
	AssignedToSelf   bool   `json:"assigned_to_self,omitempty"`
	ReturnDate       string `json:"return_date,omitempty"`
	Reason           string `json:"reason,omitempty"`
}
