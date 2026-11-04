package main

import (
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"slices"
	"strings"
	"sync"
	"text/tabwriter"

	"github.com/spf13/cobra"

	"github.com/ItzTas/sei-monitor/internal/api"
	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/notify"
	"github.com/ItzTas/sei-monitor/internal/sei"
	"github.com/ItzTas/sei-monitor/internal/whatsapp"
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

func (a *app) sendTest(cmd *cobra.Command, _ []string) (err error) {
	cfg, err := config.LoadChannels(a.source)
	if err != nil {
		return err
	}
	channels := notify.Channels(cfg, userAgent())
	if cfg.WhatsApp != nil {
		st, session, err := a.openWhatsApp(cmd.Context(), cmd.ErrOrStderr())
		if err != nil {
			return err
		}
		defer func() {
			session.Close()
			err = errors.Join(err, st.Close())
		}()
		channels = append(channels, whatsapp.NewChannel(session, cfg.WhatsApp.Recipients))
	}
	if len(channels) == 0 {
		return errors.New("no channel is configured: set SMTP_HOST, TELEGRAM_TOKEN, NTFY_URL, WEBHOOK_URL or WHATSAPP_RECIPIENTS")
	}
	results := make([]error, len(channels))
	var wg sync.WaitGroup
	for i, channel := range channels {
		wg.Go(func() { results[i] = channel.Send(cmd.Context(), notify.NewTestMessage()) })
	}
	wg.Wait()
	out := &printer{w: cmd.OutOrStdout()}
	failed := 0
	for i, channel := range channels {
		if results[i] == nil {
			out.printf("%s: ok\n", channel.Name())
			continue
		}
		failed++
		out.printf("%s: failed: %v\n", channel.Name(), results[i])
	}
	if out.err != nil {
		return out.err
	}
	if failed > 0 {
		return fmt.Errorf("%d of %d channels failed", failed, len(channels))
	}
	return nil
}
