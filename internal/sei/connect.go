package sei

import (
	"context"
	"errors"
	"fmt"
	"strings"

	"golang.org/x/sync/errgroup"

	"github.com/ItzTas/sei-monitor/internal/config"
)

var ErrUnknownUnit = errors.New("unit not found among the user's units")

type Target struct {
	SEIURL   string
	APIURL   string
	Org      string
	Username string
	Password config.PasswordSource
}

type Connection struct {
	Client *Client
	Org    Org
	APIURL string
}

func Connect(ctx context.Context, target Target, opts Options) (Connection, error) {
	apiURL := target.APIURL
	var org Org
	g, gctx := errgroup.WithContext(ctx)
	if apiURL == "" {
		g.Go(func() (err error) { apiURL, err = Discover(gctx, target.SEIURL, opts); return err })
	}
	g.Go(func() (err error) { org, err = ResolveOrg(gctx, target.SEIURL, target.Org, opts); return err })
	if err := g.Wait(); err != nil {
		return Connection{}, err
	}
	creds := Credentials{Username: target.Username, Password: target.Password, OrgID: org.ID}
	return Connection{Client: New(apiURL, creds, opts), Org: org, APIURL: apiURL}, nil
}

func (s Session) SelectUnits(acronyms []string, all bool) ([]Unit, error) {
	if all {
		return s.Units, nil
	}
	if len(acronyms) == 0 {
		if unit, ok := s.UnitByID(s.CurrentUnitID); ok {
			return []Unit{unit}, nil
		}
		return []Unit{{ID: s.CurrentUnitID}}, nil
	}
	selected := make([]Unit, 0, len(acronyms))
	for _, acronym := range acronyms {
		unit, ok := s.UnitByAcronym(acronym)
		if !ok {
			return nil, fmt.Errorf("%w: %q (yours are %s)", ErrUnknownUnit, acronym, strings.Join(s.unitAcronyms(), ", "))
		}
		selected = append(selected, unit)
	}
	return selected, nil
}

func (s Session) unitAcronyms() []string {
	acronyms := make([]string, 0, len(s.Units))
	for _, unit := range s.Units {
		acronyms = append(acronyms, unit.Acronym)
	}
	return acronyms
}
