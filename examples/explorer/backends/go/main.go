package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"time"

	"github.com/howlerops/slate-orm/clients/go/slate"

	"github.com/howlerops/slate-orm/examples/explorer/backends/go/schema"
)

const (
	authorsWidth = 4
	booksWidth   = 5
)

// identities maps the demo's three personas onto head-node identities.
//
// The whole point of the switcher: `reader` differs from `app` only in what the
// *database* grants it, and `stranger` holds a role with no grant on the tables
// the UI shows. No adapter enforces any of it.
var identities = map[string]slate.Identity{
	"app":      {Principal: "u64:1", Tenant: "u64:1", Roles: []string{"app"}},
	"reader":   {Principal: "u64:2", Tenant: "u64:1", Roles: []string{"reader"}},
	"stranger": {Principal: "u64:3", Tenant: "u64:1", Roles: []string{"stranger"}},
}

type server struct {
	address string
	clients map[string]*slate.Client
}

func main() {
	head := flag.String("head", "127.0.0.1:7421", "the head node")
	listen := flag.String("listen", "127.0.0.1:7431", "where to serve")
	seed := flag.Bool("seed", false, "seed the demo data and exit")
	flag.Parse()

	s := &server{address: *head, clients: map[string]*slate.Client{}}
	for name, identity := range identities {
		client, err := slate.Dial(*head, identity)
		if err != nil {
			log.Fatalf("dialling %s: %v", *head, err)
		}
		// Every request from this adapter now carries a schema check. The
		// declaration is generated from the node's own catalog by
		// `scripts/codegen.py`, so the check cannot be satisfied by a
		// declaration that merely agrees with itself — which is what a
		// hand-typed one would be.
		s.clients[name] = client.Declaring(schema.Tables)
	}
	defer func() {
		for _, c := range s.clients {
			_ = c.Close()
		}
	}()

	if *seed {
		if err := s.seed(); err != nil {
			log.Fatalf("seeding: %v", err)
		}
		fmt.Println("seeded")
		return
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/api/meta", s.handle(s.meta))
	mux.HandleFunc("/api/query", s.handle(s.query))
	mux.HandleFunc("/api/join", s.handle(s.join))
	mux.HandleFunc("/api/aggregate", s.handle(s.aggregate))
	mux.HandleFunc("/api/explain", s.handle(s.explain))
	mux.HandleFunc("/api/explain-aggregate", s.handle(s.explainAggregate))
	mux.HandleFunc("/api/nearest", s.handle(s.nearest))
	mux.HandleFunc("/api/chain", s.handle(s.chain))
	mux.HandleFunc("/api/page", s.handle(s.page))
	mux.HandleFunc("/api/related", s.handle(s.related))
	mux.HandleFunc("/api/path", s.handle(s.path))
	mux.HandleFunc("/api/batch", s.handle(s.batchWrite))
	mux.HandleFunc("/api/predicate-write", s.handle(s.predicateWrite))
	mux.HandleFunc("/api/conditional-update", s.handle(s.conditionalUpdate))
	mux.HandleFunc("/api/conditional-delete", s.handle(s.conditionalDelete))
	mux.HandleFunc("/api/purge", s.handle(s.purge))
	mux.HandleFunc("/api/bad-status", s.handle(s.badStatus))
	mux.HandleFunc("/api/typed", s.handle(s.typed))
	mux.HandleFunc("/api/transaction", s.handle(s.transaction))

	fmt.Printf("LISTENING %s\n", *listen)
	if err := http.ListenAndServe(*listen, cors(mux)); err != nil {
		log.Fatalf("serving: %v", err)
	}
	_ = os.Stdout.Sync()
}

// cors lets the frontend, served from another port in development, reach this.
func cors(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Access-Control-Allow-Origin", "*")
		w.Header().Set("Access-Control-Allow-Headers", "content-type, x-demo-identity")
		if r.Method == http.MethodOptions {
			w.WriteHeader(http.StatusNoContent)
			return
		}
		next.ServeHTTP(w, r)
	})
}

type handler func(context.Context, *slate.Session, json.RawMessage) (any, error)

func (s *server) handle(fn handler) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx, cancel := context.WithTimeout(r.Context(), 20*time.Second)
		defer cancel()

		persona := r.Header.Get("X-Demo-Identity")
		if persona == "" {
			persona = "app"
		}
		client, ok := s.clients[persona]
		if !ok {
			writeError(w, http.StatusBadRequest, "invalid-request",
				fmt.Sprintf("no such identity: %q", persona))
			return
		}

		var body json.RawMessage
		if r.Body != nil {
			_ = json.NewDecoder(r.Body).Decode(&body)
		}

		result, err := fn(ctx, client.Session(), body)
		if err != nil {
			// A slate error keeps its kind; anything else is the adapter's own
			// fault and says so rather than borrowing a database kind.
			var e *slate.Error
			if errors.As(err, &e) {
				writeSlateError(w, kindName(e.Kind), e.Message, e.Reason, e.Violations)
				return
			}
			writeError(w, http.StatusBadRequest, "adapter", err.Error())
			return
		}
		writeJSON(w, result)
	}
}

// kindName spells a kind the same way in all three adapters.
func kindName(k slate.Kind) string {
	switch k {
	case slate.KindInvalidRequest:
		return "invalid-request"
	case slate.KindNotFound:
		return "not-found"
	case slate.KindAlreadyExists:
		return "already-exists"
	case slate.KindPermissionDenied:
		return "permission-denied"
	case slate.KindUnauthenticated:
		return "unauthenticated"
	case slate.KindConflict:
		return "conflict"
	case slate.KindUnavailable:
		return "unavailable"
	case slate.KindNotLeader:
		return "not-leader"
	case slate.KindResourceLimit:
		return "resource-limit"
	case slate.KindUnknownOutcome:
		return "unknown-outcome"
	case slate.KindDataLoss:
		return "data-loss"
	case slate.KindDeadlineExceeded:
		return "deadline-exceeded"
	case slate.KindCancelled:
		return "cancelled"
	default:
		return "internal"
	}
}

func writeJSON(w http.ResponseWriter, body any) {
	w.Header().Set("Content-Type", "application/json")
	encoder := json.NewEncoder(w)
	encoder.SetEscapeHTML(false)
	_ = encoder.Encode(body)
}

// writeError reports a failure the adapter itself raised.
//
// No reason: it never reached the server, so there is no token to report, and
// reporting an empty one would be a claim about a classification the server
// never made.
func writeError(w http.ResponseWriter, status int, kind, message string) {
	w.WriteHeader(status)
	writeJSON(w, map[string]any{
		"error": map[string]string{"kind": kind, "message": message},
	})
}

// writeSlateError reports a failure the head node classified.
//
// reason is always present, empty string included, because whether the server
// sent a token is itself part of what the three SDKs must agree on: a client
// that silently stopped decoding the details blob would otherwise report the
// same body as one that decoded it and found nothing. It is the stronger half
// of the pair — kind is this adapter's word for a status code, the token is the
// server's own and is finer than the code — so a client decoding the blob
// differently from the other two disagrees here rather than in production.
//
// violations is the same argument one level down. The token says *that* a row
// broke a check; this says which ones, and it is the part each client decodes
// by hand out of ErrorInfo.metadata. Three hand-written decoders is exactly the
// shape of thing that drifts, and each client's own unit tests decode a
// captured fixture — which proves each agrees with a recording, not that they
// agree with each other against a live server. This is where that is checked.
// Always present, [] included, for the reason reason is.
func writeSlateError(
	w http.ResponseWriter, kind, message, reason string, violations []slate.CheckViolation,
) {
	// Rendered rather than handed to the encoder, so the JSON is this
	// adapter's shape and not Go's idea of a Go struct: the Python client
	// spells an absent column None and this one spells it "", which is each
	// language's own idiom and not a disagreement about what the server said.
	// Flattening both to "" here is the same normalisation kindName already
	// does for status codes.
	broke := make([]map[string]string, 0, len(violations))
	for _, one := range violations {
		broke = append(broke, map[string]string{
			"check": one.Check, "column": one.Column, "message": one.Message,
		})
	}
	w.WriteHeader(http.StatusOK)
	writeJSON(w, map[string]any{
		"error": map[string]any{
			"kind": kind, "message": message, "reason": reason, "violations": broke,
		},
	})
}
