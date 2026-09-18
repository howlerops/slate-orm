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

	ctx = s.ctx(ctx)
	response, err := s.client.rpc.Related(ctx, &pb.RelatedRequest{
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
		return nil, fromRPC(ctx, err)
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

// Step is one level of a relationship path.
//
// The relationship, plus the table its rows come back as — On for Children and
// the key's parent for Parents. The table is named per step rather than per
// call because a path returns a *different* table at every level, which is
// also why the schema claim the client sends is per step.
type Step struct {
	Relation
	// Table is the table this step's rows come back as.
	Table string
}

// RelatedNode is a row of one level with the rows of the next level below it.
//
// Related is empty on the last level of a path and on any row that related to
// nothing. The two are the same to a caller walking the tree, and telling them
// apart would mean promising something about the difference between "no
// children" and "no more levels".
type RelatedNode struct {
	Row     []Value
	Related []RelatedNode
}

// RelatedPath walks a path of relationships for many parents, in one request.
//
// Returns a slice the same length as keys: entry i is the first level's rows
// for keys[i], each carrying its own next level, and so on down the path. A
// key that related to nothing gets an empty slice.
//
// One read per level, whatever the number of parents. Each step's key set is
// the previous step's rows, deduplicated by the server, so a hundred libraries
// and a thousand shelves are still two reads. Resolving the path from the
// client — one Related call per level, regrouped by hand — is the same shape as
// the N+1 this layer exists to prevent, one level up.
//
// The depth is bounded by the server's max_relation_depth, because one step is
// one read and the depth arrives in the request. A longer path is refused with
// KindInvalidRequest naming the limit.
func (s *Session) RelatedPath(
	ctx context.Context,
	path []Step,
	keys ...[]Value,
) ([][]RelatedNode, error) {
	return s.relatedPath(ctx, "", s.freshness(), path, keys)
}

// RelatedPath walks the path inside the transaction, seeing its uncommitted
// writes. Otherwise exactly [Session.RelatedPath].
func (t *Transaction) RelatedPath(
	ctx context.Context,
	path []Step,
	keys ...[]Value,
) ([][]RelatedNode, error) {
	return t.session.relatedPath(ctx, t.id, nil, path, keys)
}

// RelatedThrough is a path's far rows per parent, with the levels between
// dropped — the many-to-many, where the join rows exist only to connect the
// two ends.
//
// Exactly RelatedPath with the intermediate levels flattened away, and one
// request either way, which is why it is written on top of it rather than
// beside it: two regroupings of one shape is two places for an off-by-one to
// live. `slate-orm`'s own LoadRelatedThrough is written on top of LoadNested
// for the same reason.
//
// Duplicates are kept and are not an error: two join rows pointing at one far
// row give it twice, because the caller is the one who knows whether that
// means anything.
func (s *Session) RelatedThrough(
	ctx context.Context,
	path []Step,
	keys ...[]Value,
) ([][][]Value, error) {
	trees, err := s.RelatedPath(ctx, path, keys...)
	if err != nil {
		return nil, err
	}
	out := make([][][]Value, 0, len(trees))
	for _, tree := range trees {
		out = append(out, leaves(tree, len(path)-1))
	}
	return out, nil
}

// leaves is the rows depth levels down, in the order the levels give them.
//
// By depth rather than by "nodes with no children", which is how this was
// first written in the Python client and which is wrong: a *middle* row that
// related to nothing has no children either, so that version handed back a
// shelf where the caller asked for copies. Caught by a fixture with a
// deliberately empty middle row; without one the two readings agree on every
// input.
func leaves(tree []RelatedNode, depth int) [][]Value {
	out := [][]Value{}
	for _, node := range tree {
		if depth == 0 {
			out = append(out, node.Row)
			continue
		}
		out = append(out, leaves(node.Related, depth-1)...)
	}
	return out
}

func (s *Session) relatedPath(
	ctx context.Context,
	transaction string,
	freshness *pb.Freshness,
	path []Step,
	keys [][]Value,
) ([][]RelatedNode, error) {
	if len(path) == 0 {
		return nil, &Error{
			Kind:    KindInvalidRequest,
			Code:    codes.InvalidArgument,
			Message: "a path needs at least one step; use Related for one relationship",
		}
	}

	steps := make([]*pb.RelatedStep, 0, len(path))
	for _, step := range path {
		way := pb.Relation_CHILDREN
		if step.Way == Parents {
			way = pb.Relation_PARENTS
		}
		steps = append(steps, &pb.RelatedStep{
			Relation: &pb.Relation{
				Table:      step.On,
				ForeignKey: step.Through,
				Direction:  way,
			},
			Schema: s.client.schemas.claimFor(step.Table),
		})
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

	ctx = s.ctx(ctx)
	response, err := s.client.rpc.Related(ctx, &pb.RelatedRequest{
		Transaction: transaction,
		Keys:        wire,
		Freshness:   freshness,
		Path:        steps,
	})
	if err != nil {
		return nil, fromRPC(ctx, err)
	}
	s.observeServedBy(response.ServedBy)

	// Keyed by the serialised value rather than by a Go comparison, for the
	// reason `related` gives: the server grouped on the kernel's own equality.
	encoding := proto.MarshalOptions{Deterministic: true}
	levels := make([]map[string][]*pb.Row, 0, len(response.Levels))
	for _, level := range response.Levels {
		grouped := make(map[string][]*pb.Row, len(level.Groups))
		for _, group := range level.Groups {
			encoded, err := encoding.Marshal(group.Key)
			if err != nil {
				return nil, err
			}
			grouped[string(encoded)] = group.Rows
		}
		levels = append(levels, grouped)
	}

	// The walk the server's key_ordinal makes possible: take a row from the
	// level above, read the value at the next level's ordinal, look it up. No
	// catalog and no schema knowledge, which is why all three SDKs do this the
	// same way.
	var below func(int, string) ([]RelatedNode, error)
	below = func(level int, key string) ([]RelatedNode, error) {
		if level >= len(levels) {
			return []RelatedNode{}, nil
		}
		rows := levels[level][key]
		out := make([]RelatedNode, 0, len(rows))
		for _, row := range rows {
			decoded, err := rowFromProto(row)
			if err != nil {
				return nil, err
			}
			children := []RelatedNode{}
			if level+1 < len(response.Levels) {
				ordinal := int(response.Levels[level+1].KeyOrdinal)
				if ordinal >= len(row.Values) {
					return nil, &Error{
						Kind: KindInternal,
						Code: codes.Internal,
						Message: "the server named a key ordinal past the end of a row; " +
							"the client and the catalog disagree about this table",
					}
				}
				encoded, err := encoding.Marshal(row.Values[ordinal])
				if err != nil {
					return nil, err
				}
				children, err = below(level+1, string(encoded))
				if err != nil {
					return nil, err
				}
			}
			out = append(out, RelatedNode{Row: decoded, Related: children})
		}
		return out, nil
	}

	out := make([][]RelatedNode, 0, len(wire))
	for _, key := range wire {
		encoded, err := encoding.Marshal(key)
		if err != nil {
			return nil, err
		}
		tree, err := below(0, string(encoded))
		if err != nil {
			return nil, err
		}
		out = append(out, tree)
	}
	return out, nil
}
