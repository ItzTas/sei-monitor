package server_test

import (
	"bytes"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/event"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/internal/server"
)

const (
	apiToken = "api-token-0123456789abcdefghijklmnopqrstuv"
	username = "12345678901"
)

var t0 = time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)

type fakeMonitor struct {
	state      monitor.State
	triggerErr error
	triggered  int
}

func (m *fakeMonitor) State() monitor.State {
	return m.state
}

func (m *fakeMonitor) TriggerCycle() error {
	m.triggered++
	return m.triggerErr
}

type fixture struct {
	database *core.Database
	store    *monitor.Postgres
	monitor  *fakeMonitor
	account  core.Account
	logs     *bytes.Buffer
	handler  http.Handler
}

func newFixture(t *testing.T, full bool) *fixture {
	t.Helper()
	ctx := t.Context()
	database := coretest.Open(t)
	st := monitor.NewPostgres(database.Pool)
	account, err := st.EnsureAccount(ctx, core.AccountKey{SEIBaseURL: "https://sei.example.org", Username: username, OrgID: 63}, t0)
	if err != nil {
		t.Fatal(err)
	}
	if err := st.UpdateAccountInfo(ctx, account.ID, core.AccountInfo{OrgAcronym: "SEDEMO", SEIVersion: "5.0.5", WSSEIVersion: "3.0.4"}); err != nil {
		t.Fatal(err)
	}
	run, err := st.StartCycleRun(ctx, account.ID, monitor.TriggerScheduled, t0)
	if err != nil {
		t.Fatal(err)
	}
	_, err = st.ApplyCycle(ctx, monitor.CycleChanges{
		AccountID: account.ID, RunID: run, Now: t0, FinishedAt: t0.Add(time.Minute), Channels: []string{"telegram", "webhook"},
		Processes: []core.Process{
			{ProcedureID: 9001, Number: "00000.000001/2026-00", Type: "Contrato", Description: "Compra de cadeiras", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC", AssignedUser: username, AssignedUserID: 101, ReturnDate: "2026-11-06"},
			{ProcedureID: 9002, Number: "00000.000002/2026-00", Type: "Diárias", UnitID: 1002, UnitAcronym: "SEDEMO/GAB", AssignedUser: "beltrano.silva", AssignedUserID: 202, ReturnDate: "2026-11-01", ReturnOverdue: true},
			{ProcedureID: 9003, Number: "00000.000003/2026-00", Type: "Ofício", UnitID: 1001, UnitAcronym: "SEDEMO/NUPROC"},
		},
		Documents: []core.Document{{DocumentID: 80001, ProcedureID: 9001, Protocol: "0012345", Type: "Ofício", Name: "Ofício 1 (0012345)"}},
		Events: []event.Event{
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "50001", OccurredAt: t0.Add(-3 * time.Hour), Payload: event.Payload{ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC", ActivityText: "Processo recebido na unidade", ActivityUser: username, ActivityUnit: "SEDEMO/NUPROC"}},
			{Kind: event.DocumentAdded, ProcedureID: 9001, Ref: "80001", OccurredAt: t0.Add(-2 * time.Hour), Payload: event.Payload{ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC", DocumentName: "Ofício 1 (0012345)", DocumentProtocol: "0012345"}},
			{Kind: event.AssignmentChanged, ProcedureID: 9002, Ref: "9002:a", OccurredAt: t0.Add(-time.Hour), Payload: event.Payload{ProcessNumber: "00000.000002/2026-00", Unit: "SEDEMO/GAB", AssignedUser: "beltrano.silva", AssignedUserName: "Beltrano da Silva", PreviousAssignee: username}},
			{Kind: event.ProcessLeft, ProcedureID: 9003, Ref: "9003:l", OccurredAt: t0.Add(-30 * time.Minute), Payload: event.Payload{ProcessNumber: "00000.000003/2026-00", Unit: "SEDEMO/NUPROC"}},
			{Kind: event.ReturnDueSoon, ProcedureID: 9001, Ref: "9001:2026-11-06", OccurredAt: t0, Payload: event.Payload{ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC", ReturnDate: "2026-11-06"}},
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	applyLeft(t, st, account, 9003)

	mon := &fakeMonitor{state: monitor.State{
		AccountID: account.ID, Units: []string{"SEDEMO/NUPROC", "SEDEMO/GAB"},
		Self: monitor.Self{UserID: 101, Acronym: username}, NextCycleAt: t0.Add(15 * time.Minute),
	}}
	logs := &bytes.Buffer{}
	logger := slog.New(slog.NewTextHandler(logs, &slog.HandlerOptions{Level: slog.LevelDebug}))
	deps := server.APIDeps{Pool: database.Pool, Monitor: mon, Token: core.NewSecret(apiToken), FullDetail: full, Location: time.UTC, Log: logger}
	ok := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) })
	handler := server.Handler(server.Routes{Health: ok, API: func(mux *http.ServeMux) { server.RegisterAPI(mux, deps) }}, logger)
	return &fixture{database: database, store: st, monitor: mon, account: account, logs: logs, handler: handler}
}

func applyLeft(t *testing.T, st *monitor.Postgres, account core.Account, procedureID int64) {
	t.Helper()
	if _, err := st.ApplyCycle(t.Context(), monitor.CycleChanges{AccountID: account.ID, Now: t0.Add(2 * time.Minute), Left: []int64{procedureID}}); err != nil {
		t.Fatal(err)
	}
}

func (f *fixture) do(t *testing.T, method, target, token string) (*httptest.ResponseRecorder, map[string]any) {
	t.Helper()
	request := httptest.NewRequestWithContext(t.Context(), method, target, nil)
	if token != "" {
		request.Header.Set("Authorization", "Bearer "+token)
	}
	recorder := httptest.NewRecorder()
	f.handler.ServeHTTP(recorder, request)
	var body map[string]any
	_ = json.Unmarshal(recorder.Body.Bytes(), &body)
	return recorder, body
}

func (f *fixture) get(t *testing.T, target string) map[string]any {
	t.Helper()
	response, body := f.do(t, http.MethodGet, target, apiToken)
	if response.Code != http.StatusOK {
		t.Fatalf("GET %s = %d %s", target, response.Code, response.Body.String())
	}
	return body
}

func items(body map[string]any) []map[string]any {
	var result []map[string]any
	for _, item := range body["items"].([]any) {
		result = append(result, item.(map[string]any))
	}
	return result
}

func numbers(list []map[string]any, key string) []string {
	var values []string
	for _, item := range list {
		values = append(values, fmt.Sprint(item[key]))
	}
	return values
}

func TestEveryRouteNeedsTheToken(t *testing.T) {
	f := newFixture(t, false)
	routes := [][2]string{
		{http.MethodGet, "/status"}, {http.MethodGet, "/processes"}, {http.MethodGet, "/processes/9001"},
		{http.MethodGet, "/events"}, {http.MethodGet, "/deliveries"}, {http.MethodPost, "/cycle"},
	}
	for _, route := range routes {
		for _, token := range []string{"", "wrong-token"} {
			response, body := f.do(t, route[0], route[1], token)
			if response.Code != http.StatusUnauthorized || response.Header().Get("WWW-Authenticate") != "Bearer" || body["detail"] != "missing or invalid API token" {
				t.Errorf("%s %s with token %q = %d %v %v", route[0], route[1], token, response.Code, response.Header(), body)
			}
		}
	}
	if f.monitor.triggered != 0 {
		t.Errorf("a refused request started %d cycles", f.monitor.triggered)
	}
}

func TestOpenAPIDocument(t *testing.T) {
	committed, err := os.ReadFile("../../docs/openapi.json")
	if err != nil {
		t.Fatal(err)
	}
	spec, err := server.Spec()
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(committed, append(spec, '\n')) {
		t.Fatal("docs/openapi.json differs from the API in the code; regenerate it with make openapi")
	}
	var want map[string]any
	if err := json.Unmarshal(committed, &want); err != nil {
		t.Fatal(err)
	}

	response, served := newFixture(t, false).do(t, http.MethodGet, "/openapi.json", "")

	if response.Code != http.StatusOK || response.Header().Get("Content-Type") != "application/json" || !reflect.DeepEqual(served, want) {
		t.Errorf("GET /openapi.json without a token = %d %v, want docs/openapi.json", response.Code, response.Header())
	}
}

func TestStatus(t *testing.T) {
	f := newFixture(t, true)

	response, body := f.do(t, http.MethodGet, "/status", apiToken)

	if response.Code != http.StatusOK || response.Header().Get("Cache-Control") != "no-store" || response.Header().Get("X-Content-Type-Options") != "nosniff" {
		t.Fatalf("status = %d %v", response.Code, response.Header())
	}
	want := map[string]any{
		"org_acronym": "SEDEMO", "sei_version": "5.0.5", "wssei_version": "3.0.4", "paused_reason": "",
		"detail_level": "full", "cycle_running": false, "next_cycle_at": "2026-11-04T18:15:00Z",
	}
	for key, value := range want {
		if body[key] != value {
			t.Errorf("%s = %v, want %v", key, body[key], value)
		}
	}
	if units := fmt.Sprint(body["units"]); units != "[SEDEMO/NUPROC SEDEMO/GAB]" {
		t.Errorf("units = %s", units)
	}
	last := body["last_cycle"].(map[string]any)
	if last["trigger"] != "scheduled" || last["processes_seen"] != float64(3) || last["events_created"] != float64(5) || last["finished_at"] != "2026-11-04T18:01:00Z" {
		t.Errorf("last cycle = %v", last)
	}
	if deliveries := fmt.Sprint(body["deliveries"]); deliveries != "map[telegram:map[failed:0 pending:5 sent:0] webhook:map[failed:0 pending:5 sent:0]]" {
		t.Errorf("deliveries = %s", deliveries)
	}
	if strings.Contains(response.Body.String(), username) {
		t.Errorf("/status shows the username:\n%s", response.Body.String())
	}
}

func TestProcessFilters(t *testing.T) {
	f := newFixture(t, false)
	tests := map[string][]string{
		"/processes":                       {"9002", "9001"},
		"/processes?unit=sedemo/gab":       {"9002"},
		"/processes?assigned_to_me=true":   {"9001"},
		"/processes?return_scheduled=true": {"9002", "9001"},
		"/processes?return_overdue=true":   {"9002"},
		"/processes?include_left=true":     {"9003", "9002", "9001"},
	}
	for target, want := range tests {
		got := numbers(items(f.get(t, target)), "procedure_id")
		if !slices.Equal(got, want) {
			t.Errorf("GET %s = %v, want %v", target, got, want)
		}
	}
}

func TestDetailLevelControlsTheFields(t *testing.T) {
	minimal := newFixture(t, false)
	full := newFixture(t, true)
	hidden := map[string][]string{
		"/processes":                  {"process_type", "description", "assigned_user"},
		"/events?kind=activity_added": {"activity_text", "activity_unit"},
	}
	for target, keys := range hidden {
		for _, item := range items(minimal.get(t, target)) {
			for _, key := range keys {
				if _, present := item[key]; present {
					t.Errorf("minimal %s shows %s: %v", target, key, item)
				}
				if detail, ok := item["detail"].(map[string]any); ok {
					if _, present := detail[key]; present {
						t.Errorf("minimal %s shows detail.%s: %v", target, key, detail)
					}
				}
			}
		}
	}

	processes := items(full.get(t, "/processes"))
	if processes[0]["assigned_user"] != "beltrano.silva" || processes[0]["process_type"] != "Diárias" {
		t.Errorf("full process = %v", processes[0])
	}
	if _, shown := processes[1]["assigned_user"]; shown || processes[1]["assigned_to_me"] != true || processes[1]["description"] != "Compra de cadeiras" {
		t.Errorf("full process assigned to the user = %v", processes[1])
	}
	activity := items(full.get(t, "/events?kind=activity_added"))[0]["detail"].(map[string]any)
	if activity["activity_text"] != "Processo recebido na unidade" || activity["activity_by_me"] != true {
		t.Errorf("full activity detail = %v", activity)
	}
	for _, target := range []string{"/processes", "/processes/9001", "/processes/9002", "/events", "/status"} {
		response, _ := full.do(t, http.MethodGet, target, apiToken)
		if strings.Contains(response.Body.String(), username) {
			t.Errorf("full %s shows the username:\n%s", target, response.Body.String())
		}
	}
}

func TestProcessDetail(t *testing.T) {
	f := newFixture(t, true)

	body := f.get(t, "/processes/9001")

	documents := body["documents"].([]any)
	if body["number"] != "00000.000001/2026-00" || len(documents) != 1 || documents[0].(map[string]any)["name"] != "Ofício 1 (0012345)" {
		t.Errorf("detail = %v", body)
	}
	var kinds []string
	for _, ev := range body["events"].([]any) {
		kinds = append(kinds, ev.(map[string]any)["kind"].(string))
	}
	if !slices.Equal(kinds, []string{"return_due_soon", "document_added", "activity_added"}) {
		t.Errorf("process events = %v", kinds)
	}
	response, problem := f.do(t, http.MethodGet, "/processes/9999", apiToken)
	if response.Code != http.StatusNotFound || problem["detail"] != "process not found" {
		t.Errorf("unknown process = %d %v", response.Code, problem)
	}
}

func TestEventFilters(t *testing.T) {
	f := newFixture(t, false)
	since := url.QueryEscape(t0.Add(-2 * time.Hour).Format(time.RFC3339))
	until := url.QueryEscape(t0.Format(time.RFC3339))
	tests := map[string][]string{
		"/events": {"return_due_soon", "process_left", "assignment_changed", "document_added", "activity_added"},
		"/events?kind=document_added&kind=process_left": {"process_left", "document_added"},
		"/events?procedure_id=9002":                     {"assignment_changed"},
		"/events?since=" + since + "&until=" + until:    {"process_left", "assignment_changed", "document_added"},
	}
	for target, want := range tests {
		if got := numbers(items(f.get(t, target)), "kind"); !slices.Equal(got, want) {
			t.Errorf("GET %s = %v, want %v", target, got, want)
		}
	}
}

func (f *fixture) walk(t *testing.T, target string, afterFirstPage func()) []map[string]any {
	t.Helper()
	var all []map[string]any
	page := target
	for range 20 {
		body := f.get(t, page)
		all = append(all, items(body)...)
		next, _ := body["next_cursor"].(string)
		if next == "" {
			return all
		}
		if afterFirstPage != nil {
			afterFirstPage()
			afterFirstPage = nil
		}
		page = target + "&cursor=" + url.QueryEscape(next)
	}
	t.Fatalf("GET %s still had pages after 20", target)
	return nil
}

func TestEventPagesFollowTheCursorWhileEventsArrive(t *testing.T) {
	f := newFixture(t, false)
	arrive := func() {
		_, err := f.store.ApplyCycle(t.Context(), monitor.CycleChanges{AccountID: f.account.ID, Now: t0.Add(time.Hour), Events: []event.Event{
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "late", OccurredAt: t0.Add(-150 * time.Minute)},
			{Kind: event.ActivityAdded, ProcedureID: 9001, Ref: "newest", OccurredAt: t0.Add(time.Hour)},
		}})
		if err != nil {
			t.Fatal(err)
		}
	}

	ids := numbers(f.walk(t, "/events?limit=2", arrive), "id")

	if want := []string{"5", "4", "3", "2", "6", "1"}; !slices.Equal(ids, want) {
		t.Errorf("event ids across pages = %v, want %v: the late event (6) sorts after the cursor and shows up, the newest (7) sorts before it and waits for a new first page", ids, want)
	}
}

func TestDeliveries(t *testing.T) {
	f := newFixture(t, false)

	telegram := items(f.get(t, "/deliveries?channel=telegram&status=pending"))
	sent := items(f.get(t, "/deliveries?status=sent"))
	all := f.walk(t, "/deliveries?limit=4", nil)

	if len(telegram) != 5 || telegram[0]["channel"] != "telegram" || telegram[0]["status"] != "pending" || telegram[0]["attempts"] != float64(0) {
		t.Errorf("pending telegram deliveries = %v", telegram)
	}
	if len(sent) != 0 {
		t.Errorf("sent deliveries = %v", sent)
	}
	var keys []string
	for _, delivery := range all {
		keys = append(keys, fmt.Sprint(delivery["event_id"], "/", delivery["channel"]))
	}
	want := []string{"5/telegram", "5/webhook", "4/telegram", "4/webhook", "3/telegram", "3/webhook", "2/telegram", "2/webhook", "1/telegram", "1/webhook"}
	if !slices.Equal(keys, want) {
		t.Errorf("deliveries across pages = %v, want %v", keys, want)
	}
}

func TestTriggerCycle(t *testing.T) {
	tests := []struct {
		name    string
		err     error
		status  int
		retry   string
		message string
	}{
		{name: "scheduled", status: http.StatusAccepted},
		{name: "already running", err: monitor.ErrRunning, status: http.StatusConflict, message: monitor.ErrRunning.Error()},
		{name: "paused", err: monitor.ErrPaused, status: http.StatusConflict, message: monitor.ErrPaused.Error()},
		{name: "too soon", err: &monitor.TooSoonError{RetryAfter: 4*time.Minute + 29*time.Second + 100*time.Millisecond}, status: http.StatusTooManyRequests, retry: "270"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			f := newFixture(t, false)
			f.monitor.triggerErr = tt.err

			response, body := f.do(t, http.MethodPost, "/cycle", apiToken)

			if response.Code != tt.status || response.Header().Get("Retry-After") != tt.retry || f.monitor.triggered != 1 {
				t.Fatalf("POST /cycle = %d, Retry-After %q, %v", response.Code, response.Header().Get("Retry-After"), body)
			}
			if tt.status == http.StatusAccepted && body["status"] != "scheduled" {
				t.Errorf("body = %v", body)
			}
			if tt.message != "" && body["detail"] != tt.message {
				t.Errorf("detail = %v, want %q", body["detail"], tt.message)
			}
		})
	}
}

func TestInvalidInput(t *testing.T) {
	f := newFixture(t, false)
	tests := map[string]int{
		"/events?cursor=not-a-cursor":   http.StatusBadRequest,
		"/processes?limit=500":          http.StatusUnprocessableEntity,
		"/events?kind=something_else":   http.StatusUnprocessableEntity,
		"/deliveries?status=lost":       http.StatusUnprocessableEntity,
		"/events?since=yesterday":       http.StatusUnprocessableEntity,
		"/processes/not-a-number":       http.StatusUnprocessableEntity,
		"/processes?cursor=eyJ0Ijo0Mn0": http.StatusBadRequest,
	}
	for target, want := range tests {
		if response, _ := f.do(t, http.MethodGet, target, apiToken); response.Code != want {
			t.Errorf("GET %s = %d, want %d", target, response.Code, want)
		}
	}
}

func TestServerErrorsKeepTheCauseInTheLog(t *testing.T) {
	f := newFixture(t, false)
	f.database.Pool.Close()

	response, body := f.do(t, http.MethodGet, "/processes", apiToken)

	if response.Code != http.StatusInternalServerError || body["detail"] != "internal error" || body["errors"] != nil || strings.Contains(response.Body.String(), "closed") {
		t.Errorf("response = %d %v", response.Code, body)
	}
	logs := f.logs.String()
	if !strings.Contains(logs, "closed pool") || strings.Contains(logs, apiToken) || strings.Contains(logs, "Bearer") {
		t.Errorf("logs:\n%s", logs)
	}
}
