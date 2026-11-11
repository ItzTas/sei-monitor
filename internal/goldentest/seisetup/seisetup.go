package seisetup

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

type Call struct {
	Call string `json:"call"`
	Args any    `json:"args"`
}

type User struct {
	ID      int64  `json:"id"`
	Acronym string `json:"acronym"`
	Name    string `json:"name"`
}

type Process struct {
	ID            int64  `json:"id"`
	Number        string `json:"number"`
	Type          string `json:"type"`
	Description   string `json:"description"`
	Assignee      *User  `json:"assignee"`
	ReturnDate    string `json:"return_date"`
	ReturnOverdue bool   `json:"return_overdue"`
	Generated     bool   `json:"generated"`
}

type Activity struct {
	ID   int64  `json:"id"`
	User string `json:"user"`
	Unit string `json:"unit"`
	Text string `json:"text"`
	At   string `json:"at"`
}

type Document struct {
	ID       int64  `json:"id"`
	Protocol string `json:"protocol"`
	Type     string `json:"type"`
	Number   string `json:"number"`
	Label    string `json:"label"`
}

type setProcessesArgs struct {
	UnitID    int64     `json:"unit_id"`
	Processes []Process `json:"processes"`
}

type addActivitiesArgs struct {
	ProcedureID int64      `json:"procedure_id"`
	Activities  []Activity `json:"activities"`
}

type addDocumentsArgs struct {
	ProcedureID int64      `json:"procedure_id"`
	Documents   []Document `json:"documents"`
}

type failNextArgs struct {
	Route  string `json:"route"`
	Status int    `json:"status"`
	Times  int    `json:"times"`
}

type setModuleInactiveArgs struct {
	Inactive bool `json:"inactive"`
}

func SetProcesses(unitID int64, processes ...Process) Call {
	return Call{Call: "set_processes", Args: setProcessesArgs{UnitID: unitID, Processes: append([]Process{}, processes...)}}
}

func AddActivities(procedureID int64, activities ...Activity) Call {
	return Call{Call: "add_activities", Args: addActivitiesArgs{ProcedureID: procedureID, Activities: append([]Activity{}, activities...)}}
}

func AddDocuments(procedureID int64, documents ...Document) Call {
	return Call{Call: "add_documents", Args: addDocumentsArgs{ProcedureID: procedureID, Documents: append([]Document{}, documents...)}}
}

func FailNext(route string, status, times int) Call {
	return Call{Call: "fail_next", Args: failNextArgs{Route: route, Status: status, Times: times}}
}

func SetModuleInactive(inactive bool) Call {
	return Call{Call: "set_module_inactive", Args: setModuleInactiveArgs{Inactive: inactive}}
}

func At(t time.Time) string {
	return t.UTC().Format(time.RFC3339Nano)
}

func Apply(server *seitest.Server, calls []Call) error {
	handlers := map[string]func(*seitest.Server, json.RawMessage) error{
		"set_processes":       setProcesses,
		"add_activities":      addActivities,
		"add_documents":       addDocuments,
		"fail_next":           failNext,
		"set_module_inactive": setModuleInactive,
	}
	for _, call := range calls {
		data, err := json.Marshal(call)
		if err != nil {
			return err
		}
		var raw struct {
			Call string          `json:"call"`
			Args json.RawMessage `json:"args"`
		}
		if err := strict(data, &raw); err != nil {
			return err
		}
		handler, ok := handlers[raw.Call]
		if !ok {
			return fmt.Errorf("seisetup: unknown call %q", raw.Call)
		}
		if err := handler(server, raw.Args); err != nil {
			return fmt.Errorf("seisetup: %s: %w", raw.Call, err)
		}
	}
	return nil
}

func strict(data []byte, target any) error {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	return decoder.Decode(target)
}

func setProcesses(server *seitest.Server, data json.RawMessage) error {
	var args setProcessesArgs
	if err := strict(data, &args); err != nil {
		return err
	}
	processes := make([]seitest.Process, 0, len(args.Processes))
	for _, p := range args.Processes {
		process := seitest.Process{
			ID: p.ID, Number: p.Number, Type: p.Type, Description: p.Description,
			ReturnDate: p.ReturnDate, ReturnOverdue: p.ReturnOverdue, Generated: p.Generated,
		}
		if p.Assignee != nil {
			process.Assignee = &seitest.User{ID: p.Assignee.ID, Acronym: p.Assignee.Acronym, Name: p.Assignee.Name}
		}
		processes = append(processes, process)
	}
	server.SetProcesses(args.UnitID, processes...)
	return nil
}

func addActivities(server *seitest.Server, data json.RawMessage) error {
	var args addActivitiesArgs
	if err := strict(data, &args); err != nil {
		return err
	}
	activities := make([]seitest.Activity, 0, len(args.Activities))
	for _, a := range args.Activities {
		at, err := time.Parse(time.RFC3339Nano, a.At)
		if err != nil {
			return err
		}
		activities = append(activities, seitest.Activity{ID: a.ID, User: a.User, Unit: a.Unit, Text: a.Text, At: at})
	}
	server.AddActivities(args.ProcedureID, activities...)
	return nil
}

func addDocuments(server *seitest.Server, data json.RawMessage) error {
	var args addDocumentsArgs
	if err := strict(data, &args); err != nil {
		return err
	}
	documents := make([]seitest.Document, 0, len(args.Documents))
	for _, d := range args.Documents {
		documents = append(documents, seitest.Document(d))
	}
	server.AddDocuments(args.ProcedureID, documents...)
	return nil
}

func failNext(server *seitest.Server, data json.RawMessage) error {
	var args failNextArgs
	if err := strict(data, &args); err != nil {
		return err
	}
	server.FailNext(args.Route, args.Status, args.Times)
	return nil
}

func setModuleInactive(server *seitest.Server, data json.RawMessage) error {
	var args setModuleInactiveArgs
	if err := strict(data, &args); err != nil {
		return err
	}
	server.SetModuleInactive(args.Inactive)
	return nil
}

type Unit struct {
	ID          int64  `json:"id"`
	Acronym     string `json:"acronym"`
	Description string `json:"description"`
}

type Account struct {
	Username string `json:"username"`
	Password string `json:"password"`
	OrgID    int64  `json:"org_id"`
	User     User   `json:"user"`
	Units    []Unit `json:"units"`
}

type Org struct {
	ID          int64  `json:"id"`
	Acronym     string `json:"acronym"`
	Description string `json:"description"`
}

type Versions struct {
	SEI    string `json:"sei"`
	Module string `json:"module"`
}

type Defaults struct {
	Account         Account  `json:"account"`
	Orgs            []Org    `json:"orgs"`
	OrgListTotal    int      `json:"org_list_total"`
	OrgListPageSize int      `json:"org_list_page_size"`
	Versions        Versions `json:"versions"`
	TimeZone        string   `json:"time_zone"`
	APIPath         string   `json:"api_path"`
}

func ReadDefaults(ctx context.Context, server *seitest.Server) (Defaults, error) {
	account := seitest.DefaultAccount()
	defaults := Defaults{
		Account: Account{
			Username: account.Username, Password: account.Password, OrgID: account.OrgID,
			User: User{ID: account.User.ID, Acronym: account.User.Acronym, Name: account.User.Name},
		},
		Versions: Versions{SEI: seitest.SEIVersion, Module: seitest.ModuleVersion},
		TimeZone: seitest.TimeZone,
		APIPath:  seitest.APIPath,
	}
	for _, unit := range account.Units {
		defaults.Account.Units = append(defaults.Account.Units, Unit(unit))
	}
	for _, org := range seitest.DefaultOrgs() {
		defaults.Orgs = append(defaults.Orgs, Org(org))
	}
	page, err := orgPage(ctx, server)
	if err != nil {
		return Defaults{}, err
	}
	defaults.OrgListPageSize = len(page.Data)
	if defaults.OrgListTotal, err = strconv.Atoi(page.Total); err != nil {
		return Defaults{}, fmt.Errorf("seisetup: org list total %q: %w", page.Total, err)
	}
	return defaults, nil
}

type orgListing struct {
	Data  []json.RawMessage `json:"data"`
	Total string            `json:"total"`
}

func orgPage(ctx context.Context, server *seitest.Server) (orgListing, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, server.APIURL()+seitest.RouteOrgs, nil)
	if err != nil {
		return orgListing{}, err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return orgListing{}, err
	}
	defer func() { _ = resp.Body.Close() }()
	var page orgListing
	if err := json.NewDecoder(resp.Body).Decode(&page); err != nil {
		return orgListing{}, fmt.Errorf("seisetup: read the org list: %w", err)
	}
	return page, nil
}
