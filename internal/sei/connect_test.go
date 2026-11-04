package sei_test

import (
	"errors"
	"slices"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

func TestConnectDiscoversTheModuleAndResolvesTheOrg(t *testing.T) {
	server := seitest.New(t)
	target := sei.Target{
		SEIURL:   server.URL(),
		Org:      seitest.OrgAcronym,
		Username: seitest.Username,
		Password: config.StaticPassword(config.NewSecret(seitest.Password)),
	}

	conn, err := sei.Connect(t.Context(), target, sei.Options{})
	if err != nil {
		t.Fatal(err)
	}

	if conn.APIURL != server.APIURL() || conn.Org.ID != seitest.OrgID {
		t.Errorf("connection = %q, org %+v", conn.APIURL, conn.Org)
	}
	if _, err := conn.Client.Login(t.Context()); err != nil {
		t.Errorf("login through the connection: %v", err)
	}
}

func TestConnectUsesTheGivenAPIURL(t *testing.T) {
	server := seitest.New(t)
	target := sei.Target{SEIURL: server.URL(), APIURL: server.APIURL(), Org: "63"}

	conn, err := sei.Connect(t.Context(), target, sei.Options{})
	if err != nil {
		t.Fatal(err)
	}

	if conn.APIURL != server.APIURL() || server.Calls(seitest.RouteVersion) != 0 || server.Calls(seitest.RouteLoginPage) != 0 {
		t.Errorf("connection = %q after %d discovery calls and %d login page loads", conn.APIURL, server.Calls(seitest.RouteVersion), server.Calls(seitest.RouteLoginPage))
	}
}

func TestSelectUnits(t *testing.T) {
	session := sei.Session{
		CurrentUnitID: 2,
		Units:         []sei.Unit{{ID: 1, Acronym: "SEDEMO/NUPROC"}, {ID: 2, Acronym: "SEDEMO/GAB"}, {ID: 3, Acronym: "SEDEMO/CPL"}},
	}
	tests := []struct {
		name     string
		acronyms []string
		all      bool
		want     []int64
	}{
		{"current unit by default", nil, false, []int64{2}},
		{"every unit", nil, true, []int64{1, 2, 3}},
		{"chosen units in the given order", []string{"sedemo/cpl", "SEDEMO/NUPROC"}, false, []int64{3, 1}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			units, err := session.SelectUnits(tt.acronyms, tt.all)
			if err != nil {
				t.Fatal(err)
			}
			ids := make([]int64, 0, len(units))
			for _, unit := range units {
				ids = append(ids, unit.ID)
			}
			if !slices.Equal(ids, tt.want) {
				t.Errorf("units = %v, want %v", ids, tt.want)
			}
		})
	}

	_, err := session.SelectUnits([]string{"SEDEMO/XYZ"}, false)
	if !errors.Is(err, sei.ErrUnknownUnit) {
		t.Errorf("unknown unit: error = %v, want sei.ErrUnknownUnit", err)
	}
}
