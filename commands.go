package main

import (
	"crypto/rand"
	"encoding/base64"
	"fmt"
	"slices"
	"strings"
	"text/tabwriter"

	"github.com/spf13/cobra"

	"github.com/ItzTas/sei-monitor/internal/api"
	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/sei"
)

func (a *app) orgs(cmd *cobra.Command, _ []string) error {
	seiURL, err := config.LoadSEIURL(a.source)
	if err != nil {
		return err
	}
	orgs, err := sei.ListOrgs(cmd.Context(), seiURL, seiOptions(nil))
	if err != nil {
		return err
	}
	slices.SortFunc(orgs, func(x, y sei.Org) int { return strings.Compare(x.Acronym, y.Acronym) })

	table := tabwriter.NewWriter(cmd.OutOrStdout(), 0, 0, 2, ' ', 0)
	out := &printer{w: table}
	out.printf("ACRONYM\tID\n")
	for _, org := range orgs {
		out.printf("%s\t%d\n", org.Acronym, org.ID)
	}
	if out.err != nil {
		return out.err
	}
	return table.Flush()
}

func (a *app) testLogin(cmd *cobra.Command, _ []string) error {
	cfg, err := config.LoadSEI(a.source)
	if err != nil {
		return err
	}
	ctx := cmd.Context()
	conn, err := connect(ctx, cfg)
	if err != nil {
		return err
	}
	session, err := conn.Client.Login(ctx)
	if err != nil {
		return err
	}
	version, err := conn.Client.Version(ctx)
	if err != nil {
		return err
	}
	watched, err := session.SelectUnits(cfg.Units, cfg.AllUnits)
	if err != nil {
		return fmt.Errorf("SEI_UNITS: %w", err)
	}

	out := &printer{w: cmd.OutOrStdout()}
	out.printf("API:       %s\n", conn.APIURL)
	out.printf("Agency:    %s\n", describeOrg(conn.Org))
	out.printf("Login:     ok, as %s (%s)\n", session.UserAcronym, session.UserName)
	out.printf("Versions:  SEI %s, mod-wssei %s\n", version.SEI, version.Module)
	out.printf("Your units:\n")
	for _, unit := range session.Units {
		current := ""
		if unit.ID == session.CurrentUnitID {
			current = " (current)"
		}
		out.printf("  %s%s\n", unit.Acronym, current)
	}
	out.printf("Watched:   %s\n", strings.Join(unitAcronyms(watched), ", "))
	return out.err
}

func describeOrg(org sei.Org) string {
	if org.Acronym == "" {
		return fmt.Sprintf("id %d", org.ID)
	}
	return fmt.Sprintf("%s (id %d)", org.Acronym, org.ID)
}

func unitAcronyms(units []sei.Unit) []string {
	acronyms := make([]string, 0, len(units))
	for _, unit := range units {
		acronyms = append(acronyms, unit.Acronym)
	}
	return acronyms
}

func generateToken(cmd *cobra.Command, _ []string) error {
	random := make([]byte, 32)
	if _, err := rand.Read(random); err != nil {
		return err
	}
	out := &printer{w: cmd.OutOrStdout()}
	out.printf("%s\n", base64.RawURLEncoding.EncodeToString(random))
	return out.err
}

func printOpenAPI(cmd *cobra.Command, _ []string) error {
	spec, err := api.Spec()
	if err != nil {
		return err
	}
	out := &printer{w: cmd.OutOrStdout()}
	out.printf("%s\n", spec)
	return out.err
}
