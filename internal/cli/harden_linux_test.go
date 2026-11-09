//go:build linux

package cli

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	"testing"

	"golang.org/x/sys/unix"
)

func reportHardening() int {
	if err := harden(); err != nil {
		fmt.Println("error:", err)
		return 1
	}
	var limit unix.Rlimit
	if err := unix.Getrlimit(unix.RLIMIT_CORE, &limit); err != nil {
		fmt.Println("error:", err)
		return 1
	}
	dumpable, err := unix.PrctlRetInt(unix.PR_GET_DUMPABLE, 0, 0, 0, 0)
	if err != nil {
		fmt.Println("error:", err)
		return 1
	}
	mask := unix.Umask(0o077)
	fmt.Printf("core=%d/%d dumpable=%d umask=%03o", limit.Cur, limit.Max, dumpable, mask)
	return 0
}

func TestHardenedProcessHasNoCoreDumpsNoInspectionAndPrivateFiles(t *testing.T) {
	cmd := exec.CommandContext(t.Context(), os.Args[0])
	cmd.Env = append(os.Environ(), hardenCheck+"=1")

	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%v: %s", err, output)
	}

	if want := "core=0/0 dumpable=0 umask=077"; strings.TrimSpace(string(output)) != want {
		t.Errorf("hardened process reports %q, want %s", output, want)
	}
}
