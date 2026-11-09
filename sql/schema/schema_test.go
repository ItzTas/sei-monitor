package schema_test

import (
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/monitor"
	"github.com/ItzTas/sei-monitor/sql/schema"
)

func open(t *testing.T, databaseURL string) *core.Database {
	t.Helper()
	database, err := core.OpenDatabase(t.Context(), databaseURL)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := database.Close(); err != nil {
			t.Error(err)
		}
	})
	return database
}

func TestOpeningAgainKeepsTheSchemaAndTheData(t *testing.T) {
	databaseURL := coretest.DatabaseURL(t)
	key := core.AccountKey{SEIBaseURL: "https://sei.example.org", Username: "fulano.tal", OrgID: 63}
	now := time.Date(2026, 11, 4, 18, 0, 0, 0, time.UTC)
	first, err := monitor.NewPostgres(open(t, databaseURL).Pool).EnsureAccount(t.Context(), key, now)
	if err != nil {
		t.Fatal(err)
	}

	if err := schema.Migrate(t.Context(), databaseURL); err != nil {
		t.Fatal(err)
	}
	reopened := open(t, databaseURL)
	again, err := monitor.NewPostgres(reopened.Pool).Account(t.Context(), first.ID)

	if err != nil || again != first {
		t.Errorf("account after reopening = %+v, %v, want %+v", again, err, first)
	}
}

func TestMigrateRefusesAnAddressThatIsNotPostgres(t *testing.T) {
	if err := schema.Migrate(t.Context(), "mysql://localhost/sei"); err == nil {
		t.Error("Migrate accepted a MySQL address")
	}
}
