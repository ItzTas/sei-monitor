package monitor

import (
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
)

const (
	TriggerScheduled = "scheduled"
	TriggerManual    = "manual"

	keptRunsPerAccount = 1000
)

type CycleChanges struct {
	AccountID  int64
	RunID      int64
	Now        time.Time
	FinishedAt time.Time
	Baseline   bool
	Processes  []core.Process
	Left       []int64
	Documents  []core.Document
	Events     []event.Event
	Channels   []string
}

type Pruned struct {
	Events    int64
	Processes int64
}
