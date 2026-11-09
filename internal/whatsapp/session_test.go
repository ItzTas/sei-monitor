package whatsapp_test

import (
	"errors"
	"log/slog"
	"testing"

	"go.mau.fi/whatsmeow/proto/waAdv"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	waLog "go.mau.fi/whatsmeow/util/log"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/whatsapp"
)

func openSession(t *testing.T, database *core.Database) *whatsapp.Session {
	t.Helper()
	session, err := whatsapp.Open(t.Context(), database.SQL, slog.New(slog.DiscardHandler))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(session.Close)
	return session
}

func TestNewSessionIsNotPairedAndDoesNotConnect(t *testing.T) {
	session := openSession(t, coretest.Open(t))

	err := session.ConnectContext(t.Context())

	if session.Paired() || !errors.Is(err, whatsapp.ErrNotPaired) || session.IsConnected() {
		t.Errorf("paired %v, connect = %v, connected %v", session.Paired(), err, session.IsConnected())
	}
}

func TestPairedSessionLivesInTheDatabaseAndRefusesAnotherPairing(t *testing.T) {
	database := coretest.Open(t)
	openSession(t, database)
	container := sqlstore.NewWithDB(database.SQL, "postgres", waLog.Noop)
	device := container.NewDevice()
	device.ID = &types.JID{User: "5595991234567", Device: 7, Server: types.DefaultUserServer}
	device.Account = &waAdv.ADVSignedDeviceIdentity{Details: []byte{1}, AccountSignature: make([]byte, 64), AccountSignatureKey: make([]byte, 32), DeviceSignature: make([]byte, 64)}
	if err := container.PutDevice(t.Context(), device); err != nil {
		t.Fatal(err)
	}

	session := openSession(t, database)
	err := session.Pair(t.Context(), "5595991234567", func(string) { t.Error("showed a pairing code") })

	if !session.Paired() || !errors.Is(err, whatsapp.ErrAlreadyPaired) {
		t.Errorf("paired %v, Pair = %v", session.Paired(), err)
	}
}
