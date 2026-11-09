package notify_test

import (
	"os"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/core/coretest"
)

func TestMain(m *testing.M) {
	os.Exit(coretest.Run(m))
}
