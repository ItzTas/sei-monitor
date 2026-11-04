//go:build linux

package main

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
	fmt.Printf("core=%d/%d dumpable=%d", limit.Cur, limit.Max, dumpable)
	return 0
}

func TestHardenTurnsOffCoreDumpsAndInspection(t *testing.T) {
	cmd := exec.CommandContext(t.Context(), os.Args[0])
	cmd.Env = append(os.Environ(), hardenCheck+"=1")

	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%v: %s", err, output)
	}

	if strings.TrimSpace(string(output)) != "core=0/0 dumpable=0" {
		t.Errorf("hardened process reports %q, want core=0/0 dumpable=0", output)
	}
}
