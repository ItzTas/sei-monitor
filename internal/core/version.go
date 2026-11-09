package core

import "runtime/debug"

var version string

func Version() string {
	if version != "" {
		return version
	}
	if info, ok := debug.ReadBuildInfo(); ok && info.Main.Version != "" && info.Main.Version != "(devel)" {
		return info.Main.Version
	}
	return "dev"
}

func UserAgent() string {
	return "sei-monitor/" + Version()
}
