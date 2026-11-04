package sei_test

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

type countingPassword struct {
	calls atomic.Int32
	err   error
}

func (p *countingPassword) Password(context.Context) (config.Secret, error) {
	p.calls.Add(1)
	if p.err != nil {
		return config.Secret{}, p.err
	}
	return config.NewSecret(seitest.Password), nil
}

func newClient(t *testing.T, server *seitest.Server) *sei.Client {
	t.Helper()
	return newClientWithPassword(server, config.StaticPassword(config.NewSecret(seitest.Password)))
}

func newClientWithPassword(server *seitest.Server, password config.PasswordSource) *sei.Client {
	creds := sei.Credentials{Username: seitest.Username, Password: password, OrgID: seitest.OrgID}
	return sei.New(server.APIURL(), creds, sei.Options{UserAgent: "sei-monitor/test", Location: server.Location()})
}

func processes(numbers ...int) []seitest.Process {
	result := make([]seitest.Process, 0, len(numbers))
	for _, n := range numbers {
		result = append(result, seitest.Process{ID: int64(9000 + n), Number: seitest.ProcessNumber(n), Type: "Contrato"})
	}
	return result
}

func TestLoginReturnsTheSession(t *testing.T) {
	server := seitest.New(t)
	client := newClient(t, server)

	session, err := client.Login(t.Context())
	if err != nil {
		t.Fatal(err)
	}

	if session.UserID != seitest.UserID || session.UserAcronym != seitest.Username || session.UserName != seitest.UserName {
		t.Errorf("user = %+v", session)
	}
	if session.CurrentUnitID != seitest.UnitNUPROC || len(session.Units) != 2 {
		t.Errorf("units = %+v (current %d)", session.Units, session.CurrentUnitID)
	}
	if unit, ok := session.UnitByAcronym("sedemo/gab"); !ok || unit.ID != seitest.UnitGAB {
		t.Errorf("case-insensitive unit lookup = %+v, %v", unit, ok)
	}
	if server.LastUserAgent() != "sei-monitor/test" {
		t.Errorf("User-Agent = %q", server.LastUserAgent())
	}
}

func TestVersion(t *testing.T) {
	client := newClient(t, seitest.New(t))

	version, err := client.Version(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	if version != (sei.Version{SEI: seitest.SEIVersion, Module: seitest.ModuleVersion}) {
		t.Errorf("version = %+v", version)
	}
}

func TestExpiredTokenIsRenewedOnceAndTheCallRepeated(t *testing.T) {
	server := seitest.New(t)
	server.SetProcesses(seitest.UnitNUPROC, processes(1, 2)...)
	password := &countingPassword{}
	client := newClientWithPassword(server, password)
	if _, err := client.Processes(t.Context(), seitest.UnitNUPROC); err != nil {
		t.Fatal(err)
	}

	server.ExpireTokens()
	var wg sync.WaitGroup
	errs := make([]error, 5)
	for i := range errs {
		wg.Go(func() {
			_, errs[i] = client.Processes(t.Context(), seitest.UnitNUPROC)
		})
	}
	wg.Wait()

	if err := errors.Join(errs...); err != nil {
		t.Fatal(err)
	}
	if server.Logins() != 2 {
		t.Errorf("logins = %d, want 2 (the first one and one renewal)", server.Logins())
	}
	if password.calls.Load() != 2 {
		t.Errorf("password fetched %d times, want once per login", password.calls.Load())
	}
}

func TestChangedPasswordStopsAfterOneLoginAttempt(t *testing.T) {
	server := seitest.New(t)
	client := newClient(t, server)
	if _, err := client.Login(t.Context()); err != nil {
		t.Fatal(err)
	}

	server.SetPassword("new-password")
	_, err := client.Processes(t.Context(), seitest.UnitNUPROC)

	if !errors.Is(err, sei.ErrInvalidCredentials) {
		t.Fatalf("error = %v, want sei.ErrInvalidCredentials", err)
	}
	if server.FailedLogins() != 1 {
		t.Errorf("rejected logins = %d, want 1", server.FailedLogins())
	}
}

func TestPasswordSourceFailureIsNotAnInvalidCredential(t *testing.T) {
	server := seitest.New(t)
	client := newClientWithPassword(server, &countingPassword{err: config.ErrPasswordCommand})

	_, err := client.Processes(t.Context(), seitest.UnitNUPROC)

	if !errors.Is(err, config.ErrPasswordCommand) || errors.Is(err, sei.ErrInvalidCredentials) {
		t.Fatalf("error = %v, want config.ErrPasswordCommand only", err)
	}
	if server.Calls(seitest.RouteLogin) != 0 {
		t.Errorf("SEI received %d logins without a password", server.Calls(seitest.RouteLogin))
	}
}

func TestProcessesWalksEveryPage(t *testing.T) {
	server := seitest.New(t)
	numbers := make([]int, 120)
	for i := range numbers {
		numbers[i] = i + 1
	}
	server.SetProcesses(seitest.UnitNUPROC, processes(numbers...)...)
	client := newClient(t, server)

	got, err := client.Processes(t.Context(), seitest.UnitNUPROC)
	if err != nil {
		t.Fatal(err)
	}

	if len(got) != 120 || got[0].ProcedureID != 9001 || got[119].ProcedureID != 9120 {
		t.Fatalf("processes = %d (first %d, last %d)", len(got), got[0].ProcedureID, got[len(got)-1].ProcedureID)
	}
	if calls := server.Calls(seitest.RouteProcesses); calls != 3 {
		t.Errorf("calls to %s = %d, want 3 pages of %d", seitest.RouteProcesses, calls, sei.ProcessPageSize)
	}
}

func TestUnitHeaderSelectsTheUnit(t *testing.T) {
	server := seitest.New(t)
	server.SetProcesses(seitest.UnitNUPROC, processes(1)...)
	server.SetProcesses(seitest.UnitGAB, processes(2, 3)...)
	client := newClient(t, server)

	gab, err := client.Processes(t.Context(), seitest.UnitGAB)
	if err != nil {
		t.Fatal(err)
	}
	nuproc, err := client.Processes(t.Context(), seitest.UnitNUPROC)
	if err != nil {
		t.Fatal(err)
	}

	if len(gab) != 2 || len(nuproc) != 1 || nuproc[0].ProcedureID != 9001 {
		t.Errorf("GAB = %+v, NUPROC = %+v", gab, nuproc)
	}
}

func TestReturnDateOfAnotherProcessIsIgnored(t *testing.T) {
	server := seitest.New(t)
	withReturn := seitest.Process{ID: 9001, Number: seitest.ProcessNumber(1), ReturnDate: "06/11/2026"}
	without := seitest.Process{ID: 9002, Number: seitest.ProcessNumber(2)}
	server.SetProcesses(seitest.UnitNUPROC, withReturn, without)
	client := newClient(t, server)

	got, err := client.Processes(t.Context(), seitest.UnitNUPROC)
	if err != nil {
		t.Fatal(err)
	}

	if got[0].ReturnDate.IsZero() || !got[1].ReturnDate.IsZero() {
		t.Errorf("returns = %v and %v; only the first process has a scheduled return", got[0].ReturnDate, got[1].ReturnDate)
	}
}

func TestServerErrorsAreTransient(t *testing.T) {
	server := seitest.New(t)
	server.FailNext(seitest.RouteProcesses, http.StatusBadGateway, 1)
	client := newClient(t, server)

	_, err := client.Processes(t.Context(), seitest.UnitNUPROC)

	if !errors.Is(err, sei.ErrServer) {
		t.Fatalf("error = %v, want sei.ErrServer", err)
	}
}

func TestNetworkErrorsHideTheQuery(t *testing.T) {
	closed := httptest.NewServer(http.NotFoundHandler())
	closed.Close()
	client := sei.New(closed.URL+"/api", sei.Credentials{Username: "u", Password: config.StaticPassword(config.NewSecret("p")), OrgID: 1}, sei.Options{})

	_, err := client.ProcessPage(t.Context(), 1, sei.ProcessQuery{Limit: 50})

	if err == nil || strings.Contains(err.Error(), "?") || strings.Contains(err.Error(), "limit=") {
		t.Fatalf("error = %v, want one without the query string", err)
	}
}

func TestDiscoverAndResolveOrg(t *testing.T) {
	server := seitest.New(t)
	opts := sei.Options{UserAgent: "sei-monitor/test"}

	base, err := sei.Discover(t.Context(), server.URL(), opts)
	if err != nil || base != server.APIURL() {
		t.Fatalf("Discover = %q, %v; want %q", base, err, server.APIURL())
	}

	byID, err := sei.ResolveOrg(t.Context(), server.URL(), "63", opts)
	if err != nil || byID.ID != seitest.OrgID || server.Calls(seitest.RouteLoginPage) != 0 {
		t.Errorf("org by id = %+v, %v (login page opened %d times)", byID, err, server.Calls(seitest.RouteLoginPage))
	}

	byAcronym, err := sei.ResolveOrg(t.Context(), server.URL(), "sedemo", opts)
	if err != nil || byAcronym != (sei.Org{ID: seitest.OrgID, Acronym: seitest.OrgAcronym}) {
		t.Errorf("org by acronym = %+v, %v", byAcronym, err)
	}

	orgs, err := sei.ListOrgs(t.Context(), server.URL(), opts)
	if err != nil || len(orgs) != len(seitest.DefaultOrgs()) {
		t.Errorf("login page orgs = %d, %v; want all %d, although /orgao/listar stops at 10", len(orgs), err, len(seitest.DefaultOrgs()))
	}

	_, err = sei.ResolveOrg(t.Context(), server.URL(), "NOSUCHORG", opts)
	if !errors.Is(err, sei.ErrOrgNotFound) {
		t.Errorf("unknown org: error = %v, want sei.ErrOrgNotFound", err)
	}
}

func TestDisabledModule(t *testing.T) {
	server := seitest.New(t)
	server.SetModuleInactive(true)

	_, err := sei.Discover(t.Context(), server.URL(), sei.Options{})
	if !errors.Is(err, sei.ErrModuleUnavailable) {
		t.Fatalf("Discover: error = %v, want sei.ErrModuleUnavailable", err)
	}
	_, err = newClient(t, server).Login(t.Context())
	if !errors.Is(err, sei.ErrModuleUnavailable) || errors.Is(err, sei.ErrInvalidCredentials) {
		t.Fatalf("Login: error = %v, want sei.ErrModuleUnavailable", err)
	}
}
