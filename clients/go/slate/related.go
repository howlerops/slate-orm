package slate

import (
	"context"

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
	"google.golang.org/grpc/codes"
	"google.golang.org/protobuf/proto"
)

// Way says which direction a relationship is read.
//
// Not `Direction`, which this package already uses for a sort order. Both ways
// name the *same* foreign key, because a foreign key is the relationship:
// `shelves.library_id -> libraries` read forwards is a shelf's library, and
// read backwards is a library's shelves.
type Way int

const (
	// Children reads the rows holding the foreign key — a library's shelves.
	Children Way = iota
	// Parents reads the rows it points at — a shelf's library.
	Parents
)

// Relation names one relationship by the foreign key that already declares it.
//
// Nothing about the relationship is described here: the client sends a table
// and a key name, and the server resolves them against its catalog. A client
// that described the relationship — "join these two ordinals" — could describe
// it differently from the next client, which is the divergence the conformance
// runner exists to catch.
type Relation struct {
	// On is the table holding the foreign key. Always the child, whichever
	// direction is being read.
	On string
	// Through is that key's name, as the catalog spells it.
	Through string
	// Way is Children or Parents.
	Way Way
}

// Related loads one relationship for many parents, in a single read.
//
// Returns a slice the same length as keys: entry i is the rows related to
// keys[i]. A key with nothing related to it gets an empty slice, not a missing
// entry, so the result is indexable by the caller's own loop counter.
//
// Duplicate keys are expected and are the point: two shelves in one library
// send the same value twice, the server reads it once, and both map onto the
// one group that comes back. So this is one round trip whatever the number of
// parents, which is the reason it exists rather than a loop over Get.
//
// `table` is the table the rows come back as — On for Children, and the key's
// parent for Parents. It is named separately because the client decodes
// against it and does not hold the catalog.
func (s *Session) Related(
	ctx context.Context,
	table string,
	relation Relation,
	keys ...[]Value,
) ([][][]Value, error) {
	// A read outside a transaction carries the session's freshness floor; a
	// read inside one goes to the writer and needs none, which is why the
	// transaction identifier and the floor are passed as a pair.
	return s.related(ctx, "", s.freshness(), table, relation, keys)
}

// Related loads one relationship inside the transaction, seeing its
// uncommitted writes. Otherwise exactly [Session.Related].
func (t *Transaction) Related(
	ctx context.Context,
	table string,
	relation Relation,
	keys ...[]Value,
) ([][][]Value, error) {
	return t.session.related(ctx, t.id, nil, table, relation, keys)
}

func (s *Session) related(
	ctx context.Context,
	transaction string,
	freshness *pb.Freshness,
	table string,
	relation Relation,
	keys [][]Value,
) ([][][]Value, error) {
	way := pb.Relation_CHILDREN
	if relation.Way == Parents {
		way = pb.Relation_PARENTS
	}

	// One value per parent, not one row: a relationship relates on a single
	// column, and the server refuses a key that spans more than one besides
	// the tenant.
	wire := make([]*pb.Value, 0, len(keys))
	for _, key := range keys {
		if len(key) != 1 {
			return nil, &Error{
				Kind:    KindInvalidRequest,
				Code:    codes.InvalidArgument,
				Message: "a relating key is one value; a composite relationship is not supported",
			}
		}
		wire = append(wire, key[0].toProto())
	}

	response, err := s.client.rpc.Related(s.ctx(ctx), &pb.RelatedRequest{
		Transaction: transaction,
		Relation: &pb.Relation{
			Table:      relation.On,
			ForeignKey: relation.Through,
			Direction:  way,
		},
		Keys:      wire,
		Freshness: freshness,
		Schema:    s.client.schemas.claimFor(table),
	})
	if err != nil {
		return nil, fromRPC(err)
	}
	s.observeServedBy(response.ServedBy)

	// Keyed by the serialised value rather than by a Go comparison: the server
	// grouped on the kernel's own equality, and the client has to agree with
	// it rather than with Go's.
	//
	// Deterministic because the key has to encode the same way on both sides
	// of the lookup. A Value is a flat oneof today, with nothing for the
	// default encoder to order arbitrarily, so this changes no byte now — it
	// is here so that a Value which one day carries a map does not silently
	// stop matching itself.
	encoding := proto.MarshalOptions{Deterministic: true}
	grouped := make(map[string][][]Value, len(response.Groups))
	for _, group := range response.Groups {
		encoded, err := encoding.Marshal(group.Key)
		if err != nil {
			return nil, err
		}
		rows := make([][]Value, 0, len(group.Rows))
		for _, row := range group.Rows {
			decoded, err := rowFromProto(row)
			if err != nil {
				return nil, err
			}
			rows = append(rows, decoded)
		}
		grouped[string(encoded)] = rows
	}

	out := make([][][]Value, 0, len(keys))
	for _, key := range wire {
		encoded, err := encoding.Marshal(key)
		if err != nil {
			return nil, err
		}
		if rows, ok := grouped[string(encoded)]; ok {
			out = append(out, rows)
			continue
		}
		out = append(out, [][]Value{})
	}
	return out, nil
}
