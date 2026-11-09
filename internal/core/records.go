package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/ItzTas/sei-monitor/internal/db"
	"github.com/ItzTas/sei-monitor/internal/event"
)

var ErrNotFound = errors.New("not found")

const (
	DeliveryPending = "pending"
	DeliverySent    = "sent"
	DeliveryFailed  = "failed"
)

type AccountKey struct {
	SEIBaseURL string
	Username   string
	OrgID      int64
}

type AccountInfo struct {
	OrgAcronym   string
	SEIVersion   string
	WSSEIVersion string
}

type Account struct {
	ID           int64
	Key          AccountKey
	CreatedAt    time.Time
	BaselineAt   time.Time
	PausedReason string
	Info         AccountInfo
}

type Process struct {
	ProcedureID    int64
	Number         string
	Type           string
	Description    string
	UnitID         int64
	UnitAcronym    string
	AssignedUser   string
	AssignedUserID int64
	ReturnDate     string
	ReturnOverdue  bool
	LastActivityID int64
	DocumentTotal  int
	FirstSeenAt    time.Time
	LastSeenAt     time.Time
	LeftAt         time.Time
}

type Document struct {
	DocumentID  int64
	ProcedureID int64
	Protocol    string
	Type        string
	Name        string
	FirstSeenAt time.Time
}

type CycleRun struct {
	ID            int64
	AccountID     int64
	Trigger       string
	StartedAt     time.Time
	FinishedAt    time.Time
	Error         string
	ProcessesSeen int
	EventsCreated int
}

type Delivery struct {
	EventID       int64
	Channel       string
	Status        string
	Attempts      int
	NextAttemptAt time.Time
	LastError     string
	SentAt        time.Time
}

type DueDelivery struct {
	Delivery
	Event event.Event
}

func AccountFromRow(row db.Account) Account {
	return Account{
		ID:           row.ID,
		Key:          AccountKey{SEIBaseURL: row.SeiBaseUrl, Username: row.Username, OrgID: row.OrgID},
		CreatedAt:    row.CreatedAt.UTC(),
		BaselineAt:   Deref(row.BaselineAt),
		PausedReason: row.PausedReason,
		Info:         AccountInfo{OrgAcronym: row.OrgAcronym, SEIVersion: row.SeiVersion, WSSEIVersion: row.WsseiVersion},
	}
}

func ProcessFromRow(row db.Process) Process {
	return Process{
		ProcedureID:    row.ProcedureID,
		Number:         row.Number,
		Type:           row.ProcessType,
		Description:    row.Description,
		UnitID:         row.UnitID,
		UnitAcronym:    row.UnitAcronym,
		AssignedUser:   row.AssignedUser,
		AssignedUserID: row.AssignedUserID,
		ReturnDate:     row.ReturnDate,
		ReturnOverdue:  row.ReturnOverdue,
		LastActivityID: row.LastActivityID,
		DocumentTotal:  int(row.DocumentTotal),
		FirstSeenAt:    row.FirstSeenAt.UTC(),
		LastSeenAt:     row.LastSeenAt.UTC(),
		LeftAt:         Deref(row.LeftAt),
	}
}

func DocumentFromRow(row db.Document) Document {
	return Document{
		DocumentID:  row.DocumentID,
		ProcedureID: row.ProcedureID,
		Protocol:    row.FormattedProtocol,
		Type:        row.DocumentType,
		Name:        row.Name,
		FirstSeenAt: row.FirstSeenAt.UTC(),
	}
}

func CycleRunFromRow(row db.CycleRun) CycleRun {
	return CycleRun{
		ID:            row.ID,
		AccountID:     row.AccountID,
		Trigger:       row.Trigger,
		StartedAt:     row.StartedAt.UTC(),
		FinishedAt:    Deref(row.FinishedAt),
		Error:         row.Error,
		ProcessesSeen: int(row.ProcessesSeen),
		EventsCreated: int(row.EventsCreated),
	}
}

func DeliveryFromRow(row db.Delivery) Delivery {
	return Delivery{
		EventID:       row.EventID,
		Channel:       row.Channel,
		Status:        row.Status,
		Attempts:      int(row.Attempts),
		NextAttemptAt: row.NextAttemptAt.UTC(),
		LastError:     row.LastError,
		SentAt:        Deref(row.SentAt),
	}
}

func EventFromRow(row db.Event) (event.Event, error) {
	ev := event.Event{
		ID:          row.ID,
		AccountID:   row.AccountID,
		Kind:        event.Kind(row.Kind),
		ProcedureID: row.ProcedureID,
		Ref:         row.Ref,
		OccurredAt:  row.OccurredAt.UTC(),
		DetectedAt:  row.DetectedAt.UTC(),
	}
	if err := json.Unmarshal([]byte(row.Payload), &ev.Payload); err != nil {
		return event.Event{}, fmt.Errorf("decode the payload of event %d: %w", row.ID, err)
	}
	return ev, nil
}

func EventsFromRows(rows []db.Event) ([]event.Event, error) {
	events := make([]event.Event, 0, len(rows))
	for _, row := range rows {
		ev, err := EventFromRow(row)
		if err != nil {
			return nil, err
		}
		events = append(events, ev)
	}
	return events, nil
}

func Map[From, To any](rows []From, convert func(From) To) []To {
	converted := make([]To, 0, len(rows))
	for _, row := range rows {
		converted = append(converted, convert(row))
	}
	return converted
}

func Deref(value *time.Time) time.Time {
	if value == nil {
		return time.Time{}
	}
	return value.UTC()
}

func OptionalTime(value time.Time) *time.Time {
	if value.IsZero() {
		return nil
	}
	return &value
}
