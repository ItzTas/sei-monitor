package cycle

type Output struct {
	Body Scheduled
}

type Scheduled struct {
	Status string `json:"status" enum:"scheduled" doc:"The cycle starts right away; follow it on /status."`
}
