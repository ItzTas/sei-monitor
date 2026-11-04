package sei

import (
	"strings"
	"time"
)

type Org struct {
	ID      int64
	Acronym string
}

type Unit struct {
	ID          int64
	Acronym     string
	Description string
}

type Session struct {
	UserID        int64
	UserAcronym   string
	UserName      string
	CurrentUnitID int64
	Units         []Unit
}

func (s Session) UnitByID(id int64) (Unit, bool) {
	for _, unit := range s.Units {
		if unit.ID == id {
			return unit, true
		}
	}
	return Unit{}, false
}

func (s Session) UnitByAcronym(acronym string) (Unit, bool) {
	for _, unit := range s.Units {
		if strings.EqualFold(unit.Acronym, strings.TrimSpace(acronym)) {
			return unit, true
		}
	}
	return Unit{}, false
}

type Version struct {
	SEI    string
	Module string
}

type User struct {
	ID      int64
	Acronym string
	Name    string
}

type Process struct {
	ProcedureID   int64
	Number        string
	Type          string
	Description   string
	Assignee      *User
	ReturnDate    time.Time
	ReturnOverdue bool
	Origin        string
}

type Activity struct {
	ID          int64
	ProcedureID int64
	User        string
	Unit        string
	Text        string
	At          time.Time
}

type Document struct {
	ID          int64
	ProcedureID int64
	Protocol    string
	Type        string
	Name        string
}

type Page[T any] struct {
	Items []T
	Total int
}

type ProcessQuery struct {
	Page  int
	Limit int
	Type  string
}
