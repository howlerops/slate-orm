/* @refresh reload */
import { createSignal, Match, Show, Switch, type JSX } from "solid-js";
import { render as mount } from "solid-js/web";
import {
  QueryClient,
  QueryClientProvider,
  createQuery,
  useQueryClient,
} from "@tanstack/solid-query";

import { api, SDKS, setHeadNode, type Persona, type Sdk } from "./api";
import { Segmented } from "./parts";
import {
  Agreement,
  Batches,
  Chains,
  ConditionalWrites,
  Decimals,
  Explore,
  Groups,
  Joins,
  Nearest,
  Pages,
  PredicateWrites,
  Relationships,
  Search,
  SoftDelete,
  Topology,
  Transactions,
  Unique,
  Windows,
} from "./panels";
import "./styles.css";

/** Whether this build is the hosted one: the Worker alone, at its own origin. */
const HOSTED = SDKS.includes("edge");

const TABS = [
  "rows",
  "search",
  "nearest",
  "joins",
  "chains",
  "groups",
  "windows",
  "pages",
  "relationships",
  "money",
  "writes",
  "conditional",
  "unique",
  "soft delete",
  "batches",
  "transactions",
  "topology",
  // Three adapters compared, which a hosted page with one does not have.
  ...(HOSTED ? [] : (["agreement"] as const)),
] as const;
type Tab = (typeof TABS)[number];

function App(): JSX.Element {
  const [sdk, setSdk] = createSignal<Sdk>(SDKS[0] ?? "go");
  const [persona, setPersona] = createSignal<Persona>("app");
  const [tab, setTab] = createSignal<Tab>("rows");
  const [node, setNode] = createSignal("a");
  const queries = useQueryClient();
  // No panel's query key names the node, so a switch would otherwise serve
  // answers cached from the other one.
  const chooseNode = (name: string) => {
    setHeadNode(name);
    setNode(name);
    queries.clear();
  };
  if (HOSTED) setHeadNode(node());

  const meta = createQuery(() => ({
    queryKey: ["meta", sdk(), node()],
    queryFn: () => api.meta(sdk(), "app"),
  }));

  const context = { sdk, persona };

  return (
    <div class="shell">
      <header class="top">
        <h1>slate explorer</h1>
        <span class="sub">
          {HOSTED
            ? "a Cloudflare Worker, two head nodes in containers, one database in R2"
            : "one database, three client libraries, and a flag that swaps between them"}
        </span>
      </header>

      <div class="bar">
        <Show when={!HOSTED}>
          <Segmented label="sdk" value={sdk()} options={SDKS} onChange={setSdk} />
        </Show>
        <Show when={HOSTED}>
          <Segmented label="node" value={node()} options={["a", "b"] as const} onChange={chooseNode} />
        </Show>
        <Segmented
          label="identity"
          value={persona()}
          options={["app", "reader", "analyst", "stranger"] as const}
          onChange={setPersona}
        />
        <Show when={meta.data?.ok && meta.data.value}>
          {(value) => (
            <span class="badge" data-tone={value().leader ? "good" : "warn"}>
              head node <b>{value().leader ? "leader" : "follower"}</b>
            </span>
          )}
        </Show>
      </div>

      <p class="why" style={{ "max-width": "78ch", "margin-top": "-6px" }}>
        <Show
          when={HOSTED}
          fallback={
            <>
              The <b>sdk</b> switch changes which client library builds the
              request — nothing else.{" "}
            </>
          }
        >
          Every request here goes from this page to a Cloudflare Worker, which
          asks a head node over gRPC-web with the TypeScript client. The{" "}
          <b>node</b> switch picks which of two head nodes it asks; only one of
          them can write.{" "}
        </Show>
        The <b>identity</b> switch changes who the database thinks is asking:{" "}
        <code>reader</code> is subject to a row policy and may not ask for a
        plan, <code>analyst</code> may read authors but not their{" "}
        <code>born</code> column, and <code>stranger</code> holds a role with no
        grant at all. None of that is enforced by the adapters.
      </p>

      <div class="tabs">
        {TABS.map((name) => (
          <button type="button" data-on={tab() === name} onClick={() => setTab(name)}>
            {name}
          </button>
        ))}
      </div>

      <Switch>
        <Match when={tab() === "rows"}>
          <Explore {...context} />
        </Match>
        <Match when={tab() === "search"}>
          <Search {...context} />
        </Match>
        <Match when={tab() === "joins"}>
          <Joins {...context} />
        </Match>
        <Match when={tab() === "groups"}>
          <Groups {...context} />
        </Match>
        <Match when={tab() === "relationships"}>
          <Relationships {...context} />
        </Match>
        <Match when={tab() === "writes"}>
          <PredicateWrites {...context} />
        </Match>
        <Match when={tab() === "conditional"}>
          <ConditionalWrites {...context} />
        </Match>
        <Match when={tab() === "soft delete"}>
          <SoftDelete {...context} />
        </Match>
        <Match when={tab() === "batches"}>
          <Batches {...context} />
        </Match>
        <Match when={tab() === "transactions"}>
          <Transactions {...context} />
        </Match>
        <Match when={tab() === "nearest"}>
          <Nearest {...context} />
        </Match>
        <Match when={tab() === "chains"}>
          <Chains {...context} />
        </Match>
        <Match when={tab() === "windows"}>
          <Windows {...context} />
        </Match>
        <Match when={tab() === "pages"}>
          <Pages {...context} />
        </Match>
        <Match when={tab() === "money"}>
          <Decimals {...context} />
        </Match>
        <Match when={tab() === "unique"}>
          <Unique {...context} />
        </Match>
        <Match when={tab() === "topology"}>
          <Topology {...context} />
        </Match>
        <Match when={tab() === "agreement"}>
          <Agreement {...context} />
        </Match>
      </Switch>

      <footer>
        <Show
          when={HOSTED}
          fallback={
            <>
              Start the backend with <code>./run.sh --headless</code>. The three
              adapters are in <code>examples/explorer/backends/</code>, one per
              SDK, all implementing <code>CONTRACT.md</code> against the same
              head node.
            </>
          }
        >
          The Worker is <code>examples/edge</code> in the slate-orm repository.
          It answers with the Node adapter's own code, and the same conformance
          cases hold it to the Python, Go and Node adapters.
        </Show>
      </footer>
    </div>
  );
}

const client = new QueryClient({
  defaultOptions: {
    queries: {
      // A refusal is a result, not a failure: the demo shows "you may not ask"
      // on purpose, and retrying it would only delay the point.
      retry: false,
      refetchOnWindowFocus: false,
      staleTime: 2_000,
    },
  },
});

const root = document.getElementById("root");
if (root) {
  mount(
    () => (
      <QueryClientProvider client={client}>
        <App />
      </QueryClientProvider>
    ),
    root,
  );
}
