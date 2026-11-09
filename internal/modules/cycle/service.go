package cycle

type Monitor interface {
	TriggerCycle() error
}
