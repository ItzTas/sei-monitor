package status

import "time"

type Output struct {
	Body Status
}

type Status struct {
	OrgAcronym   string                    `json:"org_acronym" doc:"Acronym of the agency."`
	Units        []string                  `json:"units" doc:"Acronyms of the watched units."`
	SEIVersion   string                    `json:"sei_version"`
	WSSEIVersion string                    `json:"wssei_version" doc:"Version of the mod-wssei module."`
	PausedReason string                    `json:"paused_reason" doc:"Why monitoring is paused; empty while it runs."`
	DetailLevel  string                    `json:"detail_level" enum:"minimal,full" doc:"How much the API shows of each process and event."`
	LastCycle    *CycleRun                 `json:"last_cycle" doc:"The latest cycle, or null before the first one."`
	NextCycleAt  *time.Time                `json:"next_cycle_at" doc:"When the next scheduled cycle starts, or null when none is scheduled."`
	CycleRunning bool                      `json:"cycle_running"`
	Deliveries   map[string]DeliveryCounts `json:"deliveries" doc:"Deliveries per channel and status."`
}

type CycleRun struct {
	StartedAt     time.Time  `json:"started_at"`
	FinishedAt    *time.Time `json:"finished_at"`
	Trigger       string     `json:"trigger" enum:"scheduled,manual"`
	Error         string     `json:"error" doc:"Why the cycle failed; empty when it worked."`
	ProcessesSeen int        `json:"processes_seen"`
	EventsCreated int        `json:"events_created"`
}

type DeliveryCounts struct {
	Pending int `json:"pending"`
	Sent    int `json:"sent"`
	Failed  int `json:"failed"`
}
