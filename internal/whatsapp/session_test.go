package whatsapp_test

import (
	"errors"
	"log/slog"
	"os"
	"testing"

	"go.mau.fi/whatsmeow/proto/waAdv"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	waLog "go.mau.fi/whatsmeow/util/log"

	"github.com/ItzTas/sei-monitor/internal/store"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
	"github.com/ItzTas/sei-monitor/internal/whatsapp"
)

func TestMain(m *testing.M) {
	os.Exit(storetest.Run(m, store.Migrate))
}

func openStore(t *testing.T, databaseURL string) *store.Store {
	t.Helper()
	st, err := store.Open(t.Context(), databaseURL)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = st.Close() })
	return st
}

func openSession(t *testing.T, st *store.Store) *whatsapp.Session {
	t.Helper()
	session, err := whatsapp.Open(t.Context(), st.PassthroughDB(), st.Dialect(), slog.New(slog.DiscardHandler))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(session.Close)
	return session
}

func TestNewSessionIsNotPairedAndDoesNotConnect(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		session := openSession(t, openStore(t, databaseURL))

		err := session.ConnectContext(t.Context())

		if session.Paired() || !errors.Is(err, whatsapp.ErrNotPaired) || session.IsConnected() {
			t.Errorf("paired %v, connect = %v, connected %v", session.Paired(), err, session.IsConnected())
		}
	})
}

func TestPairedSessionLivesInTheDatabaseAndRefusesAnotherPairing(t *testing.T) {
	storetest.Dialects(t, func(t *testing.T, databaseURL string) {
		st := openStore(t, databaseURL)
		openSession(t, st)
		container := sqlstore.NewWithDB(st.PassthroughDB(), st.Dialect(), waLog.Noop)
		device := container.NewDevice()
		device.ID = &types.JID{User: "5595991234567", Device: 7, Server: types.DefaultUserServer}
		device.Account = &waAdv.ADVSignedDeviceIdentity{Details: []byte{1}, AccountSignature: make([]byte, 64), AccountSignatureKey: make([]byte, 32), DeviceSignature: make([]byte, 64)}
		if err := container.PutDevice(t.Context(), device); err != nil {
			t.Fatal(err)
		}

		session := openSession(t, st)
		err := session.Pair(t.Context(), "5595991234567", func(string) { t.Error("showed a pairing code") })

		if !session.Paired() || !errors.Is(err, whatsapp.ErrAlreadyPaired) {
			t.Errorf("paired %v, Pair = %v", session.Paired(), err)
		}
	})
}
