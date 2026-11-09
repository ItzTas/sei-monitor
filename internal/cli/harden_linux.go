//go:build linux

package cli

import (
	"fmt"

	"golang.org/x/sys/unix"
)

func harden() error {
	if err := unix.Setrlimit(unix.RLIMIT_CORE, &unix.Rlimit{}); err != nil {
		return fmt.Errorf("set RLIMIT_CORE to 0: %w", err)
	}
	if err := unix.Prctl(unix.PR_SET_DUMPABLE, 0, 0, 0, 0); err != nil {
		return fmt.Errorf("set PR_SET_DUMPABLE to 0: %w", err)
	}
	unix.Umask(0o077)
	return nil
}
