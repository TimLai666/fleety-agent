// Command fleety-insyra is a thin sidecar that exposes the Insyra DSL
// (github.com/HazelnutParadise/insyra/engine/dsl) to Fleety over a
// line-delimited JSON protocol on stdin/stdout.
//
// Fleety keeps one process alive per workspace and runs stateful DSL sessions
// through it: each request is one JSON object on a line, each response is one
// JSON object on a line. Session state (variables, loaded data) persists in the
// process between requests, and Insyra also persists named environments to disk
// under the manager root, so state survives a restart too.
//
// Protocol (requests on stdin, responses on stdout):
//
//	{"id":"1","op":"exec","session":"default","command":"newdl 1 2 3 as x"}
//	{"id":"1","ok":true,"output":"..."}
//	{"id":"2","op":"exec_script","session":"default","script":"mean x\nshow x"}
//	{"id":"3","op":"reset","session":"default"}
//	{"id":"4","op":"ping"}
//
// The manager root defaults to <UserHomeDir>/.insyra; set -root (or env
// FLEETY_INSYRA_ROOT) to a workspace path to keep environments with the data.
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"io"
	"os"
	"strings"

	"github.com/HazelnutParadise/insyra/cli/env"
	"github.com/HazelnutParadise/insyra/engine/dsl"
)

type request struct {
	ID      string `json:"id"`
	Op      string `json:"op"`      // exec | exec_script | reset | ping
	Session string `json:"session"` // session/env name (default "default")
	Command string `json:"command"` // for op=exec: one DSL line
	Script  string `json:"script"`  // for op=exec_script: multi-line .isr text
}

type response struct {
	ID     string `json:"id"`
	OK     bool   `json:"ok"`
	Output string `json:"output,omitempty"`
	Error  string `json:"error,omitempty"`
}

type session struct {
	dsl *dsl.Session
	buf *bytes.Buffer
}

type server struct {
	mgr      *env.Manager
	sessions map[string]*session
}

// Insyra v0.3.1 still advertises the removed `accel run` action in its help
// text and returns a bare "unknown accel action" error when it is invoked.
// Normalize that stale upstream surface at the sidecar boundary so Fleety's
// users see the supported planning command instead.
func normalizeInsyraText(text string) string {
	text = strings.ReplaceAll(text, "accel <devices|cache|plan|run>", "accel <devices|cache|plan>")
	return strings.ReplaceAll(
		text,
		"unknown accel action: run",
		"accel run was removed; use accel plan instead",
	)
}

func (sv *server) session(name string) (*session, error) {
	if name == "" {
		name = "default"
	}
	if s, ok := sv.sessions[name]; ok {
		return s, nil
	}
	// Insyra requires the named environment to exist before a session binds to
	// it; create it on first use so callers don't have to manage env lifecycle.
	if err := sv.mgr.EnsureBaseStructure(); err != nil {
		return nil, err
	}
	if !sv.mgr.Exists(name) {
		if err := sv.mgr.Create(name); err != nil {
			return nil, err
		}
	}
	buf := &bytes.Buffer{}
	ds, err := dsl.NewSession(sv.mgr, name, buf)
	if err != nil {
		return nil, err
	}
	s := &session{dsl: ds, buf: buf}
	sv.sessions[name] = s
	return s, nil
}

func (sv *server) handle(req *request) response {
	resp := response{ID: req.ID}
	switch req.Op {
	case "ping":
		resp.OK = true
		resp.Output = "pong"
	case "reset":
		name := req.Session
		if name == "" {
			name = "default"
		}
		delete(sv.sessions, name)
		// Insyra persists variables to the env on disk, so dropping the
		// in-process session is not enough — clear the persisted env too.
		if sv.mgr.Exists(name) {
			if err := sv.mgr.Clear(name, false); err != nil {
				resp.Error = err.Error()
				return resp
			}
		}
		resp.OK = true
	case "exec", "exec_script", "":
		s, err := sv.session(req.Session)
		if err != nil {
			resp.Error = err.Error()
			return resp
		}
		s.buf.Reset()
		var execErr error
		if req.Op == "exec_script" {
			execErr = runScript(s.dsl, req.Script)
		} else {
			execErr = s.dsl.Execute(req.Command)
		}
		resp.Output = normalizeInsyraText(s.buf.String())
		if execErr != nil {
			resp.Error = normalizeInsyraText(execErr.Error())
		} else {
			resp.OK = true
		}
	default:
		resp.Error = "unknown op: " + req.Op
	}
	return resp
}

// runScript writes the script to a temp .isr file and runs it via ExecuteFile,
// which reports errors with line numbers.
func runScript(ds *dsl.Session, script string) error {
	f, err := os.CreateTemp("", "fleety-*.isr")
	if err != nil {
		return err
	}
	name := f.Name()
	defer os.Remove(name)
	if _, err := f.WriteString(script); err != nil {
		_ = f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return ds.ExecuteFile(name)
}

func main() {
	root := flag.String("root", os.Getenv("FLEETY_INSYRA_ROOT"),
		"environment storage root (default <UserHomeDir>/.insyra)")
	flag.Parse()

	// Isolate the protocol channel: Insyra (and its deps) may print to
	// os.Stdout. Keep the real stdout for our JSON responses and point the
	// Go-level os.Stdout at stderr so stray writes can't corrupt the protocol.
	realStdout := os.Stdout
	os.Stdout = os.Stderr

	sv := &server{
		mgr:      env.NewManager(*root, ""),
		sessions: map[string]*session{},
	}

	enc := json.NewEncoder(realStdout)
	reader := bufio.NewReader(os.Stdin)
	for {
		line, err := reader.ReadBytes('\n')
		if len(bytes.TrimSpace(line)) > 0 {
			var req request
			if e := json.Unmarshal(line, &req); e != nil {
				_ = enc.Encode(&response{OK: false, Error: "bad request json: " + e.Error()})
			} else {
				resp := sv.handle(&req)
				_ = enc.Encode(&resp)
			}
		}
		if err != nil {
			if err != io.EOF {
				_ = enc.Encode(&response{OK: false, Error: "read error: " + err.Error()})
			}
			return
		}
	}
}
