package main

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/HazelnutParadise/insyra/cli/env"
)

func newServer(t *testing.T) *server {
	t.Helper()
	root := filepath.Join(t.TempDir(), "ws")
	return &server{
		mgr:      env.NewManager(root, ""),
		sessions: map[string]*session{},
	}
}

func TestExecIsStatefulAcrossRequests(t *testing.T) {
	sv := newServer(t)

	if r := sv.handle(&request{ID: "p", Op: "ping"}); !r.OK || r.Output != "pong" {
		t.Fatalf("ping: %+v", r)
	}

	if r := sv.handle(&request{Op: "exec", Session: "s", Command: "newdl 1 2 3 4 5 as x"}); !r.OK {
		t.Fatalf("newdl: %+v", r)
	}
	// mean must see x created by the previous request (in-process state).
	r := sv.handle(&request{Op: "exec", Session: "s", Command: "mean x"})
	if !r.OK || strings.TrimSpace(r.Output) != "3" {
		t.Fatalf("mean x: %+v", r)
	}
}

func TestExecScriptMultiLine(t *testing.T) {
	sv := newServer(t)
	r := sv.handle(&request{Op: "exec_script", Session: "s", Script: "newdl 10 20 30 as y\nmean y"})
	if !r.OK || !strings.Contains(r.Output, "20") {
		t.Fatalf("script: %+v", r)
	}
}

func TestResetDropsState(t *testing.T) {
	sv := newServer(t)
	sv.handle(&request{Op: "exec", Session: "s", Command: "newdl 1 2 3 as x"})
	// Sanity: x exists before reset (Insyra persists it to the env on disk).
	if r := sv.handle(&request{Op: "exec", Session: "s", Command: "mean x"}); !r.OK {
		t.Fatalf("x should exist before reset: %+v", r)
	}
	if r := sv.handle(&request{Op: "reset", Session: "s"}); !r.OK {
		t.Fatalf("reset: %+v", r)
	}
	// reset clears the persisted env, so x is gone even via a fresh session.
	if r := sv.handle(&request{Op: "exec", Session: "s", Command: "mean x"}); r.OK {
		t.Fatalf("expected x to be gone after reset, got ok: %+v", r)
	}
}

func TestUnknownOp(t *testing.T) {
	sv := newServer(t)
	if r := sv.handle(&request{Op: "nope"}); r.OK || !strings.Contains(r.Error, "unknown op") {
		t.Fatalf("unknown op: %+v", r)
	}
}
