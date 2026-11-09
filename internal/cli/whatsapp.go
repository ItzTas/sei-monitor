package cli

import (
	"bufio"
	"context"
	"errors"
	"io"
	"log/slog"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/whatsapp"
)

const pairingTimeout = 5 * time.Minute

func (a *app) whatsappPair(cmd *cobra.Command, args []string) (err error) {
	phone, err := pairingPhone(cmd, args)
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(cmd.Context(), pairingTimeout)
	defer cancel()
	database, session, err := a.openWhatsApp(ctx, cmd.ErrOrStderr())
	if err != nil {
		return err
	}
	defer func() {
		session.Close()
		err = errors.Join(err, database.Close())
	}()
	out := &printer{w: cmd.OutOrStdout()}
	err = session.Pair(ctx, phone, func(code string) {
		out.printf("Pairing code: %s\n\n", code)
		out.printf("On the phone with number %s, open WhatsApp > Linked devices > Link a device, tap \"Link with phone number instead\" and type the code. It expires in a few minutes.\n", phone)
	})
	if errors.Is(err, whatsapp.ErrAlreadyPaired) {
		return errors.New("this database already has a paired WhatsApp number. To pair another one, unlink sei-monitor under Linked devices on the paired phone, start sei-monitor once so it drops the old session, and run whatsapp-pair again")
	}
	if err != nil {
		return err
	}
	out.printf("Paired. Restart sei-monitor so it starts sending WhatsApp messages.\n")
	return out.err
}

func pairingPhone(cmd *cobra.Command, args []string) (string, error) {
	answer, err := askPhone(cmd, args)
	if err != nil {
		return "", err
	}
	digits := strings.Map(func(r rune) rune {
		if r >= '0' && r <= '9' {
			return r
		}
		return -1
	}, answer)
	if len(digits) < 10 || len(digits) > 15 || strings.HasPrefix(digits, "0") {
		return "", usageError{errors.New("the phone number needs the country and area code, digits only (e.g. 5595991234567)")}
	}
	return digits, nil
}

func askPhone(cmd *cobra.Command, args []string) (string, error) {
	if len(args) == 1 {
		return args[0], nil
	}
	out := &printer{w: cmd.OutOrStdout()}
	out.printf("Phone number of the WhatsApp account that will send the messages, with country and area code (e.g. 5595991234567): ")
	line, err := bufio.NewReader(cmd.InOrStdin()).ReadString('\n')
	if err != nil && !errors.Is(err, io.EOF) {
		return "", err
	}
	return line, out.err
}

func pairArgs(_ *cobra.Command, args []string) error {
	if len(args) <= 1 {
		return nil
	}
	return usageError{errors.New("whatsapp-pair takes at most one argument, the phone number")}
}

func (a *app) openWhatsApp(ctx context.Context, logs io.Writer) (*core.Database, *whatsapp.Session, error) {
	databaseURL, err := core.LoadDatabaseURL(a.source)
	if err != nil {
		return nil, nil, err
	}
	database, err := core.OpenDatabase(ctx, databaseURL.Reveal())
	if err != nil {
		return nil, nil, err
	}
	logger := slog.New(slog.NewTextHandler(logs, &slog.HandlerOptions{Level: slog.LevelWarn}))
	session, err := whatsapp.Open(ctx, database.SQL, logger)
	if err != nil {
		return nil, nil, errors.Join(err, database.Close())
	}
	return database, session, nil
}
